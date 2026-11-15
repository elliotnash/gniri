//! Tier 0 of the visual checks for the date menu and the notification
//! banner: theme tokens against the values GNOME Shell 51.1 (with
//! blur-my-shell) computed for its own, as captured in
//! `tests/visual/reference/gnome-51.1/metrics/`.
//!
//! Needs no display. Whether GTK lays the widgets out the same way is tier
//! 1 (`cargo xtask diff date-menu`, `cargo xtask diff notification-banner`).

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

/// A translucent white is some fraction of 255 that St rounds down and the
/// token's conversion rounds to nearest; they may be one apart.
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

/// All four sides of a padding or margin, when they are the same.
fn all_sides(value: &Value) -> f64 {
    let sides = numbers(value);
    assert!(sides.iter().all(|side| *side == sides[0]), "{sides:?}");
    sides[0]
}

#[test]
fn date_menu() {
    let reference = reference("date-menu");
    let tree = &reference["tree"];
    let em = em(tree);
    let style = &tree["style"];

    // The menu itself: blur-my-shell's tint, upstream's shape.
    assert_eq!(numbers(&style["bg"]), colour("bms-popup-tint"));
    assert_eq!(numbers(&style["fg"]), colour("bms-popup-fg-color"));
    assert_eq!(numbers(&style["radius"])[0], pixels("datemenu-radius", em));
    let area = by_name(tree, "calendarArea");
    assert_eq!(
        all_sides(&area["style"]["padding"]),
        pixels("calendar-area-padding", em)
    );

    // The calendar column.
    let column = by_class(tree, "datemenu-calendar-column");
    assert_eq!(
        column["style"]["spacing"].as_f64().unwrap(),
        pixels("datemenu-column-spacing", em)
    );
    assert_eq!(
        numbers(&column["style"]["margin"])[3],
        pixels("datemenu-column-margin-left", em)
    );
    let today = by_class(tree, "datemenu-today-button");
    assert_eq!(
        all_sides(&today["style"]["padding"]),
        pixels("today-button-padding", em)
    );
    assert_eq!(
        numbers(&today["style"]["radius"])[0],
        pixels("card-radius", em)
    );
    assert_eq!(
        all_sides(&today["style"]["margin"]),
        pixels("card-margin", em)
    );
    assert_colour(
        &by_class(tree, "day-label")["style"]["fg"],
        "bms-dim-fg-color",
    );

    // The calendar.
    let pager = by_class(tree, "pager-button");
    assert_eq!(
        pager["style"]["width"].as_f64().unwrap(),
        minimum_pixels("calendar-pager-size", em)
    );
    let month = by_class(tree, "calendar-month-label");
    assert_eq!(
        month["style"]["width"].as_f64().unwrap(),
        minimum_pixels("calendar-month-label-width", em)
    );
    let day = by_class(tree, "calendar-day");
    assert_eq!(
        day["style"]["width"].as_f64().unwrap(),
        pixels("calendar-day-size", em)
    );
    assert_eq!(
        all_sides(&day["style"]["margin"]),
        pixels("calendar-day-margin", em)
    );
    let heading = by_class(tree, "calendar-day-heading");
    assert_eq!(
        all_sides(&heading["style"]["margin"]),
        pixels("calendar-heading-margin", em)
    );
    let padding = numbers(&heading["style"]["padding"]);
    assert_eq!(
        vec![padding[0], padding[1]],
        lengths("calendar-heading-padding", em)
    );
    assert_colour(&heading["style"]["fg"], "bms-dim-fg-color");

    // The cards under it.
    let events = by_class(tree, "events-button");
    assert_eq!(
        all_sides(&events["style"]["padding"]),
        pixels("card-padding", em)
    );
    assert_eq!(
        numbers(&events["style"]["radius"])[0],
        pixels("card-radius", em)
    );
    assert_eq!(
        all_sides(&events["style"]["margin"]),
        pixels("card-margin", em)
    );
    assert_colour(&events["style"]["bg"], "bms-card-color");
    assert_colour(&events["style"]["border_color"][0], "bms-card-border-color");
}

#[test]
fn message_list() {
    let reference = reference("date-menu");
    let tree = &reference["tree"];
    let em = em(tree);

    let list = by_class(tree, "message-list");
    assert_eq!(
        list["style"]["width"].as_f64().unwrap(),
        minimum_pixels("message-list-width", em)
    );
    assert_colour(
        &list["style"]["border_color"][1],
        "message-list-border-color",
    );
    let view = by_class(tree, "message-view");
    assert_eq!(
        numbers(&view["style"]["margin"])[1],
        pixels("message-view-margin-right", em)
    );
    let controls = by_class(tree, "message-list-controls");
    assert_eq!(
        numbers(&controls["style"]["padding"]),
        lengths("message-list-controls-padding", em)
    );

    // A message as the list shows it: a sheet of white on the menu.
    let message = by_class(tree, "message");
    let style = &message["style"];
    assert_colour(&style["bg"], "bms-card-color");
    assert_colour(&style["border_color"][0], "bms-card-border-color");
    assert_eq!(
        numbers(&style["radius"])[0],
        pixels("message-list-radius", em)
    );
    assert_eq!(all_sides(&style["padding"]), pixels("message-padding", em));
    assert_eq!(
        numbers(&style["margin"])[2],
        pixels("message-list-margin-bottom", em)
    );

    // The cards under the first of a group.
    let stacked = |pseudo: &str| {
        find(tree, &|node| {
            node["classes"] == "message"
                && node["pseudo"]
                    .as_str()
                    .is_some_and(|classes| classes.split(' ').any(|class| class == pseudo))
        })
        .unwrap_or_else(|| panic!("no message that is {pseudo}"))
    };
    assert_colour(
        &stacked("second-in-stack")["style"]["bg"],
        "bms-second-in-stack-color",
    );
    assert_colour(
        &stacked("lower-in-stack")["style"]["bg"],
        "bms-lower-in-stack-color",
    );

    // Its parts.
    let header = by_class(message, "message-header");
    assert_eq!(
        header["style"]["spacing"].as_f64().unwrap(),
        pixels("message-header-spacing", em)
    );
    assert_colour(&header["style"]["fg"], "bms-dim-fg-color");
    let content = by_class(message, "message-header-content");
    assert_eq!(
        content["style"]["min_height"].as_f64().unwrap(),
        minimum_pixels("message-header-content-height", em)
    );
    let close = by_class(message, "message-close-button");
    assert_eq!(
        all_sides(&close["style"]["padding"]),
        pixels("message-button-padding", em)
    );
    assert_eq!(
        all_sides(&close["style"]["margin"]),
        pixels("message-close-button-margin", em)
    );
    assert_colour(&close["style"]["bg"], "bms-popup-button-color");
    let message_box = by_class(message, "message-box");
    assert_eq!(
        all_sides(&message_box["style"]["padding"]),
        pixels("message-box-padding", em)
    );
    let text = by_class(message, "message-content");
    assert_eq!(
        text["style"]["spacing"].as_f64().unwrap(),
        pixels("message-content-spacing", em)
    );
}

#[test]
fn notification_banner() {
    let reference = reference("notification-banner");
    let banner = &reference["tree"];
    let em = em(banner);
    let style = &banner["style"];

    // A banner is a blurred surface like a menu, not a card on one.
    assert_eq!(numbers(&style["bg"]), colour("bms-popup-tint"));
    assert_eq!(numbers(&style["fg"]), colour("bms-popup-fg-color"));
    assert_eq!(
        numbers(&style["border_color"][0]),
        colour("bms-popup-border-color")
    );
    assert_eq!(
        numbers(&style["radius"])[0],
        pixels("bms-banner-radius", em)
    );
    assert_eq!(
        all_sides(&style["margin"]),
        pixels("notification-banner-margin", em)
    );
    assert_eq!(all_sides(&style["padding"]), pixels("message-padding", em));
    assert_eq!(
        style["min_width"].as_f64().unwrap(),
        minimum_pixels("notification-banner-width", em)
    );
    assert_eq!(
        style["min_height"].as_f64().unwrap(),
        pixels("notification-banner-height", em)
    );
}
