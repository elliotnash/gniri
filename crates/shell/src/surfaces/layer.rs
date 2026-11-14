//! Common setup for layer-shell windows.

use gtk::gdk;
use gtk4_layer_shell::{Edge, Layer, LayerShell};

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
    }
    window
}
