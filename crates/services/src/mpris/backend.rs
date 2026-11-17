// SPDX-License-Identifier: GPL-3.0-or-later

//! MPRIS players, over the session bus.
//!
//! Two halves run side by side. One only drains the bus's signals and the
//! shell's commands into a queue; the other works the queue off, asking the
//! players for their properties. zbus stops reading the socket while a
//! signal stream is full, so the half that waits for replies must never be
//! the one that empties the streams (docs/DECISIONS.md, M2).

use gniri_core::tr;
use std::collections::HashMap;

use async_channel::{Receiver, Sender};
use futures_lite::StreamExt;
use zbus::OwnedVariant;

use super::{Command, Event, Image, PREFIX, Player, Status};
use crate::bus::{Wakes, merge};
use crate::desktop_entry::DesktopEntry;

const PATH: &str = "/org/mpris/MediaPlayer2";
const ROOT: &str = "org.mpris.MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

type Properties = HashMap<String, OwnedVariant>;

enum Work {
    /// A player's bus name has an owner.
    Appeared(String),
    Vanished(String),
    /// The connection with this unique name changed a property.
    Changed(String),
    Command(Command),
}

fn text(properties: &Properties, name: &str) -> Option<String> {
    let value: &str = properties.get(name)?.downcast_ref().ok()?;
    Some(value.to_owned())
}

fn flag(properties: &Properties, name: &str) -> bool {
    properties
        .get(name)
        .is_some_and(|value| value.downcast_ref::<bool>().unwrap_or(false))
}

/// A player from the properties of its two interfaces. Port of
/// `MprisPlayer._updateState`, including what it shows for metadata that is
/// missing or of the wrong type.
pub fn player(
    bus_name: &str,
    root: &Properties,
    playback: &Properties,
    find_app: impl Fn(&str) -> Option<DesktopEntry>,
) -> Player {
    let metadata: Properties = playback
        .get("Metadata")
        .and_then(|value| value.try_clone().ok())
        .and_then(|value| Properties::try_from(value).ok())
        .unwrap_or_default();
    let artists = metadata
        .get("xesam:artist")
        .and_then(|value| value.try_clone().ok())
        .and_then(|value| Vec::<String>::try_from(value).ok())
        .unwrap_or_else(|| vec![tr("Unknown artist")]);
    let title = text(&metadata, "xesam:title").unwrap_or_else(|| tr("Unknown title"));
    // Only a local file: gniri fetches nothing from the network.
    let cover = text(&metadata, "mpris:artUrl")
        .filter(|url| url.starts_with("file://"))
        .and_then(|url| Image::from_hint(&url));

    let app = text(root, "DesktopEntry")
        .filter(|id| !id.is_empty())
        .and_then(|id| find_app(&id));
    let (name, icon, app) = match app {
        Some(app) => (
            app.name,
            app.icon.as_deref().and_then(Image::from_hint),
            Some(app.id),
        ),
        None => (text(root, "Identity").unwrap_or_default(), None, None),
    };

    Player {
        bus_name: bus_name.to_owned(),
        name,
        icon,
        app,
        status: text(playback, "PlaybackStatus")
            .map(|status| Status::from_property(&status))
            .unwrap_or_default(),
        title,
        artists,
        cover,
        can_play: flag(playback, "CanPlay"),
        can_go_next: flag(playback, "CanGoNext"),
        can_go_previous: flag(playback, "CanGoPrevious"),
        can_raise: flag(root, "CanRaise"),
    }
}

async fn get_all(
    connection: &zbus::Connection,
    bus_name: &str,
    interface: &str,
) -> zbus::Result<Properties> {
    connection
        .call_method(
            Some(bus_name),
            PATH,
            Some(PROPERTIES),
            "GetAll",
            &(interface),
        )
        .await?
        .body()
        .deserialize()
}

struct Worker {
    connection: zbus::Connection,
    events: Sender<Event>,
    /// Bus name of each known player, by the unique name that owns it.
    owners: HashMap<String, String>,
}

impl Worker {
    /// Reads a player and reports it. A player that does not answer is
    /// reported gone.
    async fn read(&mut self, bus_name: &str) -> bool {
        let root = get_all(&self.connection, bus_name, ROOT).await;
        let playback = get_all(&self.connection, bus_name, PLAYER).await;
        let event = match (root, playback) {
            (Ok(root), Ok(playback)) => Event::Player(Box::new(player(
                bus_name,
                &root,
                &playback,
                DesktopEntry::find,
            ))),
            _ => Event::Gone(bus_name.to_owned()),
        };
        self.events.send(event).await.is_ok()
    }

    /// Does one piece of work; false once the shell has gone away.
    async fn work(&mut self, work: Work) -> bool {
        match work {
            Work::Appeared(bus_name) => {
                let owner: zbus::Result<String> = async {
                    self.connection
                        .call_method(
                            Some("org.freedesktop.DBus"),
                            "/org/freedesktop/DBus",
                            Some("org.freedesktop.DBus"),
                            "GetNameOwner",
                            &(bus_name.as_str()),
                        )
                        .await?
                        .body()
                        .deserialize()
                }
                .await;
                // Gone again already: its `Vanished` follows.
                let Ok(owner) = owner else {
                    return true;
                };
                self.owners.retain(|_, name| *name != bus_name);
                self.owners.insert(owner, bus_name.clone());
                self.read(&bus_name).await
            }
            Work::Vanished(bus_name) => {
                self.owners.retain(|_, name| *name != bus_name);
                self.events.send(Event::Gone(bus_name)).await.is_ok()
            }
            Work::Changed(owner) => match self.owners.get(&owner).cloned() {
                Some(bus_name) => self.read(&bus_name).await,
                None => true,
            },
            Work::Command(command) => {
                let (bus_name, interface, method) = match &command {
                    Command::PlayPause(bus_name) => (bus_name, PLAYER, "PlayPause"),
                    Command::Next(bus_name) => (bus_name, PLAYER, "Next"),
                    Command::Previous(bus_name) => (bus_name, PLAYER, "Previous"),
                    Command::Raise(bus_name) => (bus_name, ROOT, "Raise"),
                };
                let result = self
                    .connection
                    .call_method(Some(bus_name.as_str()), PATH, Some(interface), method, &())
                    .await;
                if let Err(error) = result {
                    eprintln!("gniri: mpris: {command:?} failed: {error}");
                    // Undo the shell's prediction.
                    return self.read(&bus_name.clone()).await;
                }
                true
            }
        }
    }
}

pub async fn run(
    connection: zbus::Connection,
    events: Sender<Event>,
    commands: Receiver<Command>,
) -> zbus::Result<()> {
    // Players coming and going.
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.DBus")
        .interface("org.freedesktop.DBus")
        .member("NameOwnerChanged")
        .arg0ns(ROOT)
        .build()?;
    let names = zbus::MessageStream::for_match_rule(rule, &connection, None)
        .await?
        .filter_map(|message| {
            let message = message.ok()?;
            let (name, _old, new): (String, String, String) = message.body().deserialize().ok()?;
            if !name.starts_with(PREFIX) {
                return None;
            }
            Some(if new.is_empty() {
                Work::Vanished(name)
            } else {
                Work::Appeared(name)
            })
        });
    // Any player changing anything.
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(PROPERTIES)
        .member("PropertiesChanged")
        .path(PATH)
        .build()?;
    let changes = zbus::MessageStream::for_match_rule(rule, &connection, None)
        .await?
        .filter_map(|message| {
            let message = message.ok()?;
            let sender = message.header().sender()?.to_string();
            Some(Work::Changed(sender))
        });

    let (queue, queued) = async_channel::unbounded();
    // The players that are there already. Queued before any signal is read,
    // so a player that leaves right now is removed after it was added.
    let listed: Vec<String> = connection
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "ListNames",
            &(),
        )
        .await?
        .body()
        .deserialize()?;
    for name in listed {
        if name.starts_with(PREFIX) {
            let _ = queue.try_send(Work::Appeared(name));
        }
    }

    let mut wakes = merge(vec![
        Box::pin(names) as Wakes<Work>,
        Box::pin(changes),
        Box::pin(commands.map(Work::Command)),
    ]);
    let pump = async {
        while let Some(work) = wakes.next().await {
            if queue.try_send(work).is_err() {
                break;
            }
        }
    };
    let mut worker = Worker {
        connection: connection.clone(),
        events,
        owners: HashMap::new(),
    };
    let work = async {
        while let Ok(work) = queued.recv().await {
            if !worker.work(work).await {
                break; // The shell went away.
            }
        }
    };
    futures_lite::future::or(pump, work).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use zbus::Variant as Value;

    use super::*;

    fn properties(entries: Vec<(&str, Value<'static>)>) -> Properties {
        entries
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value.try_into().unwrap()))
            .collect()
    }

    fn no_apps(_: &str) -> Option<DesktopEntry> {
        None
    }

    #[test]
    fn reads_a_well_behaved_player() {
        let metadata: HashMap<&str, Value<'_>> = HashMap::from([
            ("xesam:title", Value::from("A Song")),
            ("xesam:artist", Value::from(vec!["One", "Two"])),
            ("mpris:artUrl", Value::from("file:///tmp/cover%201.png")),
        ]);
        let root = properties(vec![
            ("Identity", Value::from("Example Player")),
            ("DesktopEntry", Value::from("org.example.Player")),
            ("CanRaise", Value::Bool(true)),
        ]);
        let playback = properties(vec![
            ("PlaybackStatus", Value::from("Playing")),
            ("Metadata", Value::from(metadata)),
            ("CanPlay", Value::Bool(true)),
            ("CanGoNext", Value::Bool(true)),
            ("CanGoPrevious", Value::Bool(false)),
        ]);

        let plain = player("org.mpris.MediaPlayer2.example", &root, &playback, no_apps);
        assert_eq!(plain.name, "Example Player");
        assert_eq!(plain.app, None);
        assert_eq!(plain.status, Status::Playing);
        assert_eq!(plain.title, "A Song");
        assert_eq!(plain.artists, ["One", "Two"]);
        assert_eq!(plain.cover, Some(Image::File("/tmp/cover 1.png".into())));
        assert!(plain.can_play && plain.can_go_next && !plain.can_go_previous);
        assert!(plain.can_raise);

        let installed = player("org.mpris.MediaPlayer2.example", &root, &playback, |id| {
            Some(DesktopEntry {
                id: id.to_owned(),
                name: "Music".to_owned(),
                icon: Some("org.example.Player".to_owned()),
                ..DesktopEntry::default()
            })
        });
        assert_eq!(installed.name, "Music");
        assert_eq!(installed.app.as_deref(), Some("org.example.Player"));
        assert_eq!(
            installed.icon,
            Some(Image::Name("org.example.Player".to_owned()))
        );
    }

    #[test]
    fn faulty_metadata_gets_gnomes_placeholders() {
        let metadata: HashMap<&str, Value<'_>> = HashMap::from([
            ("xesam:title", Value::U32(7)),
            ("xesam:artist", Value::from("not a list")),
            ("mpris:artUrl", Value::from("https://example.org/cover.png")),
        ]);
        let playback = properties(vec![("Metadata", Value::from(metadata))]);
        let player = player("x", &Properties::new(), &playback, no_apps);
        assert_eq!(player.title, "Unknown title");
        assert_eq!(player.artists, ["Unknown artist"]);
        assert_eq!(player.cover, None, "nothing is fetched from the network");
        assert_eq!(player.status, Status::Stopped);
        assert!(!player.can_play);

        let empty = super::player("x", &Properties::new(), &Properties::new(), no_apps);
        assert_eq!(empty.title, "Unknown title");
    }
}
