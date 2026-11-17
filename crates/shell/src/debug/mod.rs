// SPDX-License-Identifier: GPL-3.0-or-later

//! Tools for tests and for looking inside the running shell.

pub mod dump;
pub mod window;

use std::rc::Rc;

use gniri_services::notifications::{Action, Event, Image, Incoming, Notification, Sound, Urgency};

use crate::ipc;
use crate::services::Services;

/// Delivers a notification as if an application had sent it over the bus,
/// and returns its id. This is how scenes and checks make notifications
/// under a fixture, where no notification server runs.
pub fn notify(services: &Rc<Services>, notify: ipc::Notify) -> u32 {
    let id = notify.replaces.unwrap_or_else(|| {
        services
            .notifications
            .state
            .with(|state| state.ids().into_iter().max().unwrap_or(0) + 1)
    });
    let mut actions = Vec::new();
    let mut default_action = false;
    for action in &notify.actions {
        let (key, label) = action.split_once('=').unwrap_or((action, action));
        if key == "default" {
            default_action = true;
        } else {
            actions.push(Action {
                key: key.to_owned(),
                label: label.to_owned(),
            });
        }
    }
    let incoming = Incoming {
        notification: Notification {
            id,
            title: notify.summary,
            body: notify.body,
            image: notify.image.as_deref().and_then(Image::from_hint),
            urgency: match notify.urgency {
                ipc::Urgency::Low => Urgency::Low,
                ipc::Urgency::Normal => Urgency::Normal,
                ipc::Urgency::Critical => Urgency::Critical,
            },
            actions,
            default_action,
            resident: notify.resident,
            transient: notify.transient,
            acknowledged: false,
            sound: notify.sound.map(Sound::Name),
            time: services.time.get(),
        },
        source_key: format!("debug:{}", notify.app),
        source_title: notify.app,
        source_icon: notify.icon.as_deref().and_then(Image::from_hint),
        source_app: None,
    };
    services
        .notifications
        .inject(Event::Notify(Box::new(incoming)));
    id
}
