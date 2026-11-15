// SPDX-License-Identifier: GPL-3.0-or-later

//! logind, over the system bus, and the screen locker.

use std::time::Duration;

use async_channel::{Receiver, Sender};
use futures_lite::StreamExt;
use zbus::OwnedFd;

use super::locker::{Exit, Locker, Report};
use super::{Command, Event};
use crate::bus::{Wakes, merge};
use crate::logind::{self, ManagerProxy, SessionProxy};

/// A locker that keeps crashing is started again this often, then given
/// up on.
const MAX_FAILURES: u32 = 5;
/// The pause before a crashed locker is started again.
const RETRY_DELAY: Duration = Duration::from_millis(500);

pub struct Options {
    /// This session's id (`XDG_SESSION_ID`), if known.
    pub session_id: Option<String>,
    /// Lock the screen before the machine sleeps.
    pub lock_on_suspend: bool,
}

enum Wake {
    Command(Command),
    /// logind's `Lock` signal: someone asked for this session to be locked.
    LockRequested,
    /// logind's `PrepareForSleep`: true before sleeping, false after.
    Sleep(bool),
    Locker(Report),
    /// Time to start a crashed locker again.
    Retry,
}

struct Lock<'a> {
    locker: Box<dyn Locker>,
    reports: Sender<Report>,
    retries: Sender<()>,
    events: Sender<Event>,
    manager: ManagerProxy<'a>,
    session: Option<SessionProxy<'a>>,
    lock_on_suspend: bool,
    /// A locker process exists.
    running: bool,
    /// It has covered the screen.
    locked: bool,
    /// Crashes in a row.
    failures: u32,
    /// Holds logind back from sleeping while it is there.
    inhibitor: Option<OwnedFd>,
}

impl Lock<'_> {
    fn start(&mut self) {
        if self.running {
            return;
        }
        match self.locker.start(self.reports.clone()) {
            Ok(()) => self.running = true,
            Err(error) => {
                eprintln!("gniri: session: cannot start the screen locker: {error}");
                // Nothing will ever say "locked": do not hold sleep up.
                self.let_sleep();
            }
        }
    }

    /// Takes the inhibitor that makes logind wait before sleeping.
    async fn hold_sleep(&mut self) {
        if !self.lock_on_suspend || self.inhibitor.is_some() {
            return;
        }
        let taken = self
            .manager
            .inhibit("sleep", "gniri", "gniri needs to lock the screen", "delay")
            .await;
        match taken {
            Ok(inhibitor) => self.inhibitor = Some(inhibitor),
            Err(error) => {
                eprintln!("gniri: session: cannot delay sleep to lock the screen first: {error}");
            }
        }
    }

    /// Closing the descriptor releases the inhibitor.
    fn let_sleep(&mut self) {
        self.inhibitor = None;
    }

    async fn set_locked(&mut self, locked: bool) -> bool {
        self.locked = locked;
        // Other programs read the hint (`loginctl show-session`).
        if let Some(session) = &self.session
            && let Err(error) = session.set_locked_hint(locked).await
        {
            eprintln!("gniri: session: cannot set the locked hint: {error}");
        }
        self.events.send(Event::Locked(locked)).await.is_ok()
    }

    /// Returns false once the shell has gone away.
    async fn handle(&mut self, report: Report) -> bool {
        match report {
            Report::Locked => {
                self.failures = 0;
                self.let_sleep();
                self.set_locked(true).await
            }
            Report::Gone(exit) => {
                self.running = false;
                match exit {
                    Exit::Unlocked => self.set_locked(false).await,
                    Exit::Refused => {
                        eprintln!(
                            "gniri: session: the compositor would not lock the screen for gniri \
                             (another screen locker has it, or it has no session lock)"
                        );
                        self.let_sleep();
                        self.set_locked(false).await
                    }
                    Exit::Failed => {
                        self.failures += 1;
                        if self.failures > MAX_FAILURES {
                            eprintln!(
                                "gniri: session: the screen locker keeps failing; giving up. If \
                                 the screen stays locked, run `gniri lock` from another terminal."
                            );
                            self.let_sleep();
                            return self.set_locked(false).await;
                        }
                        // A locker that dies after locking leaves the
                        // compositor showing nothing until another takes
                        // over; one that dies before was still asked for.
                        eprintln!("gniri: session: the screen locker failed; starting it again");
                        let retries = self.retries.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(RETRY_DELAY);
                            let _ = retries.send_blocking(());
                        });
                        true
                    }
                }
            }
        }
    }
}

/// logind answers "yes", "challenge" (after authentication), "no" or "na".
/// GNOME offers the action for "challenge" too: logind then asks itself.
fn allowed(answer: zbus::Result<String>) -> bool {
    matches!(answer.as_deref(), Ok("yes" | "challenge"))
}

pub async fn run(
    connection: zbus::Connection,
    events: Sender<Event>,
    commands: Receiver<Command>,
    locker: Box<dyn Locker>,
    options: Options,
) -> zbus::Result<()> {
    let manager = ManagerProxy::new(&connection).await?;
    // Without a session (logind missing, or gniri started outside of one)
    // suspending and locking on request still work.
    let session = match logind::session(&connection, options.session_id.as_deref()).await {
        Ok(session) => Some(session),
        Err(error) => {
            eprintln!("gniri: session: cannot find this session in logind: {error}");
            None
        }
    };

    let abilities = Event::Abilities {
        suspend: allowed(manager.can_suspend().await),
        restart: allowed(manager.can_reboot().await),
        power_off: allowed(manager.can_power_off().await),
    };
    if events.send(abilities).await.is_err() {
        return Ok(());
    }

    let (reports, locker_reports) = async_channel::unbounded();
    let (retries, retry) = async_channel::unbounded();
    let mut streams: Vec<Wakes<Wake>> = vec![
        Box::pin(commands.map(Wake::Command)),
        Box::pin(locker_reports.map(Wake::Locker)),
        Box::pin(retry.map(|()| Wake::Retry)),
        Box::pin(
            manager
                .receive_prepare_for_sleep()
                .await?
                .filter_map(|signal| signal.args().ok().map(|args| Wake::Sleep(args.start))),
        ),
    ];
    if let Some(session) = &session {
        streams.push(Box::pin(
            session
                .receive_lock_requested()
                .await?
                .map(|_| Wake::LockRequested),
        ));
    }
    let mut wakes = merge(streams);

    let mut lock = Lock {
        locker,
        reports,
        retries,
        events,
        manager: manager.clone(),
        session: session.clone(),
        lock_on_suspend: options.lock_on_suspend,
        running: false,
        locked: false,
        failures: 0,
        inhibitor: None,
    };
    lock.hold_sleep().await;

    while let Some(wake) = wakes.next().await {
        match wake {
            Wake::Command(command) => {
                let result = match command {
                    Command::Suspend => manager.suspend(true).await,
                    Command::Restart => manager.reboot(true).await,
                    Command::PowerOff => manager.power_off(true).await,
                    Command::Lock => match &session {
                        Some(session) => session.lock().await,
                        None => {
                            lock.start();
                            Ok(())
                        }
                    },
                    Command::SetLockOnSuspend(wanted) => {
                        lock.lock_on_suspend = wanted;
                        if wanted {
                            lock.hold_sleep().await;
                        } else {
                            lock.let_sleep();
                        }
                        Ok(())
                    }
                };
                if let Err(error) = result {
                    eprintln!("gniri: session: {command:?} failed: {error}");
                    // Without logind's signal, lock all the same.
                    if command == Command::Lock {
                        lock.start();
                    }
                }
            }
            Wake::LockRequested | Wake::Retry => lock.start(),
            Wake::Sleep(true) => {
                if lock.lock_on_suspend && !lock.locked {
                    // The inhibitor goes once the locker says "locked".
                    lock.start();
                } else {
                    lock.let_sleep();
                }
            }
            Wake::Sleep(false) => lock.hold_sleep().await,
            Wake::Locker(report) => {
                if !lock.handle(report).await {
                    break; // The shell went away.
                }
            }
        }
    }
    Ok(())
}
