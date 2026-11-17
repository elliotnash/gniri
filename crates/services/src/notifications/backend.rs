// SPDX-License-Identifier: GPL-3.0-or-later

//! `org.freedesktop.Notifications`, served on the session bus. Port of
//! `FdoNotificationDaemon` in gnome-shell's `js/ui/notificationDaemon.js`.
//!
//! The backend keeps a copy of the state the shell has and runs the same
//! events and commands through it. That copy answers the two questions the
//! protocol asks: whether a `replaces_id` still exists, and which
//! notifications an event or a command removed, so that their senders are
//! told with `NotificationClosed`.

use std::collections::HashMap;

use async_channel::{Receiver, Sender};
use futures_lite::StreamExt;
use gniri_core::LocalTime;
use zbus::OwnedVariant;
use zbus::fdo::{DBusProxy, RequestNameFlags, RequestNameReply};
use zbus::message::Header;
use zbus::names::BusName;
use zbus::object_server::SignalEmitter;

use super::{
    Action, ClosedReason, Command, Event, Image, Incoming, Notification, Pixels, Sound, State,
    Urgency,
};
use crate::bus::{Wakes, merge};
use crate::desktop_entry::DesktopEntry;
use crate::model::Model;

pub const NAME: &str = "org.freedesktop.Notifications";
pub const PATH: &str = "/org/freedesktop/Notifications";

/// The arguments of one `Notify` call.
pub struct NotifyCall {
    /// Unique bus name of the caller.
    pub sender: Option<String>,
    pub pid: Option<u32>,
    pub app_name: String,
    pub replaces_id: u32,
    pub app_icon: String,
    pub summary: String,
    pub body: String,
    pub actions: Vec<String>,
    pub hints: HashMap<String, OwnedVariant>,
}

enum Request {
    Notify(Box<NotifyCall>, Sender<u32>),
    Close(u32),
}

/// The object applications talk to. It only passes calls on to [`run`].
struct Server {
    requests: Sender<Request>,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Server {
    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedVariant>,
        _expire_timeout: i32,
    ) -> zbus::fdo::Result<u32> {
        let sender = header.sender().map(|sender| sender.to_string());
        // Asked here, not in `run`: that loop must never wait for the bus.
        let pid = match (&sender, DBusProxy::new(connection).await) {
            (Some(sender), Ok(bus)) => match BusName::try_from(sender.as_str()) {
                Ok(name) => bus.get_connection_unix_process_id(name).await.ok(),
                Err(_) => None,
            },
            _ => None,
        };
        let (reply, id) = async_channel::bounded(1);
        let call = NotifyCall {
            sender,
            pid,
            app_name,
            replaces_id,
            app_icon,
            summary,
            body,
            actions,
            hints,
        };
        let gone = || zbus::fdo::Error::Failed("the notification server is shutting down".into());
        self.requests
            .send(Request::Notify(Box::new(call), reply))
            .await
            .map_err(|_| gone())?;
        id.recv().await.map_err(|_| gone())
    }

    async fn close_notification(&self, id: u32) {
        let _ = self.requests.send(Request::Close(id)).await;
    }

    /// What `FdoNotificationDaemon.GetCapabilities` lists.
    fn get_capabilities(&self) -> Vec<&'static str> {
        vec![
            "actions",
            "body",
            "body-markup",
            "icon-static",
            "persistence",
            "sound",
        ]
    }

    #[zbus(out_args("name", "vendor", "version", "spec_version"))]
    fn get_server_information(&self) -> (&'static str, &'static str, &'static str, &'static str) {
        ("gniri", "gniri", env!("CARGO_PKG_VERSION"), "1.2")
    }

    #[zbus(signal)]
    async fn notification_closed(
        emitter: &SignalEmitter<'_>,
        id: u32,
        reason: u32,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn action_invoked(
        emitter: &SignalEmitter<'_>,
        id: u32,
        action_key: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn activation_token(
        emitter: &SignalEmitter<'_>,
        id: u32,
        activation_token: &str,
    ) -> zbus::Result<()>;
}

fn hint_bool(hints: &HashMap<String, OwnedVariant>, name: &str) -> bool {
    hints
        .get(name)
        .is_some_and(|value| value.downcast_ref::<bool>().unwrap_or(false))
}

fn hint_string(hints: &HashMap<String, OwnedVariant>, name: &str) -> Option<String> {
    let text: &str = hints.get(name)?.downcast_ref().ok()?;
    Some(text.to_owned()).filter(|text| !text.is_empty())
}

/// The `urgency` hint is a byte, but not every client sends one.
fn hint_urgency(hints: &HashMap<String, OwnedVariant>) -> Urgency {
    let Some(value) = hints.get("urgency") else {
        return Urgency::Normal;
    };
    let level = value
        .downcast_ref::<u8>()
        .ok()
        .or_else(|| value.downcast_ref::<i32>().ok().map(|level| level as u8))
        .or_else(|| value.downcast_ref::<u32>().ok().map(|level| level as u8));
    level.map_or(Urgency::Normal, Urgency::from_hint)
}

/// The `image-data` hint: width, height, rowstride, has-alpha, bits per
/// sample, channels and the pixels. `None` for anything GdkPixbuf's RGB
/// format could not hold.
fn hint_pixels(value: &OwnedVariant) -> Option<Pixels> {
    let (width, height, rowstride, has_alpha, bits_per_sample, channels, data) =
        <(i32, i32, i32, bool, i32, i32, Vec<u8>)>::try_from(value.try_clone().ok()?).ok()?;
    let expected_channels = if has_alpha { 4 } else { 3 };
    if width <= 0 || height <= 0 || bits_per_sample != 8 || channels != expected_channels {
        return None;
    }
    // The last row may stop at its last pixel, short of the rowstride.
    let needed = i64::from(height - 1) * i64::from(rowstride) + i64::from(width * channels);
    if rowstride < width * channels || (data.len() as i64) < needed {
        return None;
    }
    Some(Pixels {
        width,
        height,
        rowstride,
        has_alpha,
        data,
    })
}

/// `FdoNotificationDaemon._imageForNotificationData`, with the older names
/// the hints had in versions 1.1 and 1.0 of the specification.
fn hint_image(hints: &HashMap<String, OwnedVariant>) -> Option<Image> {
    let path = hint_string(hints, "image-path").or_else(|| hint_string(hints, "image_path"));
    let data = hints
        .get("image-data")
        .or_else(|| hints.get("image_data"))
        .or_else(|| hints.get("icon_data").filter(|_| path.is_none()));
    if let Some(pixels) = data.and_then(hint_pixels) {
        return Some(Image::Pixels(std::sync::Arc::new(pixels)));
    }
    path.as_deref().and_then(Image::from_hint)
}

/// What a `Notify` call asks for, as an event for the shell. `id` is the
/// notification's (new or replaced); `find_app` looks a desktop entry up.
pub fn incoming(
    call: &NotifyCall,
    id: u32,
    time: LocalTime,
    find_app: impl Fn(&str) -> Option<DesktopEntry>,
) -> Incoming {
    // `_getApp`, without the lookup by window that only a compositor has.
    let app = hint_string(&call.hints, "desktop-entry")
        .and_then(|id| find_app(&id))
        .or_else(|| find_app(&call.app_name));

    // Pairs of key and label; "default" is the notification itself.
    let mut actions = Vec::new();
    let mut default_action = false;
    for [key, label] in call.actions.as_chunks::<2>().0 {
        if key == "default" {
            default_action = true;
        } else {
            actions.push(Action {
                key: key.clone(),
                label: label.clone(),
            });
        }
    }

    let (source_key, source_title, source_icon, source_app) = match app {
        Some(app) => (
            format!("app:{}", app.id),
            app.name,
            app.icon.as_deref().and_then(Image::from_hint),
            Some(app.id),
        ),
        None => {
            // `_getSourceForPidAndName`: one source per process and name.
            let process = match (call.pid, &call.sender) {
                (Some(pid), _) => format!("pid:{pid}"),
                (None, Some(sender)) => format!("sender:{sender}"),
                (None, None) => format!("notification:{id}"),
            };
            (
                format!("{process}:{}", call.app_name),
                call.app_name.clone(),
                Image::from_hint(&call.app_icon),
                None,
            )
        }
    };

    Incoming {
        notification: Notification {
            id,
            title: call.summary.clone(),
            body: call.body.clone(),
            image: hint_image(&call.hints),
            urgency: hint_urgency(&call.hints),
            actions,
            default_action,
            resident: hint_bool(&call.hints, "resident"),
            transient: hint_bool(&call.hints, "transient"),
            acknowledged: false,
            system: hint_string(&call.hints, "x-gnome-privacy-scope").as_deref() == Some("system"),
            // A sound of the theme wins over a file, as in
            // `MessageTray.Sound.play`.
            sound: hint_string(&call.hints, "sound-name")
                .map(Sound::Name)
                .or_else(|| {
                    hint_string(&call.hints, "sound-file").map(|path| Sound::File(path.into()))
                }),
            time,
        },
        source_key,
        source_title,
        source_icon,
        source_app,
    }
}

enum Wake {
    Request(Request),
    Command(Command),
    /// A unique name left the bus.
    Vanished(String),
}

/// What the loop in [`run`] knows beyond the state.
struct Daemon {
    connection: zbus::Connection,
    events: Sender<Event>,
    /// The shell's state, as far as the backend can tell.
    state: State,
    next_id: u32,
    /// Who to tell about each notification.
    senders: HashMap<u32, String>,
    /// The connection that first notified for each application's source.
    /// When it leaves the bus the application has quit.
    app_senders: HashMap<String, String>,
}

impl Daemon {
    fn emitter(&self, id: u32) -> Option<SignalEmitter<'static>> {
        let sender = self.senders.get(&id)?;
        let destination = BusName::try_from(sender.clone()).ok()?;
        let emitter = SignalEmitter::new(&self.connection, PATH).ok()?;
        Some(emitter.set_destination(destination).into_owned())
    }

    /// Runs `change` on the state and tells the senders of the
    /// notifications it removed.
    async fn change(&mut self, reason: ClosedReason, change: impl FnOnce(&mut State)) {
        let before = self.state.ids();
        change(&mut self.state);
        let after = self.state.ids();
        for id in before {
            if after.contains(&id) {
                continue;
            }
            if let Some(emitter) = self.emitter(id) {
                // A sender that has left the bus cannot be told.
                let _ = emitter.notification_closed(id, reason as u32).await;
            }
            self.senders.remove(&id);
        }
        self.app_senders
            .retain(|key, _| self.state.sources.iter().any(|source| source.key == *key));
    }

    async fn notify(&mut self, call: NotifyCall, now: &impl Fn() -> LocalTime) -> u32 {
        let replaces = call.replaces_id != 0 && self.state.find(call.replaces_id).is_some();
        let id = if replaces {
            call.replaces_id
        } else {
            let id = self.next_id;
            // Zero means "no notification".
            self.next_id = self.next_id.checked_add(1).unwrap_or(1);
            id
        };
        let incoming = incoming(&call, id, now(), DesktopEntry::find);
        if let Some(sender) = call.sender {
            if incoming.source_app.is_some() {
                self.app_senders
                    .entry(incoming.source_key.clone())
                    .or_insert_with(|| sender.clone());
            }
            self.senders.insert(id, sender);
        }
        let event = Event::Notify(Box::new(incoming));
        // Making room for it may have dropped the source's oldest.
        self.change(ClosedReason::Expired, |state| state.apply(event.clone()))
            .await;
        let _ = self.events.send(event).await;
        id
    }

    async fn command(&mut self, command: Command) {
        match &command {
            Command::Activate { id, token } => {
                let default_action = self
                    .state
                    .find(*id)
                    .is_some_and(|(_, notification)| notification.default_action);
                if default_action {
                    self.invoke(*id, "default", token.as_deref()).await;
                }
            }
            Command::InvokeAction { id, key, token } if self.state.find(*id).is_some() => {
                self.invoke(*id, key, token.as_deref()).await;
            }
            _ => {}
        }
        self.change(command.closed_reason(), |state| state.predict(&command))
            .await;
    }

    /// `_emitActivationToken` and `_emitActionInvoked`.
    async fn invoke(&self, id: u32, key: &str, token: Option<&str>) {
        let Some(emitter) = self.emitter(id) else {
            return;
        };
        if let Some(token) = token {
            let _ = emitter.activation_token(id, token).await;
        }
        let _ = emitter.action_invoked(id, key).await;
    }
}

/// Serves notifications until the shell goes away. `now` stamps each one
/// with the time it arrived.
pub async fn run(
    connection: zbus::Connection,
    events: Sender<Event>,
    commands: Receiver<Command>,
    now: impl Fn() -> LocalTime,
) -> zbus::Result<()> {
    let (requests, incoming_requests) = async_channel::unbounded();
    connection
        .object_server()
        .at(PATH, Server { requests })
        .await?;
    // Never take the name from a running notification daemon, and do not
    // wait in line for it either.
    let reply = connection
        .request_name_with_flags(NAME, RequestNameFlags::DO_NOT_QUEUE)
        .await;
    if !matches!(reply, Ok(RequestNameReply::PrimaryOwner)) {
        let _ = connection.object_server().remove::<Server, _>(PATH).await;
        return Err(zbus::Error::Failure(format!(
            "another program owns {NAME}; notifications will not be shown by gniri"
        )));
    }

    let bus = DBusProxy::new(&connection).await?;
    let vanished = bus
        .receive_name_owner_changed()
        .await?
        .filter_map(|signal| {
            let arguments = signal.args().ok()?;
            let name = arguments.name().to_string();
            (name.starts_with(':') && arguments.new_owner().is_none())
                .then_some(Wake::Vanished(name))
        });
    let mut wakes = merge(vec![
        Box::pin(incoming_requests.map(Wake::Request)) as Wakes<Wake>,
        Box::pin(commands.map(Wake::Command)),
        Box::pin(vanished),
    ]);

    let mut daemon = Daemon {
        connection: connection.clone(),
        events,
        state: State::default(),
        next_id: 1,
        senders: HashMap::new(),
        app_senders: HashMap::new(),
    };
    while let Some(wake) = wakes.next().await {
        match wake {
            Wake::Request(Request::Notify(call, reply)) => {
                let id = daemon.notify(*call, &now).await;
                let _ = reply.send(id).await;
            }
            Wake::Request(Request::Close(id)) => {
                if daemon.state.find(id).is_some() {
                    let event = Event::Closed(id);
                    daemon
                        .change(ClosedReason::ClosedByApp, |state| {
                            state.apply(event.clone());
                        })
                        .await;
                    let _ = daemon.events.send(event).await;
                }
            }
            Wake::Command(command) => daemon.command(command).await,
            Wake::Vanished(name) => {
                // `FdoNotificationDaemonSource._onNameVanished`
                let keys: Vec<String> = daemon
                    .app_senders
                    .iter()
                    .filter(|(_, sender)| **sender == name)
                    .map(|(key, _)| key.clone())
                    .collect();
                for key in keys {
                    let event = Event::SourceGone(key);
                    daemon
                        .change(ClosedReason::ClosedByApp, |state| {
                            state.apply(event.clone());
                        })
                        .await;
                    let _ = daemon.events.send(event).await;
                }
            }
        }
        if daemon.events.is_closed() {
            break; // The shell went away.
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use zbus::Variant as Value;

    use super::*;

    fn call(app_name: &str, actions: &[&str], hints: Vec<(&str, Value<'static>)>) -> NotifyCall {
        NotifyCall {
            sender: Some(":1.42".to_owned()),
            pid: Some(4242),
            app_name: app_name.to_owned(),
            replaces_id: 0,
            app_icon: "dialog-information".to_owned(),
            summary: "Summary".to_owned(),
            body: "Body".to_owned(),
            actions: actions.iter().map(|text| (*text).to_owned()).collect(),
            hints: hints
                .into_iter()
                .map(|(name, value)| (name.to_owned(), value.try_into().unwrap()))
                .collect(),
        }
    }

    fn time() -> LocalTime {
        LocalTime::parse("2026-11-14T08:28:00").unwrap()
    }

    fn no_apps(_: &str) -> Option<DesktopEntry> {
        None
    }

    #[test]
    fn a_plain_call_is_a_source_of_its_own_process() {
        let incoming = incoming(&call("notify-send", &[], vec![]), 7, time(), no_apps);
        assert_eq!(incoming.source_key, "pid:4242:notify-send");
        assert_eq!(incoming.source_title, "notify-send");
        assert_eq!(
            incoming.source_icon,
            Some(Image::Name("dialog-information".to_owned()))
        );
        assert_eq!(incoming.source_app, None);
        let notification = incoming.notification;
        assert_eq!(notification.id, 7);
        assert_eq!(notification.title, "Summary");
        assert_eq!(notification.urgency, Urgency::Normal);
        assert_eq!(notification.image, None, "the app icon is the source's");
        assert!(!notification.default_action && notification.actions.is_empty());
        assert_eq!(notification.time, time());
    }

    #[test]
    fn hints_and_actions() {
        let incoming = incoming(
            &call(
                "Chat",
                &["default", "Open", "reply", "Reply", "dangling"],
                vec![
                    ("urgency", Value::U8(2)),
                    ("resident", Value::Bool(true)),
                    ("transient", Value::Bool(true)),
                    ("image-path", Value::from("file:///tmp/face.png")),
                    ("sound-file", Value::from("/tmp/ding.oga")),
                ],
            ),
            1,
            time(),
            no_apps,
        );
        let notification = incoming.notification;
        assert_eq!(notification.urgency, Urgency::Critical);
        assert!(notification.resident && notification.transient);
        assert!(notification.default_action);
        assert_eq!(
            notification.actions,
            [Action {
                key: "reply".to_owned(),
                label: "Reply".to_owned()
            }]
        );
        assert_eq!(
            notification.image,
            Some(Image::File("/tmp/face.png".into()))
        );
        assert_eq!(
            notification.sound,
            Some(Sound::File("/tmp/ding.oga".into()))
        );

        // An urgency sent as a plain integer still counts.
        let lazy = super::incoming(
            &call("Chat", &[], vec![("urgency", Value::I32(0))]),
            2,
            time(),
            no_apps,
        );
        assert_eq!(lazy.notification.urgency, Urgency::Low);
    }

    #[test]
    fn an_installed_application_names_the_source() {
        let find = |id: &str| {
            (id == "org.example.Chat").then(|| DesktopEntry {
                id: id.to_owned(),
                name: "Chat".to_owned(),
                icon: Some("org.example.Chat".to_owned()),
                ..DesktopEntry::default()
            })
        };
        let by_hint = incoming(
            &call(
                "chat-helper",
                &[],
                vec![("desktop-entry", Value::from("org.example.Chat"))],
            ),
            1,
            time(),
            find,
        );
        assert_eq!(by_hint.source_key, "app:org.example.Chat");
        assert_eq!(by_hint.source_title, "Chat");
        assert_eq!(
            by_hint.source_icon,
            Some(Image::Name("org.example.Chat".to_owned()))
        );
        assert_eq!(by_hint.source_app.as_deref(), Some("org.example.Chat"));

        let by_name = incoming(&call("org.example.Chat", &[], vec![]), 2, time(), find);
        assert_eq!(by_name.source_key, "app:org.example.Chat");
    }

    #[test]
    fn image_data() {
        let pixels = |width: i32, height: i32, rowstride: i32, alpha: bool, bytes: usize| {
            let channels = if alpha { 4 } else { 3 };
            let value: OwnedVariant = Value::from((
                width,
                height,
                rowstride,
                alpha,
                8i32,
                channels,
                vec![0u8; bytes],
            ))
            .try_into()
            .unwrap();
            hint_pixels(&value)
        };
        assert!(pixels(2, 2, 8, true, 16).is_some());
        assert!(
            pixels(2, 2, 8, false, 14).is_some(),
            "a short last row is fine"
        );
        assert!(pixels(2, 2, 8, true, 12).is_none(), "too few bytes");
        assert!(pixels(2, 2, 4, true, 16).is_none(), "rows overlap");
        assert!(pixels(0, 2, 8, true, 16).is_none());

        let value: OwnedVariant = Value::from("not pixels").try_into().unwrap();
        assert!(hint_pixels(&value).is_none());
    }
}
