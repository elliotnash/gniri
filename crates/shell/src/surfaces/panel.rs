// SPDX-License-Identifier: GPL-3.0-or-later

//! The top panel's surface.
//!
//! Under the bar there is room for a cover: the overview's surface starts
//! below the panel, so while the app grid or a search shows, the part of
//! the blurred wallpaper that belongs behind the bar is drawn here, where
//! GNOME has its overview's background behind the panel.

use std::rc::Rc;

use adw::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib};
use gtk4_layer_shell::Layer;

use super::layer::{self, Anchor, Zone};
use crate::panel::{self, Bar};
use crate::services::Services;

mod imp {
    use std::cell::RefCell;

    use super::*;

    /// The bar, and behind it a cover laid out at the output's size and
    /// cut to the bar's.
    #[derive(Default)]
    pub struct Frame {
        pub(super) bar: RefCell<Option<gtk::Widget>>,
        pub(super) cover: RefCell<Option<gtk::Widget>>,
        pub(super) monitor: RefCell<Option<gdk::Monitor>>,
        pub(super) on_geometry: RefCell<Option<glib::SignalHandlerId>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Frame {
        const NAME: &'static str = "GniriPanelFrame";
        type Type = super::Frame;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Frame {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().set_overflow(gtk::Overflow::Hidden);
        }

        fn dispose(&self) {
            if let (Some(monitor), Some(handler)) = (self.monitor.take(), self.on_geometry.take()) {
                monitor.disconnect(handler);
            }
            for child in [self.cover.take(), self.bar.take()].into_iter().flatten() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for Frame {
        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            // The bar's size alone: the cover is cut to it.
            match self.bar.borrow().as_ref() {
                Some(bar) => bar.measure(orientation, for_size),
                None => (0, 0, -1, -1),
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(cover) = self.cover.borrow().as_ref()
                && cover.should_layout()
            {
                // The panel is at the top of the output, so the cover's
                // top is the output's.
                let output_height = self
                    .monitor
                    .borrow()
                    .as_ref()
                    .map_or(height, |monitor| monitor.geometry().height());
                cover.size_allocate(
                    &gtk::Allocation::new(0, 0, width, output_height.max(height)),
                    -1,
                );
            }
            if let Some(bar) = self.bar.borrow().as_ref() {
                bar.allocate(width, height, baseline, None);
            }
        }
    }
}

glib::wrapper! {
    pub struct Frame(ObjectSubclass<imp::Frame>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Frame {
    fn new(bar: &impl IsA<gtk::Widget>, monitor: &gdk::Monitor) -> Self {
        let frame: Self = glib::Object::new();
        bar.set_parent(&frame);
        let imp = frame.imp();
        imp.bar.replace(Some(bar.clone().upcast()));
        // The cover's height is the output's, which may change without
        // the bar's own size changing.
        let handler = monitor.connect_geometry_notify({
            let frame = frame.downgrade();
            move |_| {
                if let Some(frame) = frame.upgrade() {
                    frame.queue_allocate();
                }
            }
        });
        imp.monitor.replace(Some(monitor.clone()));
        imp.on_geometry.replace(Some(handler));
        frame
    }

    fn set_cover(&self, cover: &gtk::Widget) {
        let imp = self.imp();
        if let Some(old) = imp.cover.replace(Some(cover.clone())) {
            old.unparent();
        }
        // Before the bar, so drawn under it.
        cover.insert_before(self, imp.bar.borrow().as_ref());
    }
}

/// A mapped panel. Dropping it stops its updates; `window` must still be
/// destroyed by whoever owns it.
pub struct Panel {
    pub window: gtk::ApplicationWindow,
    frame: Frame,
    _bar: Bar,
}

impl Panel {
    /// Puts `cover` behind the bar, in place of any cover before it. It is
    /// laid out at the output's size from the output's top, so that a
    /// picture of the whole output shows the part of it under the bar.
    /// Whoever gives it shows and hides it.
    pub fn set_cover(&self, cover: &gtk::Widget) {
        cover.add_css_class("panel-cover");
        // The bar's buttons are what is to be pointed at.
        cover.set_can_target(false);
        self.frame.set_cover(cover);
    }
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
    let frame = Frame::new(&bar.widget, monitor);
    window.set_child(Some(&frame));
    window.present();
    Panel {
        window,
        frame,
        _bar: bar,
    }
}
