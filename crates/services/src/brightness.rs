// SPDX-License-Identifier: GPL-3.0-or-later

//! Screen brightness.
//!
//! [`backend`] reads the kernel's backlight device and sets it through
//! logind, which needs no special permissions.

pub mod backend;

use serde::Deserialize;

use crate::model::Model;

/// How far one press of a brightness key moves the level
/// (gnome-settings-daemon's `BRIGHTNESS_STEP_AMOUNT`, a twentieth).
pub const STEP: f64 = 0.05;

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct State {
    /// 0.0 to 1.0, or `None` when the screen's backlight cannot be set
    /// (a desktop monitor): GNOME hides the slider then.
    pub level: Option<f64>,
}

/// What the backend reports.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Level(Option<f64>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Set(f64),
    /// Read the level again: something other than gniri may have changed it.
    Refresh,
}

impl State {
    /// The command a brightness key sends, or `None` without a backlight.
    pub fn step(&self, steps: i32) -> Option<Command> {
        let level = self.level?;
        Some(Command::Set(
            (level + f64::from(steps) * STEP).clamp(0.0, 1.0),
        ))
    }
}

impl Model for State {
    type Event = Event;
    type Command = Command;

    fn apply(&mut self, event: Event) {
        match event {
            Event::Level(level) => self.level = level,
        }
    }

    fn predict(&mut self, command: &Command) {
        match command {
            Command::Set(level) => {
                if self.level.is_some() {
                    self.level = Some(level.clamp(0.0, 1.0));
                }
            }
            Command::Refresh => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_step_by_a_twentieth_and_stop_at_the_ends() {
        let mut state = State { level: Some(0.98) };
        assert_eq!(state.step(1), Some(Command::Set(1.0)));
        state.predict(&Command::Set(0.02));
        assert_eq!(state.step(-1), Some(Command::Set(0.0)));
        state.predict(&Command::Set(7.0));
        assert_eq!(state.level, Some(1.0));

        let mut none = State::default();
        assert_eq!(none.step(1), None);
        none.predict(&Command::Set(0.5));
        assert_eq!(none.level, None);
    }
}
