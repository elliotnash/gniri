// SPDX-License-Identifier: GPL-3.0-or-later

//! A pointer of the harness's own in the nested session, for what `wlrctl`
//! cannot do: hold a button down while the pointer moves, as a drag does,
//! and go to a place on the output rather than by an offset.
//!
//! It is a client of the nested compositor's `wlr-virtual-pointer`, the
//! protocol `wlrctl` uses too.

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};

/// linux/input-event-codes.h `BTN_LEFT`.
const BUTTON_LEFT: u32 = 0x110;

#[derive(Default)]
struct Globals {
    manager: Option<ZwlrVirtualPointerManagerV1>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for Globals {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
            && interface == "zwlr_virtual_pointer_manager_v1"
        {
            state.manager = Some(registry.bind(name, version.min(2), queue, ()));
        }
    }
}

delegate_noop!(Globals: ignore ZwlrVirtualPointerManagerV1);
delegate_noop!(Globals: ignore ZwlrVirtualPointerV1);

/// A pointer on an output of `size` logical pixels.
pub struct Pointer {
    connection: Connection,
    pointer: ZwlrVirtualPointerV1,
    size: (u32, u32),
    started: Instant,
    at: (f64, f64),
}

impl Pointer {
    /// Connects to the compositor at `wayland_display` (a socket name in
    /// `XDG_RUNTIME_DIR`), whose one output is `size` large.
    pub fn connect(wayland_display: &str, size: (u32, u32)) -> Result<Self> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("XDG_RUNTIME_DIR is not set"))?;
        let stream = UnixStream::connect(runtime.join(wayland_display))
            .with_context(|| format!("connecting to {wayland_display}"))?;
        let connection = Connection::from_socket(stream)?;
        let mut queue = connection.new_event_queue();
        let handle = queue.handle();
        connection.display().get_registry(&handle, ());
        let mut globals = Globals::default();
        queue.roundtrip(&mut globals)?;
        let manager = globals
            .manager
            .clone()
            .ok_or_else(|| anyhow!("the compositor offers no virtual pointer"))?;
        let pointer = manager.create_virtual_pointer(None, &handle, ());
        queue.roundtrip(&mut globals)?;
        Ok(Self {
            connection,
            pointer,
            size,
            started: Instant::now(),
            at: (0.0, 0.0),
        })
    }

    fn time(&self) -> u32 {
        self.started.elapsed().as_millis() as u32
    }

    fn flush(&self) -> Result<()> {
        self.pointer.frame();
        self.connection.flush().context("talking to the compositor")
    }

    /// Puts the pointer at `(x, y)`, in logical pixels of the output.
    pub fn move_to(&mut self, (x, y): (f64, f64)) -> Result<()> {
        let (width, height) = self.size;
        let clamp =
            |value: f64, extent: u32| value.round().clamp(0.0, f64::from(extent - 1)) as u32;
        self.pointer.motion_absolute(
            self.time(),
            clamp(x, width),
            clamp(y, height),
            width,
            height,
        );
        self.at = (x, y);
        self.flush()
    }

    pub fn press(&self) -> Result<()> {
        self.pointer.button(
            self.time(),
            BUTTON_LEFT,
            wayland_client::protocol::wl_pointer::ButtonState::Pressed,
        );
        self.flush()
    }

    pub fn release(&self) -> Result<()> {
        self.pointer.button(
            self.time(),
            BUTTON_LEFT,
            wayland_client::protocol::wl_pointer::ButtonState::Released,
        );
        self.flush()
    }

    /// Moves to `to` in small steps, as a hand would, a few milliseconds
    /// apart.
    pub fn glide_to(&mut self, to: (f64, f64)) -> Result<()> {
        let from = self.at;
        let distance = ((to.0 - from.0).powi(2) + (to.1 - from.1).powi(2)).sqrt();
        let steps = (distance / 12.0).ceil().max(1.0) as u32;
        for step in 1..=steps {
            let progress = f64::from(step) / f64::from(steps);
            self.move_to((
                from.0 + (to.0 - from.0) * progress,
                from.1 + (to.1 - from.1) * progress,
            ))?;
            std::thread::sleep(Duration::from_millis(8));
        }
        Ok(())
    }

    /// Presses at `from`, moves to `to`, rests there for `rest`, and lets
    /// go.
    pub fn drag(&mut self, from: (f64, f64), to: (f64, f64), rest: Duration) -> Result<()> {
        self.move_to(from)?;
        std::thread::sleep(Duration::from_millis(50));
        self.press()?;
        std::thread::sleep(Duration::from_millis(50));
        self.glide_to(to)?;
        std::thread::sleep(rest);
        self.release()
    }
}

impl Drop for Pointer {
    fn drop(&mut self) {
        self.pointer.destroy();
        let _ = self.connection.flush();
    }
}
