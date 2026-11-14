//! Airplane mode: every radio switched off at once.
//!
//! [`backend`] reads and writes the kernel's `/dev/rfkill`.

pub mod backend;

use serde::Deserialize;

use crate::model::Model;

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct State {
    /// The machine has radios that can be switched off. Without any, GNOME
    /// hides the toggle.
    pub available: bool,
    /// Every radio is blocked.
    pub airplane_mode: bool,
    /// A hardware switch blocks the radios; software cannot turn them on.
    pub hardware_blocked: bool,
}

/// What the backend reports.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Everything, as it is now.
    State(State),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    SetAirplaneMode(bool),
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
            Command::SetAirplaneMode(on) => {
                if self.available && (*on || !self.hardware_blocked) {
                    self.airplane_mode = *on;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hardware_switch_cannot_be_overridden() {
        let mut state = State {
            available: true,
            airplane_mode: true,
            hardware_blocked: true,
        };
        state.predict(&Command::SetAirplaneMode(false));
        assert!(state.airplane_mode);
        state.hardware_blocked = false;
        state.predict(&Command::SetAirplaneMode(false));
        assert!(!state.airplane_mode);

        let mut none = State::default();
        none.predict(&Command::SetAirplaneMode(true));
        assert!(!none.airplane_mode);
    }
}
