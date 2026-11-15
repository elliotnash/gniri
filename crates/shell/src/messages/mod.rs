// SPDX-License-Identifier: GPL-3.0-or-later

//! Messages: notifications and media players, as the banner and the list in
//! the date menu show them. Port of `js/ui/messageList.js`.

mod group;
mod list;
mod message;

use std::rc::Rc;

use gniri_core::LocalTime;
use gniri_services::notifications::{self, Command, Notification, Source};
use gtk::prelude::*;
use gtk::{gdk, gio};

pub use self::list::MessageList;
pub use self::message::Message;
use crate::services::Services;

/// A token the application can use to bring its window to the front
/// (`FdoNotificationDaemon._emitActivationToken`).
fn activation_token() -> Option<String> {
    let context = gdk::Display::default()?.app_launch_context();
    context
        .startup_notify_id(None::<&gio::AppInfo>, &[])
        .map(Into::into)
}

/// What a click on a notification does: its default action, or starting
/// the application it came from. Either way the menu it was in closes.
pub fn activate(services: &Rc<Services>, id: u32) {
    let found = services.notifications.state.with(|state| {
        state
            .find(id)
            .map(|(source, notification)| (source.app.clone(), notification.default_action))
    });
    let Some((app, default_action)) = found else {
        return;
    };
    let token = activation_token();
    services.menus.update(|menus| menus.close());
    services.notifications.send(Command::Activate { id, token });
    if !default_action && let Some(app) = app {
        services.launch_app(&app);
    }
}

/// What a click on one of a notification's buttons does.
pub fn invoke_action(services: &Rc<Services>, id: u32, key: &str) {
    let token = activation_token();
    services.notifications.send(Command::InvokeAction {
        id,
        key: key.to_owned(),
        token,
    });
}

/// How long ago `notification` arrived, as its header says it.
pub fn age(notification: &Notification, now: &LocalTime) -> String {
    gniri_core::time_span_text(now.seconds_since(&notification.time))
}

/// Fills `message` in from a notification (`NotificationMessage`).
pub fn show_notification(
    message: &Message,
    source: &Source,
    notification: &Notification,
    now: &LocalTime,
) {
    message.set_source(source.display_title(), source.icon.as_ref());
    message.set_time(Some(&age(notification, now)));
    message.set_title(&notification.title);
    message.set_body(&notification.body);
    message.set_icon(notification.image.as_ref());
    message.set_actions(&notification.actions);
}

/// A message for the notification `id`, with what its buttons and a click on
/// it do. `stacked` says whether it is the visible card of a closed group
/// right now, and `open_group` opens that group: a closed group acts as
/// one.
pub fn notification_message(
    services: &Rc<Services>,
    id: u32,
    stacked: impl Fn() -> bool + Clone + 'static,
    open_group: impl Fn() + Clone + 'static,
) -> Message {
    let message = Message::new();
    message.connect_clicked({
        let (services, stacked, open_group) =
            (Rc::clone(services), stacked.clone(), open_group.clone());
        move || {
            if stacked() {
                open_group();
            } else {
                activate(&services, id);
            }
        }
    });
    message.connect_close({
        let (services, stacked) = (Rc::clone(services), stacked.clone());
        move || {
            let source = services
                .notifications
                .state
                .with(|state| state.find(id).map(|(source, _)| source.key.clone()));
            match source {
                // `NotificationMessageGroup.close`
                Some(key) if stacked() => {
                    services.notifications.send(Command::DismissSource(key));
                }
                _ => services.notifications.send(Command::Dismiss(id)),
            }
        }
    });
    message.connect_may_expand(move || {
        if stacked() {
            open_group();
            false
        } else {
            true
        }
    });
    message.connect_action({
        let services = Rc::clone(services);
        move |key| invoke_action(&services, id, key)
    });
    message
}

/// The state's notification `id` with its source, cloned out.
pub fn find(state: &notifications::State, id: u32) -> Option<(Source, Notification)> {
    state
        .find(id)
        .map(|(source, notification)| (source.clone(), notification.clone()))
}
