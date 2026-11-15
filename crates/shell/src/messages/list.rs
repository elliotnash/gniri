//! The list of messages in the date menu: media players, then one group per
//! application with notifications. Port of `MessageView` in
//! `js/ui/messageList.js` and `CalendarMessageList` in `js/ui/calendar.js`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use gniri_core::Subscription;
use gniri_services::mpris::{self, Player};
use gniri_services::notifications::{self, Command};
use gtk::prelude::*;

use super::group::MessageGroup;
use super::message::Message;
use super::{notification_message, show_notification};
use crate::services::Services;

struct Group {
    widget: MessageGroup,
    title: gtk::Label,
    /// By notification id.
    messages: HashMap<u32, Message>,
}

struct MediaCard {
    message: Message,
    play_pause: gtk::Button,
    previous: gtk::Button,
    next: gtk::Button,
}

struct Inner {
    services: Rc<Services>,
    /// `.message-view`
    view: gtk::Box,
    placeholder: gtk::Box,
    clear: gtk::Button,
    /// By source key.
    groups: RefCell<HashMap<String, Group>>,
    /// By bus name.
    players: RefCell<HashMap<String, MediaCard>>,
    /// Corner radius of a card in the list.
    radius: f32,
}

/// The list and what keeps it up to date. Dropping it stops the updates.
pub struct MessageList {
    /// `.message-list`
    pub root: gtk::Overlay,
    inner: Rc<Inner>,
    _subscriptions: Vec<Subscription>,
}

/// `MediaMessage`
fn media_card(inner: &Rc<Inner>, bus_name: &str) -> MediaCard {
    let message = Message::new();
    message.root().add_css_class("media-message");
    message.set_close_visible(false);
    message.set_time(None);

    let control = |icon: &str, command: fn(String) -> mpris::Command| {
        let (services, bus_name) = (Rc::clone(&inner.services), bus_name.to_owned());
        message.add_media_control(icon, move || {
            services.mpris.send(command(bus_name.clone()));
        })
    };
    let previous = control("media-skip-backward-symbolic", mpris::Command::Previous);
    let play_pause = control("media-playback-start-symbolic", mpris::Command::PlayPause);
    let next = control("media-skip-forward-symbolic", mpris::Command::Next);

    // `MediaMessage.vfunc_clicked`: bring the player up.
    message.connect_clicked({
        let (services, bus_name) = (Rc::clone(&inner.services), bus_name.to_owned());
        move || {
            let player = services.mpris.state.with(|state| {
                state
                    .players
                    .iter()
                    .find(|player| player.bus_name == bus_name)
                    .cloned()
            });
            let Some(player) = player else {
                return;
            };
            services.menus.update(|menus| menus.close());
            // The player's own `Raise` may be refused focus; starting the
            // application again is what brings its window up.
            let started = player
                .app
                .as_deref()
                .is_some_and(|app| services.launch_app(app));
            if !started && player.can_raise {
                services.mpris.send(mpris::Command::Raise(bus_name.clone()));
            }
        }
    });
    MediaCard {
        message,
        play_pause,
        previous,
        next,
    }
}

impl MediaCard {
    /// `MediaMessage._update`
    fn update(&self, player: &Player) {
        self.message.set_source(&player.name, player.icon.as_ref());
        self.message.set_title(&player.title);
        self.message.set_body(&player.artists_text());
        self.message.set_icon(Some(&player.picture()));
        if let Some(image) = self
            .play_pause
            .child()
            .and_then(|child| child.downcast::<gtk::Image>().ok())
        {
            image.set_icon_name(Some(player.play_pause_icon()));
        }
        self.previous.set_sensitive(player.can_go_previous);
        self.next.set_sensitive(player.can_go_next);
    }
}

impl Inner {
    fn build_group(self: &Rc<Self>, key: &str) -> Group {
        let title = gtk::Label::builder()
            .css_classes(["message-group-title"])
            .valign(gtk::Align::Center)
            .build();
        let collapse = gtk::Button::builder()
            .css_classes(["message-collapse-button", "icon-button"])
            .halign(gtk::Align::End)
            .valign(gtk::Align::Center)
            .hexpand(true)
            .child(&gtk::Image::from_icon_name("group-collapse-symbolic"))
            .build();
        collapse.connect_clicked({
            let inner = Rc::downgrade(self);
            move |_| {
                if let Some(inner) = inner.upgrade() {
                    inner.set_expanded(None);
                }
            }
        });
        let header = gtk::Box::builder()
            .css_classes(["message-group-header"])
            .hexpand(true)
            .build();
        header.append(&title);
        header.append(&collapse);

        let widget = MessageGroup::new(&header, self.radius);
        widget.set_widget_name(key);
        Group {
            widget,
            title,
            messages: HashMap::new(),
        }
    }

    /// Whether the group of `key` shows as a closed stack.
    fn is_stacked(&self, key: &str) -> bool {
        self.groups
            .borrow()
            .get(key)
            .is_some_and(|group| group.widget.is_stacked())
    }

    /// `MessageView._setExpandedGroup`. Which group is open is part of the
    /// menu's state, so that Escape and the debug interface reach it.
    fn set_expanded(&self, key: Option<String>) {
        self.services.menus.update(|menus| menus.set_group(key));
    }

    fn apply_expanded(&self) {
        let expanded = self
            .services
            .menus
            .with(|menus| menus.group().map(str::to_owned));
        for (key, group) in self.groups.borrow().iter() {
            let open = expanded.as_deref() == Some(key.as_str());
            if !open {
                // `NotificationMessageGroup.collapse`
                for message in group.messages.values() {
                    message.unexpand();
                }
            }
            group.widget.set_expanded(open);
            if expanded.is_some() && !open {
                group.widget.add_css_class("shaded");
            } else {
                group.widget.remove_css_class("shaded");
            }
        }
        // The cards of the players fade back with the other groups.
        for card in self.players.borrow().values() {
            if expanded.is_some() {
                card.message.root().add_css_class("shaded");
            } else {
                card.message.root().remove_css_class("shaded");
            }
        }
    }

    /// Brings the widgets in line with the services' state.
    fn sync(self: &Rc<Self>) {
        let services = &self.services;
        let now = services.time.get();
        let mut wanted: Vec<gtk::Widget> = Vec::new();

        // `MessageView._addPlayer`: players come first.
        let shown: Vec<Player> = services
            .mpris
            .state
            .with(|state| state.shown().cloned().collect());
        {
            let mut players = self.players.borrow_mut();
            players.retain(|bus_name, _| shown.iter().any(|player| player.bus_name == *bus_name));
            for player in &shown {
                let card = players
                    .entry(player.bus_name.clone())
                    .or_insert_with(|| media_card(self, &player.bus_name));
                card.update(player);
                wanted.push(card.message.root().clone().upcast());
            }
        }

        let sources = services
            .notifications
            .state
            .with(|state| state.sources.clone());
        {
            let mut groups = self.groups.borrow_mut();
            groups.retain(|key, _| sources.iter().any(|source| source.key == *key));
            for source in &sources {
                let group = groups
                    .entry(source.key.clone())
                    .or_insert_with(|| self.build_group(&source.key));
                group.title.set_label(source.display_title());
                group.messages.retain(|id, _| {
                    source
                        .notifications
                        .iter()
                        .any(|notification| notification.id == *id)
                });
                let mut cards = Vec::new();
                for notification in &source.notifications {
                    let message = group.messages.entry(notification.id).or_insert_with(|| {
                        let stacked = {
                            let (inner, key) = (Rc::downgrade(self), source.key.clone());
                            move || inner.upgrade().is_some_and(|inner| inner.is_stacked(&key))
                        };
                        let open_group = {
                            let (inner, key) = (Rc::downgrade(self), source.key.clone());
                            move || {
                                if let Some(inner) = inner.upgrade() {
                                    inner.set_expanded(Some(key.clone()));
                                }
                            }
                        };
                        notification_message(services, notification.id, stacked, open_group)
                    });
                    show_notification(message, source, notification, &now);
                    cards.push(message.root().clone().upcast());
                }
                group.widget.set_messages(&cards);
                wanted.push(group.widget.clone().upcast());
            }
        }

        // A group that is gone, or down to one card, is no longer open.
        let open = services
            .menus
            .with(|menus| menus.group().map(str::to_owned));
        let still_there = open.as_ref().is_some_and(|key| {
            sources
                .iter()
                .any(|source| source.key == *key && source.notifications.len() > 1)
        });
        if open.is_some() && !still_there {
            self.set_expanded(None);
        }
        self.apply_expanded();

        // Put the view's children in the wanted order.
        let mut current = Vec::new();
        let mut child = self.view.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            current.push(widget);
        }
        if current != wanted {
            for widget in &current {
                self.view.remove(widget);
            }
            for widget in &wanted {
                self.view.append(widget);
            }
        }

        // `CalendarMessageList`: the placeholder instead of an empty list,
        // and Clear only for what can be closed.
        self.placeholder.set_visible(wanted.is_empty());
        self.clear.set_visible(!wanted.is_empty());
        self.clear.set_sensitive(!sources.is_empty());
    }
}

/// `Placeholder` in `js/ui/calendar.js`
fn placeholder() -> gtk::Box {
    let placeholder = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .css_classes(["message-list-placeholder"])
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .can_target(false)
        .build();
    placeholder.append(&gtk::Image::from_icon_name("no-notifications-symbolic"));
    placeholder.append(&gtk::Label::new(Some("No Notifications")));
    placeholder
}

impl MessageList {
    pub fn new(services: &Rc<Services>) -> Self {
        let view = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["message-view"])
            .build();
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&view)
            .build();

        let clear = gtk::Button::builder()
            .css_classes(["message-list-clear-button", "button"])
            .halign(gtk::Align::Start)
            .child(&gtk::Label::new(Some("Clear")))
            .build();
        clear.connect_clicked({
            let services = Rc::clone(services);
            move |_| services.notifications.send(Command::Clear)
        });
        let controls = gtk::Box::builder()
            .css_classes(["message-list-controls"])
            .build();
        controls.append(&clear);

        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        column.append(&scrolled);
        column.append(&controls);

        let placeholder = placeholder();
        let root = gtk::Overlay::builder()
            .css_classes(["message-list"])
            .vexpand(true)
            .child(&column)
            .build();
        root.add_overlay(&placeholder);

        let radius = gniri_theme::token("message-list-radius")
            .and_then(|value| value.strip_suffix("px"))
            .and_then(|value| value.parse().ok())
            .unwrap_or(18.0);
        let inner = Rc::new(Inner {
            services: Rc::clone(services),
            view,
            placeholder,
            clear,
            groups: RefCell::default(),
            players: RefCell::default(),
            radius,
        });

        let sync = |inner: &Rc<Inner>| {
            let inner: Weak<Inner> = Rc::downgrade(inner);
            move || {
                if let Some(inner) = inner.upgrade() {
                    inner.sync();
                }
            }
        };
        inner.sync();
        let subscriptions = vec![
            services.notifications.state.subscribe({
                let sync = sync(&inner);
                move |_: &notifications::State| sync()
            }),
            services.mpris.state.subscribe({
                let sync = sync(&inner);
                move |_: &mpris::State| sync()
            }),
            // The ages in the headers.
            services.time.subscribe({
                let sync = sync(&inner);
                move |_| sync()
            }),
            services
                .menus
                .select(|menus| menus.group().map(str::to_owned), {
                    let inner = Rc::downgrade(&inner);
                    move |_| {
                        if let Some(inner) = inner.upgrade() {
                            inner.apply_expanded();
                        }
                    }
                }),
        ];

        Self {
            root,
            inner,
            _subscriptions: subscriptions,
        }
    }

    /// The open group, if one is.
    pub fn expanded_group(&self) -> Option<gtk::Widget> {
        let expanded = self
            .inner
            .services
            .menus
            .with(|menus| menus.group().map(str::to_owned))?;
        let groups = self.inner.groups.borrow();
        groups
            .get(&expanded)
            .map(|group| group.widget.clone().upcast())
    }
}
