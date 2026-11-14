//! Tier 0 of the visual checks for the panel: theme tokens against the
//! values GNOME Shell 51.1 computed for its own panel, as captured in
//! `tests/visual/reference/gnome-51.1/metrics/`.
//!
//! Needs no display. It checks the theme's arithmetic; whether GTK lays the
//! widgets out the same way is tier 1 (`cargo xtask diff panel`).

use gniri_theme::token;
use serde_json::Value;

fn reference(name: &str) -> Value {
    let path = format!(
        "{}/../../tests/visual/reference/gnome-51.1/metrics/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"));
    serde_json::from_str(&text).expect("valid JSON")
}

/// The first actor, depth first, for which `matches` holds.
fn find<'a>(node: &'a Value, matches: &dyn Fn(&Value) -> bool) -> Option<&'a Value> {
    if matches(node) {
        return Some(node);
    }
    node["children"]
        .as_array()?
        .iter()
        .find_map(|child| find(child, matches))
}

fn by_name<'a>(tree: &'a Value, name: &str) -> &'a Value {
    find(tree, &|node| node["name"] == name).unwrap_or_else(|| panic!("no actor named {name}"))
}

fn by_class<'a>(tree: &'a Value, class: &str) -> &'a Value {
    find(tree, &|node| {
        node["classes"]
            .as_str()
            .is_some_and(|classes| classes.split(' ').any(|other| other == class))
    })
    .unwrap_or_else(|| panic!("no actor with class {class}"))
}

fn by_type<'a>(tree: &'a Value, type_name: &str) -> &'a Value {
    find(tree, &|node| node["type"] == type_name)
        .unwrap_or_else(|| panic!("no actor of type {type_name}"))
}

fn numbers(value: &Value) -> Vec<f64> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("expected an array, got {value}"))
        .iter()
        .map(|number| number.as_f64().expect("a number"))
        .collect()
}

/// A token colour as `[r, g, b, a]` with every channel 0 to 255, the form
/// the reference dump uses.
fn colour(name: &str) -> Vec<f64> {
    let value = token(name).unwrap_or_else(|| panic!("no token {name}"));
    if let Some(hex) = value.strip_prefix('#') {
        let channel = |index: usize| {
            f64::from(u8::from_str_radix(&hex[index..index + 2], 16).expect("hex digits"))
        };
        return vec![channel(0), channel(2), channel(4), 255.0];
    }
    let inner = value
        .strip_prefix("rgba(")
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or_else(|| panic!("token {name} is not a colour: {value}"));
    let parts: Vec<f64> = inner
        .split(',')
        .map(|part| part.trim().parse().expect("a number"))
        .collect();
    vec![parts[0], parts[1], parts[2], (parts[3] * 255.0).round()]
}

/// The size of the font GNOME used, in pixels: what 1em is.
fn em(tree: &Value) -> f64 {
    let font = tree["style"]["font"].as_str().expect("a font description");
    font.rsplit(' ')
        .next()
        .and_then(|size| size.strip_suffix("px"))
        .and_then(|size| size.parse().ok())
        .unwrap_or_else(|| panic!("no pixel size in {font:?}"))
}

fn length(name: &str, em: f64) -> f64 {
    let value = token(name).unwrap_or_else(|| panic!("no token {name}"));
    if let Some(ems) = value.strip_suffix("em") {
        ems.parse::<f64>().expect("a number") * em
    } else if let Some(pixels) = value.strip_suffix("px") {
        pixels.parse::<f64>().expect("a number")
    } else {
        panic!("token {name} is not a length: {value}")
    }
}

/// A token length in whole pixels as GTK lays it out when it is a padding,
/// margin, border or icon size: rounded down.
fn pixels(name: &str, em: f64) -> f64 {
    length(name, em).floor()
}

/// The same for a minimum size, which GTK rounds up.
fn minimum_pixels(name: &str, em: f64) -> f64 {
    length(name, em).ceil()
}

#[test]
fn panel_on_the_desktop() {
    let reference = reference("panel");
    let panel = by_name(&reference["tree"], "panel");
    let em = em(panel);
    let style = &panel["style"];

    // blur-my-shell's tint, before it is folded for niri's undimmed blur.
    assert_eq!(numbers(&style["bg"]), colour("bms-panel-tint"));
    assert_eq!(numbers(&style["fg"]), colour("panel-fg-color"));
    assert_eq!(
        style["height"].as_f64(),
        Some(minimum_pixels("panel-height", em))
    );
    assert_eq!(
        style["transition_ms"]
            .as_f64()
            .map(|ms| format!("{ms}ms"))
            .as_deref(),
        token("bms-panel-transition-duration")
    );

    let activities = by_name(panel, "panelActivities");
    let border = pixels("panel-button-border", em);
    assert_eq!(numbers(&activities["style"]["border_width"]), [border; 4]);
    // The button's content starts one natural padding in from its edge.
    let indicators = by_type(activities, "Gjs_ui_panel_WorkspaceIndicators");
    assert_eq!(
        numbers(&indicators["box"])[0] - numbers(&activities["box"])[0],
        pixels("panel-button-hpadding", em)
    );
    let padding = pixels("workspace-indicators-padding", em);
    assert_eq!(
        numbers(&indicators["style"]["padding"]),
        [0.0, padding, 0.0, padding]
    );
    assert_eq!(
        indicators["style"]["spacing"].as_f64(),
        Some(pixels("workspace-indicators-spacing", em))
    );

    let dot = by_class(activities, "workspace-dot");
    assert_eq!(
        dot["style"]["min_width"].as_f64(),
        Some(pixels("workspace-dot-size", em))
    );
    assert_eq!(numbers(&dot["style"]["bg"]), colour("panel-fg-color"));

    let clock = by_class(panel, "clock");
    let padding = pixels("panel-clock-padding", em);
    assert_eq!(
        numbers(&clock["style"]["padding"]),
        [0.0, padding, 0.0, padding]
    );
    assert_eq!(numbers(&clock["style"]["border_width"]), [border; 4]);

    let icon = by_class(panel, "system-status-icon");
    let margin = pixels("panel-status-icon-margin", em);
    assert_eq!(
        numbers(&icon["style"]["margin"]),
        [0.0, margin, 0.0, margin]
    );
    // The icon's texture is its first child.
    let texture = numbers(&icon["children"][0]["box"]);
    let size = pixels("panel-status-icon-size", em);
    assert_eq!(texture[2..], [size, size]);

    let quick_settings = by_type(panel, "Gjs_ui_panel_QuickSettings");
    let status_box = by_class(quick_settings, "panel-status-indicators-box");
    assert_eq!(
        status_box["style"]["spacing"].as_f64(),
        Some(pixels("panel-status-spacing", em))
    );
}

#[test]
fn panel_in_the_overview() {
    let reference = reference("panel-overview");
    let panel = by_name(&reference["tree"], "panel");

    // Transparent: the dump has no background colour at all.
    assert!(panel["style"]["bg"].is_null());

    let activities = by_name(panel, "panelActivities");
    assert_eq!(
        numbers(&activities["style"]["fg"]),
        colour("panel-overview-fg-color")
    );
    // The activities button is checked while the overview is open; St draws
    // the fill as an inset shadow.
    assert_eq!(
        numbers(&activities["style"]["box_shadow"]["color"]),
        colour("panel-overview-button-active-fill")
    );
    let dot = by_class(activities, "workspace-dot");
    assert_eq!(
        numbers(&dot["style"]["bg"]),
        colour("panel-overview-fg-color")
    );
}
