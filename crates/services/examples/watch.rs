//! Prints what a backend reports from the real system, for development:
//!
//! ```text
//! cargo run -p gniri-services --example watch -- power
//! ```
//!
//! Services: power, network, bluetooth, brightness, audio, rfkill, mpris,
//! session, idle.
//!
//! It only listens; it never sends a command. `session` shows what logind
//! would let this session do and never locks: a request to lock is only
//! printed. `idle` reports five and ten seconds without input.

use std::time::Duration;

use gniri_services::session::locker::{Locker, Report};
use gniri_services::{Bus, Link, Model};
use gniri_services::{
    audio, bluetooth, brightness, idle, logind, mpris, network, power, rfkill, session,
};

/// A locker that locks nothing.
struct Printer;

impl Locker for Printer {
    fn start(&self, _reports: async_channel::Sender<Report>) -> std::io::Result<()> {
        println!("the screen locker would start now");
        Ok(())
    }
}

fn print<M: Model + std::fmt::Debug + Default>(link: Link<M>) {
    let mut state = M::default();
    while let Ok(event) = link.events.recv_blocking() {
        println!("event: {event:?}");
        state.apply(event);
        println!("state: {state:#?}");
    }
}

fn main() {
    let service = std::env::args().nth(1).unwrap_or_default();
    let mut bus = Bus::system();
    // Starts the bus thread once the chosen backend is on it.
    macro_rules! on_bus {
        ($module:ident) => {{
            let link = bus.add::<$module::State, _, _>(stringify!($module), $module::backend::run);
            bus.start();
            print(link);
        }};
    }
    match service.as_str() {
        "power" => on_bus!(power),
        "network" => on_bus!(network),
        "bluetooth" => on_bus!(bluetooth),
        "brightness" => on_bus!(brightness),
        "audio" => print(audio::backend::spawn()),
        "rfkill" => print(rfkill::backend::spawn()),
        "mpris" => {
            let mut bus = Bus::session();
            let link = bus.add::<mpris::State, _, _>("mpris", mpris::backend::run);
            bus.start();
            print(link);
        }
        "session" => {
            let link =
                bus.add::<session::State, _, _>("session", |connection, events, commands| {
                    session::backend::run(
                        connection,
                        events,
                        commands,
                        Box::new(Printer),
                        session::backend::Options {
                            session_id: logind::session_id_from_env(),
                            // No inhibitor: this must not hold up a real
                            // suspend.
                            lock_on_suspend: false,
                        },
                    )
                });
            bus.start();
            print(link);
        }
        "idle" => print(idle::backend::spawn(vec![
            (idle::Timer::Dim, Duration::from_secs(5)),
            (idle::Timer::Blank, Duration::from_secs(10)),
        ])),
        _ => eprintln!(
            "usage: watch power|network|bluetooth|brightness|audio|rfkill|mpris|session|idle"
        ),
    }
}
