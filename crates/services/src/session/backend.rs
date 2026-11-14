//! logind, over the system bus.

use async_channel::{Receiver, Sender};

use super::{Command, Event, State};

#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait Manager {
    /// "yes", "challenge" (allowed after authentication), "no" or "na".
    fn can_suspend(&self) -> zbus::Result<String>;
    fn suspend(&self, interactive: bool) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.freedesktop.login1.Session",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1/session/auto"
)]
trait Session {
    fn lock(&self) -> zbus::Result<()>;
}

pub async fn run(
    connection: zbus::Connection,
    events: Sender<Event>,
    commands: Receiver<Command>,
) -> zbus::Result<()> {
    let manager = ManagerProxy::new(&connection).await?;
    let session = SessionProxy::new(&connection).await?;

    // GNOME offers Suspend for "challenge" too: logind then asks for
    // authentication itself.
    let can_suspend = matches!(
        manager.can_suspend().await.as_deref(),
        Ok("yes" | "challenge")
    );
    if events
        .send(Event::State(State { can_suspend }))
        .await
        .is_err()
    {
        return Ok(());
    }

    while let Ok(command) = commands.recv().await {
        let result = match command {
            Command::Suspend => manager.suspend(true).await,
            Command::Lock => session.lock().await,
        };
        if let Err(error) = result {
            eprintln!("gniri: session: {command:?} failed: {error}");
        }
    }
    Ok(())
}
