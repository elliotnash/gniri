// SPDX-License-Identifier: GPL-3.0-or-later

//! Media players: what is playing, and the buttons to control it. Port of
//! gnome-shell's `js/ui/mpris.js`.

pub mod backend;

use serde::Deserialize;

use crate::model::Model;
pub use crate::notifications::Image;

/// The bus names media players take start with this.
pub const PREFIX: &str = "org.mpris.MediaPlayer2.";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Playing,
    Paused,
    #[default]
    Stopped,
}

impl Status {
    /// The `PlaybackStatus` property's text.
    pub fn from_property(text: &str) -> Self {
        match text {
            "Playing" => Self::Playing,
            "Paused" => Self::Paused,
            _ => Self::Stopped,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Player {
    /// What tells players apart, such as "org.mpris.MediaPlayer2.vlc".
    pub bus_name: String,
    /// The application's name, or what the player calls itself.
    pub name: String,
    /// The application's icon.
    pub icon: Option<Image>,
    /// The desktop entry (without ".desktop") of the application, if it is
    /// installed. Clicking the card starts it.
    pub app: Option<String>,
    pub status: Status,
    pub title: String,
    pub artists: Vec<String>,
    /// The cover, if the player gave one gniri can read: a local file.
    pub cover: Option<Image>,
    /// Only a player that can play is shown.
    pub can_play: bool,
    pub can_go_next: bool,
    pub can_go_previous: bool,
    /// The player can bring its own window up.
    pub can_raise: bool,
}

impl Player {
    /// The card's second line (`MediaMessage._update`).
    pub fn artists_text(&self) -> String {
        self.artists.join(", ")
    }

    /// The icon of the middle button.
    pub fn play_pause_icon(&self) -> &'static str {
        if self.status == Status::Playing {
            "media-playback-pause-symbolic"
        } else {
            "media-playback-start-symbolic"
        }
    }

    /// The picture on the card: the cover, or a note.
    pub fn picture(&self) -> Image {
        self.cover
            .clone()
            .unwrap_or_else(|| Image::Name("audio-x-generic-symbolic".to_owned()))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct State {
    /// Every player on the bus, in the order they appeared.
    pub players: Vec<Player>,
}

impl State {
    /// The players that get a card (`MprisSource.players`).
    pub fn shown(&self) -> impl Iterator<Item = &Player> {
        self.players.iter().filter(|player| player.can_play)
    }
}

/// What the backend reports.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A player appeared or something about it changed.
    Player(Box<Player>),
    /// The player with this bus name is gone.
    Gone(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    PlayPause(String),
    Next(String),
    Previous(String),
    /// Ask the player to show its window, for a player whose application is
    /// not installed as a desktop entry.
    Raise(String),
}

impl Model for State {
    type Event = Event;
    type Command = Command;

    fn apply(&mut self, event: Event) {
        match event {
            Event::Player(player) => {
                match self
                    .players
                    .iter_mut()
                    .find(|other| other.bus_name == player.bus_name)
                {
                    Some(existing) => *existing = *player,
                    None => self.players.push(*player),
                }
            }
            Event::Gone(bus_name) => self.players.retain(|player| player.bus_name != bus_name),
        }
    }

    /// Only the play button shows a change before the player confirms it.
    fn predict(&mut self, command: &Command) {
        if let Command::PlayPause(bus_name) = command
            && let Some(player) = self
                .players
                .iter_mut()
                .find(|player| player.bus_name == *bus_name)
        {
            player.status = match player.status {
                Status::Playing => Status::Paused,
                Status::Paused | Status::Stopped => Status::Playing,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player(bus_name: &str, can_play: bool) -> Player {
        Player {
            bus_name: bus_name.to_owned(),
            can_play,
            ..Player::default()
        }
    }

    #[test]
    fn players_come_change_and_go() {
        let mut state = State::default();
        state.apply(Event::Player(Box::new(player("a", true))));
        state.apply(Event::Player(Box::new(player("b", false))));
        assert_eq!(state.shown().count(), 1, "one cannot play");

        state.apply(Event::Player(Box::new(Player {
            title: "Song".to_owned(),
            ..player("b", true)
        })));
        let names: Vec<&str> = state.shown().map(|p| p.bus_name.as_str()).collect();
        assert_eq!(names, ["a", "b"], "a change keeps the order");
        assert_eq!(state.players[1].title, "Song");

        state.apply(Event::Gone("a".to_owned()));
        assert_eq!(state.players.len(), 1);
    }

    #[test]
    fn the_play_button_answers_at_once() {
        let mut state = State {
            players: vec![player("a", true)],
        };
        assert_eq!(
            state.players[0].play_pause_icon(),
            "media-playback-start-symbolic"
        );
        state.predict(&Command::PlayPause("a".to_owned()));
        assert_eq!(state.players[0].status, Status::Playing);
        assert_eq!(
            state.players[0].play_pause_icon(),
            "media-playback-pause-symbolic"
        );
        state.predict(&Command::PlayPause("a".to_owned()));
        assert_eq!(state.players[0].status, Status::Paused);
        state.predict(&Command::Next("a".to_owned()));
        assert_eq!(state.players[0].status, Status::Paused);
    }

    #[test]
    fn card_texts() {
        let player = Player {
            artists: vec!["One".to_owned(), "Two".to_owned()],
            ..Player::default()
        };
        assert_eq!(player.artists_text(), "One, Two");
        assert_eq!(
            player.picture(),
            Image::Name("audio-x-generic-symbolic".to_owned())
        );
        assert_eq!(Status::from_property("Playing"), Status::Playing);
        assert_eq!(Status::from_property("nonsense"), Status::Stopped);
    }

    #[test]
    fn a_scene_describes_a_player() {
        let state: State = toml::from_str(
            r#"
            [[players]]
            bus_name = "org.mpris.MediaPlayer2.example"
            name = "Music"
            status = "playing"
            title = "A Song"
            artists = ["An Artist"]
            can_play = true
            can_go_next = true
            "#,
        )
        .unwrap();
        assert_eq!(state.shown().count(), 1);
        assert_eq!(state.players[0].status, Status::Playing);
    }
}
