//! The login session: suspending, restarting, powering off and locking,
//! through logind.
//!
//! Locking has three parts. Asking for it is a call to logind
//! ([`Command::Lock`]), which answers with a signal to whoever locks this
//! session's screen. That is the backend here: it starts the screen locker,
//! a process of its own ([`locker`]), and follows it. And before the machine
//! sleeps the backend holds logind back until the locker has covered the
//! screen, so that waking up never shows the desktop.

pub mod backend;
pub mod locker;

use serde::Deserialize;

use crate::model::Model;

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct State {
    /// logind would let this session suspend the machine.
    pub can_suspend: bool,
    pub can_restart: bool,
    pub can_power_off: bool,
    /// The screen locker has the screen.
    pub locked: bool,
}

/// What the backend reports.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// What logind would let this session do.
    Abilities {
        suspend: bool,
        restart: bool,
        power_off: bool,
    },
    Locked(bool),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Suspend,
    Restart,
    PowerOff,
    /// Ask logind to lock this session. The backend hears logind's `Lock`
    /// signal like any other screen locker would and starts the locker.
    Lock,
}

impl Model for State {
    type Event = Event;
    type Command = Command;

    fn apply(&mut self, event: Event) {
        match event {
            Event::Abilities {
                suspend,
                restart,
                power_off,
            } => {
                self.can_suspend = suspend;
                self.can_restart = restart;
                self.can_power_off = power_off;
            }
            Event::Locked(locked) => self.locked = locked,
        }
    }

    /// No command changes anything the shell shows: `locked` only turns
    /// true once the locker says the screen is covered.
    fn predict(&mut self, _command: &Command) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_fold_in_and_commands_predict_nothing() {
        let mut state = State::default();
        state.apply(Event::Abilities {
            suspend: true,
            restart: false,
            power_off: true,
        });
        state.apply(Event::Locked(true));
        assert_eq!(
            state,
            State {
                can_suspend: true,
                can_restart: false,
                can_power_off: true,
                locked: true,
            }
        );
        let before = state.clone();
        state.predict(&Command::Lock);
        state.predict(&Command::PowerOff);
        assert_eq!(state, before);
    }
}
