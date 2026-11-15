//! Night light through `wlr-gamma-control`, on a Wayland connection of its
//! own.
//!
//! Two threads share the connection. One reads the compositor's events:
//! outputs coming and going, the size of each output's gamma table, and a
//! control failing (another client owns the output's gamma, or the
//! compositor cannot set it, as when niri runs nested in a window). The
//! other waits for commands. Either may send a table, so what they share is
//! behind a lock.
//!
//! The compositor puts an output's own table back when the control that
//! changed it is destroyed, which is what happens when gniri exits.

use std::io::{Seek, Write};
use std::os::fd::AsFd;
use std::sync::{Arc, Mutex};

use async_channel::Sender;
use wayland_client::protocol::{wl_output::WlOutput, wl_registry};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_wlr::gamma_control::v1::client::{
    zwlr_gamma_control_manager_v1::ZwlrGammaControlManagerV1,
    zwlr_gamma_control_v1::{self, ZwlrGammaControlV1},
};

use super::{gamma_table, white_point};
use crate::model::Link;
use crate::toggles::{Command, Event, State};

struct Output {
    /// The registry's name for it.
    name: u32,
    output: WlOutput,
    control: Option<ZwlrGammaControlV1>,
    /// Entries per channel in its gamma table, once the compositor said.
    size: Option<u32>,
}

struct Shared {
    on: bool,
    white: [f64; 3],
    outputs: Vec<Output>,
    events: Sender<Event>,
}

impl Shared {
    /// Sends `output`'s table for the current setting.
    fn apply(&self, output: &Output) {
        let (Some(control), Some(size)) = (&output.control, output.size) else {
            return;
        };
        let white = if self.on { self.white } else { [1.0; 3] };
        match table_file(&gamma_table(size as usize, white)) {
            Ok(file) => control.set_gamma(file.as_fd()),
            Err(error) => eprintln!("gniri: night light: cannot write a gamma table: {error}"),
        }
    }

    fn report(&self) {
        let available = self
            .outputs
            .iter()
            .any(|output| output.control.is_some() && output.size.is_some());
        let _ = self.events.try_send(Event::State(State {
            available,
            on: self.on && available,
        }));
    }
}

/// The table as the protocol wants it: a file descriptor to read it from.
fn table_file(table: &[u16]) -> std::io::Result<std::fs::File> {
    let directory = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let path = directory.join(format!("gniri-gamma-{}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)?;
    // Only the descriptor is needed; the name can go at once.
    std::fs::remove_file(&path)?;
    let bytes: Vec<u8> = table.iter().flat_map(|entry| entry.to_ne_bytes()).collect();
    file.write_all(&bytes)?;
    file.rewind()?;
    Ok(file)
}

/// What the event thread dispatches into.
struct Client {
    shared: Arc<Mutex<Shared>>,
    manager: Option<ZwlrGammaControlManagerV1>,
}

impl Client {
    /// Asks for the gamma control of every output that has none yet.
    fn take_controls(&self, queue: &QueueHandle<Self>) {
        let Some(manager) = &self.manager else {
            return;
        };
        let mut shared = self.shared.lock().expect("no thread panics holding it");
        for output in &mut shared.outputs {
            if output.control.is_none() {
                output.control =
                    Some(manager.get_gamma_control(&output.output, queue, output.name));
            }
        }
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
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => match interface.as_str() {
                "wl_output" => {
                    let output = registry.bind::<WlOutput, _, _>(name, version.min(4), queue, ());
                    client
                        .shared
                        .lock()
                        .expect("no thread panics holding it")
                        .outputs
                        .push(Output {
                            name,
                            output,
                            control: None,
                            size: None,
                        });
                    client.take_controls(queue);
                }
                "zwlr_gamma_control_manager_v1" => {
                    client.manager = Some(registry.bind(name, 1, queue, ()));
                    client.take_controls(queue);
                }
                _ => {}
            },
            wl_registry::Event::GlobalRemove { name } => {
                let mut shared = client.shared.lock().expect("no thread panics holding it");
                if let Some(index) = shared.outputs.iter().position(|output| output.name == name) {
                    let output = shared.outputs.remove(index);
                    if let Some(control) = output.control {
                        control.destroy();
                    }
                    shared.report();
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrGammaControlV1, u32> for Client {
    fn event(
        client: &mut Self,
        control: &ZwlrGammaControlV1,
        event: zwlr_gamma_control_v1::Event,
        name: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut shared = client.shared.lock().expect("no thread panics holding it");
        let Some(index) = shared
            .outputs
            .iter()
            .position(|output| output.name == *name)
        else {
            return;
        };
        match event {
            zwlr_gamma_control_v1::Event::GammaSize { size } => {
                shared.outputs[index].size = Some(size);
                // An output that appears while night light is on gets it too.
                if shared.on {
                    shared.apply(&shared.outputs[index]);
                }
            }
            zwlr_gamma_control_v1::Event::Failed => {
                control.destroy();
                shared.outputs[index].control = None;
                shared.outputs[index].size = None;
            }
            _ => {}
        }
        shared.report();
    }
}

// Nothing an output or the manager says matters here.
impl Dispatch<WlOutput, ()> for Client {
    fn event(
        _: &mut Self,
        _: &WlOutput,
        _: <WlOutput as wayland_client::Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrGammaControlManagerV1, ()> for Client {
    fn event(
        _: &mut Self,
        _: &ZwlrGammaControlManagerV1,
        _: <ZwlrGammaControlManagerV1 as wayland_client::Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// Starts the backend. `temperature` is the colour temperature, in kelvin,
/// the screen has while night light is on.
///
/// Without a Wayland display, or on a compositor without gamma control,
/// the state stays "not available" and the shell hides the toggle.
pub fn spawn(temperature: u32) -> Link<State> {
    let (link, events, commands) = Link::<State>::new();
    let connection = match Connection::connect_to_env() {
        Ok(connection) => connection,
        Err(error) => {
            eprintln!("gniri: night light: no Wayland connection: {error}");
            return link;
        }
    };
    let shared = Arc::new(Mutex::new(Shared {
        on: false,
        white: white_point(temperature),
        outputs: Vec::new(),
        events,
    }));

    let reader = std::thread::Builder::new()
        .name("night-light".to_owned())
        .spawn({
            let (connection, shared) = (connection.clone(), Arc::clone(&shared));
            move || {
                let mut queue = connection.new_event_queue();
                let handle = queue.handle();
                connection.display().get_registry(&handle, ());
                let mut client = Client {
                    shared,
                    manager: None,
                };
                // Ends when the compositor goes away.
                while queue.blocking_dispatch(&mut client).is_ok() {}
            }
        });
    if let Err(error) = reader {
        eprintln!("gniri: night light: cannot start a thread: {error}");
        return link;
    }

    let writer = std::thread::Builder::new()
        .name("night-light-commands".to_owned())
        .spawn(move || {
            while let Ok(Command::Set(on)) = commands.recv_blocking() {
                let mut shared = shared.lock().expect("no thread panics holding it");
                shared.on = on;
                for output in &shared.outputs {
                    shared.apply(output);
                }
                shared.report();
                drop(shared);
                if connection.flush().is_err() {
                    break;
                }
            }
        });
    if let Err(error) = writer {
        eprintln!("gniri: night light: cannot start a thread: {error}");
    }
    link
}
