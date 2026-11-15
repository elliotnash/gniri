// SPDX-License-Identifier: GPL-3.0-or-later

//! The media player backend against a python-dbusmock object that stands in
//! for a player.

mod common;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use common::{Follower, MockBus, TIMEOUT};
use gniri_services::mpris::{self, Command, Status};
use zbus::Variant;

const BUS_NAME: &str = "org.mpris.MediaPlayer2.example";
const PATH: &str = "/org/mpris/MediaPlayer2";
const ROOT: &str = "org.mpris.MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";
const MOCK: &str = "org.freedesktop.DBus.Mock";

fn metadata(title: &str) -> Variant<'static> {
    let entries: HashMap<&str, Variant<'_>> = HashMap::from([
        ("xesam:title", Variant::from(title.to_owned())),
        ("xesam:artist", Variant::from(vec!["An Artist".to_owned()])),
    ]);
    Variant::from(entries).try_to_owned().unwrap().into()
}

/// Puts a player on the bus: paused on "First Song", able to play.
fn add_player(bus: &mut MockBus) {
    bus.object(BUS_NAME, PATH, ROOT);
    let root: HashMap<&str, Variant<'_>> = HashMap::from([
        ("Identity", Variant::from("Example Player")),
        ("CanRaise", Variant::Bool(true)),
    ]);
    bus.call(BUS_NAME, PATH, MOCK, "AddProperties", &(ROOT, root));
    let player: HashMap<&str, Variant<'_>> = HashMap::from([
        ("PlaybackStatus", Variant::from("Paused")),
        ("Metadata", metadata("First Song")),
        ("CanPlay", Variant::Bool(true)),
        ("CanGoNext", Variant::Bool(true)),
        ("CanGoPrevious", Variant::Bool(false)),
    ]);
    bus.call(BUS_NAME, PATH, MOCK, "AddProperties", &(PLAYER, player));
    for method in ["PlayPause", "Next", "Previous"] {
        bus.call(
            BUS_NAME,
            PATH,
            MOCK,
            "AddMethod",
            &(PLAYER, method, "", "", ""),
        );
    }
}

/// Waits until the mock has been called `method`.
fn wait_for_call(bus: &MockBus, method: &str) {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let calls: Vec<(u64, Vec<zbus::OwnedVariant>)> = bus
            .call(BUS_NAME, PATH, MOCK, "GetMethodCalls", &(method))
            .body()
            .deserialize()
            .expect("the calls so far");
        if !calls.is_empty() {
            return;
        }
        assert!(Instant::now() < deadline, "{method} was never called");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_player_that_is_already_there() {
    let Some(mut bus) = MockBus::start() else {
        return;
    };
    add_player(&mut bus);

    let mut players: Follower<mpris::State> = Follower::new(bus.run(mpris::backend::run));
    players.wait("the player", |state| state.shown().count() == 1);
    let player = players.state.players[0].clone();
    assert_eq!(player.bus_name, BUS_NAME);
    assert_eq!(player.name, "Example Player");
    assert_eq!(player.status, Status::Paused);
    assert_eq!(player.title, "First Song");
    assert_eq!(player.artists, ["An Artist"]);
    assert!(player.can_go_next && !player.can_go_previous && player.can_raise);

    // The buttons call the player.
    players.send(Command::PlayPause(BUS_NAME.to_owned()));
    wait_for_call(&bus, "PlayPause");
    players.send(Command::Next(BUS_NAME.to_owned()));
    wait_for_call(&bus, "Next");

    // The player moves on and says so.
    let changed: HashMap<&str, Variant<'_>> = HashMap::from([
        ("PlaybackStatus", Variant::from("Playing")),
        ("Metadata", metadata("Second Song")),
        ("CanGoPrevious", Variant::Bool(true)),
    ]);
    bus.call(BUS_NAME, PATH, MOCK, "UpdateProperties", &(PLAYER, changed));
    players.wait("the next song", |state| {
        state
            .players
            .first()
            .is_some_and(|player| player.title == "Second Song")
    });
    let player = &players.state.players[0];
    assert_eq!(player.status, Status::Playing);
    assert!(player.can_go_previous);

    // It quits.
    bus.stop_templates();
    players.wait("the player to go", |state| state.players.is_empty());
}

#[test]
fn a_player_that_starts_later() {
    let Some(mut bus) = MockBus::start() else {
        return;
    };
    let mut players: Follower<mpris::State> = Follower::new(bus.run(mpris::backend::run));
    // Give the backend time to find the bus empty.
    std::thread::sleep(Duration::from_millis(200));
    assert!(players.state.players.is_empty());

    // The mock claims its name before it has any properties, which no real
    // player does; the change it announces afterwards is what a player
    // sends once it has something to play.
    add_player(&mut bus);
    let changed: HashMap<&str, Variant<'_>> =
        HashMap::from([("PlaybackStatus", Variant::from("Playing"))]);
    bus.call(BUS_NAME, PATH, MOCK, "UpdateProperties", &(PLAYER, changed));
    players.wait("the player", |state| {
        state
            .players
            .first()
            .is_some_and(|player| player.status == Status::Playing)
    });
    assert_eq!(players.state.shown().count(), 1);
    assert_eq!(players.state.players[0].title, "First Song");
}
