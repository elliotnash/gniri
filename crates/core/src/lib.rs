//! Shared building blocks for gniri that depend on no toolkit.

pub mod clock;
pub mod store;

pub use clock::{Clock, ClockFormat, FixedClock, LocalTime, Weekday, panel_clock_text};
pub use store::{ImmediateScheduler, ManualScheduler, Scheduler, Store, Subscription};
