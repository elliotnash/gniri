//! Notifications: what applications asked to show, which of them the banner
//! shows next, and what the list in the date menu holds.
//!
//! Port of the bookkeeping in gnome-shell's `js/ui/messageTray.js`
//! (`Source`, `Notification`, the banner queue of `MessageTray`) and of the
//! ordering `js/ui/messageList.js` gives groups and messages. The backend
//! (`backend`) serves `org.freedesktop.Notifications`; everything it and the
//! shell decide is in this file, as plain functions of the state.

pub mod backend;
pub mod markup;

use std::path::PathBuf;
use std::sync::Arc;

use gniri_core::LocalTime;
use serde::Deserialize;

use crate::model::Model;

/// messageTray.js `MAX_NOTIFICATIONS_IN_QUEUE`.
const MAX_NOTIFICATIONS_IN_QUEUE: usize = 3;
/// messageTray.js `MAX_NOTIFICATIONS_PER_SOURCE`.
const MAX_NOTIFICATIONS_PER_SOURCE: usize = 10;

/// The urgency hint of the notification specification. (GNOME's own HIGH
/// level is only reachable through its private interface.)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Urgency {
    Low,
    #[default]
    Normal,
    Critical,
}

impl Urgency {
    /// The `urgency` hint's byte.
    pub fn from_hint(value: u8) -> Self {
        match value {
            0 => Self::Low,
            2 => Self::Critical,
            _ => Self::Normal,
        }
    }
}

/// Raw pixels from an `image-data` hint: 8 bits per sample, RGB or RGBA.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pixels {
    pub width: i32,
    pub height: i32,
    pub rowstride: i32,
    pub has_alpha: bool,
    pub data: Vec<u8>,
}

/// A picture a notification or its source comes with.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Image {
    /// An icon of the icon theme.
    Name(String),
    File(PathBuf),
    /// Shared, since the state is copied on every change.
    #[serde(skip)]
    Pixels(Arc<Pixels>),
}

impl Image {
    /// `FdoNotificationDaemon._iconForNotificationData`: a path, a file URI
    /// or an icon name.
    pub fn from_hint(text: &str) -> Option<Self> {
        if text.is_empty() {
            None
        } else if let Some(path) = text.strip_prefix("file://") {
            Some(Self::File(PathBuf::from(percent_decode(path))))
        } else if text.starts_with('/') {
            Some(Self::File(PathBuf::from(text)))
        } else {
            Some(Self::Name(text.to_owned()))
        }
    }

    /// Whether this is an icon the theme draws in the text colour.
    pub fn is_symbolic(&self) -> bool {
        matches!(self, Self::Name(name) if name.ends_with("-symbolic"))
    }
}

/// Decodes `%XX` escapes; anything malformed is kept as it is.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let escape = (bytes[index] == b'%')
            .then(|| text.get(index + 1..index + 3))
            .flatten()
            .and_then(|digits| u8::from_str_radix(digits, 16).ok());
        match escape {
            Some(byte) => {
                decoded.push(byte);
                index += 3;
            }
            None => {
                decoded.push(bytes[index]);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// A button on a notification.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Action {
    /// What the application is told was chosen.
    pub key: String,
    pub label: String,
}

fn deserialize_time<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<LocalTime, D::Error> {
    let text = String::deserialize(deserializer)?;
    LocalTime::parse(&text).map_err(serde::de::Error::custom)
}

fn epoch() -> LocalTime {
    LocalTime {
        year: 1970,
        month: 1,
        day: 1,
        hour: 0,
        minute: 0,
        second: 0,
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Notification {
    pub id: u32,
    pub title: String,
    /// As the application sent it: it may hold `<b>`, `<i>` and `<u>`
    /// (see [`markup::fix`]).
    pub body: String,
    pub image: Option<Image>,
    pub urgency: Urgency,
    pub actions: Vec<Action>,
    /// The application wants to hear about a click on the notification
    /// itself (the "default" action).
    pub default_action: bool,
    /// Stays after one of its actions was invoked.
    pub resident: bool,
    /// Goes away with its banner instead of staying in the list.
    pub transient: bool,
    /// Seen: shown as a banner, or listed while the date menu was open.
    pub acknowledged: bool,
    /// When it arrived or last changed, `YYYY-MM-DDTHH:MM:SS` in a scene.
    #[serde(deserialize_with = "deserialize_time")]
    pub time: LocalTime,
}

impl Default for Notification {
    fn default() -> Self {
        Self {
            id: 0,
            title: String::new(),
            body: String::new(),
            image: None,
            urgency: Urgency::Normal,
            actions: Vec::new(),
            default_action: false,
            resident: false,
            transient: false,
            acknowledged: false,
            time: epoch(),
        }
    }
}

/// An application's notifications: one group in the list.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Source {
    /// What tells sources apart: the application, or the sending process.
    pub key: String,
    /// Empty when the sender gave no name; shown as "Unknown App".
    pub title: String,
    pub icon: Option<Image>,
    /// The desktop entry (without ".desktop") of the application, if one
    /// was found. Clicking a notification that has no default action
    /// starts it.
    pub app: Option<String>,
    /// As the list shows them: critical ones first, then newest first.
    pub notifications: Vec<Notification>,
}

impl Source {
    /// `MessageHeader`'s fallback for a source without a name.
    pub fn display_title(&self) -> &str {
        if self.title.is_empty() {
            "Unknown App"
        } else {
            &self.title
        }
    }

    fn has_critical(&self) -> bool {
        self.notifications
            .iter()
            .any(|notification| notification.urgency == Urgency::Critical)
    }
}

/// The notification the banner shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Banner {
    pub id: u32,
    /// Counts changes to the notification while its banner is up; each one
    /// restarts the banner's time on screen.
    pub updates: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct State {
    /// As the list shows them: sources with a critical notification first,
    /// then the one that notified last.
    pub sources: Vec<Source>,
    pub banner: Option<Banner>,
    /// Notifications waiting for the banner, most urgent first.
    pub queue: Vec<u32>,
    /// No banners, except for critical notifications. GNOME's
    /// `show-banners` setting, inverted.
    pub do_not_disturb: bool,
}

/// A notification as the backend received it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Incoming {
    /// A new id, or that of the notification this one replaces.
    pub notification: Notification,
    pub source_key: String,
    pub source_title: String,
    pub source_icon: Option<Image>,
    pub source_app: Option<String>,
}

/// What the backend reports.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Notify(Box<Incoming>),
    /// The application withdrew a notification (`CloseNotification`).
    Closed(u32),
    /// The application behind a source left the bus.
    SourceGone(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// The user closed one notification.
    Dismiss(u32),
    /// The user closed a collapsed group: every notification of a source.
    DismissSource(String),
    /// The Clear button.
    Clear,
    /// A click on the notification itself. `token` is an activation token
    /// the application may use to focus its window.
    Activate { id: u32, token: Option<String> },
    /// A click on one of its buttons.
    InvokeAction {
        id: u32,
        key: String,
        token: Option<String>,
    },
    /// The banner is free: show the next waiting notification on it.
    ShowNextBanner,
    /// The banner finished hiding by itself.
    BannerHidden,
    /// The list is on screen: everything in it has been seen.
    AcknowledgeAll,
}

/// Why a notification is gone, as `NotificationClosed` reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClosedReason {
    Expired = 1,
    Dismissed = 2,
    ClosedByApp = 3,
}

impl State {
    pub fn notifications(&self) -> impl Iterator<Item = &Notification> {
        self.sources
            .iter()
            .flat_map(|source| source.notifications.iter())
    }

    pub fn ids(&self) -> Vec<u32> {
        self.notifications()
            .map(|notification| notification.id)
            .collect()
    }

    pub fn find(&self, id: u32) -> Option<(&Source, &Notification)> {
        self.sources.iter().find_map(|source| {
            source
                .notifications
                .iter()
                .find(|notification| notification.id == id)
                .map(|notification| (source, notification))
        })
    }

    fn find_mut(&mut self, id: u32) -> Option<&mut Notification> {
        self.sources
            .iter_mut()
            .flat_map(|source| source.notifications.iter_mut())
            .find(|notification| notification.id == id)
    }

    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    /// Whether the panel shows the dot beside the clock: something has not
    /// been seen and is not about to be shown as a banner either.
    /// `MessagesIndicator` in `js/ui/dateMenu.js`.
    pub fn shows_indicator(&self) -> bool {
        let unseen = self
            .notifications()
            .filter(|notification| !notification.acknowledged)
            .count();
        !self.do_not_disturb && unseen > self.queue.len()
    }

    /// `MessageTray._onNotificationRequestBanner`.
    fn request_banner(&mut self, id: u32) {
        let Some((_, notification)) = self.find(id) else {
            return;
        };
        let urgency = notification.urgency;
        if notification.acknowledged || urgency == Urgency::Low {
            return;
        }
        if self.do_not_disturb && urgency != Urgency::Critical {
            return;
        }
        if let Some(banner) = self.banner.as_mut().filter(|banner| banner.id == id) {
            // The banner showing it is updated and stays up longer.
            banner.updates += 1;
            self.acknowledge(id);
            return;
        }
        if self.queue.contains(&id) {
            return;
        }
        // A full queue only takes critical notifications: the rest wait in
        // the list, with the dot beside the clock.
        let waiting = self.queue.len() + usize::from(self.banner.is_some());
        if waiting < MAX_NOTIFICATIONS_IN_QUEUE || urgency == Urgency::Critical {
            self.queue.push(id);
            let mut queue = std::mem::take(&mut self.queue);
            // Stable: equally urgent ones keep their order of arrival.
            queue.sort_by_key(|id| {
                let urgency = self
                    .find(*id)
                    .map_or(Urgency::Low, |(_, notification)| notification.urgency);
                std::cmp::Reverse(urgency)
            });
            self.queue = queue;
        }
    }

    fn acknowledge(&mut self, id: u32) {
        if let Some(notification) = self.find_mut(id) {
            notification.acknowledged = true;
        }
        self.queue.retain(|other| *other != id);
    }

    /// Where a source goes in the list: one with a critical notification at
    /// the top, any other below those (`MessageView._addNotificationSource`
    /// and its `notification-added` handler).
    fn place_source(&mut self, index: usize) {
        let source = self.sources.remove(index);
        let position = if source.has_critical() {
            0
        } else {
            self.sources
                .iter()
                .filter(|other| other.has_critical())
                .count()
        };
        self.sources.insert(position, source);
    }

    /// Where a notification goes in its group: critical ones at the top,
    /// any other below those (`NotificationMessageGroup._addNotification`).
    fn place_notification(source: &mut Source, notification: Notification) {
        let position = if notification.urgency == Urgency::Critical {
            0
        } else {
            source
                .notifications
                .iter()
                .filter(|other| other.urgency == Urgency::Critical)
                .count()
        };
        source.notifications.insert(position, notification);
    }

    fn notify(&mut self, incoming: Incoming) {
        let Incoming {
            mut notification,
            source_key,
            source_title,
            source_icon,
            source_app,
        } = incoming;
        let id = notification.id;

        // A replacement keeps its place, unless its urgency changed.
        if let Some(index) = self
            .sources
            .iter()
            .position(|source| source.notifications.iter().any(|other| other.id == id))
        {
            let source = &mut self.sources[index];
            if source.app.is_none() {
                source.title = source_title;
                source.icon = source_icon;
            }
            let position = source
                .notifications
                .iter()
                .position(|other| other.id == id)
                .expect("the source was found by this id");
            let was_acknowledged = source.notifications[position].acknowledged;
            let urgency_changed = source.notifications[position].urgency != notification.urgency;
            notification.acknowledged = false;
            if urgency_changed {
                source.notifications.remove(position);
                Self::place_notification(source, notification);
                self.place_source(index);
            } else {
                source.notifications[position] = notification;
            }
            // Something already seen asks for the banner again; something
            // still waiting keeps its turn.
            if was_acknowledged || self.banner.is_some_and(|banner| banner.id == id) {
                self.request_banner(id);
            }
            return;
        }

        let index = match self
            .sources
            .iter()
            .position(|source| source.key == source_key)
        {
            Some(index) => {
                let source = &mut self.sources[index];
                if source.app.is_none() {
                    source.title = source_title;
                    source.icon = source_icon;
                }
                index
            }
            None => {
                self.sources.push(Source {
                    key: source_key,
                    title: source_title,
                    icon: source_icon,
                    app: source_app,
                    notifications: Vec::new(),
                });
                self.sources.len() - 1
            }
        };

        // Make room: the oldest go first (`Source.addNotification`).
        while self.sources[index].notifications.len() >= MAX_NOTIFICATIONS_PER_SOURCE {
            let oldest = self.sources[index]
                .notifications
                .iter()
                .map(|other| other.id)
                .min()
                .expect("the source is not empty");
            self.sources[index]
                .notifications
                .retain(|other| other.id != oldest);
            self.forget(oldest);
        }

        notification.acknowledged = false;
        Self::place_notification(&mut self.sources[index], notification);
        self.place_source(index);
        self.request_banner(id);
    }

    /// Drops what refers to a notification that is no longer there.
    fn forget(&mut self, id: u32) {
        self.queue.retain(|other| *other != id);
        if self.banner.is_some_and(|banner| banner.id == id) {
            self.banner = None;
        }
    }

    /// Removes the notifications `gone` holds for, and any source left
    /// empty.
    fn remove_where(&mut self, gone: impl Fn(&Source, &Notification) -> bool) {
        let mut removed = Vec::new();
        for source in &mut self.sources {
            let (kept, dropped): (Vec<_>, Vec<_>) = std::mem::take(&mut source.notifications)
                .into_iter()
                .partition(|notification| !gone(source, notification));
            source.notifications = kept;
            removed.extend(dropped.into_iter().map(|notification| notification.id));
        }
        self.sources
            .retain(|source| !source.notifications.is_empty());
        for id in removed {
            self.forget(id);
        }
    }

    fn remove(&mut self, id: u32) {
        self.remove_where(|_, notification| notification.id == id);
    }
}

impl Model for State {
    type Event = Event;
    type Command = Command;

    fn apply(&mut self, event: Event) {
        match event {
            Event::Notify(incoming) => self.notify(*incoming),
            Event::Closed(id) => self.remove(id),
            Event::SourceGone(key) => self.remove_where(|source, _| source.key == key),
        }
    }

    /// The shell's commands are the whole truth about what the user did, so
    /// these are not guesses: no event follows to correct them.
    fn predict(&mut self, command: &Command) {
        match command {
            Command::Dismiss(id) => self.remove(*id),
            Command::DismissSource(key) => self.remove_where(|source, _| source.key == *key),
            Command::Clear => self.remove_where(|_, _| true),
            Command::Activate { id, .. } => {
                let Some((source, notification)) = self.find(*id) else {
                    return;
                };
                if notification.default_action {
                    // `Notification.activate`
                    if !notification.resident {
                        self.remove(*id);
                    }
                } else {
                    // `FdoNotificationDaemonSource.open`: the application is
                    // brought up, and what it had to say counts as read.
                    let key = source.key.clone();
                    self.remove_where(|source, notification| {
                        source.key == key && !notification.resident
                    });
                }
            }
            Command::InvokeAction { id, .. } => {
                // `Notification.addAction`
                if self
                    .find(*id)
                    .is_some_and(|(_, notification)| !notification.resident)
                {
                    self.remove(*id);
                }
            }
            Command::ShowNextBanner => {
                if self.banner.is_none() && !self.queue.is_empty() {
                    let id = self.queue.remove(0);
                    self.banner = Some(Banner { id, updates: 0 });
                    // `MessageTray._updateShowingNotification`
                    self.acknowledge(id);
                }
            }
            Command::BannerHidden => {
                // `MessageTray._hideNotificationCompleted`
                if let Some(banner) = self.banner.take()
                    && self
                        .find(banner.id)
                        .is_some_and(|(_, notification)| notification.transient)
                {
                    self.remove(banner.id);
                }
            }
            Command::AcknowledgeAll => {
                // `NotificationMessageGroup.vfunc_map`
                for source in &mut self.sources {
                    for notification in &mut source.notifications {
                        notification.acknowledged = true;
                    }
                }
                self.queue.clear();
            }
        }
    }
}

impl Command {
    /// Why the notifications this command removes are gone.
    pub fn closed_reason(&self) -> ClosedReason {
        match self {
            Self::BannerHidden => ClosedReason::Expired,
            _ => ClosedReason::Dismissed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn incoming(id: u32, source: &str, urgency: Urgency) -> Event {
        Event::Notify(Box::new(Incoming {
            notification: Notification {
                id,
                title: format!("notification {id}"),
                urgency,
                ..Notification::default()
            },
            source_key: source.to_owned(),
            source_title: source.to_owned(),
            ..Incoming::default()
        }))
    }

    fn state_with(events: Vec<Event>) -> State {
        let mut state = State::default();
        for event in events {
            state.apply(event);
        }
        state
    }

    fn order(state: &State) -> Vec<(&str, Vec<u32>)> {
        state
            .sources
            .iter()
            .map(|source| {
                (
                    source.key.as_str(),
                    source.notifications.iter().map(|n| n.id).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn the_list_puts_the_newest_first_and_critical_above_all() {
        let mut state = state_with(vec![
            incoming(1, "mail", Urgency::Normal),
            incoming(2, "chat", Urgency::Normal),
            incoming(3, "mail", Urgency::Normal),
        ]);
        assert_eq!(order(&state), [("mail", vec![3, 1]), ("chat", vec![2])]);

        state.apply(incoming(4, "battery", Urgency::Critical));
        state.apply(incoming(5, "chat", Urgency::Normal));
        assert_eq!(
            order(&state),
            [
                ("battery", vec![4]),
                ("chat", vec![5, 2]),
                ("mail", vec![3, 1])
            ]
        );

        // A critical notification leads its group, and its group the list.
        state.apply(incoming(6, "mail", Urgency::Critical));
        state.apply(incoming(7, "mail", Urgency::Normal));
        assert_eq!(order(&state)[0], ("mail", vec![6, 7, 3, 1]));
    }

    #[test]
    fn low_urgency_never_gets_a_banner() {
        let state = state_with(vec![incoming(1, "updates", Urgency::Low)]);
        assert!(state.queue.is_empty());
        assert!(state.shows_indicator(), "but it is unseen");
    }

    #[test]
    fn the_queue_takes_three_and_then_only_critical_ones() {
        let mut state = state_with(vec![
            incoming(1, "a", Urgency::Normal),
            incoming(2, "a", Urgency::Normal),
            incoming(3, "a", Urgency::Normal),
            incoming(4, "a", Urgency::Normal),
        ]);
        assert_eq!(state.queue, [1, 2, 3]);
        assert!(state.shows_indicator(), "the fourth waits in the list");

        state.apply(incoming(5, "b", Urgency::Critical));
        assert_eq!(state.queue, [5, 1, 2, 3], "critical jumps the queue");

        // One on the banner counts towards the three.
        state.predict(&Command::ShowNextBanner);
        assert_eq!(state.banner, Some(Banner { id: 5, updates: 0 }));
        state.apply(incoming(6, "a", Urgency::Normal));
        assert_eq!(state.queue, [1, 2, 3]);
    }

    #[test]
    fn do_not_disturb_lets_only_critical_through() {
        let mut state = State {
            do_not_disturb: true,
            ..State::default()
        };
        state.apply(incoming(1, "chat", Urgency::Normal));
        state.apply(incoming(2, "battery", Urgency::Critical));
        assert_eq!(state.queue, [2]);
        assert!(!state.shows_indicator(), "no dot while not to be disturbed");
        assert_eq!(state.ids().len(), 2, "both are in the list");
    }

    #[test]
    fn showing_a_banner_acknowledges_and_hiding_drops_a_transient_one() {
        let mut state = State::default();
        let mut transient = Incoming {
            notification: Notification {
                id: 1,
                transient: true,
                ..Notification::default()
            },
            source_key: "volume".to_owned(),
            ..Incoming::default()
        };
        state.apply(Event::Notify(Box::new(transient.clone())));
        state.apply(incoming(2, "chat", Urgency::Normal));

        state.predict(&Command::ShowNextBanner);
        assert_eq!(state.banner.map(|banner| banner.id), Some(1));
        assert!(state.find(1).unwrap().1.acknowledged);
        assert_eq!(state.queue, [2]);
        state.predict(&Command::ShowNextBanner);
        assert_eq!(
            state.banner.map(|banner| banner.id),
            Some(1),
            "one at a time"
        );

        state.predict(&Command::BannerHidden);
        assert!(
            state.find(1).is_none(),
            "a transient one leaves with its banner"
        );
        state.predict(&Command::ShowNextBanner);
        state.predict(&Command::BannerHidden);
        assert!(state.find(2).is_some(), "any other stays in the list");
        assert!(!state.shows_indicator());

        // The same again, replaced while its banner is up.
        transient.notification.id = 3;
        state.apply(Event::Notify(Box::new(transient.clone())));
        state.predict(&Command::ShowNextBanner);
        state.apply(Event::Notify(Box::new(transient)));
        assert_eq!(state.banner, Some(Banner { id: 3, updates: 1 }));
        assert!(state.queue.is_empty());
    }

    #[test]
    fn a_replacement_keeps_its_place_and_asks_again_once_seen() {
        let mut state = state_with(vec![
            incoming(1, "mail", Urgency::Normal),
            incoming(2, "mail", Urgency::Normal),
            incoming(3, "chat", Urgency::Normal),
        ]);
        state.predict(&Command::AcknowledgeAll);
        assert!(state.queue.is_empty());
        assert!(!state.shows_indicator());

        state.apply(incoming(1, "mail", Urgency::Normal));
        assert_eq!(order(&state), [("chat", vec![3]), ("mail", vec![2, 1])]);
        assert_eq!(state.queue, [1]);
        assert!(!state.find(1).unwrap().1.acknowledged);

        // Turning critical moves it, and its group, to the top.
        state.apply(incoming(1, "mail", Urgency::Critical));
        assert_eq!(order(&state), [("mail", vec![1, 2]), ("chat", vec![3])]);
    }

    #[test]
    fn a_source_keeps_ten_and_drops_the_oldest() {
        let mut state = State::default();
        for id in 1..=12 {
            state.apply(incoming(id, "chat", Urgency::Low));
        }
        let ids = state.ids();
        assert_eq!(ids.len(), 10);
        assert!(!ids.contains(&1) && !ids.contains(&2));
        assert_eq!(ids[0], 12);
    }

    #[test]
    fn closing_removes_and_empties_sources() {
        let mut state = state_with(vec![
            incoming(1, "mail", Urgency::Normal),
            incoming(2, "mail", Urgency::Normal),
            incoming(3, "chat", Urgency::Normal),
        ]);
        state.predict(&Command::ShowNextBanner);
        state.apply(Event::Closed(1));
        assert_eq!(state.banner, None, "its banner goes with it");
        assert_eq!(state.queue, [2, 3]);

        state.predict(&Command::Dismiss(3));
        assert_eq!(order(&state), [("mail", vec![2])]);
        assert_eq!(state.queue, [2]);

        state.apply(incoming(4, "chat", Urgency::Normal));
        state.predict(&Command::DismissSource("mail".to_owned()));
        assert_eq!(order(&state), [("chat", vec![4])]);
        state.apply(Event::SourceGone("chat".to_owned()));
        assert!(state.is_empty());

        state.apply(incoming(5, "chat", Urgency::Normal));
        state.predict(&Command::Clear);
        assert!(state.is_empty() && state.queue.is_empty());
    }

    #[test]
    fn activating_follows_the_default_action_and_resident_hints() {
        let with = |id, default_action, resident| {
            Event::Notify(Box::new(Incoming {
                notification: Notification {
                    id,
                    default_action,
                    resident,
                    ..Notification::default()
                },
                source_key: "player".to_owned(),
                ..Incoming::default()
            }))
        };
        let activate = |id| Command::Activate { id, token: None };

        // With a default action only the clicked one goes.
        let mut state = state_with(vec![with(1, true, false), with(2, true, false)]);
        state.predict(&activate(1));
        assert_eq!(state.ids(), [2]);

        // Without one the application opens and all it said is read.
        let mut state = state_with(vec![
            with(1, false, false),
            with(2, false, false),
            with(3, false, true),
        ]);
        state.predict(&activate(1));
        assert_eq!(state.ids(), [3], "a resident one stays");

        // A resident notification survives its own actions.
        state.predict(&Command::InvokeAction {
            id: 3,
            key: "pause".to_owned(),
            token: None,
        });
        assert_eq!(state.ids(), [3]);
        let mut state = state_with(vec![with(4, true, false)]);
        state.predict(&Command::InvokeAction {
            id: 4,
            key: "reply".to_owned(),
            token: None,
        });
        assert!(state.is_empty());
    }

    #[test]
    fn images_from_hints() {
        assert_eq!(Image::from_hint(""), None);
        assert_eq!(
            Image::from_hint("mail-unread-symbolic"),
            Some(Image::Name("mail-unread-symbolic".to_owned()))
        );
        assert!(
            Image::from_hint("mail-unread-symbolic")
                .unwrap()
                .is_symbolic()
        );
        assert_eq!(
            Image::from_hint("/tmp/cover.png"),
            Some(Image::File(PathBuf::from("/tmp/cover.png")))
        );
        assert_eq!(
            Image::from_hint("file:///tmp/a%20cover.png"),
            Some(Image::File(PathBuf::from("/tmp/a cover.png")))
        );
        assert_eq!(Urgency::from_hint(2), Urgency::Critical);
        assert_eq!(Urgency::from_hint(7), Urgency::Normal);
    }

    #[test]
    fn a_scene_describes_notifications() {
        let state: State = toml::from_str(
            r#"
            [[sources]]
            key = "system"
            title = "System"
            icon = { name = "cog-wheel-symbolic" }
            notifications = [
                { id = 2, title = "Reference notification", body = "Body", time = "2026-11-13T22:09:30", acknowledged = true },
                { id = 1, title = "Older", urgency = "low", time = "2026-11-13T09:00:00" },
            ]
            "#,
        )
        .unwrap();
        assert_eq!(state.sources[0].display_title(), "System");
        assert_eq!(state.sources[0].notifications[0].time.hour, 22);
        assert_eq!(state.sources[0].notifications[1].urgency, Urgency::Low);
        assert!(state.shows_indicator());
        assert_eq!(Source::default().display_title(), "Unknown App");
    }
}
