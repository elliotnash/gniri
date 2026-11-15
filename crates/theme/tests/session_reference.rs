// SPDX-License-Identifier: GPL-3.0-or-later

//! Tier 0 of the visual checks for the authentication dialog and the lock
//! screen: theme tokens against the values GNOME Shell 51.1 (with
//! blur-my-shell) computed for its own, as captured in
//! `tests/visual/reference/gnome-51.1/metrics/`.
//!
//! Needs no display. Whether GTK lays the widgets out the same way is tier
//! 1 (`cargo xtask diff polkit-dialog`, `lock-clock`, `lock-prompt`).

mod common;

use common::*;
use serde_json::Value;

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

/// A colour of the reference against a token. An alpha such as 0.1 is
/// 25.5 of 255: St keeps 25 where the token rounds up, so the two may be
/// one apart.
fn assert_colour(theirs: &Value, token: &str) {
    let (theirs, ours) = (numbers(theirs), colour(token));
    assert!(
        theirs
            .iter()
            .zip(&ours)
            .all(|(theirs, ours)| (theirs - ours).abs() <= 1.0),
        "{token}: GNOME has {theirs:?}, the theme {ours:?}"
    );
}

/// The top and right of a four-sided value, which is how the theme's
/// two-length tokens are written.
fn top_right(value: &Value) -> Vec<f64> {
    let sides = numbers(value);
    vec![sides[0], sides[1]]
}

#[test]
fn authentication_dialog() {
    let reference = reference("polkit-dialog");
    let dialog = by_class(&reference["tree"], "modal-dialog");
    let em = em(dialog);
    let style = &dialog["style"];

    // blur-my-shell's tint, text colour, border and corner radius.
    assert_eq!(numbers(&style["bg"]), colour("bms-popup-tint"));
    assert_eq!(numbers(&style["fg"]), colour("bms-popup-fg-color"));
    assert_colour(&style["border_color"][0], "bms-dialog-border-color");
    assert_eq!(
        numbers(&style["radius"])[0],
        pixels("bms-dialog-radius", em)
    );
    assert_eq!(
        numbers(&style["padding"])[0],
        pixels("modal-dialog-padding", em)
    );
    assert_eq!(
        style["spacing"].as_f64().unwrap(),
        pixels("modal-dialog-spacing", em)
    );
    assert_eq!(
        style["width"].as_f64().unwrap(),
        minimum_pixels("prompt-dialog-width", em)
    );

    let content = by_class(dialog, "modal-dialog-content-box");
    assert_eq!(
        numbers(&content["style"]["padding"])[0],
        pixels("modal-dialog-content-padding-top", em)
    );
    assert_eq!(
        content["style"]["spacing"].as_f64().unwrap(),
        pixels("modal-dialog-spacing", em)
    );

    // The user and the entry.
    let user = by_class(dialog, "polkit-dialog-user-layout");
    assert_eq!(
        user["style"]["spacing"].as_f64().unwrap(),
        pixels("polkit-dialog-user-spacing", em)
    );
    assert_eq!(
        numbers(&user["style"]["margin"])[2],
        pixels("polkit-dialog-user-margin-bottom", em)
    );
    let avatar = by_class(user, "user-avatar");
    assert_eq!(
        numbers(&avatar["box"])[2],
        pixels("polkit-dialog-avatar-size", em)
    );
    assert_colour(&avatar["style"]["bg"], "user-icon-color");
    assert_colour(
        &avatar["style"]["box_shadow"]["color"],
        "user-avatar-outline-color",
    );

    let layout = by_class(dialog, "prompt-dialog-password-layout");
    assert_eq!(
        layout["style"]["spacing"].as_f64().unwrap(),
        pixels("prompt-dialog-layout-spacing", em)
    );
    let entry = by_class(dialog, "prompt-dialog-password-entry");
    assert_eq!(
        top_right(&entry["style"]["padding"]),
        lengths("prompt-dialog-entry-padding", em)
    );
    assert_eq!(
        entry["style"]["width"].as_f64().unwrap(),
        minimum_pixels("prompt-dialog-entry-width", em)
    );
    assert_eq!(
        numbers(&entry["style"]["radius"])[0],
        pixels("entry-radius", em)
    );
    assert_colour(&entry["style"]["bg"], "bms-entry-color");
    let hint = by_class(entry, "hint-text");
    assert_eq!(
        numbers(&hint["style"]["margin"])[3],
        pixels("entry-hint-margin", em)
    );
    assert_colour(&hint["style"]["fg"], "bms-entry-hint-color");
    let peek = by_class(entry, "peek-password");
    assert_eq!(
        numbers(&peek["style"]["padding"])[1],
        pixels("entry-icon-padding", em)
    );

    // The line under the entry, in its empty look.
    let message = by_class(dialog, "prompt-dialog-null-label");
    assert_eq!(
        numbers(&message["style"]["padding"])[0],
        pixels("prompt-dialog-label-padding", em)
    );
    assert_eq!(
        numbers(&message["style"]["margin"])[0],
        pixels("prompt-dialog-label-margin", em)
    );

    // The buttons: one ready, one with nothing to send yet.
    let buttons = by_class(dialog, "modal-dialog-button-box");
    assert_eq!(
        numbers(&buttons["style"]["padding"])[0],
        pixels("modal-dialog-button-box-padding-top", em)
    );
    let both = buttons["children"].as_array().expect("two buttons");
    let (cancel, authenticate) = (&both[0]["style"], &both[1]["style"]);
    assert_eq!(
        numbers(&cancel["padding"])[0],
        pixels("modal-dialog-button-padding", em)
    );
    assert_eq!(
        numbers(&cancel["radius"])[0],
        pixels("modal-dialog-button-radius", em)
    );
    assert_colour(&cancel["bg"], "bms-dialog-button-color");
    assert_colour(&authenticate["bg"], "bms-dialog-button-disabled-color");
    assert_colour(&authenticate["fg"], "bms-dialog-button-disabled-fg-color");
}

#[test]
fn lock_screen_clock() {
    let reference = reference("lock-clock");
    let clock = by_class(&reference["tree"], "unlock-dialog-clock");
    let em = em(clock);

    assert_eq!(numbers(&clock["style"]["fg"]), colour("lock-fg-color"));
    // 2em, which St rounds.
    assert_eq!(
        clock["style"]["spacing"].as_f64().unwrap().round(),
        pixels("lock-clock-spacing", em)
    );
    let hint = by_class(clock, "unlock-dialog-clock-hint");
    assert_eq!(
        top_right(&hint["style"]["padding"]),
        lengths("lock-hint-padding", em)
    );
    assert_eq!(
        numbers(&hint["style"]["radius"])[0],
        pixels("lock-hint-radius", em)
    );
    assert_eq!(
        numbers(&hint["style"]["margin"])[0],
        pixels("lock-clock-spacing", em)
    );
}

#[test]
fn lock_screen_prompt() {
    let reference = reference("lock-prompt");
    let prompt = by_class(&reference["tree"], "login-dialog-prompt-layout");
    let em = em(prompt);

    assert_eq!(
        prompt["style"]["width"].as_f64().unwrap(),
        minimum_pixels("lock-prompt-width", em)
    );

    let user = by_class(prompt, "user-widget");
    assert_eq!(
        user["style"]["spacing"].as_f64().unwrap(),
        pixels("lock-user-spacing", em)
    );
    let avatar = by_class(user, "user-avatar");
    assert_eq!(numbers(&avatar["box"])[2], pixels("lock-avatar-size", em));
    assert_colour(&avatar["style"]["bg"], "lock-avatar-color");

    let well = by_class(prompt, "login-dialog-input-well");
    assert_eq!(
        numbers(&well["style"]["margin"])[0],
        pixels("lock-input-well-margin", em)
    );
    let buttons = by_class(prompt, "login-dialog-button-box");
    assert_eq!(
        buttons["style"]["height"].as_f64().unwrap(),
        minimum_pixels("lock-button-box-height", em)
    );
    let area = by_class(prompt, "login-dialog-prompt-entry-area");
    assert_eq!(
        top_right(&area["style"]["margin"]),
        lengths("lock-entry-area-margin", em)
    );

    // The entry, which has the keyboard focus in the capture.
    let entry = by_class(prompt, "login-dialog-prompt-entry");
    let style = &entry["style"];
    assert_eq!(
        numbers(&style["radius"])[0],
        pixels("lock-entry-radius", em)
    );
    assert_eq!(top_right(&style["padding"]), lengths("entry-padding", em));
    assert_colour(&style["bg"], "lock-entry-focus-color");
    assert_colour(&style["box_shadow"]["color"], "lock-entry-focus-ring-color");
    assert_eq!(numbers(&style["fg"]), colour("lock-fg-color"));
    // The room it leaves at its end for the arrow that submits.
    let peek = by_class(entry, "peek-password");
    assert_eq!(
        numbers(&peek["style"]["padding"])[1],
        pixels("lock-entry-icon-room", em)
    );

    let message = by_class(prompt, "login-dialog-message");
    assert_eq!(
        numbers(&message["style"]["fg"]),
        colour("lock-message-color")
    );
    assert_eq!(
        message["style"]["height"].as_f64().unwrap(),
        minimum_pixels("lock-message-height", em)
    );
    assert_eq!(
        numbers(&message["style"]["margin"])[0],
        pixels("lock-message-margin-top", em)
    );
}
