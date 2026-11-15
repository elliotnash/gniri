// SPDX-License-Identifier: GPL-3.0-or-later

//! The notification server on a private session bus, talked to the way
//! applications do: `notify-send`, and calls and signals over the bus.

mod common;

use std::collections::HashMap;
use std::process::Command as Process;
use std::time::{Duration, Instant};

use common::{Follower, MockBus, TIMEOUT};
use futures_lite::StreamExt;
use futures_lite::future::block_on;
use gniri_core::LocalTime;
use gniri_services::notifications::backend::{NAME, PATH, run};
use gniri_services::notifications::{Command, Image, State, Urgency};
use zbus::Variant;

fn now() -> LocalTime {
    LocalTime::parse("2026-11-14T08:28:00").unwrap()
}

fn start() -> Option<(MockBus, Follower<State>)> {
    let bus = MockBus::start_empty()?;
    let link = bus.run(|connection, events, commands| run(connection, events, commands, now));
    bus.wait_for_name(NAME);
    Some((bus, Follower::new(link)))
}

/// Calls `Notify` from the test's own connection and returns the id.
fn notify(
    bus: &MockBus,
    replaces: u32,
    summary: &str,
    actions: &[&str],
    hints: HashMap<&str, Variant<'_>>,
) -> u32 {
    bus.call(
        NAME,
        PATH,
        NAME,
        "Notify",
        &(
            "Tester", replaces, "", summary, "Body", actions, hints, -1i32,
        ),
    )
    .body()
    .deserialize()
    .expect("Notify returns an id")
}

/// The signals the server sends to the test's connection.
struct Signals(zbus::MessageStream);

impl Signals {
    fn new(bus: &MockBus) -> Self {
        Self(zbus::MessageStream::from(&bus.control))
    }

    /// Waits for the next signal called `member` and returns its body.
    fn next<B>(&mut self, member: &str) -> B
    where
        B: serde::de::DeserializeOwned + zbus::zvariant::Type,
    {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            assert!(Instant::now() < deadline, "no {member} signal arrived");
            let message = block_on(futures_lite::future::or(
                async { self.0.next().await.and_then(Result::ok) },
                async {
                    let (sender, receiver) = async_channel::bounded::<()>(1);
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(200));
                        let _ = sender.send_blocking(());
                    });
                    let _ = receiver.recv().await;
                    None
                },
            ));
            let Some(message) = message else {
                continue;
            };
            let header = message.header();
            if header.message_type() == zbus::message::Type::Signal
                && header.member().is_some_and(|name| name.as_str() == member)
            {
                return message
                    .body()
                    .deserialize()
                    .expect("the signal's arguments");
            }
        }
    }
}

#[test]
fn notify_send_end_to_end() {
    let has_notify_send = Process::new("notify-send")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success());
    if !has_notify_send {
        eprintln!("skipped: notify-send is not installed");
        return;
    }
    let Some((bus, mut notifications)) = start() else {
        return;
    };

    let status = Process::new("notify-send")
        .env("DBUS_SESSION_BUS_ADDRESS", bus.address())
        .args(["--app-name", "Tester", "--icon", "dialog-information"])
        .args(["--urgency", "critical", "Hello", "Some <b>bold</b> words"])
        .status()
        .expect("notify-send runs");
    assert!(status.success());

    notifications.wait("the notification", |state| !state.is_empty());
    let source = &notifications.state.sources[0];
    assert_eq!(source.title, "Tester");
    assert!(source.key.starts_with("pid:"), "{}", source.key);
    let notification = &source.notifications[0];
    // libnotify sends `--icon` as the application icon or, in newer
    // versions, as the notification's own picture.
    let icon = Some(Image::Name("dialog-information".to_owned()));
    assert!(
        source.icon == icon || notification.image == icon,
        "{source:?}"
    );
    assert_eq!(notification.title, "Hello");
    assert_eq!(notification.body, "Some <b>bold</b> words");
    assert_eq!(notification.urgency, Urgency::Critical);
    assert_eq!(notification.time, now());
    assert_eq!(notifications.state.queue, [notification.id]);

    // A second run is another process, so another source, above the first.
    let status = Process::new("notify-send")
        .env("DBUS_SESSION_BUS_ADDRESS", bus.address())
        .args(["--app-name", "Tester", "Again"])
        .status()
        .expect("notify-send runs");
    assert!(status.success());
    notifications.wait("the second notification", |state| state.sources.len() == 2);
    // The critical one's source stays on top.
    assert_eq!(
        notifications.state.sources[0].notifications[0].title,
        "Hello"
    );
    assert_eq!(
        notifications.state.sources[1].notifications[0].title,
        "Again"
    );
}

#[test]
fn replacing_closing_and_the_signals_back() {
    let Some((bus, mut notifications)) = start() else {
        return;
    };
    let mut signals = Signals::new(&bus);

    let first = notify(&bus, 0, "First", &[], HashMap::new());
    assert_ne!(first, 0);
    notifications.wait("the first", |state| state.find(first).is_some());

    // Replacing keeps the id; replacing something unknown gets a new one.
    let again = notify(&bus, first, "First, changed", &[], HashMap::new());
    assert_eq!(again, first);
    notifications.wait("the change", |state| {
        state
            .find(first)
            .is_some_and(|(_, notification)| notification.title == "First, changed")
    });
    assert_eq!(notifications.state.ids(), [first]);
    let other = notify(&bus, 999, "Other", &[], HashMap::new());
    assert!(other != 999 && other != first);
    notifications.wait("the other", |state| state.find(other).is_some());

    // The application withdraws one.
    bus.call(NAME, PATH, NAME, "CloseNotification", &(first));
    assert_eq!(signals.next::<(u32, u32)>("NotificationClosed"), (first, 3));
    notifications.wait("the withdrawal", |state| state.find(first).is_none());

    // The user dismisses the other.
    notifications.send(Command::Dismiss(other));
    assert_eq!(signals.next::<(u32, u32)>("NotificationClosed"), (other, 2));
}

#[test]
fn actions_reach_the_application() {
    let Some((bus, mut notifications)) = start() else {
        return;
    };
    let mut signals = Signals::new(&bus);

    let hints = HashMap::from([("resident", Variant::Bool(true))]);
    let resident = notify(
        &bus,
        0,
        "Call",
        &["default", "Open", "answer", "Answer"],
        hints,
    );
    notifications.wait("the call", |state| state.find(resident).is_some());
    let (_, notification) = notifications.state.find(resident).unwrap();
    assert!(notification.default_action && notification.resident);
    assert_eq!(notification.actions.len(), 1);
    assert_eq!(notification.actions[0].label, "Answer");

    notifications.send(Command::InvokeAction {
        id: resident,
        key: "answer".to_owned(),
        token: Some("token-1".to_owned()),
    });
    assert_eq!(
        signals.next::<(u32, String)>("ActivationToken"),
        (resident, "token-1".to_owned())
    );
    assert_eq!(
        signals.next::<(u32, String)>("ActionInvoked"),
        (resident, "answer".to_owned())
    );

    // A click on the notification itself is the "default" action. It is
    // resident, so it stays; a plain one goes once activated.
    notifications.send(Command::Activate {
        id: resident,
        token: None,
    });
    assert_eq!(
        signals.next::<(u32, String)>("ActionInvoked"),
        (resident, "default".to_owned())
    );
    let plain = notify(&bus, 0, "Message", &["default", "Open"], HashMap::new());
    notifications.wait("the message", |state| state.find(plain).is_some());
    notifications.send(Command::Activate {
        id: plain,
        token: None,
    });
    assert_eq!(
        signals.next::<(u32, String)>("ActionInvoked"),
        (plain, "default".to_owned())
    );
    assert_eq!(signals.next::<(u32, u32)>("NotificationClosed"), (plain, 2));

    // A transient one expires with its banner.
    let hints = HashMap::from([("transient", Variant::Bool(true))]);
    let transient = notify(&bus, 0, "Volume", &[], hints);
    notifications.wait("the transient one", |state| state.find(transient).is_some());
    notifications.send(Command::AcknowledgeAll);
    notifications.send(Command::Dismiss(resident));
    assert_eq!(
        signals.next::<(u32, u32)>("NotificationClosed"),
        (resident, 2)
    );
    let again = notify(
        &bus,
        transient,
        "Volume",
        &[],
        HashMap::from([("transient", Variant::Bool(true))]),
    );
    assert_eq!(again, transient);
    notifications.send(Command::ShowNextBanner);
    notifications.send(Command::BannerHidden);
    assert_eq!(
        signals.next::<(u32, u32)>("NotificationClosed"),
        (transient, 1)
    );
}

#[test]
fn server_information_and_a_taken_name() {
    let Some((bus, _notifications)) = start() else {
        return;
    };
    let capabilities: Vec<String> = bus
        .call(NAME, PATH, NAME, "GetCapabilities", &())
        .body()
        .deserialize()
        .unwrap();
    for needed in ["actions", "body", "body-markup", "persistence"] {
        assert!(capabilities.iter().any(|have| have == needed), "{needed}");
    }
    let (name, _vendor, _version, spec): (String, String, String, String) = bus
        .call(NAME, PATH, NAME, "GetServerInformation", &())
        .body()
        .deserialize()
        .unwrap();
    assert_eq!((name.as_str(), spec.as_str()), ("gniri", "1.2"));

    // A second server on the same bus gives up instead of waiting in line.
    let (_link, events, commands) = gniri_services::Link::<State>::new();
    let result = block_on(run(bus.connection(), events, commands, now));
    assert!(result.is_err(), "the name is taken");
}
