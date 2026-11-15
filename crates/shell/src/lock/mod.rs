//! `gniri lock`: the lock screen, a process of its own. Port of
//! `js/ui/screenShield.js` and `js/ui/unlockDialog.js`.
//!
//! The compositor does the locking (`ext-session-lock`): from the moment it
//! confirms, nothing of the session is shown or takes input, on any output,
//! until this process says "unlock". If this process dies first the session
//! stays locked, showing nothing, until another locker takes over. So the
//! only ways out of here are a password PAM accepts, a fingerprint, and
//! logind's `Unlock` signal (`loginctl unlock-session`, the way back in from
//! another terminal).
//!
//! The process prints `locked` once the compositor has confirmed, for the
//! shell, which is holding the machine's sleep back until then, and its exit
//! status says how it ended (crates/services/src/session/locker.rs).

mod prompt;
mod screen;

use std::cell::{Cell, RefCell};
use std::io::Write;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use adw::prelude::*;
use gniri_core::{FixedClock, LocalTime, Store};
use gniri_services::auth::{self, Authenticator, Message, Outcome};
use gniri_services::logind;
use gniri_services::session::locker::{LOCKED_LINE, REFUSED_STATUS};
use gniri_services::user::User;
use gtk::{gdk, glib};

use crate::Cli;
use crate::app;
use crate::ipc::Role;
use crate::services::{self, SystemClock};
use crate::surfaces::wallpaper::WallpaperSource;

/// What the prompt says and whether it takes an answer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// A password is with PAM; the entry waits.
    pub checking: bool,
    /// The line under the entry.
    pub message: Option<Notice>,
    /// How many passwords were wrong. The prompt shakes when this rises.
    pub failures: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    /// Something went wrong, as against a hint.
    pub warning: bool,
}

/// What a wrong password is answered with (`gdm/util.js`).
const TRY_AGAIN: &str = "Sorry, that didn’t work. Please try again.";

/// What the worker threads tell the main loop.
enum Report {
    /// The password check is over.
    Checked(Outcome),
    /// A finger was accepted, or logind says to unlock.
    Unlock,
    Say(Message),
}

/// How the lock screen is on screen.
enum Mode {
    /// As the session's lock.
    Session(gtk4_session_lock::Instance),
    /// As ordinary surfaces that lock nothing, for a scene's screenshot.
    Preview,
}

/// The lock screen: what its surfaces share.
pub struct Lock {
    app: adw::Application,
    mode: Mode,
    authenticator: Arc<dyn Authenticator>,
    reports: async_channel::Sender<Report>,
    pub user: User,
    pub hour12: bool,
    pub wallpaper: Option<WallpaperSource>,
    pub time: Store<LocalTime>,
    pub status: Store<Status>,
    /// True under a fixture: nothing moves by itself.
    pub still: bool,
    /// A fixture's surfaces start on the password prompt.
    start_on_prompt: bool,
    /// A prompt is on screen somewhere: a finger may unlock.
    fingerprint_wanted: Arc<AtomicBool>,
    fingerprint_wake: Option<async_channel::Sender<()>>,
    /// The surfaces, one per output.
    screens: RefCell<Vec<screen::Screen>>,
    unlocking: Cell<bool>,
}

impl Lock {
    /// Hands `password` to PAM, on a thread: it takes its time.
    pub fn submit(&self, password: String) {
        if password.is_empty() || self.status.with(|status| status.checking) {
            return;
        }
        self.status.update(|status| {
            status.checking = true;
            status.message = None;
        });
        let (authenticator, reports) = (Arc::clone(&self.authenticator), self.reports.clone());
        let spawned = std::thread::Builder::new()
            .name("authenticate".to_owned())
            .spawn(move || {
                let tell = |message: Message| {
                    let _ = reports.send_blocking(Report::Say(message));
                };
                let outcome = authenticator.check(&password, &tell);
                let _ = reports.send_blocking(Report::Checked(outcome));
            });
        if let Err(error) = spawned {
            eprintln!("gniri: lock: cannot start a thread: {error}");
            self.status.update(|status| status.checking = false);
        }
    }

    /// A prompt came on screen, or the last one left it.
    pub fn prompt_shown(&self, shown: bool) {
        let any = shown
            || self
                .screens
                .borrow()
                .iter()
                .any(screen::Screen::shows_prompt);
        self.fingerprint_wanted.store(any, Ordering::SeqCst);
        if any && let Some(wake) = &self.fingerprint_wake {
            let _ = wake.try_send(());
        }
        if !any {
            self.status.update(|status| status.message = None);
        }
    }

    fn unlock(&self) {
        if self.unlocking.replace(true) {
            return;
        }
        match &self.mode {
            // Its `unlocked` signal ends the process.
            Mode::Session(instance) => instance.unlock(),
            Mode::Preview => app::quit_with(&self.app, 0),
        }
    }

    /// Whether the surfaces are to be ordinary ones.
    pub fn is_preview(&self) -> bool {
        matches!(self.mode, Mode::Preview)
    }

    fn handle(&self, report: Report) {
        match report {
            Report::Unlock | Report::Checked(Outcome::Granted) => self.unlock(),
            Report::Checked(Outcome::Denied) => self.status.update(|status| {
                status.checking = false;
                status.failures += 1;
                // What a module said about it says more.
                if !status.message.as_ref().is_some_and(|notice| notice.warning) {
                    status.message = Some(Notice {
                        text: TRY_AGAIN.to_owned(),
                        warning: true,
                    });
                }
            }),
            Report::Checked(Outcome::Unavailable(reason)) => {
                eprintln!("gniri: lock: the password could not be checked: {reason}");
                self.status.update(|status| {
                    status.checking = false;
                    status.message = Some(Notice {
                        text: "Passwords cannot be checked right now".to_owned(),
                        warning: true,
                    });
                });
            }
            Report::Say(message) => {
                let notice = match message {
                    Message::Info(text) => Notice {
                        text,
                        warning: false,
                    },
                    Message::Error(text) => Notice {
                        text,
                        warning: true,
                    },
                };
                self.status.update(|status| status.message = Some(notice));
            }
        }
    }
}

/// Waits for fingers while a prompt shows. One thread for as long as the
/// screen is locked; PAM blocks until the reader has an answer or gives up
/// waiting.
fn watch_fingerprint(
    user: String,
    wanted: Arc<AtomicBool>,
    wake: async_channel::Receiver<()>,
    reports: async_channel::Sender<Report>,
) {
    let spawned = std::thread::Builder::new()
        .name("fingerprint".to_owned())
        .spawn(move || {
            while wake.recv_blocking().is_ok() {
                while wanted.load(Ordering::SeqCst) {
                    let tell = |message: Message| {
                        if wanted.load(Ordering::SeqCst) {
                            let _ = reports.send_blocking(Report::Say(message));
                        }
                    };
                    match auth::fingerprint(&user, &tell) {
                        Outcome::Granted => {
                            let _ = reports.send_blocking(Report::Unlock);
                            return;
                        }
                        // Not recognised, or nobody tried in time: again,
                        // after a breath, for as long as a prompt shows.
                        Outcome::Denied => std::thread::sleep(std::time::Duration::from_secs(1)),
                        // No reader, or no finger enrolled.
                        Outcome::Unavailable(_) => return,
                    }
                }
            }
        });
    if let Err(error) = spawned {
        eprintln!("gniri: lock: cannot start a thread: {error}");
    }
}

/// Tells whoever started this process that the screen is covered.
fn announce_locked() {
    let mut output = std::io::stdout();
    // Nobody listening is fine.
    let _ = writeln!(output, "{LOCKED_LINE}");
    let _ = output.flush();
}

pub fn run(cli: Cli) -> glib::ExitCode {
    // A scene's lock screen takes the scene's password instead of asking
    // PAM. That must never be possible in a build someone locks their
    // real session with.
    if cli.fixture.is_some() && !cfg!(debug_assertions) {
        eprintln!("gniri: lock: --fixture is only for debug builds");
        return glib::ExitCode::FAILURE;
    }
    let options = cli.clone();
    app::run_helper(cli, Role::Lock, move |app, config, fixture| {
        let preview = fixture.is_some_and(|fixture| fixture.lock.preview);
        if !preview && !gtk4_session_lock::is_supported() {
            eprintln!("gniri: lock: the compositor has no session lock (ext-session-lock-v1)");
            app::quit_with(app, REFUSED_STATUS as u8);
            return None;
        }

        let scheduler = services::scheduler();
        let (user, authenticator, time): (User, Arc<dyn Authenticator>, Store<LocalTime>) =
            match fixture {
                Some(fixture) => {
                    let now = fixture.clock().ok().flatten().map_or_else(
                        || gniri_core::Clock::now(&SystemClock),
                        |FixedClock(now)| now,
                    );
                    (
                        fixture.user.clone(),
                        Arc::new(auth::Fake {
                            password: fixture.lock.password.clone(),
                        }),
                        Store::new(now, Rc::clone(&scheduler)),
                    )
                }
                None => {
                    let user = User::current();
                    let authenticator = Arc::new(auth::Pam::password(&user.name));
                    let time =
                        Store::new(gniri_core::Clock::now(&SystemClock), Rc::clone(&scheduler));
                    // It runs for as long as the process does.
                    let _ = services::Ticker::start(time.clone(), false);
                    (user, authenticator, time)
                }
            };

        let (reports, incoming) = async_channel::unbounded();
        let fingerprint_wanted = Arc::new(AtomicBool::new(false));
        let mut fingerprint_wake = None;
        if fixture.is_none() {
            // `loginctl unlock-session`: the way back in from another
            // terminal if the prompt ever fails.
            logind::watch_unlock({
                let reports = reports.clone();
                move || reports.send_blocking(Report::Unlock).is_ok()
            });
            if auth::fingerprint_configured() {
                let (wake, woken) = async_channel::unbounded();
                watch_fingerprint(
                    user.name.clone(),
                    Arc::clone(&fingerprint_wanted),
                    woken,
                    reports.clone(),
                );
                fingerprint_wake = Some(wake);
            }
        }

        let instance = (!preview).then(gtk4_session_lock::Instance::new);
        let lock = Rc::new(Lock {
            app: app.clone(),
            mode: match &instance {
                Some(instance) => Mode::Session(instance.clone()),
                None => Mode::Preview,
            },
            authenticator,
            reports,
            user,
            hour12: config.clock.format().hour12,
            wallpaper: app::wallpaper_source(&options, config),
            time,
            status: Store::new(Status::default(), scheduler),
            still: fixture.is_some(),
            start_on_prompt: fixture.is_some_and(|fixture| fixture.lock.prompt),
            fingerprint_wanted,
            fingerprint_wake,
            screens: RefCell::new(Vec::new()),
            unlocking: Cell::new(false),
        });

        glib::spawn_future_local({
            let lock = Rc::downgrade(&lock);
            async move {
                while let Ok(report) = incoming.recv().await {
                    let Some(lock) = lock.upgrade() else { break };
                    lock.handle(report);
                }
            }
        });

        let Some(instance) = instance else {
            // A preview: one surface per output there is now.
            let monitors = gdk::Display::default().map(|display| display.monitors());
            for monitor in monitors
                .iter()
                .flat_map(|monitors| monitors.iter::<gdk::Monitor>())
                .filter_map(Result::ok)
            {
                let screen = screen::build(&lock, &monitor);
                screen.window.present();
                if lock.start_on_prompt {
                    screen.show_prompt();
                }
                lock.screens.borrow_mut().push(screen);
            }
            return Some(Box::new(lock));
        };

        instance.connect_locked(|_| announce_locked());
        // Another locker has the session, or the compositor will not lock.
        instance.connect_failed({
            let app = app.clone();
            move |_| {
                eprintln!("gniri: lock: the compositor refused to lock the session");
                app::quit_with(&app, REFUSED_STATUS as u8);
            }
        });
        instance.connect_unlocked({
            let app = app.clone();
            move |_| {
                // The compositor must have heard "unlock" before this
                // process goes: a locker that vanishes without it leaves
                // the session locked.
                if let Some(display) = gdk::Display::default() {
                    display.sync();
                }
                app::quit_with(&app, 0);
            }
        });
        // One surface per output, now and for outputs that appear later.
        instance.connect_monitor({
            let lock = Rc::downgrade(&lock);
            move |instance, monitor| {
                let Some(lock) = lock.upgrade() else { return };
                let screen = screen::build(&lock, monitor);
                instance.assign_window_to_monitor(&screen.window, monitor);
                if lock.start_on_prompt {
                    screen.show_prompt();
                }
                let mut screens = lock.screens.borrow_mut();
                // Those of outputs that went away were destroyed with them.
                screens.retain(|screen| screen.window.is_realized());
                screens.push(screen);
            }
        });
        if !instance.lock() {
            eprintln!("gniri: lock: the session could not be locked");
            app::quit_with(app, REFUSED_STATUS as u8);
            return None;
        }
        Some(Box::new(lock))
    })
}

impl Lock {
    pub fn application(&self) -> &adw::Application {
        &self.app
    }
}
