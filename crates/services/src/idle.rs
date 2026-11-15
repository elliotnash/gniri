//! How long the user has been away, and what follows from it: the screen
//! dims, then turns off, and the session locks.
//!
//! The compositor does the watching (`ext-idle-notify`): the backend asks it
//! to say when there has been no input for each of a few durations, and
//! when input came back. A video player or a call holds the timers off by
//! inhibiting idleness, which the compositor accounts for.
//!
//! The durations follow GNOME: gnome-settings-daemon's power plugin dims at
//! four fifths of the "Blank Screen" delay and turns the screens off at the
//! delay itself, and GNOME Shell locks when they turn off, or some time
//! after. While the screen is locked the screens turn off much sooner.

pub mod backend;

use std::time::Duration;

use serde::Deserialize;

use crate::model::Model;

/// One point in the idle time at which something happens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Timer {
    /// The screen dims.
    Dim,
    /// The screens turn off.
    Blank,
    /// The session locks.
    Lock,
    /// The screens turn off while the lock screen shows.
    BlankLocked,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct State {
    /// The compositor can say when the user is idle.
    pub available: bool,
    /// The timers the idle time has passed, in order. Empty while the user
    /// is there.
    pub passed: Vec<Timer>,
}

impl State {
    pub fn has_passed(&self, timer: Timer) -> bool {
        self.passed.contains(&timer)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Available(bool),
    /// No input for as long as this timer is.
    Idle(Timer),
    /// Input again, after this timer had passed.
    Resumed(Timer),
}

/// Nothing can be asked of it.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {}

impl Model for State {
    type Event = Event;
    type Command = Command;

    fn apply(&mut self, event: Event) {
        match event {
            Event::Available(available) => {
                self.available = available;
                if !available {
                    self.passed.clear();
                }
            }
            Event::Idle(timer) => {
                if !self.has_passed(timer) {
                    self.passed.push(timer);
                    self.passed.sort_unstable();
                }
            }
            Event::Resumed(timer) => self.passed.retain(|passed| *passed != timer),
        }
    }

    fn predict(&mut self, command: &Command) {
        match *command {}
    }
}

/// What the user chose (the config file's `[idle]` and `[lock]`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Idle time after which the screens turn off. `None` for never.
    pub blank_after: Option<Duration>,
    /// Dim the screen for the last part of that time.
    pub dim: bool,
    /// Lock when the screens turn off for idleness.
    pub lock: bool,
    /// How long after that.
    pub lock_delay: Duration,
}

/// gsd-power.c `SCREENSAVER_TIMEOUT_BLANK`: the screens turn off this long
/// after the last input while the screen is locked.
pub const BLANK_LOCKED_AFTER: Duration = Duration::from_secs(15);
/// gsd-power.c `IDLE_DELAY_TO_IDLE_DIM_MULTIPLIER`.
const DIM_SHARE: (u32, u32) = (4, 5);
/// gsd-power.c `MINIMUM_IDLE_DIM_DELAY`: with a shorter delay the screen
/// is not dimmed first.
const MINIMUM_DIM_DELAY: Duration = Duration::from_secs(10);
/// The `idle-brightness` setting's default: the share of full brightness
/// a dimmed screen has. A screen that is darker already is left alone.
pub const DIM_BRIGHTNESS: f64 = 0.3;

/// The timers to ask the compositor for. Port of `idle_configure` in
/// gnome-settings-daemon's `plugins/power/gsd-power-manager.c`.
pub fn timers(settings: &Settings) -> Vec<(Timer, Duration)> {
    let mut timers = vec![(Timer::BlankLocked, BLANK_LOCKED_AFTER)];
    let Some(blank_after) = settings.blank_after else {
        return timers;
    };
    let dim_after = blank_after * DIM_SHARE.0 / DIM_SHARE.1;
    if settings.dim && dim_after >= MINIMUM_DIM_DELAY {
        timers.push((Timer::Dim, dim_after));
    }
    timers.push((Timer::Blank, blank_after));
    if settings.lock {
        timers.push((Timer::Lock, blank_after + settings.lock_delay));
    }
    timers.sort_by_key(|(timer, after)| (*after, *timer));
    timers
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(blank_after: u64) -> Settings {
        Settings {
            blank_after: Some(Duration::from_secs(blank_after)),
            dim: true,
            lock: true,
            lock_delay: Duration::ZERO,
        }
    }

    fn seconds(timers: &[(Timer, Duration)]) -> Vec<(Timer, u64)> {
        timers
            .iter()
            .map(|(timer, after)| (*timer, after.as_secs()))
            .collect()
    }

    #[test]
    fn gnome_defaults_dim_at_four_minutes_and_blank_and_lock_at_five() {
        assert_eq!(
            seconds(&timers(&settings(300))),
            [
                (Timer::BlankLocked, 15),
                (Timer::Dim, 240),
                (Timer::Blank, 300),
                (Timer::Lock, 300),
            ]
        );
    }

    #[test]
    fn settings_change_the_chain() {
        let delayed = Settings {
            lock_delay: Duration::from_secs(30),
            ..settings(60)
        };
        assert_eq!(
            seconds(&timers(&delayed)),
            [
                (Timer::BlankLocked, 15),
                (Timer::Dim, 48),
                (Timer::Blank, 60),
                (Timer::Lock, 90),
            ]
        );

        // Too short to dim first.
        assert_eq!(
            seconds(&timers(&settings(10))),
            [
                (Timer::Blank, 10),
                (Timer::Lock, 10),
                (Timer::BlankLocked, 15),
            ]
        );

        let plain = Settings {
            dim: false,
            lock: false,
            ..settings(300)
        };
        assert_eq!(
            seconds(&timers(&plain)),
            [(Timer::BlankLocked, 15), (Timer::Blank, 300)]
        );

        // Never blanking still blanks a locked screen.
        let never = Settings {
            blank_after: None,
            ..settings(0)
        };
        assert_eq!(seconds(&timers(&never)), [(Timer::BlankLocked, 15)]);
    }

    #[test]
    fn timers_pass_and_come_back() {
        let mut state = State::default();
        state.apply(Event::Available(true));
        state.apply(Event::Idle(Timer::Blank));
        state.apply(Event::Idle(Timer::Dim));
        state.apply(Event::Idle(Timer::Dim));
        assert_eq!(state.passed, [Timer::Dim, Timer::Blank]);
        assert!(state.has_passed(Timer::Dim));
        state.apply(Event::Resumed(Timer::Dim));
        assert_eq!(state.passed, [Timer::Blank]);
        state.apply(Event::Available(false));
        assert_eq!(state, State::default());
    }
}
