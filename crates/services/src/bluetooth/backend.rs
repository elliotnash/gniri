//! The BlueZ backend. Not written yet: it reports nothing.

use async_channel::{Receiver, Sender};

use super::{Command, Event};

pub async fn run(
    _connection: zbus::Connection,
    _events: Sender<Event>,
    _commands: Receiver<Command>,
) -> zbus::Result<()> {
    Ok(())
}
