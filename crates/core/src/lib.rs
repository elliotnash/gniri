//! Shared building blocks for gniri that depend on no toolkit.

pub mod store;

pub use store::{ImmediateScheduler, ManualScheduler, Scheduler, Store, Subscription};
