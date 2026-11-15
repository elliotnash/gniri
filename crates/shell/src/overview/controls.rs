// SPDX-License-Identifier: GPL-3.0-or-later

//! Lays the overview's parts out on its surface. Port of
//! `ControlsManagerLayout` in `js/ui/overviewControls.js`.
//!
//! ```text
//! GniriControls.controls-manager        ControlsManager
//!   picture | box .overview-cover       (no actor: see below)
//!   box.search-entry                    SearchEntry
//!   box.app-display                     AppDisplay
//!   box#searchResults                   SearchResultsView
//!   box#dash                            Dash
//!   label.dash-label                    the name of a dash item
//! ```
//!
//! The cover is gniri's own. GNOME shrinks the workspaces to a strip when
//! the app grid or a search shows; niri's stay where they are. So the
//! blurred wallpaper that is behind niri's workspaces is drawn once more
//! in front of them, in exactly the same place, and the grid goes on
//! that.

use std::cell::{Cell, RefCell};

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use super::layout;

/// The parts, in the order they are drawn.
pub struct Parts {
    pub cover: gtk::Widget,
    pub entry: gtk::Widget,
    pub app_display: gtk::Widget,
    pub search_results: gtk::Widget,
    pub dash: gtk::Widget,
    pub dash_label: gtk::Widget,
}

/// `.dash-label { -y-offset }`: how far above its item the label floats.
const DASH_LABEL_OFFSET: i32 = 8;

/// Told the width and height the dash may take.
type DashRoom = Box<dyn Fn(i32, i32)>;
/// Says where the dash item is that the label names: its left edge, its
/// width and its top.
type LabelAnchor = Box<dyn Fn() -> Option<(i32, i32, i32)>>;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Controls {
        pub(super) parts: RefCell<Option<Parts>>,
        /// How far above the surface the output starts: the panel's
        /// height. The cover is the output's size, not the surface's.
        pub(super) above: Cell<i32>,
        /// Told how much room the dash may take, before it is measured.
        pub(super) on_dash_room: RefCell<Option<DashRoom>>,
        pub(super) label_anchor: RefCell<Option<LabelAnchor>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Controls {
        const NAME: &'static str = "GniriControls";
        type Type = super::Controls;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Controls {
        fn dispose(&self) {
            if let Some(parts) = self.parts.take() {
                for part in [
                    parts.cover,
                    parts.entry,
                    parts.app_display,
                    parts.search_results,
                    parts.dash,
                    parts.dash_label,
                ] {
                    part.unparent();
                }
            }
        }
    }

    impl WidgetImpl for Controls {
        fn measure(&self, _orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            // The compositor gives the surface the output's size.
            (0, 0, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            let parts = self.parts.borrow();
            let Some(parts) = parts.as_ref() else {
                return;
            };
            let place = |widget: &gtk::Widget, y: i32, height: i32| {
                if widget.should_layout() {
                    widget.size_allocate(&gtk::Allocation::new(0, y, width, height), -1);
                }
            };

            let (_, search_height, _, _) = parts.entry.measure(gtk::Orientation::Vertical, width);
            place(&parts.entry, 0, search_height);

            if let Some(dash_room) = self.on_dash_room.borrow().as_ref() {
                dash_room(width, layout::max_dash_height(height));
            }
            let (_, dash_height, _, _) = parts.dash.measure(gtk::Orientation::Vertical, width);
            let boxes = layout::controls(height, search_height, dash_height);
            place(&parts.dash, boxes.dash.y, boxes.dash.height);
            place(
                &parts.app_display,
                boxes.app_display.y,
                boxes.app_display.height,
            );
            place(
                &parts.search_results,
                boxes.search_results.y,
                boxes.search_results.height,
            );

            let above = self.above.get();
            place(&parts.cover, -above, height + above);

            // `DashItemContainer.showLabel`: centred over its item, kept
            // on the screen.
            let label = &parts.dash_label;
            let anchor = self.label_anchor.borrow();
            if label.should_layout()
                && let Some((x, item_width, top)) = anchor.as_ref().and_then(|anchor| anchor())
            {
                let (_, label_width, _, _) = label.measure(gtk::Orientation::Horizontal, -1);
                let (_, label_height, _, _) =
                    label.measure(gtk::Orientation::Vertical, label_width);
                let x = (x + (item_width - label_width).div_euclid(2))
                    .clamp(0, (width - label_width).max(0));
                let y = top - label_height - DASH_LABEL_OFFSET;
                label.size_allocate(&gtk::Allocation::new(x, y, label_width, label_height), -1);
            }
        }
    }
}

glib::wrapper! {
    pub struct Controls(ObjectSubclass<imp::Controls>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Controls {
    /// `dash_room` is told the width and height the dash may take whenever
    /// the surface is laid out. `label_anchor` says where the dash item is
    /// that the label names, in this widget's coordinates.
    pub fn new(
        parts: Parts,
        dash_room: impl Fn(i32, i32) + 'static,
        label_anchor: impl Fn() -> Option<(i32, i32, i32)> + 'static,
    ) -> Self {
        let controls: Self = glib::Object::new();
        controls.add_css_class("controls-manager");
        for part in [
            &parts.cover,
            &parts.entry,
            &parts.app_display,
            &parts.search_results,
            &parts.dash,
            &parts.dash_label,
        ] {
            part.set_parent(&controls);
        }
        controls
            .imp()
            .label_anchor
            .replace(Some(Box::new(label_anchor)));
        controls.imp().parts.replace(Some(parts));
        controls
            .imp()
            .on_dash_room
            .replace(Some(Box::new(dash_room)));
        controls
    }

    /// Says how much of the output is above the surface.
    pub fn set_above(&self, above: i32) {
        if self.imp().above.replace(above) != above {
            self.queue_allocate();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The label's distance from the dash is a style property in St; the
    /// theme lists it and the code uses it.
    #[test]
    fn the_label_floats_as_far_above_the_dash_as_the_theme_says() {
        assert_eq!(
            gniri_theme::token("dash-label-offset"),
            Some(format!("{DASH_LABEL_OFFSET}px").as_str())
        );
    }
}
