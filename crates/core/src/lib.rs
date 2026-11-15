//! Shared building blocks for gniri that depend on no toolkit.

pub mod calendar;
pub mod clock;
pub mod store;

pub use calendar::{Date, GridDay, month_grid, time_span_text};
pub use clock::{
    Clock, ClockFormat, FixedClock, LocalTime, Weekday, lock_clock_date, lock_clock_time,
    panel_clock_text,
};
pub use store::{ImmediateScheduler, ManualScheduler, Scheduler, Store, Subscription};
