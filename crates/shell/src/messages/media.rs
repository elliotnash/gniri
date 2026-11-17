// SPDX-License-Identifier: GPL-3.0-or-later

//! A media player's card: what it plays, and its controls. Port of
//! `MediaMessage` (mpris.js). The date menu's list shows one per player,
//! and so does the lock screen.

use std::rc::Rc;

use gniri_services::mpris::{self, Player};
use gtk::prelude::*;

use super::message::Message;

pub struct MediaCard {
    pub message: Message,
    play_pause: gtk::Button,
    previous: gtk::Button,
    next: gtk::Button,
}

impl MediaCard {
    /// The card of the player at `bus_name`: `send` takes its controls'
    /// commands, and `clicked` what a click on the card does.
    pub fn new(
        bus_name: &str,
        send: impl Fn(mpris::Command) + 'static,
        clicked: impl Fn(&str) + 'static,
    ) -> Self {
        let message = Message::new();
        message.root().add_css_class("media-message");
        message.set_close_visible(false);
        message.set_time(None);

        let send = Rc::new(send);
        let control = |icon: &str, command: fn(String) -> mpris::Command| {
            let (send, bus_name) = (Rc::clone(&send), bus_name.to_owned());
            message.add_media_control(icon, move || send(command(bus_name.clone())))
        };
        let previous = control("media-skip-backward-symbolic", mpris::Command::Previous);
        let play_pause = control("media-playback-start-symbolic", mpris::Command::PlayPause);
        let next = control("media-skip-forward-symbolic", mpris::Command::Next);

        // `MediaMessage.vfunc_clicked`
        message.connect_clicked({
            let bus_name = bus_name.to_owned();
            move || clicked(&bus_name)
        });
        Self {
            message,
            play_pause,
            previous,
            next,
        }
    }

    /// `MediaMessage._update`
    pub fn update(&self, player: &Player) {
        self.message.set_source(&player.name, player.icon.as_ref());
        self.message.set_title(&player.title);
        self.message.set_body(&player.artists_text());
        self.message.set_icon(Some(&player.picture()));
        if let Some(image) = self
            .play_pause
            .child()
            .and_then(|child| child.downcast::<gtk::Image>().ok())
        {
            image.set_icon_name(Some(player.play_pause_icon()));
        }
        self.previous.set_sensitive(player.can_go_previous);
        self.next.set_sensitive(player.can_go_next);
    }
}
