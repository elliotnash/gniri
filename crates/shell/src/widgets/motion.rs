// SPDX-License-Identifier: GPL-3.0-or-later

//! A value that eases towards where it is sent, as Clutter's
//! `ease_property` does for an actor's property.
//!
//! The animation is libadwaita's, tied to a widget: it skips to its end
//! while the widget is not mapped, and when GTK's animations are off (as
//! under a test fixture).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;

pub struct Eased {
    widget: gtk::Widget,
    value: Rc<Cell<f64>>,
    apply: Rc<dyn Fn(f64)>,
    animation: RefCell<Option<adw::TimedAnimation>>,
}

impl Eased {
    /// Starts at `value`; `apply` shows each value as it comes.
    pub fn new(widget: &impl IsA<gtk::Widget>, value: f64, apply: impl Fn(f64) + 'static) -> Self {
        Self {
            widget: widget.clone().upcast(),
            value: Rc::new(Cell::new(value)),
            apply: Rc::new(apply),
            animation: RefCell::new(None),
        }
    }

    /// Where it is now.
    pub fn value(&self) -> f64 {
        self.value.get()
    }

    fn stop(&self) {
        if let Some(animation) = self.animation.borrow_mut().take() {
            animation.pause();
        }
    }

    /// Goes to `value` at once.
    pub fn jump(&self, value: f64) {
        self.stop();
        self.value.set(value);
        (self.apply)(value);
    }

    /// Eases from where it is to `to`. `done` runs once it is there, and
    /// not if it is sent elsewhere first.
    pub fn ease(
        &self,
        to: f64,
        duration_ms: u32,
        easing: adw::Easing,
        done: impl FnOnce() + 'static,
    ) {
        self.stop();
        let (value, apply) = (Rc::clone(&self.value), Rc::clone(&self.apply));
        let target = adw::CallbackAnimationTarget::new(move |now| {
            value.set(now);
            apply(now);
        });
        let animation = adw::TimedAnimation::builder()
            .widget(&self.widget)
            .value_from(self.value.get())
            .value_to(to)
            .duration(duration_ms)
            .easing(easing)
            .target(&target)
            .build();
        let done = RefCell::new(Some(done));
        animation.connect_done(move |animation| {
            // A paused animation is one that was replaced.
            if animation.state() == adw::AnimationState::Finished
                && let Some(done) = done.take()
            {
                done();
            }
        });
        *self.animation.borrow_mut() = Some(animation.clone());
        animation.play();
    }
}
