// SPDX-License-Identifier: GPL-3.0-or-later

//! Desktop-wide settings gniri reads and writes through `GSettings`.
//!
//! The one exception to "direct services only": the colour scheme key is how
//! every GTK and Qt application learns whether the desktop is dark.

use gniri_core::Store;
use gniri_services::Link;
use gniri_services::toggles::{Command, Event, State};
use gniri_services::world_clocks::{self, Clock};
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

const INTERFACE_SCHEMA: &str = "org.gnome.desktop.interface";
const COLOR_SCHEME: &str = "color-scheme";

/// Whether `schema` is installed with `key`: creating a `Settings` for a
/// schema that is not aborts the process.
fn installed(schema: &str, key: &str) -> bool {
    gio::SettingsSchemaSource::default()
        .and_then(|source| source.lookup(schema, true))
        .is_some_and(|schema| schema.has_key(key))
}

/// The name and the place, in radians, of a location as libgweather
/// serializes it: version 2, `(ssbm(dd)m(dd))`.
fn location(serialized: &glib::Variant) -> Option<(String, f64, f64)> {
    // A `v` holds it, as GNOME Clocks and GNOME Shell keep it.
    let serialized = serialized
        .as_variant()
        .unwrap_or_else(|| serialized.clone());
    if serialized.n_children() != 2 || serialized.child_value(0).get::<u32>()? != 2 {
        return None;
    }
    let inner = serialized.child_value(1).as_variant()?;
    let name = inner.child_value(0).str()?.to_owned();
    let place = inner.child_value(3);
    let place = (place.n_children() > 0).then(|| place.child_value(0))?;
    let latitude = place.child_value(0).get::<f64>()?;
    let longitude = place.child_value(1).get::<f64>()?;
    Some((name, latitude, longitude))
}

/// The clocks of the places GNOME Clocks keeps, as `settings` has them.
fn clocks(settings: &gio::Settings, key: &str, zones: &[world_clocks::Zone]) -> Vec<Clock> {
    let value = settings.value(key);
    (0..value.n_children())
        .filter_map(|index| {
            let entry = value.child_value(index);
            // GNOME Clocks: a dictionary with the location under "location";
            // GNOME Shell: the location itself.
            let serialized = if entry.is_container() && entry.type_().as_str() == "a{sv}" {
                glib::VariantDict::new(Some(&entry)).lookup_value("location", None)?
            } else {
                entry
            };
            let (city, latitude, longitude) = location(&serialized)?;
            let zone =
                world_clocks::nearest_zone(zones, latitude.to_degrees(), longitude.to_degrees())?;
            Some(Clock {
                city,
                zone: zone.to_owned(),
            })
        })
        .collect()
}

/// Keeps `store` the world clocks of GNOME Clocks (`world-clocks`), or of
/// GNOME Shell's copy of them (`org.gnome.shell.world-clocks`) where
/// GNOME Clocks' own are not to be had. Returns what keeps it watching.
pub fn world_clocks(store: &Store<Vec<Clock>>) -> Vec<gio::Settings> {
    let zones = std::rc::Rc::new(world_clocks::zone_table());
    let mut watched = Vec::new();
    for (schema, key) in [
        ("org.gnome.clocks", "world-clocks"),
        ("org.gnome.shell.world-clocks", "locations"),
    ] {
        if !installed(schema, key) {
            continue;
        }
        let settings = gio::Settings::new(schema);
        let read = {
            let (store, zones) = (store.clone(), std::rc::Rc::clone(&zones));
            move |settings: &gio::Settings| {
                let clocks = clocks(settings, key, &zones);
                store.update(|current| *current = clocks);
            }
        };
        read(&settings);
        settings.connect_changed(Some(key), move |settings, _| read(settings));
        watched.push(settings);
        // The first that is installed is the one to follow.
        break;
    }
    watched
}

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
