// SPDX-License-Identifier: GPL-3.0-or-later

//! The quick settings toggles that are a single on/off value: dark style,
//! night light and do not disturb.
//!
//! Their backends are small and live with what they act on (the shell's
//! `GSettings` for dark style, the Wayland gamma protocol for night light,
//! the notification server for do not disturb), so each is only a model
//! here.

use serde::Deserialize;

use crate::model::Model;

/// One on/off setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct State {
    /// The setting exists on this system. GNOME hides a toggle that would
    /// do nothing.
    pub available: bool,
    pub on: bool,
}

/// What the backend reports.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    State(State),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Set(bool),
}

impl Model for State {
    type Event = Event;
    type Command = Command;

    fn apply(&mut self, event: Event) {
        match event {
            Event::State(state) => *self = state,
        }
    }

    fn predict(&mut self, command: &Command) {
        match command {
            Command::Set(on) => {
                if self.available {
                    self.on = *on;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_available_setting_changes() {
        let mut state = State::default();
        state.predict(&Command::Set(true));
        assert!(!state.on);
        state.apply(Event::State(State {
            available: true,
            on: false,
        }));
        state.predict(&Command::Set(true));
        assert!(state.on);
    }
}
