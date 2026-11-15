// SPDX-License-Identifier: GPL-3.0-or-later

//! A container that is as wide as it is tall. Port of `ShellSquareBin`
//! (src/shell-square-bin.c), which GNOME's `BaseIcon` is: an icon with its
//! label under it asks for a square as large as its longer side.

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct SquareBin {
        pub(super) child: std::cell::RefCell<Option<gtk::Widget>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SquareBin {
        const NAME: &'static str = "GniriSquareBin";
        type Type = super::SquareBin;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for SquareBin {
        fn dispose(&self) {
            if let Some(child) = self.child.take() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for SquareBin {
        fn measure(&self, _orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            // `shell_square_bin_get_preferred_width`: the larger of the
            // child's two sizes, whichever way is asked.
            let Some(child) = self.child.borrow().clone() else {
                return (0, 0, -1, -1);
            };
            let (min_width, natural_width, _, _) = child.measure(gtk::Orientation::Horizontal, -1);
            let (min_height, natural_height, _, _) = child.measure(gtk::Orientation::Vertical, -1);
            (
                min_width.max(min_height),
                natural_width.max(natural_height),
                -1,
                -1,
            )
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(child) = self.child.borrow().as_ref() {
                child.allocate(width, height, baseline, None);
            }
        }
    }
}

glib::wrapper! {
    pub struct SquareBin(ObjectSubclass<imp::SquareBin>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl SquareBin {
    pub fn new(child: &impl IsA<gtk::Widget>) -> Self {
        let bin: Self = glib::Object::new();
        child.set_parent(&bin);
        bin.imp().child.replace(Some(child.clone().upcast()));
        bin
    }
}
