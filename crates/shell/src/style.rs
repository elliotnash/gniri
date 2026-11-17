// SPDX-License-Identifier: GPL-3.0-or-later

//! Loads the compiled theme into GTK.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gniri_config::Style;
use gniri_services::Fixture;
use gniri_services::toggles::ColorScheme;
use gniri_theme::Variant;
use gtk::prelude::*;
use gtk::{gdk, gio};

/// The stylesheet the shell has for a style setting and the desktop's
/// colour scheme. GNOME Shell's `getStyleVariant` (main.js), whose session
/// modes have the same four settings; an ordinary session's is
/// "prefer-dark".
pub fn variant(style: Style, scheme: ColorScheme) -> Variant {
    match style {
        Style::Dark => Variant::Dark,
        Style::Light => Variant::Light,
        Style::PreferDark if scheme == ColorScheme::PreferLight => Variant::Light,
        Style::PreferDark => Variant::Dark,
        Style::PreferLight if scheme == ColorScheme::PreferDark => Variant::Dark,
        Style::PreferLight => Variant::Light,
    }
}

thread_local! {
    /// The theme in use, once [`load`] has run.
    static THEME: RefCell<Option<(Variant, gtk::CssProvider)>> = const { RefCell::new(None) };
}

/// A design token of the theme in use (of the dark one before any is).
pub fn token(name: &str) -> Option<&'static str> {
    match THEME.with_borrow(|theme| theme.as_ref().map(|(variant, _)| *variant)) {
        Some(Variant::Light) => gniri_theme::light_token(name),
        _ => gniri_theme::token(name),
    }
}

fn provider(variant: Variant) -> gtk::CssProvider {
    // CSS that GTK cannot parse is reported on stderr with its location.
    // With `GNIRI_STRICT_CSS=1` (set by the test harness) it aborts
    // instead, so a bad rule cannot slip through as a silently unstyled
    // widget.
    let strict = std::env::var_os("GNIRI_STRICT_CSS").is_some_and(|value| value == "1");

    let provider = gtk::CssProvider::new();
    provider.connect_parsing_error(move |_, section, error| {
        let location = section.start_location();
        let message = format!(
            "gniri: theme CSS error at line {}, column {}: {}",
            location.lines() + 1,
            location.line_chars() + 1,
            error.message()
        );
        if strict {
            panic!("{message}");
        }
        eprintln!("{message}");
    });
    provider.load_from_string(variant.css());
    provider
}

/// Installs the gniri stylesheet for the default display, dark until
/// [`set_variant`] says otherwise.
pub fn load() {
    let display = gdk::Display::default().expect("GTK is initialised, so there is a display");

    // Icons GNOME Shell bundles instead of taking from the icon theme
    // (crates/shell/resources/icons).
    gio::resources_register_include!("gniri.gresource").expect("the bundled resources are valid");
    gtk::IconTheme::for_display(&display).add_resource_path("/dev/gniri/icons");

    set_variant(Variant::Dark);
}

/// Changes to the dark or the light stylesheet. Everything on screen is
/// styled anew.
pub fn set_variant(variant: Variant) {
    let display = gdk::Display::default().expect("GTK is initialised, so there is a display");
    THEME.with_borrow_mut(|theme| {
        if let Some((current, old)) = theme.as_ref() {
            if *current == variant {
                return;
            }
            gtk::style_context_remove_provider_for_display(&display, old);
        }
        // libadwaita's own colours, which the theme's are derived from
        // (the accent) or laid over (a text field's selection).
        adw::StyleManager::default().set_color_scheme(match variant {
            Variant::Dark => adw::ColorScheme::ForceDark,
            Variant::Light => adw::ColorScheme::ForceLight,
        });
        let provider = provider(variant);
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        *theme = Some((variant, provider));
    });
}

const INTERFACE_SCHEMA: &str = "org.gnome.desktop.interface";
const COLOR_SCHEME: &str = "color-scheme";

fn read_scheme(settings: &gio::Settings) -> ColorScheme {
    match settings.string(COLOR_SCHEME).as_str() {
        "prefer-dark" => ColorScheme::PreferDark,
        "prefer-light" => ColorScheme::PreferLight,
        _ => ColorScheme::Default,
    }
}

/// Keeps the stylesheet the one the style setting and the desktop's
/// colour scheme ask for, for as long as it lives.
pub struct Follower {
    style: Rc<Cell<Style>>,
    /// The desktop's setting; absent in a scene, and where GNOME's
    /// settings are not installed.
    settings: Option<gio::Settings>,
    /// What a scene says the colour scheme is.
    fixed: ColorScheme,
}

impl Follower {
    fn apply(style: Style, scheme: ColorScheme) {
        set_variant(variant(style, scheme));
    }

    fn scheme(&self) -> ColorScheme {
        self.settings.as_ref().map_or(self.fixed, read_scheme)
    }

    /// Takes a changed style setting into use.
    pub fn set_style(&self, style: Style) {
        self.style.set(style);
        Self::apply(style, self.scheme());
    }
}

/// Chooses the stylesheet now and whenever the desktop's colour scheme
/// changes. A scene's colour scheme is the scene's: nothing of the
/// desktop's is read there.
pub fn follow(style: Style, fixture: Option<&Fixture>) -> Follower {
    let style = Rc::new(Cell::new(style));
    let follower = match fixture {
        Some(fixture) => Follower {
            style,
            settings: None,
            fixed: fixture.color_scheme.unwrap_or(if fixture.dark_style.on {
                ColorScheme::PreferDark
            } else {
                ColorScheme::Default
            }),
        },
        None => {
            // Creating a `Settings` for a schema that is not installed
            // aborts the process, so look first.
            let installed = gio::SettingsSchemaSource::default()
                .and_then(|source| source.lookup(INTERFACE_SCHEMA, true))
                .is_some_and(|schema| schema.has_key(COLOR_SCHEME));
            let settings = installed.then(|| gio::Settings::new(INTERFACE_SCHEMA));
            if let Some(settings) = &settings {
                settings.connect_changed(Some(COLOR_SCHEME), {
                    let style = Rc::clone(&style);
                    move |settings, _| Follower::apply(style.get(), read_scheme(settings))
                });
            }
            Follower {
                style,
                settings,
                fixed: ColorScheme::Default,
            }
        }
    };
    Follower::apply(follower.style.get(), follower.scheme());
    follower
}

/// Adds one extra rule for the default display, above the theme.
pub fn add_rule(css: &str) {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(css);
    gtk::style_context_add_provider_for_display(
        &gdk::Display::default().expect("GTK is initialised, so there is a display"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
    );
}

thread_local! {
    /// The rules [`set_rule`] has in place, by name.
    static NAMED_RULES: RefCell<Vec<(&'static str, gtk::CssProvider)>> =
        const { RefCell::new(Vec::new()) };
}

/// Like [`add_rule`] for a rule that changes: the rule called `name`
/// replaces the one of that name from before. Empty `css` only takes that
/// one away.
pub fn set_rule(name: &'static str, css: &str) {
    let display = gdk::Display::default().expect("GTK is initialised, so there is a display");
    NAMED_RULES.with_borrow_mut(|rules| {
        if let Some(index) = rules.iter().position(|(other, _)| *other == name) {
            let (_, old) = rules.swap_remove(index);
            gtk::style_context_remove_provider_for_display(&display, &old);
        }
        if css.is_empty() {
            return;
        }
        let provider = gtk::CssProvider::new();
        provider.load_from_string(css);
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );
        rules.push((name, provider));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shell_is_dark_unless_told_or_asked_outright() {
        use ColorScheme::{Default, PreferDark, PreferLight};
        // GNOME's own rule: the Dark Style switch (default or prefer-dark)
        // changes nothing about the shell.
        assert_eq!(variant(Style::PreferDark, Default), Variant::Dark);
        assert_eq!(variant(Style::PreferDark, PreferDark), Variant::Dark);
        assert_eq!(variant(Style::PreferDark, PreferLight), Variant::Light);
        // GNOME Classic's: the shell follows the switch.
        assert_eq!(variant(Style::PreferLight, Default), Variant::Light);
        assert_eq!(variant(Style::PreferLight, PreferLight), Variant::Light);
        assert_eq!(variant(Style::PreferLight, PreferDark), Variant::Dark);
        for scheme in [Default, PreferDark, PreferLight] {
            assert_eq!(variant(Style::Dark, scheme), Variant::Dark);
            assert_eq!(variant(Style::Light, scheme), Variant::Light);
        }
    }
}
