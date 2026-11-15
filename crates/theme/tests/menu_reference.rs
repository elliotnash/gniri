// SPDX-License-Identifier: GPL-3.0-or-later

//! Tier 0 of the visual checks for the quick settings menu and the
//! on-screen display: theme tokens against the values GNOME Shell 51.1
//! (with blur-my-shell) computed for its own, as captured in
//! `tests/visual/reference/gnome-51.1/metrics/`.
//!
//! Needs no display. Whether GTK lays the widgets out the same way is tier
//! 1 (`cargo xtask diff quick-settings`, `cargo xtask diff osd`).

mod common;

use common::*;

/// A token of several lengths, such as a padding, each as GTK lays it out.
fn lengths(name: &str, em: f64) -> Vec<f64> {
    let value = gniri_theme::token(name).unwrap_or_else(|| panic!("no token {name}"));
    value
        .split_whitespace()
        .map(|part| {
            let pixels = match part.strip_suffix("em") {
                Some(ems) => ems.parse::<f64>().expect("a number") * em,
                None => part
                    .strip_suffix("px")
                    .and_then(|pixels| pixels.parse().ok())
                    .unwrap_or_else(|| panic!("token {name} is not lengths: {value}")),
            };
            pixels.floor()
        })
        .collect()
}

#[test]
fn quick_settings_menu() {
    let reference = reference("quick-settings");
    let menu = by_class(&reference["tree"], "quick-settings");
    let em = em(menu);
    let style = &menu["style"];

    // blur-my-shell's tint and border, before the tint is folded.
    assert_eq!(numbers(&style["bg"]), colour("bms-popup-tint"));
    assert_eq!(numbers(&style["fg"]), colour("bms-popup-fg-color"));
    assert_eq!(
        numbers(&style["border_color"][0]),
        colour("bms-popup-border-color")
    );
    assert_eq!(
        numbers(&style["padding"])[0],
        pixels("quick-settings-padding", em)
    );
    assert_eq!(
        numbers(&style["radius"])[0],
        pixels("quick-settings-radius", em)
    );

    // A plain toggle: its size, and the extension's button colour.
    let toggle = by_type(&reference["tree"], "Gjs_status_nightLight_NightLightToggle");
    assert_eq!(
        toggle["style"]["min_width"].as_f64().unwrap(),
        length("quick-toggle-width", em).round()
    );
    assert_eq!(
        toggle["style"]["min_height"].as_f64().unwrap(),
        pixels("quick-toggle-height", em)
    );
    // An alpha of 0.1 is 25.5 of 255; St keeps 25 where the token rounds up.
    let (theirs, ours) = (
        numbers(&toggle["style"]["bg"]),
        colour("bms-popup-button-color"),
    );
    assert!(
        theirs
            .iter()
            .zip(&ours)
            .all(|(theirs, ours)| (theirs - ours).abs() <= 1.0),
        "{theirs:?} against {ours:?}"
    );

    // The round buttons of the first row, and the flat ones beside a slider.
    let lock = by_type(&reference["tree"], "Gjs_status_system_LockItem");
    assert_eq!(
        numbers(&lock["style"]["padding"])[0],
        pixels("quick-settings-button-padding", em)
    );
    let slider = by_class(&reference["tree"], "quick-slider");
    let slider_button = by_class(slider, "icon-button");
    assert_eq!(
        numbers(&slider_button["style"]["padding"])[0],
        pixels("quick-slider-button-padding", em)
    );
}

#[test]
fn quick_settings_submenu() {
    let reference = reference("quick-settings-submenu");
    let menu = by_class(&reference["tree"], "quick-toggle-menu");
    let em = em(menu);
    let style = &menu["style"];

    // The white the extension lays over its own blur; gniri folds it into
    // one colour (`bms-submenu-color`).
    assert_eq!(numbers(&style["bg"]), colour("bms-submenu-white"));
    assert_eq!(
        numbers(&style["border_color"][0]),
        colour("bms-popup-border-color")
    );
    assert_eq!(
        numbers(&style["radius"])[0],
        pixels("quick-toggle-menu-radius", em)
    );
    assert_eq!(
        numbers(&style["padding"])[0],
        pixels("quick-toggle-menu-padding", em)
    );
    assert_eq!(
        numbers(&style["margin"])[0],
        pixels("quick-toggle-menu-margin-top", em)
    );

    let icon = by_class(by_class(menu, "header"), "icon");
    assert_eq!(
        numbers(&icon["style"]["padding"])[0],
        pixels("quick-toggle-menu-icon-padding", em)
    );

    let item = by_class(menu, "popup-menu-item");
    let padding = numbers(&item["style"]["padding"]);
    assert_eq!(
        vec![padding[0], padding[1]],
        lengths("popup-menu-item-padding", em)
    );
    assert_eq!(
        numbers(&item["style"]["radius"])[0],
        pixels("popup-menu-item-radius", em)
    );
    assert_eq!(
        item["style"]["spacing"].as_f64().unwrap(),
        pixels("popup-menu-item-spacing", em)
    );
}

#[test]
fn on_screen_display() {
    let reference = reference("osd");
    let osd = by_class(&reference["tree"], "osd-window");
    let em = em(osd);
    let style = &osd["style"];

    assert_eq!(numbers(&style["bg"]), colour("bms-popup-tint"));
    assert_eq!(numbers(&style["fg"]), colour("bms-popup-fg-color"));
    assert_eq!(numbers(&style["radius"])[0], pixels("bms-osd-radius", em));
    let padding = numbers(&style["padding"]);
    assert_eq!(vec![padding[0], padding[1]], lengths("osd-padding", em));

    let level = by_class(osd, "level");
    assert_eq!(
        level["style"]["min_width"].as_f64().unwrap(),
        pixels("osd-level-width", em)
    );
}
