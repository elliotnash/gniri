//! gniri's backends: one module per system service.
//!
//! Each module owns a plain state struct that the shell keeps in a
//! `gniri_core::Store`, the pure logic that derives what widgets show from
//! it, and (where it exists yet) the backend that talks to the system. No
//! GTK here: backends hand events to the main loop over a channel.

pub mod audio;
pub mod fixture;
pub mod keyboard;
pub mod network;
pub mod niri;
pub mod power;

pub use fixture::Fixture;
