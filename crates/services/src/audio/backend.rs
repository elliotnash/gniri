//! The PulseAudio backend. Not written yet: it reports nothing.

use super::State;
use crate::model::Link;

/// Starts the backend on its own thread.
pub fn spawn() -> Link<State> {
    let (link, _events, _commands) = Link::new();
    link
}
