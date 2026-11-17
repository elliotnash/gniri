// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared building blocks for gniri that depend on no toolkit.

pub mod calendar;
pub mod clock;
pub mod i18n;
pub mod store;

pub use calendar::{Date, GridDay, event_time_text, month_grid, offset_text, time_span_text};
pub use clock::{
    Clock, ClockFormat, FixedClock, LocalTime, Weekday, lock_clock_date, lock_clock_time,
    panel_clock_text,
};
pub use i18n::{fill, tr, tr_in, tr_n};
pub use store::{ImmediateScheduler, ManualScheduler, Scheduler, Store, Subscription};
