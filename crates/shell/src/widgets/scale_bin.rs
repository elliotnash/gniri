// SPDX-License-Identifier: GPL-3.0-or-later

//! A bin whose size and look are its child's, scaled about the middle:
//! what a message takes up grows from nothing as it is drawn growing.
//! Port of `ScaleLayout` (messageList.js) with Clutter's actor scale, and
//! the actor's `translation_x` with it, for Clear's sweep.
//!
//! Only for what is not blurred by the compositor: GTK's blur region would
//! not follow a scale.

use std::cell::Cell;

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{glib, graphene, gsk};

/// Below this the child is not drawn: a scale of nothing cannot be undone
/// to find what is under the pointer.
const SMALLEST: f64 = 0.001;

mod imp {
    use super::*;

    pub struct ScaleBin {
        pub(super) child: std::cell::RefCell<Option<gtk::Widget>>,
        pub(super) scale: Cell<f64>,
        /// How far right the child is drawn, in pixels.
        pub(super) shift: Cell<f64>,
    }

    impl Default for ScaleBin {
        fn default() -> Self {
            Self {
                child: std::cell::RefCell::new(None),
                scale: Cell::new(1.0),
                shift: Cell::new(0.0),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ScaleBin {
        const NAME: &'static str = "GniriScaleBin";
        type Type = super::ScaleBin;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for ScaleBin {
        fn dispose(&self) {
            if let Some(child) = self.child.take() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for ScaleBin {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let Some(child) = &*self.child.borrow() else {
                return (0, 0, -1, -1);
            };
            let (min, natural, _, _) = child.measure(orientation, for_size);
            let scale = self.scale.get();
            if scale >= 1.0 {
                return (min, natural, -1, -1);
            }
            // "Math.floor(min * container.scale_y)"
            (
                (f64::from(min) * scale).floor() as i32,
                (f64::from(natural) * scale).floor() as i32,
                -1,
                -1,
            )
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            let Some(child) = &*self.child.borrow() else {
                return;
            };
            let (scale, shift) = (self.scale.get(), self.shift.get());
            child.set_child_visible(scale > SMALLEST);
            if scale >= 1.0 && shift == 0.0 {
                child.size_allocate(&gtk::Allocation::new(0, 0, width, height), -1);
                return;
            }
            // The child at its own height, drawn scaled about the middle of
            // what this takes up.
            let (_, natural, _, _) = child.measure(gtk::Orientation::Vertical, width);
            let scale = scale.max(SMALLEST);
            let (w, h) = (f64::from(width), f64::from(natural));
            let x = (w - w * scale) / 2.0 + shift;
            let y = (f64::from(height) - h * scale) / 2.0;
            let transform = gsk::Transform::new()
                .translate(&graphene::Point::new(x as f32, y as f32))
                .scale(scale as f32, scale as f32);
            child.allocate(width, natural, -1, Some(transform));
        }
    }
}

glib::wrapper! {
    pub struct ScaleBin(ObjectSubclass<imp::ScaleBin>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ScaleBin {
    pub fn new(child: &impl IsA<gtk::Widget>) -> Self {
        let bin: Self = glib::Object::new();
        child.set_parent(&bin);
        bin.imp().child.replace(Some(child.clone().upcast()));
        bin
    }

    pub fn child(&self) -> Option<gtk::Widget> {
        self.imp().child.borrow().clone()
    }

    /// Lets go of the child, to be put elsewhere.
    pub fn take_child(&self) -> Option<gtk::Widget> {
        let child = self.imp().child.take()?;
        child.unparent();
        Some(child)
    }

    pub fn set_scale(&self, scale: f64) {
        if self.imp().scale.replace(scale) != scale {
            self.queue_resize();
        }
    }

    pub fn set_shift(&self, shift: f64) {
        if self.imp().shift.replace(shift) != shift {
            self.queue_allocate();
        }
    }
}
