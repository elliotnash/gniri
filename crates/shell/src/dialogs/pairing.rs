// SPDX-License-Identifier: GPL-3.0-or-later

//! The dialog that asks what BlueZ wants to know while a Bluetooth device
//! is paired: a PIN to type, a number to compare, a yes or no.
//!
//! GNOME Shell has no such dialog; GNOME Settings shows one of its own
//! (gnome-bluetooth's `BluetoothPairingDialog`) while its Bluetooth panel
//! is open. This is that dialog's content in the shell's dialog: one
//! request of the pairing agent's state
//! (crates/services/src/bluetooth/agent.rs).

use std::rc::Rc;

use gniri_services::bluetooth::agent::{Command, Request};
use gtk::prelude::*;

use super::{Modal, message_content};
use crate::services::Services;
use crate::widgets::password_entry::PasswordEntry;

/// An open pairing dialog.
pub struct Dialog {
    modal: Modal,
}

impl Dialog {
    pub fn close(self) {
        self.modal.close();
    }
}

/// Opens the dialog for `request` on `monitor`.
pub fn open(
    app: &adw::Application,
    monitor: &gtk::gdk::Monitor,
    services: &Rc<Services>,
    request: &Request,
) -> Dialog {
    let modal = Modal::new(app, monitor, "prompt-dialog");

    let content = message_content();
    content.title.set_label(&request.title());
    content.description.set_label(&request.description());
    if let Some(pin) = request.pin() {
        content.root.append(
            &gtk::Label::builder()
                .label(pin)
                .css_classes(["pairing-dialog-pin"])
                .build(),
        );
    }
    // A PIN is typed in the open, as gnome-bluetooth has it: it is read
    // off the device, or made up for it.
    let entry = request.asks_for_text().then(|| {
        let entry = PasswordEntry::plain("prompt-dialog-password-entry");
        entry.set_hint("PIN");
        entry.text.set_input_purpose(gtk::InputPurpose::Pin);
        content.root.append(&entry.widget);
        entry
    });
    modal.content.append(&content.root);

    let reject = {
        let services = Rc::clone(services);
        move || services.bluetooth_agent.send(Command::Reject)
    };
    let reject_button = modal.add_button(&request.reject_label(), reject.clone());
    modal.on_escape(reject);

    if let Some(label) = request.accept_label() {
        // The answer: what was typed, or just yes.
        let accept = {
            let (services, entry, request) = (Rc::clone(services), entry.clone(), request.clone());
            move || match &entry {
                Some(entry) => {
                    let text = entry.password();
                    if request.accepts(&text) {
                        services.bluetooth_agent.send(Command::Respond(text));
                    }
                }
                None => services.bluetooth_agent.send(Command::Accept),
            }
        };
        let accept_button = modal.add_button(&label, accept.clone());
        if let Some(entry) = &entry {
            let update_button = {
                let (entry, request) = (entry.clone(), request.clone());
                move || accept_button.set_sensitive(request.accepts(&entry.password()))
            };
            update_button();
            entry.text.connect_changed(move |_| update_button());
            entry.text.connect_activate(move |_| accept());
        }
    }

    modal.open();
    // Nothing is agreed to by a stray Enter: the keyboard starts on the
    // entry, or on the button that says no.
    match &entry {
        Some(entry) => entry.grab_focus(),
        None => {
            reject_button.grab_focus();
        }
    }
    Dialog { modal }
}
