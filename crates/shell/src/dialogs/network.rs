//! The dialog that asks for a network's password on NetworkManager's
//! behalf. Port of `NetworkSecretDialog` in
//! `js/ui/components/networkAgent.js`.
//!
//! It shows one request of the secret agent's state and sends what the
//! user types back (crates/services/src/network/agent.rs).

use std::rc::Rc;

use gniri_services::network::agent::{Command, Request};
use gtk::prelude::*;

use super::{Modal, message_content, wrapped_label};
use crate::services::Services;
use crate::widgets::password_entry::{PasswordEntry, caps_lock_warning};

/// An open password dialog.
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
    content.title.set_label(&request.title);
    // `MessageDialogContent._updateLabelVisibility`
    match &request.message {
        Some(message) => content.description.set_label(message),
        None => content.description.set_visible(false),
    }

    // One entry per secret, in the content box itself.
    let entries: Vec<PasswordEntry> = request
        .fields
        .iter()
        .map(|field| {
            let entry = if field.password {
                PasswordEntry::new("prompt-dialog-password-entry")
            } else {
                PasswordEntry::plain("prompt-dialog-password-entry")
            };
            entry.set_hint(&field.label);
            if field.key.is_some() {
                entry.text.set_text(&field.value);
            } else {
                entry.set_fixed(&field.value);
            }
            content.root.append(&entry.widget);
            entry
        })
        .collect();
    if request.fields.iter().any(|field| field.password) {
        content.root.append(&caps_lock_warning());
    }
    if request.wps {
        let note = wrapped_label("message-dialog-description");
        note.set_label("Alternatively you can connect by pushing the “WPS” button on your router");
        content.root.append(&note);
    }
    modal.content.append(&content.root);

    let cancel = {
        let services = Rc::clone(services);
        move || services.network_agent.send(Command::Cancel)
    };
    modal.add_button("Cancel", cancel.clone());
    modal.on_escape(cancel);

    let values = {
        let entries = entries.clone();
        move || -> Vec<String> { entries.iter().map(PasswordEntry::password).collect() }
    };
    // `_onOk`: "do nothing if not valid".
    let connect = {
        let (services, request, values) = (Rc::clone(services), request.clone(), values.clone());
        move || {
            let values = values();
            if request.accepts(&values) {
                services.network_agent.send(Command::Respond(values));
            }
        }
    };
    let connect_button = modal.add_button("Connect", connect.clone());

    // `_updateOkButton`
    let update_button = {
        let (request, connect_button) = (request.clone(), connect_button.clone());
        move || connect_button.set_sensitive(request.accepts(&values()))
    };
    for entry in &entries {
        entry.text.connect_changed({
            let update_button = update_button.clone();
            move |_| update_button()
        });
        entry.text.connect_activate({
            let connect = connect.clone();
            move |_| connect()
        });
    }
    update_button();

    modal.open();
    // `setInitialKeyFocus`: the first entry there is something to type in.
    let first = request.fields.iter().position(|field| field.key.is_some());
    if let Some(entry) = first.and_then(|index| entries.get(index)) {
        entry.grab_focus();
    }
    Dialog { modal }
}
