//! Prints what a backend reports from the real system, for development:
//!
//! ```text
//! cargo run -p gniri-services --example watch -- power
//! ```
//!
//! Services: power, network, bluetooth, brightness, audio, rfkill, mpris.
//!
//! It only listens; it never sends a command.

use gniri_services::{Bus, Link, Model};
use gniri_services::{audio, bluetooth, brightness, mpris, network, power, rfkill};

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
        _ => eprintln!("usage: watch power|network|bluetooth|brightness|audio|rfkill|mpris"),
    }
}
