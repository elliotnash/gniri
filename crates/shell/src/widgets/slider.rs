//! A slider: a level bar with a round handle that can be dragged. Port of
//! `js/ui/slider.js`.

use std::cell::{Cell, RefCell};
use std::f64::consts::TAU;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};

use super::level::{self, Colours, Level, token_colour};

/// slider.js `SLIDER_SCROLL_STEP`.
const SCROLL_STEP: f64 = 0.02;
/// What the left and right arrow keys move by.
const KEY_STEP: f64 = 0.1;
/// `.slider { -barlevel-height }`
const BAR_HEIGHT: f64 = 4.0;

struct Inner {
    level: Cell<Level>,
    dragging: Cell<bool>,
    changed: RefCell<Option<Box<dyn Fn(f64)>>>,
}

#[derive(Clone)]
pub struct Slider {
    pub widget: gtk::DrawingArea,
    inner: Rc<Inner>,
}

impl Slider {
    pub fn new() -> Self {
        let inner = Rc::new(Inner {
            level: Cell::new(Level {
                value: 0.0,
                maximum: 1.0,
                overdrive_start: 1.0,
                bar_height: BAR_HEIGHT,
                separator_width: 1.0,
            }),
            dragging: Cell::new(false),
            changed: RefCell::new(None),
        });
        let widget = gtk::DrawingArea::builder()
            .css_classes(["slider"])
            .hexpand(true)
            .focusable(true)
            .build();
        let slider = Self { widget, inner };
        slider.set_up_drawing();
        slider.set_up_input();
        slider
    }

    fn set_up_drawing(&self) {
        let trough = token_colour("slider-trough-color");
        let overdrive = token_colour("destructive-color");
        self.widget.set_draw_func({
            let inner = Rc::clone(&self.inner);
            move |area, cr, width, height| {
                let (width, height) = (f64::from(width), f64::from(height));
                let level = inner.level.get();
                let colours = Colours {
                    trough,
                    // `-st-accent-color`, which follows the system setting.
                    active: adw::StyleManager::default().accent_color_rgba(),
                    overdrive,
                };
                // The handle is drawn in the widget's CSS `color`, which is
                // how the theme brightens it on hover.
                let foreground = area.color();
                level::draw(cr, width, height, &level, &colours, &foreground);

                // `Slider.vfunc_repaint`: the handle, `-slider-handle-radius`
                // (half the slider's height) in radius.
                let radius = height / 2.0;
                let x = radius + (width - 2.0 * radius) * (level.value / level.maximum);
                cr.set_source_rgba(
                    f64::from(foreground.red()),
                    f64::from(foreground.green()),
                    f64::from(foreground.blue()),
                    f64::from(foreground.alpha()),
                );
                cr.arc(x, height / 2.0, radius, 0.0, TAU);
                let _ = cr.fill();
            }
        });
    }

    fn set_up_input(&self) {
        // `_moveHandle`: the handle's centre follows the pointer.
        let move_to = {
            let slider = self.clone();
            move |x: f64| {
                let width = f64::from(slider.widget.width());
                let radius = f64::from(slider.widget.height()) / 2.0;
                let span = width - 2.0 * radius;
                if span > 0.0 {
                    let maximum = slider.inner.level.get().maximum;
                    slider.change(((x - radius) / span).clamp(0.0, 1.0) * maximum);
                }
            }
        };

        let drag = gtk::GestureDrag::new();
        drag.connect_drag_begin({
            let (slider, move_to) = (self.clone(), move_to.clone());
            move |_, x, _| {
                slider.inner.dragging.set(true);
                slider.widget.grab_focus();
                move_to(x);
            }
        });
        drag.connect_drag_update({
            let move_to = move_to.clone();
            move |gesture, offset_x, _| {
                if let Some((start_x, _)) = gesture.start_point() {
                    move_to(start_x + offset_x);
                }
            }
        });
        drag.connect_drag_end({
            let slider = self.clone();
            move |_, _, _| slider.inner.dragging.set(false)
        });
        self.widget.add_controller(drag);

        let scroll = gtk::EventControllerScroll::new(
            gtk::EventControllerScrollFlags::BOTH_AXES | gtk::EventControllerScrollFlags::DISCRETE,
        );
        scroll.connect_scroll({
            let slider = self.clone();
            move |_, dx, dy| {
                // Scrolling up or right raises the value.
                slider.change(slider.value() + (dx - dy) * SCROLL_STEP);
                glib::Propagation::Stop
            }
        });
        self.widget.add_controller(scroll);

        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed({
            let slider = self.clone();
            move |_, key, _, _| {
                let step = match key {
                    gdk::Key::Left => -KEY_STEP,
                    gdk::Key::Right => KEY_STEP,
                    _ => return glib::Propagation::Proceed,
                };
                slider.change(slider.value() + step);
                glib::Propagation::Stop
            }
        });
        self.widget.add_controller(keys);
    }

    pub fn value(&self) -> f64 {
        self.inner.level.get().value
    }

    /// The user moved the slider: show it and tell whoever listens.
    fn change(&self, value: f64) {
        let mut level = self.inner.level.get();
        let value = value.clamp(0.0, level.maximum);
        if value == level.value {
            return;
        }
        level.value = value;
        self.inner.level.set(level);
        self.widget.queue_draw();
        if let Some(changed) = &*self.inner.changed.borrow() {
            changed(value);
        }
    }

    /// Shows a value that came from the system. Ignored while the user is
    /// dragging the handle: the system's answers to the drag would otherwise
    /// pull the handle back under the pointer's past positions.
    pub fn set_value(&self, value: f64) {
        if self.inner.dragging.get() {
            return;
        }
        let mut level = self.inner.level.get();
        level.value = value.clamp(0.0, level.maximum);
        self.inner.level.set(level);
        self.widget.queue_draw();
    }

    /// Calls `changed` with the new value whenever the user moves the
    /// slider.
    pub fn connect_changed(&self, changed: impl Fn(f64) + 'static) {
        *self.inner.changed.borrow_mut() = Some(Box::new(changed));
    }
}
