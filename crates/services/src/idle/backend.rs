//! Idle notifications through `ext-idle-notify`, on a Wayland connection
//! of its own (never GTK's: docs/DECISIONS.md).
//!
//! One thread reads the compositor's events. Each timer is one notification
//! object, which says `idled` when its time has passed without input and
//! `resumed` at the next input.

use std::time::Duration;

use async_channel::Sender;
use wayland_client::protocol::{wl_registry, wl_seat::WlSeat};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1::{self, ExtIdleNotificationV1},
    ext_idle_notifier_v1::ExtIdleNotifierV1,
};

use super::{Event, State, Timer};
use crate::model::Link;

struct Client {
    timers: Vec<(Timer, Duration)>,
    events: Sender<Event>,
    notifier: Option<ExtIdleNotifierV1>,
    seat: Option<WlSeat>,
    /// Kept so the compositor keeps them.
    notifications: Vec<ExtIdleNotificationV1>,
}

impl Client {
    /// Asks for the notifications once both the notifier and a seat are
    /// known.
    fn subscribe(&mut self, queue: &QueueHandle<Self>) {
        let (Some(notifier), Some(seat)) = (&self.notifier, &self.seat) else {
            return;
        };
        if !self.notifications.is_empty() {
            return;
        }
        for (timer, after) in &self.timers {
            let milliseconds = u32::try_from(after.as_millis()).unwrap_or(u32::MAX);
            self.notifications.push(notifier.get_idle_notification(
                milliseconds,
                seat,
                queue,
                *timer,
            ));
        }
        let _ = self.events.try_send(Event::Available(true));
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for Client {
    fn event(
        client: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        (): &(),
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name, interface, ..
        } = event
        else {
            return;
        };
        match interface.as_str() {
            // The first seat: input on any other is not looked at.
            "wl_seat" if client.seat.is_none() => {
                client.seat = Some(registry.bind(name, 1, queue, ()));
                client.subscribe(queue);
            }
            "ext_idle_notifier_v1" => {
                client.notifier = Some(registry.bind(name, 1, queue, ()));
                client.subscribe(queue);
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtIdleNotificationV1, Timer> for Client {
    fn event(
        client: &mut Self,
        _: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        timer: &Timer,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let event = match event {
            ext_idle_notification_v1::Event::Idled => Event::Idle(*timer),
            ext_idle_notification_v1::Event::Resumed => Event::Resumed(*timer),
            _ => return,
        };
        let _ = client.events.try_send(event);
    }
}

// Nothing a seat or the notifier says matters here.
impl Dispatch<WlSeat, ()> for Client {
    fn event(
        _: &mut Self,
        _: &WlSeat,
        _: <WlSeat as wayland_client::Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtIdleNotifierV1, ()> for Client {
    fn event(
        _: &mut Self,
        _: &ExtIdleNotifierV1,
        _: <ExtIdleNotifierV1 as wayland_client::Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// Starts the backend with the timers from [`super::timers`].
///
/// Without a Wayland display, or on a compositor without idle
/// notifications, the state stays "not available" and nothing ever passes.
pub fn spawn(timers: Vec<(Timer, Duration)>) -> Link<State> {
    let (link, events, _commands) = Link::<State>::new();
    let connection = match Connection::connect_to_env() {
        Ok(connection) => connection,
        Err(error) => {
            eprintln!("gniri: idle: no Wayland connection: {error}");
            return link;
        }
    };
    let reader = std::thread::Builder::new()
        .name("idle".to_owned())
        .spawn(move || {
            let mut queue = connection.new_event_queue();
            let handle = queue.handle();
            connection.display().get_registry(&handle, ());
            let mut client = Client {
                timers,
                events,
                notifier: None,
                seat: None,
                notifications: Vec::new(),
            };
            // Ends when the compositor goes away.
            while queue.blocking_dispatch(&mut client).is_ok() {}
        });
    if let Err(error) = reader {
        eprintln!("gniri: idle: cannot start a thread: {error}");
    }
    link
}
