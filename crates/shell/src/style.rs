// SPDX-License-Identifier: GPL-3.0-or-later

//! Loads the compiled theme into GTK.

use gtk::{gdk, gio};

/// Installs the gniri stylesheet for the default display.
///
/// CSS that GTK cannot parse is reported on stderr with its location. With
/// `GNIRI_STRICT_CSS=1` (set by the test harness) it aborts instead, so a bad
/// rule cannot slip through as a silently unstyled widget.
pub fn load() {
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
    provider.load_from_string(gniri_theme::DARK_CSS);

    let display = gdk::Display::default().expect("GTK is initialised, so there is a display");
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    // Icons GNOME Shell bundles instead of taking from the icon theme
    // (crates/shell/resources/icons).
    gio::resources_register_include!("gniri.gresource").expect("the bundled resources are valid");
    gtk::IconTheme::for_display(&display).add_resource_path("/dev/gniri/icons");
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
    static NAMED_RULES: std::cell::RefCell<Vec<(&'static str, gtk::CssProvider)>> =
        const { std::cell::RefCell::new(Vec::new()) };
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
