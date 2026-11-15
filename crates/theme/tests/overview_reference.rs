// SPDX-License-Identifier: GPL-3.0-or-later

//! Tier 0 of the visual checks for the overview: theme tokens against the
//! values GNOME Shell 51.1 (with blur-my-shell) computed for its dash,
//! search entry, app grid and search results, as captured in
//! `tests/visual/reference/gnome-51.1/metrics/`.
//!
//! Needs no display. Whether GTK lays the widgets out the same way is tier
//! 1 (`cargo xtask diff dash`, `search-entry`, `page-indicators`,
//! `page-arrow`, `search-apps`, `search-list`).

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
                    .unwrap_or(part)
                    .parse()
                    .unwrap_or_else(|_| panic!("token {name} is not lengths: {value}")),
            };
            pixels.floor()
        })
        .collect()
}

/// A colour of the reference against a token. An alpha such as 0.07 is
/// 17.85 of 255: St keeps 17 where the token rounds up, so the two may be
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

fn number(value: &Value) -> f64 {
    value
        .as_f64()
        .unwrap_or_else(|| panic!("expected a number, got {value}"))
}

/// A token that is a time, in milliseconds.
fn milliseconds(name: &str) -> f64 {
    let value = gniri_theme::token(name).unwrap_or_else(|| panic!("no token {name}"));
    value
        .strip_suffix("ms")
        .and_then(|number| number.parse().ok())
        .unwrap_or_else(|| panic!("token {name} is not a time: {value}"))
}

#[test]
fn dash() {
    let reference = reference("dash");
    let dash = by_name(&reference["tree"], "dash");
    let em = em(dash);
    assert_eq!(
        top_right(&dash["style"]["padding"]),
        lengths("dash-padding", em)
    );

    // blur-my-shell's tint and border on the stock shape.
    let background = by_class(dash, "dash-background");
    let style = &background["style"];
    assert_colour(&style["bg"], "bms-dash-tint");
    assert_colour(&style["border_color"][0], "bms-dash-border");
    assert_eq!(
        numbers(&style["radius"])[0],
        pixels("dash-background-radius", em)
    );
    assert_eq!(
        top_right(&style["padding"]),
        lengths("dash-background-padding", em)
    );
    assert_eq!(numbers(&style["margin"])[2], pixels("dash-edge-offset", em));

    // An item: a button that reaches the screen edge, around the icon's
    // box.
    let button = by_class(dash, "overview-tile");
    assert_eq!(
        numbers(&button["style"]["padding"])[2],
        pixels("dash-edge-offset", em)
    );
    assert_eq!(
        numbers(&button["style"]["margin"])[1],
        pixels("dash-item-margin", em)
    );
    let icon = by_class(button, "overview-icon");
    assert_eq!(
        numbers(&icon["style"]["radius"])[0],
        pixels("tile-radius", em)
    );
    assert_eq!(
        numbers(&icon["style"]["padding"])[0],
        pixels("tile-padding", em)
    );
    assert_eq!(
        number(&icon["style"]["transition_ms"]),
        milliseconds("tile-transition")
    );

    let separator = by_class(dash, "dash-separator");
    assert_colour(&separator["style"]["bg"], "bms-dash-separator");
    let margin = numbers(&separator["style"]["margin"]);
    assert_eq!(margin[1], pixels("dash-separator-margin", em));
    assert_eq!(margin[2], pixels("dash-edge-offset", em));

    let dot = by_class(dash, "app-grid-running-dot");
    assert_colour(&dot["style"]["bg"], "bms-dash-fg");
    assert_eq!(
        number(&dot["style"]["width"]),
        pixels("running-dot-size", em)
    );
    // `offset-y: -$dash_padding` is no style St reports; it shows in the
    // boxes: the dot ends this far above the button's inside.
    let inside = &button["children"][0];
    let (inside, dot) = (numbers(&inside["box"]), numbers(&dot["box"]));
    assert_eq!(
        (inside[1] + inside[3]) - (dot[1] + dot[3]),
        top_right(&style["padding"])[0]
    );
}

#[test]
fn search_entry() {
    let reference = reference("search-entry");
    let entry = by_class(&reference["tree"], "search-entry");
    let em = em(entry);
    let style = &entry["style"];
    assert_colour(&style["bg"], "bms-overview-fill");
    assert_eq!(top_right(&style["padding"]), lengths("entry-padding", em));
    assert_eq!(
        numbers(&style["margin"]),
        lengths("search-entry-margin", em)
    );
    assert_eq!(
        number(&style["width"]),
        length("search-entry-width", em).round()
    );

    let hint = by_class(entry, "hint-text");
    assert_colour(&hint["style"]["fg"], "bms-overview-dim-fg");
    assert_eq!(
        numbers(&hint["style"]["margin"])[3],
        pixels("entry-hint-margin", em)
    );
    let icon = by_class(entry, "search-entry-icon");
    assert_colour(&icon["style"]["fg"], "bms-overview-dim-fg");
    assert_eq!(
        top_right(&icon["style"]["padding"]),
        lengths("search-entry-icon-padding", em)
    );
}

#[test]
fn app_grid() {
    let reference = reference("app-grid");
    let tree = &reference["tree"];

    let tile = by_class(tree, "overview-tile");
    let em = em(tile);
    let style = &tile["style"];
    assert_eq!(
        numbers(&style["radius"])[0],
        pixels("overview-tile-radius", em)
    );
    assert_eq!(
        numbers(&style["padding"])[0],
        pixels("overview-tile-padding", em)
    );
    assert_eq!(
        number(&style["spacing"]),
        pixels("overview-tile-spacing", em)
    );
    assert_eq!(
        number(&style["transition_ms"]),
        milliseconds("tile-transition")
    );

    // The dot of an application that runs hangs below its icon's box.
    let running = find(tree, &|node| {
        node["children"].as_array().is_some_and(|children| {
            children
                .iter()
                .any(|child| child["classes"] == "app-grid-running-dot")
        })
    })
    .expect("an application that runs");
    let dot = by_class(running, "app-grid-running-dot");
    assert_colour(&dot["style"]["bg"], "system-fg-color");
    let (inside, dot) = (numbers(&running["box"]), numbers(&dot["box"]));
    assert_eq!(
        (dot[1] + dot[3]) - (inside[1] + inside[3]),
        pixels("running-dot-offset", em)
    );

    let indicator = by_class(tree, "page-indicator");
    assert_eq!(
        numbers(&indicator["style"]["padding"]),
        lengths("page-indicator-padding", em)
    );
    let icon = by_class(indicator, "page-indicator-icon");
    assert_colour(&icon["style"]["bg"], "system-fg-color");
    assert_eq!(
        number(&icon["style"]["width"]),
        pixels("page-indicator-size", em)
    );

    let arrow = by_class(tree, "page-navigation-arrow");
    let style = &arrow["style"];
    assert_colour(&style["bg"], "bms-overview-fill");
    assert_colour(&style["border_color"][0], "bms-overview-border");
    assert_eq!(
        numbers(&style["margin"])[0],
        pixels("page-arrow-margin", em)
    );
    assert_eq!(
        numbers(&style["padding"])[0],
        pixels("page-arrow-padding", em)
    );
    assert_eq!(number(&style["width"]), pixels("page-arrow-size", em));
}

#[test]
fn search_results() {
    let apps = reference("search-apps");
    let grid = by_class(&apps["tree"], "grid-search-results");
    let em = em(grid);
    assert_eq!(
        number(&grid["style"]["spacing"]),
        pixels("grid-search-spacing", em)
    );
    // A result is the app grid's tile.
    let tile = by_class(grid, "overview-tile");
    assert_eq!(
        numbers(&tile["style"]["padding"])[0],
        pixels("overview-tile-padding", em)
    );

    let reference = reference("search-list");
    let content = by_class(&reference["tree"], "search-section-content");
    let style = &content["style"];
    assert_colour(&style["bg"], "bms-overview-fill");
    assert_colour(&style["border_color"][0], "bms-overview-border");
    assert_eq!(
        numbers(&style["radius"])[0],
        pixels("search-section-radius", em)
    );
    assert_eq!(
        numbers(&style["padding"])[0],
        pixels("search-section-padding", em)
    );
    assert_eq!(
        top_right(&style["margin"]),
        lengths("search-section-margin", em)
    );

    let provider = by_class(content, "search-provider-icon");
    let style = &provider["style"];
    assert_eq!(
        numbers(&style["radius"])[0],
        pixels("search-item-radius", em)
    );
    assert_eq!(numbers(&style["padding"])[0], pixels("tile-padding", em));
    assert_eq!(
        numbers(&style["margin"]),
        lengths("search-provider-margin", em)
    );
    assert_eq!(
        number(&by_class(provider, "list-search-provider-content")["style"]["spacing"]),
        pixels("search-provider-spacing", em)
    );
    assert_eq!(
        number(&by_class(provider, "list-search-provider-details")["style"]["width"]),
        pixels("search-provider-details-width", em)
    );

    let results = by_class(content, "list-search-results");
    assert_eq!(
        number(&results["style"]["spacing"]),
        pixels("search-results-spacing", em)
    );
    let result = by_class(results, "list-search-result");
    assert_eq!(
        numbers(&result["style"]["radius"])[0],
        pixels("search-item-radius", em)
    );
    assert_eq!(
        numbers(&result["style"]["padding"])[0],
        pixels("tile-padding", em)
    );
    assert_eq!(
        number(&by_class(result, "list-search-result-content")["style"]["spacing"]),
        pixels("search-results-spacing", em)
    );
    assert_eq!(
        number(&by_class(result, "list-search-result-title")["style"]["spacing"]),
        pixels("search-result-title-spacing", em)
    );
    assert_colour(
        &by_class(result, "list-search-result-description")["style"]["fg"],
        "bms-overview-description-fg",
    );
}
