//! gniri's backends: one module per system service.
//!
//! Each module owns a plain state struct that the shell keeps in a
//! `gniri_core::Store`, the pure logic that derives what widgets show from
//! it, and (where it exists yet) the backend that talks to the system. No
//! GTK here: backends hand events to the main loop over a channel.

pub mod audio;
pub mod bluetooth;
pub mod brightness;
pub mod fixture;
pub mod keyboard;
pub mod model;
pub mod network;
pub mod night_light;
pub mod niri;
pub mod power;
pub mod rfkill;
pub mod session;
pub mod system_bus;
pub mod toggles;

pub use fixture::Fixture;
pub use model::{Link, Model};
pub use system_bus::SystemBus;
