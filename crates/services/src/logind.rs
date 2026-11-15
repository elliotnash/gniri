// SPDX-License-Identifier: GPL-3.0-or-later

//! Finding this login session in logind.
//!
//! logind offers `/org/freedesktop/login1/session/auto` for "the caller's
//! session", but only method calls work on it: the `Lock` and `Unlock`
//! signals come from the session's real object. And a process started by
//! the systemd user manager (gniri as a user unit) is in no session at all.
//! So the session is looked up once: by its id if the caller knows it
//! (`XDG_SESSION_ID`), then by this process, then as the user's graphical
//! session.

use zbus::{OwnedFd, OwnedObjectPath};

pub const SERVICE: &str = "org.freedesktop.login1";

#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
pub trait Manager {
    fn get_session(&self, id: &str) -> zbus::Result<OwnedObjectPath>;
    #[zbus(name = "GetSessionByPID")]
    fn get_session_by_pid(&self, pid: u32) -> zbus::Result<OwnedObjectPath>;

    /// "yes", "challenge" (allowed after authentication), "no" or "na".
    fn can_suspend(&self) -> zbus::Result<String>;
    fn can_reboot(&self) -> zbus::Result<String>;
    fn can_power_off(&self) -> zbus::Result<String>;
    fn suspend(&self, interactive: bool) -> zbus::Result<()>;
    fn reboot(&self, interactive: bool) -> zbus::Result<()>;
    fn power_off(&self, interactive: bool) -> zbus::Result<()>;

    /// Takes an inhibitor lock: `what` is a colon-separated list such as
    /// "sleep", `mode` is "block" or "delay". The lock is held until the
    /// returned descriptor is closed.
    fn inhibit(&self, what: &str, who: &str, why: &str, mode: &str) -> zbus::Result<OwnedFd>;

    /// True just before the machine sleeps, false just after it woke.
    #[zbus(signal)]
    fn prepare_for_sleep(&self, start: bool) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.freedesktop.login1.Session",
    default_service = "org.freedesktop.login1"
)]
pub trait Session {
    /// Asks whoever locks this session's screen to do so: logind answers
    /// with the `Lock` signal.
    fn lock(&self) -> zbus::Result<()>;
    fn set_locked_hint(&self, locked: bool) -> zbus::Result<()>;

    #[zbus(property)]
    fn id(&self) -> zbus::Result<String>;

    #[zbus(signal, name = "Lock")]
    fn lock_requested(&self) -> zbus::Result<()>;
    #[zbus(signal, name = "Unlock")]
    fn unlock_requested(&self) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.freedesktop.login1.User",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1/user/self"
)]
trait User {
    /// The user's graphical session: its id and its object.
    #[zbus(property)]
    fn display(&self) -> zbus::Result<(String, OwnedObjectPath)>;
}

/// The id of this session, if the environment names it.
pub fn session_id_from_env() -> Option<String> {
    std::env::var("XDG_SESSION_ID")
        .ok()
        .filter(|id| !id.is_empty())
}

/// The object of the session gniri runs in. `id` is the session's id where
/// it is known.
pub async fn session_path(
    connection: &zbus::Connection,
    id: Option<&str>,
) -> zbus::Result<OwnedObjectPath> {
    let manager = ManagerProxy::new(connection).await?;
    if let Some(id) = id
        && let Ok(path) = manager.get_session(id).await
    {
        return Ok(path);
    }
    if let Ok(path) = manager.get_session_by_pid(std::process::id()).await {
        return Ok(path);
    }
    let user = UserProxy::new(connection).await?;
    Ok(user.display().await?.1)
}

/// A proxy for the session gniri runs in.
pub async fn session(
    connection: &zbus::Connection,
    id: Option<&str>,
) -> zbus::Result<SessionProxy<'static>> {
    let path = session_path(connection, id).await?;
    SessionProxy::builder(connection).path(path).build().await
}

/// Runs `unlock` whenever logind tells this session to unlock (`loginctl
/// unlock-session`), until it returns false. For the lock screen, which
/// has no bus thread of its own: this starts one.
pub fn watch_unlock(unlock: impl Fn() -> bool + Send + 'static) {
    use futures_lite::StreamExt;

    let spawned = std::thread::Builder::new()
        .name("logind".to_owned())
        .spawn(move || {
            let watched: zbus::Result<()> = futures_lite::future::block_on(async {
                let connection = zbus::Connection::system().await?;
                let session = session(&connection, session_id_from_env().as_deref()).await?;
                let mut unlocks = session.receive_unlock_requested().await?;
                while unlocks.next().await.is_some() {
                    if !unlock() {
                        break;
                    }
                }
                Ok(())
            });
            if let Err(error) = watched {
                eprintln!("gniri: not listening for logind's unlock signal: {error}");
            }
        });
    if let Err(error) = spawned {
        eprintln!("gniri: cannot start a thread: {error}");
    }
}
