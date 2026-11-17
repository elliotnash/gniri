// SPDX-License-Identifier: GPL-3.0-or-later

//! The calendar events backend against a stand-in for GNOME Shell's
//! calendar server: the test owns `org.gnome.Shell.CalendarServer` on a
//! private bus, answers `SetTimeRange` with the events of the span, and
//! takes some away again, as the server does.

mod common;

use std::collections::HashMap;

use async_channel::{Receiver, Sender};
use common::{Follower, MockBus, TIMEOUT};
use futures_lite::future::block_on;
use gniri_services::events::{self, Command, backend};
use zbus::OwnedVariant;
use zbus::object_server::SignalEmitter;

const NAME: &str = "org.gnome.Shell.CalendarServer";
const PATH: &str = "/org/gnome/Shell/CalendarServer";

type Appointment = (String, String, i64, i64, HashMap<String, OwnedVariant>);

/// The server, as far as gniri uses it.
struct Server {
    asked: Sender<(i64, i64, bool)>,
}

#[zbus::interface(name = "org.gnome.Shell.CalendarServer")]
impl Server {
    async fn set_time_range(
        &self,
        since: i64,
        until: i64,
        force_reload: bool,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) {
        let _ = self.asked.send((since, until, force_reload)).await;
        // What there is in the span: one event, and two occurrences of a
        // recurring one.
        let event = |id: &str, summary: &str, start: i64| -> Appointment {
            (
                id.to_owned(),
                summary.to_owned(),
                start,
                start + 60,
                HashMap::new(),
            )
        };
        let _ = Self::events_added_or_updated(
            &emitter,
            vec![
                event("work\nreview\n", "Review", since + 100),
                event("work\nstandup\n1", "Standup", since + 200),
                event("work\nstandup\n2", "Standup", since + 300),
            ],
        )
        .await;
    }

    #[zbus(property)]
    fn has_calendars(&self) -> bool {
        true
    }

    #[zbus(signal)]
    async fn events_added_or_updated(
        emitter: &SignalEmitter<'_>,
        events: Vec<Appointment>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn events_removed(emitter: &SignalEmitter<'_>, ids: Vec<String>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn client_disappeared(emitter: &SignalEmitter<'_>, source: &str) -> zbus::Result<()>;
}

/// A connection that is the calendar server.
fn server(bus: &MockBus) -> (zbus::Connection, Receiver<(i64, i64, bool)>) {
    let (asked, asks) = async_channel::unbounded();
    let connection = bus.connection();
    block_on(async {
        connection
            .object_server()
            .at(PATH, Server { asked })
            .await
            .unwrap();
        connection.request_name(NAME).await.unwrap();
    });
    (connection, asks)
}

fn emitter(connection: &zbus::Connection) -> SignalEmitter<'static> {
    SignalEmitter::new(connection, PATH).unwrap().into_owned()
}

#[test]
fn events_follow_the_calendar_server() {
    let Some(bus) = MockBus::start_empty() else {
        return;
    };
    let (connection, asks) = server(&bus);
    let link: gniri_services::Link<events::State> = bus.run(backend::run);
    let mut calendar = Follower::new(link);
    calendar.wait("the server says it has calendars", |state| {
        state.has_calendars
    });

    calendar.send(Command::SetRange(1000, 2000));
    let asked = block_on(futures_lite::future::or(
        async { asks.recv().await.ok() },
        async {
            let (sender, receiver) = async_channel::bounded::<()>(1);
            std::thread::spawn(move || {
                std::thread::sleep(TIMEOUT);
                let _ = sender.send_blocking(());
            });
            let _ = receiver.recv().await;
            None
        },
    ));
    assert_eq!(
        asked,
        Some((1000, 2000, true)),
        "asked for the span, afresh"
    );
    calendar.wait("the span's events", |state| state.events.len() == 3);
    let names: Vec<&str> = calendar
        .state
        .between(1000, 2000)
        .iter()
        .map(|event| event.summary.as_str())
        .collect();
    assert_eq!(names, ["Review", "Standup", "Standup"]);

    // The recurring one is removed, every occurrence of it.
    block_on(Server::events_removed(
        &emitter(&connection),
        vec!["work\nstandup\n".to_owned()],
    ))
    .unwrap();
    calendar.wait("the occurrences to go", |state| state.events.len() == 1);

    // The calendar goes away, and what was in it.
    block_on(Server::client_disappeared(&emitter(&connection), "work")).unwrap();
    calendar.wait("the calendar's events to go", |state| {
        state.events.is_empty()
    });

    // The server goes: nothing is known, and there are no calendars.
    block_on(connection.release_name(NAME)).unwrap();
    calendar.wait("the server to be gone", |state| {
        *state
            == events::State {
                range: state.range,
                ..events::State::default()
            }
    });
}
