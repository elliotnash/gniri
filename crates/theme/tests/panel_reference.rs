//! Tier 0 of the visual checks for the panel: theme tokens against the
//! values GNOME Shell 51.1 computed for its own panel, as captured in
//! `tests/visual/reference/gnome-51.1/metrics/`.
//!
//! Needs no display. It checks the theme's arithmetic; whether GTK lays the
//! widgets out the same way is tier 1 (`cargo xtask diff panel`).

mod common;

use common::*;

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
