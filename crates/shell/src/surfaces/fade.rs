// SPDX-License-Identifier: GPL-3.0-or-later

//! The screens fading to black as the session goes idle: GNOME's "long
//! lightbox" (screenShield.js), ten seconds, after which they turn off.
//! Any input before then brings everything back.
//!
//! One black surface per output, over everything, that takes no input:
//! what is pressed or moved reaches what is under it, and the compositor
//! says the session is no longer idle.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gdk;
use gtk4_layer_shell::Layer;

use super::layer::{self, Anchor, Zone};
use crate::widgets::motion::Eased;

pub struct Fade {
    app: adw::Application,
    /// The surfaces, with their opacity, while the screens fade.
    shown: RefCell<Vec<(gtk::ApplicationWindow, Eased)>>,
}

impl Fade {
    pub fn new(app: &adw::Application) -> Rc<Self> {
        Rc::new(Self {
            app: app.clone(),
            shown: RefCell::default(),
        })
    }

    /// `Lightbox.lightOn(STANDARD_FADE_TIME)`: fades every output to black;
    /// `done` runs once they are, unless [`Fade::stop`] came first.
    pub fn start(&self, done: impl FnOnce() + 'static) {
        self.stop();
        let monitors = gdk::Display::default().map(|display| display.monitors());
        let mut done = Some(done);
        let mut shown = self.shown.borrow_mut();
        for monitor in monitors
            .iter()
            .flat_map(|monitors| monitors.iter::<gdk::Monitor>())
            .filter_map(Result::ok)
        {
            let window = layer::window(
                &self.app,
                &monitor,
                "fade",
                Layer::Overlay,
                Anchor::Fill,
                Zone::Ignore,
            );
            layer::take_no_input(&window);
            let black = gtk::Box::builder().css_classes(["gniri-fade"]).build();
            window.set_child(Some(&black));
            black.set_opacity(0.0);
            window.present();
            let opacity = Eased::new(&black, 0.0, {
                let black = black.downgrade();
                move |opacity| {
                    if let Some(black) = black.upgrade() {
                        black.set_opacity(opacity);
                    }
                }
            });
            let finished = done.take();
            opacity.ease(
                1.0,
                gniri_services::idle::FADE_TIME.as_millis() as u32,
                adw::Easing::EaseOutQuad,
                move || {
                    if let Some(finished) = finished {
                        finished();
                    }
                },
            );
            shown.push((window, opacity));
        }
        // No output to fade.
        if let Some(done) = done {
            done();
        }
    }

    /// `lightOff`: takes the black away at once.
    pub fn stop(&self) {
        for (window, _) in self.shown.take() {
            window.destroy();
        }
    }
}
