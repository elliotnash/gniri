//! The quick settings grid. Port of `QuickSettingsLayout` in
//! `js/ui/quickSettings.js`.
//!
//! Items flow left to right into rows of equal-width columns; an item can
//! span several columns. An item may own a menu, which is laid out under
//! the row its item is in, across the full width, pushing the rows below it
//! down.

use std::cell::{Cell, RefCell};

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

/// GNOME Shell places an open menu one pixel left of the grid: the menus
/// live in an overlay bound to the menu's outer box plus an offset, and the
/// offset comes out one pixel short. Measured on GNOME Shell 51.1
/// (tests/visual/reference: the grid is at x 1403, the menu at 1402).
const MENU_OFFSET_X: i32 = -1;

struct Item {
    widget: gtk::Widget,
    span: usize,
    menu: Option<gtk::Widget>,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct QuickGrid {
        pub(super) items: RefCell<Vec<Item>>,
        pub(super) columns: Cell<usize>,
        pub(super) row_spacing: Cell<i32>,
        pub(super) column_spacing: Cell<i32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for QuickGrid {
        const NAME: &'static str = "GniriQuickGrid";
        type Type = super::QuickGrid;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for QuickGrid {
        fn dispose(&self) {
            for item in self.items.take() {
                item.widget.unparent();
                if let Some(menu) = item.menu {
                    menu.unparent();
                }
            }
        }
    }

    impl QuickGrid {
        /// The visible items, row by row (`_getRows`).
        fn rows<'a>(&self, items: &'a [Item]) -> Vec<Vec<&'a Item>> {
            let columns = self.columns.get().max(1);
            let mut rows: Vec<Vec<&Item>> = Vec::new();
            let mut line = 0;
            for item in items.iter().filter(|item| item.widget.is_visible()) {
                let span = item.span.clamp(1, columns);
                if line == 0 || line + span > columns {
                    rows.push(Vec::new());
                    line = 0;
                }
                rows.last_mut().expect("just pushed").push(item);
                line = (line + span) % columns;
            }
            rows
        }

        fn row_height(row: &[&Item]) -> (i32, i32) {
            row.iter().fold((0, 0), |(min, natural), item| {
                let (item_min, item_natural, _, _) =
                    item.widget.measure(gtk::Orientation::Vertical, -1);
                (min.max(item_min), natural.max(item_natural))
            })
        }

        /// The open menu of an item in `row`, if any.
        fn open_menu<'a>(row: &[&'a Item]) -> Option<&'a gtk::Widget> {
            row.iter()
                .filter_map(|item| item.menu.as_ref())
                .find(|menu| menu.is_visible())
        }
    }

    impl WidgetImpl for QuickGrid {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let items = self.items.borrow();
            let columns = self.columns.get().max(1) as i32;
            if orientation == gtk::Orientation::Horizontal {
                // `_getMaxChildWidth`: every column is as wide as the widest
                // item per column it spans.
                let (min, natural) = items.iter().filter(|item| item.widget.is_visible()).fold(
                    (0, 0),
                    |(min, natural), item| {
                        let span = item.span.clamp(1, columns as usize) as i32;
                        let (item_min, item_natural, _, _) =
                            item.widget.measure(gtk::Orientation::Horizontal, -1);
                        (
                            min.max((item_min + span - 1) / span),
                            natural.max((item_natural + span - 1) / span),
                        )
                    },
                );
                let spacing = (columns - 1) * self.column_spacing.get();
                (columns * min + spacing, columns * natural + spacing, -1, -1)
            } else {
                let rows = self.rows(&items);
                let spacing = (rows.len() as i32 - 1).max(0) * self.row_spacing.get();
                let (mut min, mut natural) = (spacing, spacing);
                for row in &rows {
                    let (row_min, row_natural) = Self::row_height(row);
                    min += row_min;
                    natural += row_natural;
                    if let Some(menu) = Self::open_menu(row) {
                        let (menu_min, menu_natural, _, _) =
                            menu.measure(gtk::Orientation::Vertical, -1);
                        min += menu_min;
                        natural += menu_natural;
                    }
                }
                (min, natural, -1, -1)
            }
        }

        fn size_allocate(&self, width: i32, _height: i32, _baseline: i32) {
            let items = self.items.borrow();
            let columns = self.columns.get().max(1) as i32;
            let column_spacing = self.column_spacing.get();
            let child_width = (width - (columns - 1) * column_spacing) / columns;

            let mut y = 0;
            for row in self.rows(&items) {
                let (_, row_height) = Self::row_height(&row);
                let mut line = 0;
                for item in &row {
                    let span = item.span.clamp(1, columns as usize) as i32;
                    let item_width = child_width * span + column_spacing * (span - 1);
                    let x = line * (child_width + column_spacing);
                    item.widget
                        .size_allocate(&gtk::Allocation::new(x, y, item_width, row_height), -1);
                    line = (line + span) % columns;
                }
                y += row_height;
                if let Some(menu) = Self::open_menu(&row) {
                    let (_, menu_height, _, _) = menu.measure(gtk::Orientation::Vertical, width);
                    menu.size_allocate(
                        &gtk::Allocation::new(MENU_OFFSET_X, y, width, menu_height),
                        -1,
                    );
                    y += menu_height;
                }
                y += self.row_spacing.get();
            }
        }
    }
}

glib::wrapper! {
    pub struct QuickGrid(ObjectSubclass<imp::QuickGrid>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl QuickGrid {
    /// `row_spacing` and `column_spacing` are `.quick-settings-grid`'s
    /// `spacing-rows` and `spacing-columns`, St properties GTK has no CSS
    /// for.
    pub fn new(columns: usize, row_spacing: i32, column_spacing: i32) -> Self {
        let grid: Self = glib::Object::new();
        grid.add_css_class("quick-settings-grid");
        let imp = grid.imp();
        imp.columns.set(columns);
        imp.row_spacing.set(row_spacing);
        imp.column_spacing.set(column_spacing);
        grid
    }

    /// Adds `widget` after the items already there, spanning `span` columns.
    /// `menu`, if given, is placed under the item's row whenever it is
    /// visible.
    pub fn add(&self, widget: &impl IsA<gtk::Widget>, span: usize, menu: Option<&gtk::Widget>) {
        widget.set_parent(self);
        if let Some(menu) = menu {
            menu.set_parent(self);
        }
        self.imp().items.borrow_mut().push(Item {
            widget: widget.clone().upcast(),
            span,
            menu: menu.cloned(),
        });
        self.queue_resize();
    }

    /// Dims every item, leaving the menus as they are. GNOME dims the whole
    /// menu under the open submenu (`QuickSettingsMenu._setDimmed`); the
    /// submenu is a child of this grid here, so the items are dimmed one by
    /// one instead.
    pub fn set_dimmed(&self, dimmed: bool) {
        for item in self.imp().items.borrow().iter() {
            if dimmed {
                item.widget.add_css_class("dimmed");
            } else {
                item.widget.remove_css_class("dimmed");
            }
        }
    }
}
