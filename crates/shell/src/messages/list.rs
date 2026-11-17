// SPDX-License-Identifier: GPL-3.0-or-later

//! The list of messages in the date menu: media players, then one group per
//! application with notifications. Port of `MessageView` in
//! `js/ui/messageList.js` and `CalendarMessageList` in `js/ui/calendar.js`.

use gniri_core::tr;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use gniri_core::Subscription;
use gniri_services::mpris::{self, Player};
use gniri_services::notifications::{self, Command};
use gtk::glib;
use gtk::prelude::*;

use super::group::MessageGroup;
use super::media::MediaCard;
use super::message::Message;
use super::{notification_message, show_notification};
use crate::services::Services;
use crate::widgets::motion::Eased;
use crate::widgets::scale_bin::ScaleBin;

struct Group {
    widget: MessageGroup,
    title: gtk::Label,
    /// By notification id.
    messages: HashMap<u32, Message>,
}

/// messageList.js `MESSAGE_ANIMATION_TIME`
const MESSAGE_ANIMATION_TIME: u32 = 100;

/// One of the view's items, a group or a player's card, in the bin it
/// grows and shrinks in (`MessageView._addMessageAtIndex`).
struct Item {
    /// The group's widget or the card's root.
    widget: gtk::Widget,
    bin: ScaleBin,
    scale: Rc<Eased>,
    /// Clear's sweep: 0 in place, 1 gone to the right, faded out.
    sweep: Rc<Eased>,
    /// Shrinking away; it goes once it has.
    leaving: Cell<bool>,
    /// A notification group, which Clear takes, rather than a player.
    closable: bool,
}

struct Inner {
    services: Rc<Services>,
    /// `.message-view`
    view: gtk::Box,
    /// The view's children, those shrinking away included, in order.
    items: RefCell<Vec<Rc<Item>>>,
    /// Clear is sweeping the groups out.
    clearing: Cell<bool>,
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

/// `MediaMessage` in the list: its controls go to the player, and a
/// click brings the player up and closes the menu.
fn media_card(inner: &Rc<Inner>, bus_name: &str) -> MediaCard {
    let send = {
        let services = Rc::clone(&inner.services);
        move |command: mpris::Command| services.mpris.send(command)
    };
    let raise = {
        let services = Rc::clone(&inner.services);
        move |bus_name: &str| {
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
                services
                    .mpris
                    .send(mpris::Command::Raise(bus_name.to_owned()));
            }
        }
    };
    MediaCard::new(bus_name, send, raise)
}

impl Inner {
    /// A bin for `widget`, growing in.
    fn new_item(self: &Rc<Self>, widget: &gtk::Widget, closable: bool) -> Rc<Item> {
        let bin = ScaleBin::new(widget);
        bin.set_scale(0.0);
        let scale = Rc::new(Eased::new(&bin, 0.0, {
            let bin = bin.downgrade();
            move |scale| {
                if let Some(bin) = bin.upgrade() {
                    bin.set_scale(scale);
                }
            }
        }));
        let sweep = Rc::new(Eased::new(&bin, 0.0, {
            let bin = bin.downgrade();
            move |sweep| {
                if let Some(bin) = bin.upgrade() {
                    bin.set_shift(sweep * f64::from(bin.width()));
                    bin.set_opacity(1.0 - sweep);
                }
            }
        }));
        Rc::new(Item {
            widget: widget.clone(),
            bin,
            scale,
            sweep,
            leaving: Cell::new(false),
            closable,
        })
    }

    /// `MessageView._removeMessage`: shrinks `item` away, then lets it go.
    fn remove_item(self: &Rc<Self>, item: &Rc<Item>) {
        item.leaving.set(true);
        let (inner, gone) = (Rc::downgrade(self), Rc::downgrade(item));
        item.scale.ease(
            0.0,
            MESSAGE_ANIMATION_TIME,
            adw::Easing::EaseOutQuad,
            move || {
                // Not from inside a sync, which may have started this.
                glib::idle_add_local_once(move || {
                    let (Some(inner), Some(gone)) = (inner.upgrade(), gone.upgrade()) else {
                        return;
                    };
                    inner
                        .items
                        .borrow_mut()
                        .retain(|item| !Rc::ptr_eq(item, &gone));
                    if gone.bin.parent().is_some() {
                        inner.view.remove(&gone.bin);
                    }
                    gone.bin.take_child();
                });
            },
        );
    }

    /// `MessageView.clear`: one or no group goes as any would; more slide
    /// out to the right one after another first.
    fn clear(self: &Rc<Self>) {
        if self.clearing.get() {
            return;
        }
        let swept: Vec<Rc<Item>> = self
            .items
            .borrow()
            .iter()
            .filter(|item| item.closable && !item.leaving.get())
            .cloned()
            .collect();
        if swept.len() < 2 {
            self.services.notifications.send(Command::Clear);
            return;
        }
        self.clearing.set(true);
        // "MESSAGE_ANIMATION_TIME / Math.max(messages.length, 5)"
        let delay = MESSAGE_ANIMATION_TIME / swept.len().max(5) as u32;
        let last = swept.len() - 1;
        for (index, item) in swept.into_iter().enumerate() {
            let inner = Rc::downgrade(self);
            let start = move || {
                item.sweep.ease(
                    1.0,
                    MESSAGE_ANIMATION_TIME,
                    adw::Easing::EaseOutQuad,
                    move || {
                        if index == last
                            && let Some(inner) = inner.upgrade()
                        {
                            inner.clearing.set(false);
                            inner.services.notifications.send(Command::Clear);
                        }
                    },
                );
            };
            if index == 0 {
                start();
            } else {
                glib::timeout_add_local_once(
                    std::time::Duration::from_millis(u64::from(delay) * index as u64),
                    start,
                );
            }
        }
    }

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
        let mut wanted: Vec<(gtk::Widget, bool)> = Vec::new();

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
                wanted.push((card.message.root().clone().upcast(), false));
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
                group.title.set_label(&source.display_title());
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
                wanted.push((group.widget.clone().upcast(), true));
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

        // Each wanted widget in its bin, new ones growing in; what is no
        // longer wanted shrinks away where it was.
        let old: Vec<Rc<Item>> = self.items.borrow().clone();
        let mut row: Vec<Rc<Item>> = Vec::new();
        let mut grown = Vec::new();
        for (widget, closable) in &wanted {
            match old
                .iter()
                .find(|item| item.widget == *widget && !item.leaving.get())
            {
                Some(item) => row.push(Rc::clone(item)),
                None => {
                    let item = self.new_item(widget, *closable);
                    grown.push(Rc::clone(&item));
                    row.push(item);
                }
            }
        }
        let gone: Vec<Rc<Item>> = old
            .iter()
            .filter(|item| !item.leaving.get() && !row.iter().any(|kept| Rc::ptr_eq(kept, item)))
            .cloned()
            .collect();
        for (index, item) in old.iter().enumerate() {
            if !row.iter().any(|kept| Rc::ptr_eq(kept, item)) {
                row.insert(index.min(row.len()), Rc::clone(item));
            }
        }
        let mut previous: Option<gtk::Widget> = None;
        for item in &row {
            if item.bin.parent().is_none() {
                self.view.insert_child_after(&item.bin, previous.as_ref());
            } else {
                self.view.reorder_child_after(&item.bin, previous.as_ref());
            }
            previous = Some(item.bin.clone().upcast());
        }
        *self.items.borrow_mut() = row;
        for item in grown {
            item.scale
                .ease(1.0, MESSAGE_ANIMATION_TIME, adw::Easing::EaseOutQuad, || {});
        }
        for item in &gone {
            self.remove_item(item);
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
    placeholder.append(&gtk::Label::new(Some(&tr("No Notifications"))));
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
            .child(&gtk::Label::new(Some(&tr("Clear"))))
            .build();

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
            items: RefCell::default(),
            clearing: Cell::new(false),
        });
        inner.clear.connect_clicked({
            let inner = Rc::downgrade(&inner);
            move |_| {
                if let Some(inner) = inner.upgrade() {
                    inner.clear();
                }
            }
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
