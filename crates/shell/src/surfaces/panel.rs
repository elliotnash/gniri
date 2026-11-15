// SPDX-License-Identifier: GPL-3.0-or-later

//! The top panel's surface.

use std::rc::Rc;

use adw::prelude::*;
use gtk::gdk;
use gtk4_layer_shell::Layer;

use super::layer::{self, Anchor, Zone};
use crate::panel::{self, Bar};
use crate::services::Services;

/// A mapped panel. Dropping it stops its updates; `window` must still be
/// destroyed by whoever owns it.
pub struct Panel {
    pub window: gtk::ApplicationWindow,
    _bar: Bar,
}

/// Builds and maps the panel on `monitor`.
pub fn build(app: &adw::Application, monitor: &gdk::Monitor, services: &Rc<Services>) -> Panel {
    let window = layer::window(
        app,
        monitor,
        "panel",
        Layer::Top,
        Anchor::TopBar,
        Zone::Reserve,
    );
    let output = monitor.connector();
    let bar = panel::build(services, output.as_deref());
    window.set_child(Some(&bar.widget));
    window.present();
    Panel { window, _bar: bar }
}
