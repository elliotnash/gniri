// SPDX-License-Identifier: GPL-3.0-or-later

//! The dialog that asks before logging out, restarting or powering off,
//! and does it by itself after a minute. Port of
//! `js/ui/endSessionDialog.js`.
//!
//! In GNOME the dialog belongs to gnome-session, which also lists the
//! applications that object. Here the shell asks, then tells logind (or,
//! to log out, niri) itself.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gniri_services::session;
use gtk::glib;
use gtk::prelude::*;
use serde::{Deserialize, Serialize};

use super::{Modal, message_content};
use crate::services::Services;

/// gnome-session opens the dialog with this many seconds to decide.
pub const SECONDS: u32 = 60;

/// What the dialog asks about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
pub enum Kind {
    LogOut,
    Restart,
    PowerOff,
}

impl Kind {
    /// The dialog's title. `user` is the user's full name, which the
    /// log-out dialog names when there is one.
    pub fn title(self, user: &str) -> String {
        match self {
            Self::LogOut if !user.is_empty() => format!("Log Out {user}"),
            Self::LogOut => "Log Out".to_owned(),
            Self::Restart => "Restart".to_owned(),
            Self::PowerOff => "Power Off".to_owned(),
        }
    }

    /// The sentence under it, with `seconds` left.
    pub fn description(self, user: &str, seconds: u32) -> String {
        let unit = if seconds == 1 { "second" } else { "seconds" };
        match self {
            Self::LogOut if !user.is_empty() => {
                format!("{user} will be logged out automatically in {seconds} {unit}")
            }
            Self::LogOut => {
                format!("You will be logged out automatically in {seconds} {unit}")
            }
            Self::Restart => {
                format!("The system will restart automatically in {seconds} {unit}")
            }
            Self::PowerOff => {
                format!("The system will power off automatically in {seconds} {unit}")
            }
        }
    }

    /// The label of the button that goes ahead.
    pub fn confirm_label(self) -> &'static str {
        match self {
            Self::LogOut => "Log Out",
            Self::Restart => "Restart",
            Self::PowerOff => "Power Off",
        }
    }
}

/// The seconds the dialog shows with `left` of `total` remaining: it
/// counts in tens until the last ten. Port of `_roundSecondsToInterval`.
pub fn display_seconds(total: u32, left: f64, interval: u32) -> u32 {
    let mut time = left.ceil().max(0.0) as u32;
    // Final count down is in decrements of 1
    if time <= interval {
        return time;
    }
    // Round up higher than last displayable time interval
    time += interval - 1;
    // Then round down to that time interval
    if time > total {
        total
    } else {
        time - time % interval
    }
}

/// An open end-session dialog.
pub struct Dialog {
    modal: Modal,
    timer: Rc<Cell<Option<glib::SourceId>>>,
}

impl Dialog {
    pub fn close(self) {
        if let Some(timer) = self.timer.take() {
            timer.remove();
        }
        self.modal.close();
    }
}

/// Does what the user confirmed.
fn confirm(services: &Services, kind: Kind) {
    services.end_session.update(|open| *open = None);
    match kind {
        Kind::LogOut => services.log_out(),
        Kind::Restart => services.session.send(session::Command::Restart),
        Kind::PowerOff => services.session.send(session::Command::PowerOff),
    }
}

pub fn open(
    app: &adw::Application,
    monitor: &gtk::gdk::Monitor,
    services: &Rc<Services>,
    kind: Kind,
) -> Dialog {
    let modal = Modal::new(app, monitor, "end-session-dialog");
    let user = services.user.real_name.clone();

    let message = message_content();
    message.title.set_label(&kind.title(&user));
    message
        .description
        .set_label(&kind.description(&user, SECONDS));
    modal.content.append(&message.root);

    let cancel = {
        let services = Rc::clone(services);
        move || services.end_session.update(|open| *open = None)
    };
    // `_updateButtons`: Cancel first, which also makes it the button the
    // keyboard starts on.
    let cancel_button = modal.add_button("Cancel", cancel.clone());
    modal.add_button(kind.confirm_label(), {
        let services = Rc::clone(services);
        move || confirm(&services, kind)
    });
    modal.on_escape(cancel);

    // `_startTimer`. A fixture's clock stands still, and so does this.
    let timer: Rc<Cell<Option<glib::SourceId>>> = Rc::default();
    if !services.sandboxed {
        let started = Instant::now();
        let source = glib::timeout_add_local(Duration::from_secs(1), {
            let (services, timer) = (Rc::clone(services), Rc::clone(&timer));
            let description = message.description.clone();
            move || {
                let left = f64::from(SECONDS) - started.elapsed().as_secs_f64();
                if left > 0.0 {
                    let shown = display_seconds(SECONDS, left, 10);
                    description.set_label(&kind.description(&user, shown));
                    glib::ControlFlow::Continue
                } else {
                    // Returning `Break` removes the source; nobody else may.
                    timer.take();
                    confirm(&services, kind);
                    glib::ControlFlow::Break
                }
            }
        });
        timer.set(Some(source));
    }

    modal.open();
    cancel_button.grab_focus();
    Dialog { modal, timer }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_countdown_shows_tens_then_ones() {
        let shown = |left: f64| display_seconds(60, left, 10);
        assert_eq!(shown(60.0), 60);
        assert_eq!(shown(59.2), 60);
        assert_eq!(shown(50.9), 60);
        assert_eq!(shown(50.0), 50);
        assert_eq!(shown(49.9), 50);
        assert_eq!(shown(10.5), 20);
        assert_eq!(shown(10.0), 10);
        assert_eq!(shown(9.1), 10);
        assert_eq!(shown(9.0), 9);
        assert_eq!(shown(0.2), 1);
        assert_eq!(shown(0.0), 0);
        assert_eq!(shown(-3.0), 0);
    }

    #[test]
    fn texts_are_gnome_s() {
        assert_eq!(Kind::PowerOff.title("Ada Lovelace"), "Power Off");
        assert_eq!(
            Kind::PowerOff.description("Ada Lovelace", 60),
            "The system will power off automatically in 60 seconds"
        );
        assert_eq!(
            Kind::Restart.description("", 1),
            "The system will restart automatically in 1 second"
        );
        assert_eq!(Kind::LogOut.title("Ada Lovelace"), "Log Out Ada Lovelace");
        assert_eq!(Kind::LogOut.title(""), "Log Out");
        assert_eq!(
            Kind::LogOut.description("Ada Lovelace", 30),
            "Ada Lovelace will be logged out automatically in 30 seconds"
        );
        assert_eq!(
            Kind::LogOut.description("", 30),
            "You will be logged out automatically in 30 seconds"
        );
        assert_eq!(Kind::Restart.confirm_label(), "Restart");
    }
}
