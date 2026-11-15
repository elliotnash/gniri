//! The dialog that asks for a password on polkit's behalf. Port of
//! `AuthenticationDialog` in `js/ui/components/polkitAgent.js`.
//!
//! It shows one request of the polkit service's state and sends what the
//! user types back; whether that was right is for polkit's helper to say
//! (crates/services/src/polkit.rs).

use std::rc::Rc;

use gniri_core::Subscription;
use gniri_services::polkit::{Command, Notice, Prompt, Request, State};
use gniri_services::user::User;
use gtk::prelude::*;

use super::{Modal, message_content};
use crate::services::Service;
use crate::widgets::avatar;
use crate::widgets::password_entry::{PasswordEntry, caps_lock_warning};

/// An open authentication dialog. Dropping it stops it following the
/// request; [`Dialog::close`] takes it off the screen.
pub struct Dialog {
    modal: Modal,
    _subscriptions: Vec<Subscription>,
}

impl Dialog {
    pub fn close(self) {
        self.modal.close();
    }
}

/// The classes of the line under the entry: one label that changes its
/// look, where GNOME swaps three.
const NOTICE_CLASSES: [&str; 3] = [
    "prompt-dialog-null-label",
    "prompt-dialog-info-label",
    "prompt-dialog-error-label",
];

/// Opens the dialog for `request` on `monitor`. `user` is the account
/// whose password is asked for.
pub fn open(
    app: &adw::Application,
    monitor: &gtk::gdk::Monitor,
    agent: &Rc<Service<State>>,
    request: &Request,
    user: &User,
) -> Dialog {
    let modal = Modal::new(app, monitor, "prompt-dialog");

    let header = message_content();
    header.title.set_label("Authentication Required");
    header.description.set_label(&request.message);
    modal.content.append(&header.root);

    // The user whose password it is.
    let user_layout = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .css_classes(["polkit-dialog-user-layout"])
        .build();
    user_layout.append(&avatar::build(user));
    let (name, class) = if user.name == "root" {
        ("Administrator", "polkit-dialog-user-root-label")
    } else {
        (user.display_name(), "polkit-dialog-user-label")
    };
    user_layout.append(
        &gtk::Label::builder()
            .label(name)
            .css_classes([class])
            .build(),
    );

    let entry = PasswordEntry::new("prompt-dialog-password-entry");
    let notice = gtk::Label::builder()
        .css_classes(["prompt-dialog-null-label"])
        .wrap(true)
        .justify(gtk::Justification::Center)
        .max_width_chars(1)
        .hexpand(true)
        .build();
    let warnings = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    warnings.append(&caps_lock_warning());
    warnings.append(&notice);
    let password_layout = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .css_classes(["prompt-dialog-password-layout"])
        .build();
    password_layout.append(&entry.widget);
    password_layout.append(&warnings);

    // `bodyContent`: a second MessageDialogContent, without its labels.
    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .css_classes(["message-dialog-content"])
        .hexpand(true)
        .build();
    body.append(&user_layout);
    body.append(&password_layout);
    modal.content.append(&body);

    let cancel = {
        let agent = Rc::clone(agent);
        move || agent.send(Command::Cancel)
    };
    modal.add_button("Cancel", cancel.clone());
    modal.on_escape(cancel);

    // `_onEntryActivate`
    let respond = {
        let (agent, entry) = (Rc::clone(agent), entry.clone());
        move || {
            let answer = entry.password();
            if !answer.is_empty() {
                agent.send(Command::Respond(answer));
            }
        }
    };
    let authenticate = modal.add_button("Authenticate", respond.clone());
    entry.text.connect_activate(move |_| respond());

    // Only something to send can be sent.
    let update_button = {
        let (authenticate, entry) = (authenticate.clone(), entry.clone());
        move || {
            authenticate.set_sensitive(entry.widget.is_sensitive() && !entry.password().is_empty());
        }
    };
    entry.text.connect_changed({
        let update_button = update_button.clone();
        move |_| update_button()
    });

    let mut subscriptions = Vec::new();
    // `_onSessionRequest`: the helper asks, the entry opens for an answer.
    // While an answer is checked there is no prompt and the entry waits.
    subscriptions.push(agent.state.select(
        |state| {
            state
                .request
                .as_ref()
                .and_then(|request| request.prompt.clone())
        },
        {
            let entry = entry.clone();
            move |prompt: &Option<Prompt>| {
                entry.widget.set_sensitive(prompt.is_some());
                if let Some(prompt) = prompt {
                    entry.set_hint(prompt.hint());
                    entry.set_shown(prompt.echo);
                    entry.clear();
                    entry.grab_focus();
                }
                update_button();
            }
        },
    ));
    subscriptions.push(agent.state.select(
        |state| {
            state
                .request
                .as_ref()
                .map(|request| request.notice.clone())
                .unwrap_or_default()
        },
        {
            let entry = entry.clone();
            move |shown: &Notice| {
                let (class, text) = match shown {
                    Notice::None => (NOTICE_CLASSES[0], ""),
                    Notice::Info(text) => (NOTICE_CLASSES[1], text.as_str()),
                    Notice::Error(text) => (NOTICE_CLASSES[2], text.as_str()),
                };
                for other in NOTICE_CLASSES {
                    notice.remove_css_class(other);
                }
                notice.add_css_class(class);
                notice.set_label(text);
                // `_onSessionShowError` and `_onSessionShowInfo`
                if *shown != Notice::None {
                    entry.clear();
                }
            }
        },
    ));
    subscriptions.push(agent.state.select(
        |state| state.request.as_ref().map_or(0, |request| request.failures),
        {
            let entry = entry.clone();
            move |failures: &u32| {
                if *failures > 0 {
                    entry.wiggle();
                }
            }
        },
    ));

    modal.open();
    entry.grab_focus();
    Dialog {
        modal,
        _subscriptions: subscriptions,
    }
}
