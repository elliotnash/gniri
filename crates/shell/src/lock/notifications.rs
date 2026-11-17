// SPDX-License-Identifier: GPL-3.0-or-later

//! The notifications the lock screen lists: for each application with
//! notifications nobody has seen yet, its icon, its name and how many.
//! Port of `NotificationsBox` (unlockDialog.js).
//!
//! The lock screen is a process of its own and the notifications are the
//! shell's: it asks the shell with what it already shows, and the shell
//! answers when that is no longer what there is ([`Request::LockNotifications`]).
//!
//! As in GNOME, only notifications about the system
//! (`x-gnome-privacy-scope` "system") are shown in full; of the others
//! only that they are there.
//!
//! [`Request::LockNotifications`]: crate::ipc::Request::LockNotifications

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gniri_services::mpris::{self, Player};
use gniri_services::notifications::{self, Image, Urgency, markup};
use gtk::prelude::*;
use gtk::{gdk, glib};
use serde::{Deserialize, Serialize};

use crate::ipc::{self, Request, Role};
use crate::messages::media::MediaCard;
use crate::services::Services;

/// One application's line on the lock screen.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listed {
    pub title: String,
    /// An icon name or a file; none for one sent as pixels.
    pub icon: Option<String>,
    /// How many of its notifications nobody has seen.
    pub count: usize,
    /// One of them is critical.
    pub critical: bool,
    /// Their titles and texts, where they may be shown.
    pub details: Option<Vec<(String, String)>>,
}

/// What the lock screen lists (`NotificationsBox._sourceAdded` and
/// `_showSource`), in the order the notification list has the sources.
pub fn listed(state: &notifications::State) -> Vec<Listed> {
    state
        .sources
        .iter()
        .filter_map(|source| {
            let unseen: Vec<&notifications::Notification> = source
                .notifications
                .iter()
                .filter(|notification| !notification.acknowledged)
                .collect();
            if unseen.is_empty() {
                return None;
            }
            // `_shouldShowDetails`: every notification of it is about the
            // system.
            let details = source
                .notifications
                .iter()
                .all(|notification| notification.system)
                .then(|| {
                    unseen
                        .iter()
                        .map(|notification| {
                            (
                                notification.title.replace('\n', " "),
                                notification.body.replace('\n', " "),
                            )
                        })
                        .collect()
                });
            Some(Listed {
                title: source.display_title(),
                icon: match &source.icon {
                    Some(Image::Name(name)) => Some(name.clone()),
                    Some(Image::File(path)) => Some(path.display().to_string()),
                    _ => None,
                },
                count: unseen.len(),
                critical: source
                    .notifications
                    .iter()
                    .any(|notification| notification.urgency == Urgency::Critical),
                details,
            })
        })
        .collect()
}

/// How long the shell keeps a request waiting before it answers with what
/// is unchanged: the lock screen asks again at once.
const LONGEST_WAIT: Duration = Duration::from_secs(30);

/// The shell's answer to the lock screen: what it lists now, once that is
/// not `known`.
pub async fn answer(services: Rc<Services>, known: Option<Vec<Listed>>) -> ipc::Reply {
    let now = || services.notifications.state.with(listed);
    if known.as_ref() != Some(&now()) {
        return serde_json::to_value(now()).map_err(|error| error.to_string());
    }
    let (changed, change) = async_channel::unbounded::<()>();
    let subscription = services.notifications.state.subscribe(move |_| {
        let _ = changed.try_send(());
    });
    let timeout = glib::timeout_future(LONGEST_WAIT);
    let mut timeout = std::pin::pin!(timeout);
    loop {
        let changed = futures_or(change.recv(), &mut timeout).await;
        if !changed || known.as_ref() != Some(&now()) {
            break;
        }
    }
    drop(subscription);
    serde_json::to_value(now()).map_err(|error| error.to_string())
}

/// Waits for `first`, true, or `second`, false.
async fn futures_or<A, B>(first: A, second: &mut std::pin::Pin<&mut B>) -> bool
where
    A: std::future::Future,
    B: std::future::Future,
{
    use std::task::Poll;
    let mut first = std::pin::pin!(first);
    std::future::poll_fn(|context| {
        if first.as_mut().poll(context).is_ready() {
            Poll::Ready(true)
        } else if second.as_mut().poll(context).is_ready() {
            Poll::Ready(false)
        } else {
            Poll::Pending
        }
    })
    .await
}

/// Keeps asking the shell for what to list, on a thread of its own, and
/// hands each change to the main loop.
pub fn follow() -> async_channel::Receiver<Vec<Listed>> {
    let (sender, receiver) = async_channel::unbounded();
    let spawned = std::thread::Builder::new()
        .name("lock-notifications".to_owned())
        .spawn(move || {
            let mut known: Option<Vec<Listed>> = None;
            loop {
                let request = Request::LockNotifications(known.clone());
                match ipc::send(Role::Shell, &request)
                    .ok()
                    .and_then(|value| serde_json::from_value::<Vec<Listed>>(value).ok())
                {
                    Some(listed) => {
                        if known.as_ref() != Some(&listed) {
                            if sender.send_blocking(listed.clone()).is_err() {
                                return;
                            }
                            known = Some(listed);
                        }
                    }
                    // No shell to ask (a lock screen started by hand), or
                    // one that went: ask again later.
                    None => std::thread::sleep(Duration::from_secs(5)),
                }
            }
        });
    if let Err(error) = spawned {
        eprintln!("gniri: lock: cannot start a thread: {error}");
    }
    receiver
}

/// The box at the foot of a lock surface (`#unlockDialogNotifications`):
/// the media players' cards, then the applications.
pub struct NotificationsBox {
    pub widget: gtk::ScrolledWindow,
    container: gtk::Box,
    /// By bus name, in the order they are shown.
    cards: RefCell<Vec<(String, MediaCard)>>,
    rows: RefCell<Vec<gtk::Box>>,
    /// Where the cards' controls send their commands.
    send: Rc<dyn Fn(mpris::Command)>,
}

impl NotificationsBox {
    pub fn new(send: impl Fn(mpris::Command) + 'static) -> Self {
        let container = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["unlock-dialog-notifications-container"])
            .build();
        let widget = gtk::ScrolledWindow::builder()
            .name("unlockDialogNotifications")
            .child(&container)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::End)
            .visible(false)
            .build();
        Self {
            widget,
            container,
            cards: RefCell::default(),
            rows: RefCell::default(),
            send: Rc::new(send),
        }
    }

    /// Shows `listed` (`_sourceAdded` and `_showSource`).
    pub fn show(&self, listed: &[Listed]) {
        *self.rows.borrow_mut() = listed.iter().map(source_box).collect();
        self.arrange();
    }

    /// Shows a card for each of `players` (`_addPlayer`, `_removePlayer`).
    pub fn show_players(&self, players: &[Player]) {
        {
            let mut cards = self.cards.borrow_mut();
            cards.retain(|(bus_name, _)| players.iter().any(|player| player.bus_name == *bus_name));
            for player in players {
                let index = match cards
                    .iter()
                    .position(|(bus_name, _)| *bus_name == player.bus_name)
                {
                    Some(index) => index,
                    None => {
                        let send = Rc::clone(&self.send);
                        // `MediaMessage.vfunc_clicked`: nothing to raise
                        // while the session is locked.
                        let card =
                            MediaCard::new(&player.bus_name, move |command| send(command), |_| {});
                        // The newest player first, as `insert_child_at_index(…, 0)`.
                        cards.insert(0, (player.bus_name.clone(), card));
                        0
                    }
                };
                cards[index].1.update(player);
            }
        }
        self.arrange();
    }

    /// The players, then the applications; nothing to show hides the box
    /// (`_updateVisibility`).
    fn arrange(&self) {
        while let Some(child) = self.container.first_child() {
            self.container.remove(&child);
        }
        for (_, card) in self.cards.borrow().iter() {
            self.container.append(card.message.root());
        }
        for row in self.rows.borrow().iter() {
            self.container.append(row);
        }
        self.widget
            .set_visible(self.container.first_child().is_some());
    }
}

fn icon(name: Option<&str>) -> gtk::Image {
    let image = gtk::Image::builder()
        .css_classes(["unlock-dialog-notification-icon"])
        .build();
    match name {
        Some(path) if path.starts_with('/') => match gdk::Texture::from_filename(path) {
            Ok(texture) => image.set_paintable(Some(&texture)),
            Err(_) => image.set_icon_name(Some("application-x-executable")),
        },
        Some(name) => image.set_icon_name(Some(name)),
        None => image.set_icon_name(Some("application-x-executable")),
    }
    image
}

/// `_makeNotificationSource` and `_makeNotificationDetailedSource`.
fn source_box(source: &Listed) -> gtk::Box {
    let row = gtk::Box::builder()
        .css_classes(["unlock-dialog-notification-source"])
        .hexpand(true)
        .build();
    if source.critical {
        row.add_css_class("critical");
    }
    row.append(&icon(source.icon.as_deref()));
    let title = gtk::Label::builder()
        .label(source.title.replace('\n', " "))
        .css_classes(["unlock-dialog-notification-label"])
        .xalign(0.0)
        .build();
    match &source.details {
        Some(details) => {
            let text = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .build();
            text.append(&title);
            for (summary, body) in details {
                let line = gtk::Label::builder()
                    .css_classes(["unlock-dialog-notification-count-text"])
                    .xalign(0.0)
                    .wrap(true)
                    .build();
                let summary = glib::markup_escape_text(summary);
                line.set_markup(&format!("<b>{summary}</b> {}", markup::fix(body, true)));
                text.append(&line);
            }
            row.append(&text);
        }
        None => {
            let text = gtk::Box::builder()
                .hexpand(true)
                .valign(gtk::Align::Center)
                .build();
            title.set_hexpand(true);
            text.append(&title);
            let count = gtk::Label::builder()
                .label(source.count.to_string())
                .css_classes(["unlock-dialog-notification-count-text"])
                .visible(source.count > 1)
                .build();
            text.append(&count);
            row.append(&text);
        }
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;
    use gniri_services::notifications::{Notification, Source};

    fn notification(id: u32, seen: bool, system: bool) -> Notification {
        Notification {
            id,
            title: format!("Title {id}"),
            body: "Line\nbreak".to_owned(),
            acknowledged: seen,
            system,
            ..Notification::default()
        }
    }

    fn source(key: &str, notifications: Vec<Notification>) -> Source {
        Source {
            key: key.to_owned(),
            title: key.to_owned(),
            icon: Some(Image::Name(format!("{key}-icon"))),
            app: None,
            notifications,
        }
    }

    #[test]
    fn what_nobody_has_seen_is_listed_by_application() {
        let state = notifications::State {
            sources: vec![
                source(
                    "Chat",
                    vec![notification(1, false, false), notification(2, false, false)],
                ),
                source("Seen", vec![notification(3, true, false)]),
                source(
                    "Updates",
                    vec![notification(4, false, true), notification(5, true, true)],
                ),
            ],
            ..notifications::State::default()
        };
        let listed = listed(&state);
        assert_eq!(listed.len(), 2, "nothing for what was seen");
        assert_eq!(
            listed[0],
            Listed {
                title: "Chat".to_owned(),
                icon: Some("Chat-icon".to_owned()),
                count: 2,
                critical: false,
                details: None,
            }
        );
        // About the system only: what it says, of what was not seen.
        assert_eq!(
            listed[1].details,
            Some(vec![("Title 4".to_owned(), "Line break".to_owned())])
        );
    }
}
