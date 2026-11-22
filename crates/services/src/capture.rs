// SPDX-License-Identifier: GPL-3.0-or-later

//! Pictures of what is on screen, through `wlr-screencopy`, on a Wayland
//! connection of its own.
//!
//! The shell takes one of a menu as it starts to close. The compositor
//! blurs what is behind a menu at full strength for as long as the menu's
//! surface says where, whatever the menu's own opacity, so a menu that
//! fades out would keep its blur to the last frame. A picture of the
//! screen where the menu is has the blur in it, and fades as one
//! (docs/DECISIONS.md, "Closing what is blurred").
//!
//! A picture is asked for from the main loop and made by a thread that
//! reads the compositor's events. Nothing here waits: the answer comes
//! over a channel, and is `None` when there is no picture to be had.

use std::fs::File;
use std::os::fd::AsFd;
use std::os::unix::fs::FileExt;
use std::sync::{Arc, Mutex};

use async_channel::{Receiver, Sender};
use wayland_client::protocol::{wl_buffer, wl_output, wl_registry, wl_shm, wl_shm_pool};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};

/// How a picture's pixels lie in memory, four bytes each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// Blue, green, red, and a byte that says nothing.
    Bgrx,
    /// Red, green, blue, and a byte that says nothing.
    Rgbx,
}

/// A part of an output as it was on screen, without the pointer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    /// In device pixels.
    pub width: u32,
    pub height: u32,
    /// Bytes from one row to the next.
    pub stride: u32,
    pub format: Format,
    /// The rows from the top down.
    pub pixels: Vec<u8>,
}

/// A rectangle on an output or a surface, in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// The smallest number of logical pixels that is a whole number of device
/// pixels at `scale`: 1 at 1 and 2, 2 at 1.5, 4 at 1.25 and 1.75.
///
/// Compositors give fractional scales in 120ths (`wp-fractional-scale`).
pub fn whole_step(scale: f64) -> i32 {
    fn gcd(a: i32, b: i32) -> i32 {
        if b == 0 { a } else { gcd(b, a % b) }
    }
    let scale = ((scale * 120.0).round() as i32).max(1);
    120 / gcd(120, scale)
}

/// The smallest rectangle around `rect` whose edges are multiples of
/// `step`.
///
/// With [`whole_step`] and a rectangle on a surface, that is one whose
/// corners are on whole device pixels of the surface's buffer, wherever
/// on its output the compositor put the surface: a picture of it can be
/// drawn back pixel for pixel.
pub fn outward(rect: Rect, step: i32) -> Rect {
    let step = step.max(1);
    let x = rect.x.div_euclid(step) * step;
    let y = rect.y.div_euclid(step) * step;
    let right = (rect.x + rect.width + step - 1).div_euclid(step) * step;
    let bottom = (rect.y + rect.height + step - 1).div_euclid(step) * step;
    Rect {
        x,
        y,
        width: right - x,
        height: bottom - y,
    }
}

/// Where a surface that is not attached to both sides of its output is
/// put along that axis: in the middle of the `room` there is for it.
///
/// As smithay's `LayerMap::arrange` works it out, each half rounded down
/// on its own.
pub fn centred(room: i32, size: i32) -> i32 {
    room / 2 - size / 2
}

struct Output {
    /// The registry's name for it.
    global: u32,
    output: wl_output::WlOutput,
    /// Its connector, such as `eDP-1`, once the compositor said.
    name: Option<String>,
}

#[derive(Default)]
struct Shared {
    shm: Option<wl_shm::WlShm>,
    manager: Option<ZwlrScreencopyManagerV1>,
    outputs: Vec<Output>,
}

/// One picture on its way.
struct Pending {
    reply: Sender<Option<Picture>>,
    /// The buffer the compositor copies into, and the file behind it.
    buffer: Option<(wl_buffer::WlBuffer, File, Picture)>,
    /// The compositor's rows run from the bottom up.
    upside_down: bool,
}

/// What the event thread dispatches into.
struct Client {
    shared: Arc<Mutex<Shared>>,
}

/// Asks for pictures. Cheap to clone; all clones ask the same backend.
#[derive(Clone, Default)]
pub struct Capturer {
    /// Absent where there is no compositor to ask.
    target: Option<(Connection, QueueHandle<Client>, Arc<Mutex<Shared>>)>,
}

impl Capturer {
    /// Starts the backend. Without a Wayland display every answer is
    /// `None`, as it is from a compositor without `wlr-screencopy`.
    pub fn spawn() -> Self {
        let connection = match Connection::connect_to_env() {
            Ok(connection) => connection,
            Err(error) => {
                eprintln!("gniri: capture: no Wayland connection: {error}");
                return Self::default();
            }
        };
        let shared = Arc::new(Mutex::new(Shared::default()));
        let mut queue = connection.new_event_queue();
        let handle = queue.handle();
        connection.display().get_registry(&handle, ());
        let thread = std::thread::Builder::new()
            .name("capture".to_owned())
            .spawn({
                let shared = Arc::clone(&shared);
                move || {
                    let mut client = Client { shared };
                    // Ends when the compositor goes away.
                    while queue.blocking_dispatch(&mut client).is_ok() {}
                }
            });
        if let Err(error) = thread {
            eprintln!("gniri: capture: cannot start a thread: {error}");
            return Self::default();
        }
        Self {
            target: Some((connection, handle, shared)),
        }
    }

    /// Asks for a picture of `rect` on the output whose connector is
    /// `output`. The answer is `None` if there is none to be had, and the
    /// channel closes unanswered if the compositor goes away first.
    pub fn capture(&self, output: &str, rect: Rect) -> Receiver<Option<Picture>> {
        let (reply, answer) = async_channel::bounded(1);
        let Some((connection, handle, shared)) = &self.target else {
            let _ = reply.try_send(None);
            return answer;
        };
        let shared = shared.lock().expect("no thread panics holding it");
        let found = shared
            .outputs
            .iter()
            .find(|known| known.name.as_deref() == Some(output));
        let (Some(manager), Some(_), Some(found)) = (&shared.manager, &shared.shm, found) else {
            let _ = reply.try_send(None);
            return answer;
        };
        manager.capture_output_region(
            // Without the pointer.
            0,
            &found.output,
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            handle,
            Mutex::new(Pending {
                reply,
                buffer: None,
                upside_down: false,
            }),
        );
        drop(shared);
        let _ = connection.flush();
        answer
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for Client {
    fn event(
        client: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        (): &(),
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        let mut shared = client.shared.lock().expect("no thread panics holding it");
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => match interface.as_str() {
                "wl_shm" => shared.shm = Some(registry.bind(name, 1, queue, ())),
                // An output says its name from version 4 on.
                "wl_output" if version >= 4 => {
                    let output = registry.bind(name, 4, queue, name);
                    shared.outputs.push(Output {
                        global: name,
                        output,
                        name: None,
                    });
                }
                // Version 1 has one kind of buffer, which is all that is
                // wanted here.
                "zwlr_screencopy_manager_v1" => {
                    shared.manager = Some(registry.bind(name, 1, queue, ()));
                }
                _ => {}
            },
            wl_registry::Event::GlobalRemove { name } => {
                shared.outputs.retain(|output| {
                    if output.global == name {
                        output.output.release();
                    }
                    output.global != name
                });
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, u32> for Client {
    fn event(
        client: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        global: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event {
            let mut shared = client.shared.lock().expect("no thread panics holding it");
            if let Some(output) = shared
                .outputs
                .iter_mut()
                .find(|output| output.global == *global)
            {
                output.name = Some(name);
            }
        }
    }
}

/// A file of `size` bytes with no name, for the compositor to write into.
fn buffer_file(size: u64) -> std::io::Result<File> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let directory = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let path = directory.join(format!(
        "gniri-capture-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)?;
    // Only the descriptor is needed; the name can go at once.
    std::fs::remove_file(&path)?;
    file.set_len(size)?;
    Ok(file)
}

/// The rows of `pixels` in the opposite order.
fn flipped(pixels: &[u8], stride: usize) -> Vec<u8> {
    pixels
        .chunks_exact(stride)
        .rev()
        .flatten()
        .copied()
        .collect()
}

impl Dispatch<ZwlrScreencopyFrameV1, Mutex<Pending>> for Client {
    fn event(
        client: &mut Self,
        frame: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        pending: &Mutex<Pending>,
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        let mut pending = pending.lock().expect("no thread panics holding it");
        // Answers once, and lets go of the frame and its buffer.
        let finish = |pending: &mut Pending, picture: Option<Picture>| {
            let _ = pending.reply.try_send(picture);
            if let Some((buffer, _, _)) = pending.buffer.take() {
                buffer.destroy();
            }
            frame.destroy();
        };
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                // What is on screen is opaque: a byte of alpha says no
                // more than one of padding.
                let known = match format {
                    WEnum::Value(
                        format @ (wl_shm::Format::Xrgb8888 | wl_shm::Format::Argb8888),
                    ) => Some((format, Format::Bgrx)),
                    WEnum::Value(
                        format @ (wl_shm::Format::Xbgr8888 | wl_shm::Format::Abgr8888),
                    ) => Some((format, Format::Rgbx)),
                    _ => None,
                };
                let size = u64::from(stride) * u64::from(height);
                let shm = client
                    .shared
                    .lock()
                    .expect("no thread panics holding it")
                    .shm
                    .clone();
                let made = known.zip(shm).and_then(|((format, ours), shm)| {
                    // The pool's size is a signed 32-bit number.
                    let length = i32::try_from(size).ok().filter(|length| *length > 0)?;
                    let file = buffer_file(size)
                        .inspect_err(|error| {
                            eprintln!("gniri: capture: cannot make a buffer: {error}");
                        })
                        .ok()?;
                    let pool = shm.create_pool(file.as_fd(), length, queue, ());
                    let buffer = pool.create_buffer(
                        0,
                        width as i32,
                        height as i32,
                        stride as i32,
                        format,
                        queue,
                        (),
                    );
                    pool.destroy();
                    let picture = Picture {
                        width,
                        height,
                        stride,
                        format: ours,
                        pixels: Vec::new(),
                    };
                    Some((buffer, file, picture))
                });
                match made {
                    Some(made) => {
                        frame.copy(&made.0);
                        pending.buffer = Some(made);
                    }
                    None => finish(&mut pending, None),
                }
            }
            zwlr_screencopy_frame_v1::Event::Flags {
                flags: WEnum::Value(flags),
            } => {
                pending.upside_down = flags.contains(zwlr_screencopy_frame_v1::Flags::YInvert);
            }
            zwlr_screencopy_frame_v1::Event::Ready { .. } => {
                let upside_down = pending.upside_down;
                let picture = pending.buffer.as_mut().and_then(|(_, file, picture)| {
                    let mut pixels = vec![0; picture.stride as usize * picture.height as usize];
                    file.read_exact_at(&mut pixels, 0).ok()?;
                    if upside_down {
                        pixels = flipped(&pixels, picture.stride as usize);
                    }
                    picture.pixels = pixels;
                    Some(picture.clone())
                });
                finish(&mut pending, picture);
            }
            zwlr_screencopy_frame_v1::Event::Failed => finish(&mut pending, None),
            _ => {}
        }
    }
}

// Nothing these say matters here.
macro_rules! ignore {
    ($($proxy:ty),*) => {$(
        impl Dispatch<$proxy, ()> for Client {
            fn event(
                _: &mut Self,
                _: &$proxy,
                _: <$proxy as wayland_client::Proxy>::Event,
                (): &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
    )*};
}
ignore!(
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    ZwlrScreencopyManagerV1
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_is_a_whole_number_of_device_pixels() {
        assert_eq!(whole_step(1.0), 1);
        assert_eq!(whole_step(2.0), 1);
        assert_eq!(whole_step(1.5), 2);
        assert_eq!(whole_step(1.25), 4);
        assert_eq!(whole_step(1.75), 4);
        // 132/120.
        assert_eq!(whole_step(1.1), 10);
        for scale in [1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5] {
            let pixels = f64::from(whole_step(scale)) * scale;
            assert!((pixels - pixels.round()).abs() < 1e-9, "{scale}");
        }
    }

    #[test]
    fn a_rectangle_grows_to_the_steps_around_it() {
        let rect = Rect {
            x: 1301,
            y: 38,
            width: 485,
            height: 467,
        };
        assert_eq!(
            outward(rect, 4),
            Rect {
                x: 1300,
                y: 36,
                width: 488,
                height: 472,
            }
        );
        // One that is on the steps already stays as it is.
        assert_eq!(outward(outward(rect, 4), 4), outward(rect, 4));
        assert_eq!(outward(rect, 1), rect);
        // Left of and above the surface's corner too.
        let rect = Rect {
            x: -3,
            y: -8,
            width: 10,
            height: 8,
        };
        assert_eq!(
            outward(rect, 4),
            Rect {
                x: -4,
                y: -8,
                width: 12,
                height: 8,
            }
        );
    }

    #[test]
    fn the_middle_is_found_as_the_compositor_finds_it() {
        assert_eq!(centred(1792, 500), 646);
        // Each half rounded down: not (1792 - 515) / 2, which is 638.
        assert_eq!(centred(1792, 515), 639);
        assert_eq!(centred(1281, 515), 383);
    }

    #[test]
    fn rows_from_the_bottom_up_are_turned_over() {
        assert_eq!(flipped(&[1, 2, 3, 4, 5, 6], 2), [5, 6, 3, 4, 1, 2]);
    }

    #[test]
    fn nothing_to_ask_answers_none() {
        let answer = Capturer::default().capture(
            "eDP-1",
            Rect {
                x: 0,
                y: 0,
                width: 8,
                height: 8,
            },
        );
        assert_eq!(answer.try_recv(), Ok(None));
    }
}
