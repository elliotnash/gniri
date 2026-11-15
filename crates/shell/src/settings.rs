// SPDX-License-Identifier: GPL-3.0-or-later

//! Desktop-wide settings gniri reads and writes through `GSettings`.
//!
//! The one exception to "direct services only": the colour scheme key is how
//! every GTK and Qt application learns whether the desktop is dark.

use gniri_services::Link;
use gniri_services::toggles::{Command, Event, State};
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

const INTERFACE_SCHEMA: &str = "org.gnome.desktop.interface";
const COLOR_SCHEME: &str = "color-scheme";

/// The Dark Style toggle: `color-scheme` is `prefer-dark` or `default`.
/// Port of `status/darkMode.js`.
pub fn dark_style() -> Link<State> {
    let (link, events, commands) = Link::<State>::new();

    // Creating a `Settings` for a schema that is not installed aborts the
    // process, so look first.
    let installed = gio::SettingsSchemaSource::default()
        .and_then(|source| source.lookup(INTERFACE_SCHEMA, true))
        .is_some_and(|schema| schema.has_key(COLOR_SCHEME));
    if !installed {
        let _ = events.try_send(Event::State(State::default()));
        return link;
    }

    let settings = gio::Settings::new(INTERFACE_SCHEMA);
    let report = {
        let events = events.clone();
        move |settings: &gio::Settings| {
            let _ = events.try_send(Event::State(State {
                available: true,
                on: settings.string(COLOR_SCHEME) == "prefer-dark",
            }));
        }
    };
    report(&settings);
    settings.connect_changed(Some(COLOR_SCHEME), move |settings, _| report(settings));

    glib::spawn_future_local(async move {
        while let Ok(Command::Set(dark)) = commands.recv().await {
            let value = if dark { "prefer-dark" } else { "default" };
            if let Err(error) = settings.set_string(COLOR_SCHEME, value) {
                eprintln!("gniri: cannot set the colour scheme: {error}");
            }
        }
    });
    link
}
