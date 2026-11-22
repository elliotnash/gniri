// SPDX-License-Identifier: GPL-3.0-or-later

//! One wlr-screencopy capture of a part of the first output, without the
//! pointer, written as raw pixels.
//!
//!     gniri-spike-shot X Y WIDTH HEIGHT FILE [COUNT INTERVAL_MS]
//!
//! The rectangle is in logical pixels. Prints one line per capture:
//! `FILE WIDTH HEIGHT STRIDE FORMAT MILLISECONDS`, the size in physical
//! pixels and the format a `wl_shm` name (`Xrgb8888` is B, G, R, X in
//! memory). With COUNT, FILE gets `.0`, `.1`... appended, one capture
//! every INTERVAL_MS.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsFd;
use std::time::{Duration, Instant};

use wayland_client::protocol::{wl_buffer, wl_output, wl_registry, wl_shm, wl_shm_pool};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};

#[derive(Default)]
struct Client {
    shm: Option<wl_shm::WlShm>,
    output: Option<wl_output::WlOutput>,
    manager: Option<ZwlrScreencopyManagerV1>,
    /// What the compositor wants the buffer to be.
    wanted: Option<(wl_shm::Format, u32, u32, u32)>,
    /// The buffer's file once it was made.
    file: Option<File>,
    buffer: Option<wl_buffer::WlBuffer>,
    done: bool,
    failed: bool,
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
        if let wl_registry::Event::Global {
            name, interface, ..
        } = event
        {
            match interface.as_str() {
                "wl_shm" => client.shm = Some(registry.bind(name, 1, queue, ())),
                "wl_output" if client.output.is_none() => {
                    client.output = Some(registry.bind(name, 1, queue, ()));
                }
                "zwlr_screencopy_manager_v1" => {
                    client.manager = Some(registry.bind(name, 1, queue, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, ()> for Client {
    fn event(
        client: &mut Self,
        frame: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        (): &(),
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        match event {
            // Version 1: the one buffer event is all there is to wait for.
            zwlr_screencopy_frame_v1::Event::Buffer {
                format: WEnum::Value(format),
                width,
                height,
                stride,
            } => {
                client.wanted = Some((format, width, height, stride));
                let size = (stride * height) as u64;
                let dir = std::env::var("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
                let path = format!("{dir}/gniri-spike-shot-{}", std::process::id());
                let file = File::options()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(&path)
                    .expect("making the buffer's file");
                std::fs::remove_file(&path).ok();
                file.set_len(size).expect("sizing the buffer's file");
                let shm = client.shm.as_ref().expect("wl_shm");
                let pool = shm.create_pool(file.as_fd(), size as i32, queue, ());
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
                frame.copy(&buffer);
                client.file = Some(file);
                client.buffer = Some(buffer);
            }
            zwlr_screencopy_frame_v1::Event::Ready { .. } => client.done = true,
            zwlr_screencopy_frame_v1::Event::Failed => {
                client.done = true;
                client.failed = true;
            }
            _ => {}
        }
    }
}

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
    wl_output::WlOutput,
    ZwlrScreencopyManagerV1
);

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 5 && args.len() != 7 {
        eprintln!("usage: gniri-spike-shot X Y WIDTH HEIGHT FILE [COUNT INTERVAL_MS]");
        std::process::exit(2);
    }
    let number = |index: usize| -> i32 { args[index].parse().expect("a number") };
    let (x, y, width, height) = (number(0), number(1), number(2), number(3));
    let (count, interval) = if args.len() == 7 {
        (number(5), Duration::from_millis(number(6) as u64))
    } else {
        (1, Duration::ZERO)
    };

    let connection = Connection::connect_to_env().expect("a Wayland compositor");
    let mut queue = connection.new_event_queue();
    let handle = queue.handle();
    connection.display().get_registry(&handle, ());
    let mut client = Client::default();
    queue.roundtrip(&mut client).expect("the registry");

    let start = Instant::now();
    for index in 0..count {
        let due = start + interval * index as u32;
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
        let began = Instant::now();
        client.done = false;
        let frame = client
            .manager
            .as_ref()
            .expect("zwlr_screencopy_manager_v1")
            .capture_output_region(
                0,
                client.output.as_ref().expect("an output"),
                x,
                y,
                width,
                height,
                &handle,
                (),
            );
        while !client.done {
            queue.blocking_dispatch(&mut client).expect("the capture");
        }
        frame.destroy();
        if client.failed {
            eprintln!("the compositor failed the capture");
            std::process::exit(1);
        }

        let (format, width, height, stride) = client.wanted.expect("a buffer event");
        let mut pixels = vec![0; (stride * height) as usize];
        let mut file = client.file.take().expect("a buffer");
        file.seek(SeekFrom::Start(0)).expect("seeking");
        file.read_exact(&mut pixels).expect("reading the capture");
        if let Some(buffer) = client.buffer.take() {
            buffer.destroy();
        }

        let path = if count == 1 {
            args[4].clone()
        } else {
            format!("{}.{index}", args[4])
        };
        // Written beside and renamed, so a reader never sees half of it.
        let partial = format!("{path}.part");
        File::create(&partial)
            .and_then(|mut out| out.write_all(&pixels))
            .and_then(|()| std::fs::rename(&partial, &path))
            .expect("writing the capture");
        println!(
            "{path} {width} {height} {stride} {format:?} {:.1}",
            began.elapsed().as_secs_f64() * 1000.0
        );
    }
}
