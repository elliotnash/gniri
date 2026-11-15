// SPDX-License-Identifier: GPL-3.0-or-later

//! Common setup for layer-shell windows.

use std::sync::atomic::{AtomicBool, Ordering};

use gtk::prelude::*;
use gtk::{cairo, gdk};
use gtk4_layer_shell::{Edge, Layer, LayerShell};

/// Set by `--no-pointer`: no surface takes pointer input.
static POINTER_IGNORED: AtomicBool = AtomicBool::new(false);

/// Makes every surface ignore the pointer from now on.
///
/// A screenshot of a nested session must not depend on where the pointer of
/// the session around it happens to be: a widget under it would be drawn
/// hovered.
pub fn ignore_pointer() {
    POINTER_IGNORED.store(true, Ordering::Relaxed);
}

pub fn pointer_ignored() -> bool {
    POINTER_IGNORED.load(Ordering::Relaxed)
}

/// Gives `window`'s surface an empty input region, now and whenever GTK
/// lays the surface out again.
pub fn take_no_input(window: &gtk::ApplicationWindow) {
    window.connect_map(|window| {
        let Some(surface) = window.surface() else {
            return;
        };
        surface.set_input_region(Some(&cairo::Region::create()));
        surface.connect_layout(|surface, _, _| {
            surface.set_input_region(Some(&cairo::Region::create()));
        });
    });
}

/// Which screen edges a surface is attached to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    /// Top edge, full width.
    TopBar,
    /// All four edges: covers the output.
    Fill,
}

/// How a surface interacts with the space reserved by other surfaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    /// Reserve the surface's own size, pushing windows away (the panel).
    Reserve,
    /// Ignore reserved space and extend under it (wallpaper, stage).
    Ignore,
    /// Reserve nothing, and stay out of what others reserve: the surface
    /// starts under the panel (the overview).
    Respect,
}

/// Creates an unmapped, transparent layer-shell window on `monitor`.
///
/// `name` becomes the namespace `gniri-<name>`, which niri layer rules in
/// `data/niri/gniri.kdl` match on.
pub fn window(
    app: &adw::Application,
    monitor: &gdk::Monitor,
    name: &str,
    layer: Layer,
    anchor: Anchor,
    zone: Zone,
) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .css_classes(["gniri-layer", &format!("gniri-{name}-window")])
        .build();

    window.init_layer_shell();
    window.set_namespace(Some(&format!("gniri-{name}")));
    window.set_layer(layer);
    window.set_monitor(Some(monitor));

    let edges: &[Edge] = match anchor {
        Anchor::TopBar => &[Edge::Top, Edge::Left, Edge::Right],
        Anchor::Fill => &[Edge::Top, Edge::Bottom, Edge::Left, Edge::Right],
    };
    for edge in edges {
        window.set_anchor(*edge, true);
    }

    match zone {
        Zone::Reserve => window.auto_exclusive_zone_enable(),
        Zone::Ignore => window.set_exclusive_zone(-1),
        Zone::Respect => window.set_exclusive_zone(0),
    }
    if pointer_ignored() {
        take_no_input(&window);
    }
    window
}
