// SPDX-License-Identifier: GPL-3.0-or-later

//! The password prompt of the lock screen. Port of `AuthPrompt` in
//! `js/gdm/authPrompt.js` and `UserWidget` in `js/ui/userWidget.js`, as the
//! unlock dialog uses them: one user, one password entry.

use gniri_core::tr;
use std::rc::Rc;

use gniri_core::Subscription;
use gtk::prelude::*;

use super::{Lock, Notice};
use crate::widgets::avatar;
use crate::widgets::password_entry::{PasswordEntry, caps_lock_warning, wiggle};

pub struct Prompt {
    /// `.login-dialog-prompt-layout`
    pub root: gtk::Box,
    pub entry: PasswordEntry,
    _subscriptions: Vec<Subscription>,
}

/// Room as wide as the cancel button GDM's login screen has beside the
/// entry, and its twin on the other side. The unlock dialog has neither
/// button, but keeps their room.
fn button_room() -> gtk::Box {
    gtk::Box::builder()
        .css_classes(["login-dialog-button-room"])
        .build()
}

pub fn build(lock: &Rc<Lock>) -> Prompt {
    // `UserWidget` in its vertical layout.
    let user = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .css_classes(["user-widget", "vertical"])
        .halign(gtk::Align::Center)
        .build();
    user.append(&avatar::build(&lock.user));
    user.append(
        &gtk::Label::builder()
            .label(lock.user.display_name())
            .css_classes(["user-widget-label"])
            .build(),
    );

    let entry = PasswordEntry::new("login-dialog-prompt-entry");
    entry.widget.set_halign(gtk::Align::Fill);
    entry.widget.set_hexpand(true);
    entry.set_hint(&tr("Password"));

    // `_defaultButtonWell`: the arrow that submits, inside the entry's
    // right end, and the spinner that takes its place while PAM works.
    let next = gtk::Button::builder()
        .css_classes(["login-dialog-button", "next-button"])
        .child(&gtk::Image::from_icon_name("go-next-symbolic"))
        .build();
    let spinner = adw::Spinner::builder().visible(false).build();
    let well = gtk::Box::builder()
        .css_classes(["login-dialog-default-button-well"])
        .halign(gtk::Align::End)
        .valign(gtk::Align::Center)
        .build();
    well.append(&next);
    well.append(&spinner);

    let entry_area = gtk::Overlay::builder()
        .css_classes(["login-dialog-prompt-entry-area"])
        .hexpand(true)
        .child(&entry.widget)
        .build();
    entry_area.add_overlay(&well);

    let button_box = gtk::Box::builder()
        .css_classes(["login-dialog-button-box"])
        .build();
    button_box.append(&button_room());
    button_box.append(&entry_area);
    button_box.append(&button_room());

    let message = gtk::Label::builder()
        .css_classes(["login-dialog-message"])
        .wrap(true)
        .justify(gtk::Justification::Center)
        .valign(gtk::Align::Start)
        .max_width_chars(1)
        .hexpand(true)
        .build();

    let input_well = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .css_classes(["login-dialog-input-well"])
        .build();
    input_well.append(&button_box);
    input_well.append(&caps_lock_warning());
    input_well.append(&message);

    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .css_classes(["login-dialog-prompt-layout"])
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    root.append(&user);
    root.append(&input_well);

    let submit = {
        let (lock, entry) = (Rc::downgrade(lock), entry.clone());
        move || {
            if let Some(lock) = lock.upgrade() {
                lock.submit(entry.password());
            }
        }
    };
    next.connect_clicked({
        let submit = submit.clone();
        move |_| submit()
    });
    entry.text.connect_activate(move |_| submit());

    let mut subscriptions = Vec::new();
    subscriptions.push(lock.status.select(|status| status.checking, {
        let entry = entry.clone();
        move |checking: &bool| {
            entry.widget.set_sensitive(!*checking);
            next.set_visible(!*checking);
            spinner.set_visible(*checking);
            // Back from PAM with a "no": ready for the next try. (Not
            // while the prompt is still being built.)
            if !*checking && entry.text.root().is_some() {
                entry.grab_focus();
            }
        }
    }));
    subscriptions.push(lock.status.select(
        |status| status.message.clone(),
        move |notice: &Option<Notice>| {
            let (text, warning) = match notice {
                Some(notice) => (notice.text.as_str(), notice.warning),
                None => ("", false),
            };
            message.set_label(text);
            if warning {
                message.add_css_class("login-dialog-message-warning");
            } else {
                message.remove_css_class("login-dialog-message-warning");
            }
        },
    ));
    // A wrong password: the entry empties and shakes.
    subscriptions.push(lock.status.select(|status| status.failures, {
        let entry = entry.clone();
        move |failures: &u32| {
            if *failures > 0 {
                entry.clear();
                wiggle(&entry_area);
            }
        }
    }));

    Prompt {
        root,
        entry,
        _subscriptions: subscriptions,
    }
}
