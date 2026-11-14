//! The top panel.

use adw::prelude::*;
use gtk4_layer_shell::Layer;

use super::layer::{self, Anchor, Zone};

/// Builds and maps the panel. M0: the bar only.
pub fn build(app: &adw::Application) -> gtk::ApplicationWindow {
    let window = layer::window(app, "panel", Layer::Top, Anchor::TopBar, Zone::Reserve);
    let bar = gtk::CenterBox::builder()
        .css_classes(["gniri-panel"])
        .build();
    window.set_child(Some(&bar));
    window.present();
    window
}
