//! The login session: suspending and locking, through logind.
//!
//! Restart and power off are not here yet: GNOME asks first, and the dialog
//! that asks arrives with the session milestone (M4).

pub mod backend;

use serde::Deserialize;

use crate::model::Model;

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct State {
    /// logind would let this session suspend the machine.
    pub can_suspend: bool,
}

/// What the backend reports.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    State(State),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Suspend,
    /// Ask logind to lock this session. Whatever screen locker listens for
    /// logind's `Lock` signal does the locking.
    Lock,
}

impl Model for State {
    type Event = Event;
    type Command = Command;

    fn apply(&mut self, event: Event) {
        match event {
            Event::State(state) => *self = state,
        }
    }

    /// Neither command changes anything the shell shows.
    fn predict(&mut self, _command: &Command) {}
}
