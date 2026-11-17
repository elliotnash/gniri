// SPDX-License-Identifier: GPL-3.0-or-later

//! The on-screen display for volume and brightness keys. Port of
//! `js/ui/osdWindow.js`.
//!
//! A small overlay surface at the bottom centre of each output, mapped only
//! while it shows something.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{cairo, gdk, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::widgets::level::{Colours, LevelBar, token_colour};

/// osdWindow.js `HIDE_TIMEOUT`.
const HIDE_TIMEOUT: Duration = Duration::from_millis(1500);
/// osdWindow.js `FADE_TIME`. The theme's transition on `.osd-window` lasts
/// as long.
const FADE_TIME: Duration = Duration::from_millis(100);
/// `.osd-window .level { -barlevel-height }`
const LEVEL_HEIGHT: f64 = 6.0;
/// `.osd-window .level { -barlevel-overdrive-separator-width }`
const SEPARATOR_WIDTH: f64 = 3.0;

/// What one press of a key shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Content {
    pub icon: String,
    pub label: Option<String>,
    /// The bar's value and its right end (above 1.0 with over-amplification).
    pub level: Option<(f64, f64)>,
}

struct Inner {
    window: gtk::ApplicationWindow,
    bubble: gtk::Box,
    icon: gtk::Image,
    label: gtk::Label,
    level: LevelBar,
    hide_timer: RefCell<Option<glib::SourceId>>,
    /// Counts shows, so a fade-out overtaken by a new show does not unmap.
    generation: Cell<u64>,
}

#[derive(Clone)]
pub struct Osd {
    pub window: gtk::ApplicationWindow,
    inner: Rc<Inner>,
}

pub fn build(app: &adw::Application, monitor: &gdk::Monitor) -> Osd {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .css_classes(["gniri-layer", "gniri-osd-window"])
        .build();
    window.init_layer_shell();
    window.set_namespace(Some("gniri-osd"));
    window.set_layer(Layer::Overlay);
    window.set_monitor(Some(monitor));
    window.set_anchor(Edge::Bottom, true);
    // Above windows that reach the screen edge, not pushed up by them.
    window.set_exclusive_zone(-1);
    window.set_keyboard_mode(KeyboardMode::None);

    let icon = gtk::Image::new();
    let label = gtk::Label::builder().visible(false).build();
    let level = LevelBar::new(LEVEL_HEIGHT, SEPARATOR_WIDTH, || Colours {
        trough: token_colour("osd-level-trough-color"),
        active: token_colour("osd-level-active-color"),
        overdrive: token_colour("destructive-color"),
    });
    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .valign(gtk::Align::Center)
        .build();
    column.append(&label);
    column.append(&level.widget);

    let bubble = gtk::Box::builder().css_classes(["osd-window"]).build();
    bubble.append(&icon);
    bubble.append(&column);
    window.set_child(Some(&bubble));

    // The display takes no input: clicks go to what is under it.
    window.connect_map(|window| {
        if let Some(surface) = window.surface() {
            surface.set_input_region(Some(&cairo::Region::create()));
        }
    });

    Osd {
        window: window.clone(),
        inner: Rc::new(Inner {
            window,
            bubble,
            icon,
            label,
            level,
            hide_timer: RefCell::new(None),
            generation: Cell::new(0),
        }),
    }
}

impl Osd {
    /// Shows `content` and (re)starts the timer that hides it.
    /// `OsdWindow.show`.
    pub fn show(&self, content: &Content) {
        let inner = &self.inner;
        let generation = inner.generation.get() + 1;
        inner.generation.set(generation);

        inner.icon.set_icon_name(Some(&content.icon));
        inner.label.set_visible(content.label.is_some());
        inner
            .label
            .set_label(content.label.as_deref().unwrap_or_default());
        inner.level.widget.set_visible(content.level.is_some());
        if let Some((value, maximum)) = content.level {
            if inner.window.is_visible() {
                inner.level.glide(value, maximum);
            } else {
                inner.level.set(value, maximum);
            }
        }

        if !inner.window.is_visible() {
            inner.window.present();
            glib::idle_add_local_once({
                let inner = Rc::clone(inner);
                move || {
                    if inner.generation.get() == generation {
                        inner.bubble.add_css_class("open");
                    }
                }
            });
        } else {
            inner.bubble.add_css_class("open");
        }

        if let Some(timer) = inner.hide_timer.take() {
            timer.remove();
        }
        let timer = glib::timeout_add_local_once(HIDE_TIMEOUT, {
            let inner = Rc::clone(inner);
            move || {
                inner.hide_timer.take();
                inner.bubble.remove_css_class("open");
                let unmap = {
                    let inner = Rc::clone(&inner);
                    move || {
                        if inner.generation.get() == generation {
                            inner.window.set_visible(false);
                        }
                    }
                };
                let animated = gtk::Settings::default()
                    .is_some_and(|settings| settings.is_gtk_enable_animations());
                if animated {
                    glib::timeout_add_local_once(FADE_TIME, unmap);
                } else {
                    unmap();
                }
            }
        });
        inner.hide_timer.replace(Some(timer));
    }
}
