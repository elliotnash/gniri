// SPDX-License-Identifier: GPL-3.0-or-later

//! Airplane mode: every radio switched off at once.
//!
//! [`backend`] reads and writes the kernel's `/dev/rfkill`. GNOME has
//! gnome-settings-daemon's rfkill plugin in between; each field here means
//! what the plugin's D-Bus property of the same purpose means, as far as
//! `status/rfkill.js` and `status/bluetooth.js` read them.

pub mod backend;

use serde::Deserialize;

use crate::model::Model;

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct State {
    /// The Airplane Mode toggle has something to do: the machine has kill
    /// switches and is not a desktop or a server, where GNOME hides it
    /// (`HasAirplaneMode && ShouldShowAirplaneMode`).
    pub available: bool,
    /// No radio is left unblocked (`AirplaneMode`).
    pub airplane_mode: bool,
    /// A hardware switch blocks every radio; software cannot turn them on
    /// (`HardwareAirplaneMode`).
    pub hardware_blocked: bool,
    /// Bluetooth has a kill switch (`BluetoothHasAirplaneMode`).
    pub has_bluetooth: bool,
    /// A hardware switch blocks every Bluetooth radio
    /// (`BluetoothHardwareAirplaneMode`).
    pub bluetooth_hardware_blocked: bool,
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

impl State {
    /// What clicking the toggle asks for. Port of the `clicked` handler of
    /// `RfkillToggle` in `status/rfkill.js`.
    pub fn toggled(&self) -> Command {
        Command::SetAirplaneMode(!self.airplane_mode)
    }

    /// GNOME shows the panel icon only while airplane mode is on. Port of
    /// `Indicator._sync` in `status/rfkill.js`.
    pub fn shows_indicator(&self) -> bool {
        self.available && self.airplane_mode
    }

    /// Whether GNOME shows the Bluetooth toggle: only with a kill switch
    /// that software can get out of. Port of `BtClient.available` in
    /// `status/bluetooth.js`.
    pub fn bluetooth_available(&self) -> bool {
        self.has_bluetooth && !self.bluetooth_hardware_blocked
    }
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
            ..State::default()
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

    #[test]
    fn the_toggle_flips_airplane_mode() {
        let mut state = State {
            available: true,
            ..State::default()
        };
        assert_eq!(state.toggled(), Command::SetAirplaneMode(true));
        state.predict(&state.toggled());
        assert!(state.airplane_mode);
        assert_eq!(state.toggled(), Command::SetAirplaneMode(false));
    }

    #[test]
    fn the_indicator_shows_only_in_airplane_mode_where_the_toggle_shows() {
        let mut state = State {
            available: true,
            ..State::default()
        };
        assert!(!state.shows_indicator());
        state.airplane_mode = true;
        assert!(state.shows_indicator());
        // A desktop with its radios blocked: GNOME shows neither.
        state.available = false;
        assert!(!state.shows_indicator());
    }

    #[test]
    fn bluetooth_needs_a_switch_that_software_controls() {
        let mut state = State::default();
        assert!(!state.bluetooth_available());
        state.has_bluetooth = true;
        assert!(state.bluetooth_available());
        state.bluetooth_hardware_blocked = true;
        assert!(!state.bluetooth_available());
    }
}
