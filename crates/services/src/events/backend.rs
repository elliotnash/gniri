// SPDX-License-Identifier: GPL-3.0-or-later

//! Talks to `org.gnome.Shell.CalendarServer` on the session bus, as
//! GNOME Shell's `DBusEventSource` does. The server is started by the bus
//! the first time it is asked something, which is how GNOME starts it
//! too; where it is not installed nothing is shown.

use std::collections::HashMap;

use async_channel::{Receiver, Sender};
use futures_lite::stream::StreamExt;
use zbus::OwnedVariant as OwnedValue;

use super::{CalendarEvent, Command, Event};
use crate::bus::{Wakes, merge};

const NAME: &str = "org.gnome.Shell.CalendarServer";
const PATH: &str = "/org/gnome/Shell/CalendarServer";
const INTERFACE: &str = "org.gnome.Shell.CalendarServer";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

/// What the backend has to do next.
enum Work {
    Signal(zbus::Message),
    /// The server came (true) or went.
    Owner(bool),
    Command(Command),
}

type Appointment = (String, String, i64, i64, HashMap<String, OwnedValue>);

/// The event a signal stands for, if it is one of the server's.
fn event_of(message: &zbus::Message) -> Option<Event> {
    let header = message.header();
    let member = header.member()?.to_string();
    let interface = header.interface()?.to_string();
    let body = message.body();
    match (interface.as_str(), member.as_str()) {
        (INTERFACE, "EventsAddedOrUpdated") => {
            let (appointments,): (Vec<Appointment>,) = body.deserialize().ok()?;
            Some(Event::Added(
                appointments
                    .into_iter()
                    .map(|(id, summary, start, end, _)| CalendarEvent {
                        id,
                        summary,
                        start,
                        end,
                    })
                    .collect(),
            ))
        }
        (INTERFACE, "EventsRemoved") => {
            let (ids,): (Vec<String>,) = body.deserialize().ok()?;
            Some(Event::Removed(ids))
        }
        (INTERFACE, "ClientDisappeared") => {
            let (source,): (String,) = body.deserialize().ok()?;
            Some(Event::CalendarGone(source))
        }
        (PROPERTIES, "PropertiesChanged") => {
            let (_, changed, _): (String, HashMap<String, OwnedValue>, Vec<String>) =
                body.deserialize().ok()?;
            let has = changed.get("HasCalendars")?;
            Some(Event::HasCalendars(bool::try_from(has).ok()?))
        }
        _ => None,
    }
}

async fn has_calendars(connection: &zbus::Connection) -> zbus::Result<bool> {
    let reply = connection
        .call_method(
            Some(NAME),
            PATH,
            Some(PROPERTIES),
            "Get",
            &(INTERFACE, "HasCalendars"),
        )
        .await?;
    let value: OwnedValue = reply.body().deserialize()?;
    Ok(bool::try_from(value).unwrap_or(false))
}

pub async fn run(
    connection: zbus::Connection,
    events: Sender<Event>,
    commands: Receiver<Command>,
) -> zbus::Result<()> {
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .path(PATH)
        .build()?;
    let signals = zbus::MessageStream::for_match_rule(rule, &connection, None)
        .await?
        .filter_map(|message| message.ok().map(Work::Signal));
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.DBus")
        .interface("org.freedesktop.DBus")
        .member("NameOwnerChanged")
        .arg(0, NAME)
        .build()?;
    let owners = zbus::MessageStream::for_match_rule(rule, &connection, None)
        .await?
        .filter_map(|message| {
            let message = message.ok()?;
            let (_, _, new): (String, String, String) = message.body().deserialize().ok()?;
            Some(Work::Owner(!new.is_empty()))
        });

    let (queue, queued) = async_channel::unbounded();
    let mut wakes = merge(vec![
        Box::pin(signals) as Wakes<Work>,
        Box::pin(owners),
        Box::pin(commands.map(Work::Command)),
    ]);
    // Moves what arrives into the queue, so that no signal waits while a
    // call is answered (docs/DECISIONS.md, M2).
    let pump = async {
        while let Some(work) = wakes.next().await {
            if queue.try_send(work).is_err() {
                break;
            }
        }
    };
    let work = async {
        // Asking starts the server; one that is not installed answers
        // with an error, and nothing is shown.
        match has_calendars(&connection).await {
            Ok(has) => {
                if events.send(Event::HasCalendars(has)).await.is_err() {
                    return;
                }
            }
            Err(error) => eprintln!("gniri: calendar events: {error}"),
        }
        let mut range = None;
        while let Ok(work) = queued.recv().await {
            let event = match work {
                Work::Signal(message) => event_of(&message),
                Work::Owner(false) => Some(Event::Vanished),
                Work::Owner(true) => {
                    // A server that came back is asked again for what it
                    // was asked before (`_onNameAppeared`).
                    if let Ok(has) = has_calendars(&connection).await {
                        let _ = events.send(Event::HasCalendars(has)).await;
                    }
                    if let Some((since, until)) = range {
                        set_range(&connection, since, until).await;
                    }
                    None
                }
                Work::Command(Command::SetRange(since, until)) => {
                    if range == Some((since, until)) {
                        continue;
                    }
                    range = Some((since, until));
                    if events.send(Event::Reloading).await.is_err() {
                        return;
                    }
                    set_range(&connection, since, until).await;
                    None
                }
            };
            if let Some(event) = event
                && events.send(event).await.is_err()
            {
                return;
            }
        }
    };
    futures_lite::future::or(pump, work).await;
    Ok(())
}

async fn set_range(connection: &zbus::Connection, since: i64, until: i64) {
    let reply = connection
        .call_method(
            Some(NAME),
            PATH,
            Some(INTERFACE),
            "SetTimeRange",
            &(since, until, true),
        )
        .await;
    if let Err(error) = reply {
        eprintln!("gniri: calendar events: {error}");
    }
}
