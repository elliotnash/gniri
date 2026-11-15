// SPDX-License-Identifier: GPL-3.0-or-later

//! The shape every service shares.
//!
//! A service is a plain state struct plus two kinds of message:
//!
//! - an **event** is the backend reporting what the system now looks like;
//! - a **command** is the shell asking the system to change.
//!
//! The backend turns commands into calls on the system and its replies into
//! events. Both directions are pure functions on the state here, so they can
//! be tested without a system: [`Model::apply`] folds an event in, and
//! [`Model::predict`] says what a command is expected to do. The shell uses
//! the prediction to respond at once (a slider must not wait for a round
//! trip) and, under a fixture, as the whole backend.

use std::fmt::Debug;

pub trait Model: Clone + PartialEq + 'static {
    type Event: Debug + Send + 'static;
    type Command: Clone + Debug + Send + 'static;

    /// Folds in what the backend reported.
    fn apply(&mut self, event: Self::Event);

    /// Changes the state the way `command` is expected to change the system.
    /// The backend's next event overwrites the guess.
    fn predict(&mut self, command: &Self::Command);
}

/// The two ends the shell holds of a running backend.
pub struct Link<M: Model> {
    pub events: async_channel::Receiver<M::Event>,
    pub commands: async_channel::Sender<M::Command>,
}

impl<M: Model> Link<M> {
    /// A link and the ends the backend holds.
    pub fn new() -> (
        Self,
        async_channel::Sender<M::Event>,
        async_channel::Receiver<M::Command>,
    ) {
        let (event_sender, events) = async_channel::unbounded();
        let (commands, command_receiver) = async_channel::unbounded();
        (Self { events, commands }, event_sender, command_receiver)
    }
}
