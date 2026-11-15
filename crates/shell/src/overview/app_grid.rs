// SPDX-License-Identifier: GPL-3.0-or-later

//! The app grid: every installed application, a page at a time. Port of
//! `AppDisplay`, `BaseAppView` and `AppGrid` (appDisplay.js), `IconGrid`
//! (iconGrid.js) and `PageIndicators` (pageIndicators.js), without
//! folders, dragging and the icons' menus.
//!
//! Widget tree, with the actors it stands for:
//!
//! ```text
//! box.app-display                       AppDisplay
//!   GniriIconGrid.icon-grid             the scroll view and its AppGrid
//!     button.overview-tile ...            AppIcon (overview/icon.rs)
//!     button.page-navigation-arrow        the arrow to the page before
//!     button.page-navigation-arrow        and to the page after
//!   box.page-indicators                 PageIndicators
//!     button.page-indicator
//!       box.page-indicator-icon
//! ```

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use gniri_core::Subscription;
use gtk::glib;
use gtk::subclass::prelude::*;

use super::icon::{Tile, set_class};
use super::layout::{self, Grid, GridStyle};
use crate::services::Services;

/// `.icon-grid` in the theme (crates/theme/scss/widgets/_app-grid.scss).
/// St reads these lengths from the style; GTK has no such properties.
const STYLE: GridStyle = GridStyle {
    row_spacing: 12.0,
    column_spacing: 12.0,
    max_row_spacing: 36.0,
    max_column_spacing: 36.0,
    page_padding_top: 24.0,
    page_padding_bottom: 24.0,
    page_padding_left: 18.0,
    page_padding_right: 18.0,
};
/// iconGrid.js `PAGE_SWITCH_TIME`.
const PAGE_SWITCH_TIME_MS: u32 = 300;
/// appDisplay.js `SCROLL_TIMEOUT_TIME`: at most one page per this long.
const SCROLL_INTERVAL: Duration = Duration::from_millis(150);
/// The largest of iconGrid.js `IconSize`, which the grid starts from.
const LARGEST_ICON: i32 = layout::GRID_ICON_SIZES[0];

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct IconGrid {
        pub(super) tiles: RefCell<Vec<Tile>>,
        pub(super) arrows: RefCell<Option<[gtk::Button; 2]>>,
        /// The page that shows, or is being turned to.
        pub(super) page: Cell<usize>,
        /// Where the turning is: `page` once it has arrived.
        pub(super) position: Cell<f64>,
        pub(super) animation: RefCell<Option<adw::TimedAnimation>>,
        pub(super) icon_size: Cell<i32>,
        /// How the last layout came out.
        pub(super) geometry: Cell<Option<Grid>>,
        /// Told when the number of pages or their shape changes.
        pub(super) on_reshaped: RefCell<Option<Box<dyn Fn()>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for IconGrid {
        const NAME: &'static str = "GniriIconGrid";
        type Type = super::IconGrid;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for IconGrid {
        fn constructed(&self) {
            self.parent_constructed();
            self.icon_size.set(LARGEST_ICON);
            // A page being turned is cut off at the grid's edge.
            self.obj().set_overflow(gtk::Overflow::Hidden);
        }

        fn dispose(&self) {
            for tile in self.tiles.take() {
                tile.button.unparent();
            }
            if let Some(arrows) = self.arrows.take() {
                for arrow in arrows {
                    arrow.unparent();
                }
            }
        }
    }

    impl IconGrid {
        /// The side of an item's square as it is now.
        fn child_size(&self) -> Option<i32> {
            let tiles = self.tiles.borrow();
            let tile = tiles.first()?;
            let (width, _, _, _) = tile.button.measure(gtk::Orientation::Horizontal, -1);
            let (height, _, _, _) = tile.button.measure(gtk::Orientation::Vertical, -1);
            Some(width.max(height))
        }

        /// What an arrow needs, its margins included.
        fn arrow_size(&self) -> i32 {
            self.arrows.borrow().as_ref().map_or(0, |arrows| {
                arrows[0].measure(gtk::Orientation::Horizontal, -1).1
            })
        }
    }

    impl WidgetImpl for IconGrid {
        fn measure(&self, _orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            // It takes the room it is given, like the scroll view it
            // stands for.
            (0, 0, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            let obj = self.obj();
            let size = (f64::from(width), f64::from(height));
            let arrow = self.arrow_size();
            let icon = self.icon_size.get();
            let child = self.child_size().unwrap_or(icon);

            // `IconGridLayout._findBestIconSize`: the largest icons whose
            // items fit. An item is this much larger than its icon.
            let extra = child - icon;
            let best = layout::grid_icon_size(size, &STYLE, f64::from(arrow), |icon| {
                f64::from(icon + extra)
            });
            if best != icon {
                // Not in the middle of a layout: afterwards, and then the
                // layout is done again.
                self.icon_size.set(best);
                glib::idle_add_local_once({
                    let obj = obj.downgrade();
                    move || {
                        if let Some(obj) = obj.upgrade() {
                            let size = obj.imp().icon_size.get();
                            for tile in obj.imp().tiles.borrow().iter() {
                                tile.set_icon_size(size);
                            }
                            obj.queue_resize();
                        }
                    }
                });
            }

            let grid = layout::grid(size, &STYLE, f64::from(arrow), f64::from(child));
            let tiles = self.tiles.borrow();
            let position = self.position.get();
            for (index, tile) in tiles.iter().enumerate() {
                let page = index / grid.per_page();
                let (x, y) = grid.origin(index % grid.per_page());
                let offset = (page as f64 - position) * size.0;
                // Only the page that shows and one being turned from are
                // drawn or take the pointer.
                tile.button.set_child_visible(offset.abs() < size.0);
                tile.button.size_allocate(
                    &gtk::Allocation::new(x + offset.round() as i32, y, child, child),
                    -1,
                );
            }

            // The arrows are centred in the bands at the sides
            // (`BaseAppViewGridLayout.vfunc_allocate`).
            if let Some([previous, next]) = self.arrows.borrow().as_ref() {
                let band = grid.indicators_width;
                let inset = ((band - f64::from(arrow)) / 2.0).max(0.0);
                let y = (height - arrow) / 2;
                let left = inset.floor() as i32;
                let right = (size.0 - band + inset).floor() as i32;
                previous.size_allocate(&gtk::Allocation::new(left, y, arrow, arrow), -1);
                next.size_allocate(&gtk::Allocation::new(right, y, arrow, arrow), -1);
            }

            let before = self.geometry.replace(Some(grid));
            let reshaped = before
                .is_none_or(|before| (before.columns, before.rows) != (grid.columns, grid.rows));
            if reshaped {
                glib::idle_add_local_once({
                    let obj = obj.downgrade();
                    move || {
                        if let Some(obj) = obj.upgrade()
                            && let Some(reshaped) = obj.imp().on_reshaped.borrow().as_ref()
                        {
                            reshaped();
                        }
                    }
                });
            }
        }
    }
}

glib::wrapper! {
    pub struct IconGrid(ObjectSubclass<imp::IconGrid>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl IconGrid {
    fn new(previous: &gtk::Button, next: &gtk::Button) -> Self {
        let grid: Self = glib::Object::new();
        grid.add_css_class("icon-grid");
        previous.set_parent(&grid);
        next.set_parent(&grid);
        grid.imp()
            .arrows
            .replace(Some([previous.clone(), next.clone()]));
        grid
    }

    /// Replaces the icons. The arrows stay on top of them.
    fn set_tiles(&self, tiles: Vec<Tile>) {
        let imp = self.imp();
        for tile in imp.tiles.take() {
            tile.button.unparent();
        }
        let arrows = imp.arrows.borrow();
        let below = arrows.as_ref().map(|arrows| &arrows[0]);
        for tile in &tiles {
            tile.set_icon_size(imp.icon_size.get());
            tile.button.insert_before(self, below);
        }
        drop(arrows);
        imp.tiles.replace(tiles);
        self.queue_resize();
    }

    /// The columns and rows of a page, once the grid has been laid out.
    fn shape(&self) -> Option<(usize, usize)> {
        self.imp()
            .geometry
            .get()
            .map(|grid| (grid.columns, grid.rows))
    }

    fn pages(&self) -> usize {
        let count = self.imp().tiles.borrow().len();
        self.imp()
            .geometry
            .get()
            .map_or(1, |grid| grid.pages(count))
    }

    /// Turns to `page` (`IconGrid.goToPage`).
    fn set_page(&self, page: usize) {
        let imp = self.imp();
        if imp.page.replace(page) == page && imp.animation.borrow().is_none() {
            return;
        }
        if let Some(animation) = imp.animation.take() {
            animation.pause();
        }
        let target = page as f64;
        if imp.position.get() == target {
            return;
        }
        let grid = self.downgrade();
        let callback = adw::CallbackAnimationTarget::new(move |value| {
            if let Some(grid) = grid.upgrade() {
                grid.imp().position.set(value);
                grid.queue_allocate();
            }
        });
        let animation = adw::TimedAnimation::builder()
            .widget(self)
            .value_from(imp.position.get())
            .value_to(target)
            .duration(PAGE_SWITCH_TIME_MS)
            .easing(adw::Easing::EaseOutCubic)
            .target(&callback)
            .build();
        animation.connect_done({
            let grid = self.downgrade();
            move |_| {
                if let Some(grid) = grid.upgrade() {
                    grid.imp().animation.take();
                }
            }
        });
        animation.play();
        // With animations off it is done already, and `done` has run.
        if animation.state() == adw::AnimationState::Playing {
            imp.animation.replace(Some(animation));
        }
    }
}

/// One application in the grid.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Shown {
    id: String,
    name: String,
    icon: Option<String>,
    running: bool,
}

struct Inner {
    services: Rc<Services>,
    grid: IconGrid,
    arrows: [gtk::Button; 2],
    indicators: gtk::Box,
    shown: RefCell<Vec<Shown>>,
}

pub struct AppDisplay {
    pub widget: gtk::Box,
    inner: Rc<Inner>,
    _subscriptions: Vec<Subscription>,
}

impl Inner {
    /// What the grid should show now: every application that is not in
    /// the dash for good, with a dot under those that run.
    fn wanted(&self) -> Vec<Shown> {
        let favorites = &self
            .services
            .config
            .with(|config| config.dash.favorites.clone());
        self.services.apps.state.with(|apps| {
            let dash = self
                .services
                .niri
                .with(|niri| apps.dash(favorites, &niri.windows));
            apps.grid(favorites)
                .into_iter()
                .map(|app| Shown {
                    id: app.id.clone(),
                    name: app.name.clone(),
                    icon: app.icon.clone(),
                    running: dash.running.iter().any(|item| item.id == app.id),
                })
                .collect()
        })
    }

    fn sync(self: &Rc<Self>) {
        let wanted = self.wanted();
        if *self.shown.borrow() == wanted {
            return;
        }
        let same_icons = {
            let shown = self.shown.borrow();
            shown.len() == wanted.len()
                && shown.iter().zip(&wanted).all(|(shown, wanted)| {
                    (&shown.id, &shown.name, &shown.icon)
                        == (&wanted.id, &wanted.name, &wanted.icon)
                })
        };
        if same_icons {
            // Only the dots changed.
            for (tile, wanted) in self.grid.imp().tiles.borrow().iter().zip(&wanted) {
                tile.set_running(wanted.running);
            }
        } else {
            let tiles = wanted
                .iter()
                .map(|app| {
                    let tile = Tile::new(LARGEST_ICON);
                    tile.set(&app.name, app.icon.as_deref());
                    tile.set_running(app.running);
                    tile.button.connect_clicked({
                        let (services, id) = (Rc::clone(&self.services), app.id.clone());
                        move |_| {
                            let app = services.apps.state.with(|apps| apps.app(&id).cloned());
                            if let Some(app) = app {
                                super::activate_app(&services, &app);
                            }
                        }
                    });
                    tile
                })
                .collect();
            self.grid.set_tiles(tiles);
        }
        self.shown.replace(wanted);
        self.pages_changed();
    }

    /// Brings the page dots, the arrows and the page itself in line with
    /// how many pages there are.
    fn pages_changed(self: &Rc<Self>) {
        let pages = self.grid.pages();
        let page = self.services.overview.with(|overview| overview.page);
        if page >= pages {
            self.services
                .overview
                .update(|overview| overview.page = pages - 1);
        }

        let mut count = 0;
        let mut child = self.indicators.first_child();
        while let Some(current) = child {
            count += 1;
            child = current.next_sibling();
        }
        while count > pages {
            if let Some(last) = self.indicators.last_child() {
                self.indicators.remove(&last);
            }
            count -= 1;
        }
        while count < pages {
            let dot = gtk::Box::builder()
                .css_classes(["page-indicator-icon"])
                .build();
            let indicator = gtk::Button::builder()
                .css_classes(["page-indicator"])
                .child(&dot)
                .focusable(false)
                .build();
            indicator.connect_clicked({
                let (services, page) = (Rc::clone(&self.services), count);
                move |_| services.overview.update(|overview| overview.page = page)
            });
            self.indicators.append(&indicator);
            count += 1;
        }
        // `PageIndicators.setNPages`: one page needs no dots.
        self.indicators
            .set_opacity(if pages > 1 { 1.0 } else { 0.0 });
        self.show_page(page.min(pages - 1));
    }

    /// Marks `page` as the one that shows.
    fn show_page(&self, page: usize) {
        self.grid.set_page(page);
        let mut index = 0;
        let mut child = self.indicators.first_child();
        while let Some(current) = child {
            set_class(&current, "current", index == page);
            index += 1;
            child = current.next_sibling();
        }
        let pages = self.grid.pages();
        // Not hidden outright: the layout counts with an arrow's size
        // whether or not there is a page for it to turn to.
        self.arrows[0].set_child_visible(page > 0);
        self.arrows[1].set_child_visible(page + 1 < pages);
    }

    /// Shows which icon the arrow keys are on.
    fn select(&self, selected: Option<usize>) {
        for (index, tile) in self.grid.imp().tiles.borrow().iter().enumerate() {
            tile.set_selected(selected == Some(index));
        }
    }
}

impl AppDisplay {
    /// How many icons there are and how a page holds them, for the arrow
    /// keys.
    pub fn shape(&self) -> (usize, (usize, usize)) {
        (
            self.inner.shown.borrow().len(),
            self.inner.grid.shape().unwrap_or((1, 1)),
        )
    }

    /// The application at `index`.
    pub fn app_at(&self, index: usize) -> Option<String> {
        self.inner
            .shown
            .borrow()
            .get(index)
            .map(|shown| shown.id.clone())
    }
}

fn arrow(icon: &str) -> gtk::Button {
    gtk::Button::builder()
        .css_classes(["page-navigation-arrow"])
        .icon_name(icon)
        .focusable(false)
        .build()
}

pub fn build(services: &Rc<Services>) -> AppDisplay {
    let arrows = [
        arrow("carousel-arrow-previous-symbolic"),
        arrow("carousel-arrow-next-symbolic"),
    ];
    let grid = IconGrid::new(&arrows[0], &arrows[1]);
    grid.set_vexpand(true);
    let indicators = gtk::Box::builder()
        .css_classes(["page-indicators"])
        .halign(gtk::Align::Center)
        .build();
    let widget = gtk::Box::builder()
        .css_classes(["app-display"])
        .orientation(gtk::Orientation::Vertical)
        .build();
    widget.append(&grid);
    widget.append(&indicators);

    let inner = Rc::new(Inner {
        services: Rc::clone(services),
        grid: grid.clone(),
        arrows,
        indicators,
        shown: RefCell::new(Vec::new()),
    });

    let turn = |by: isize| {
        let inner = Rc::downgrade(&inner);
        move || {
            let Some(inner) = inner.upgrade() else {
                return;
            };
            let pages = inner.grid.pages();
            inner.services.overview.update(|overview| {
                overview.page = overview
                    .page
                    .saturating_add_signed(by)
                    .min(pages.saturating_sub(1));
            });
        }
    };
    inner.arrows[0].connect_clicked({
        let turn = turn(-1);
        move |_| turn()
    });
    inner.arrows[1].connect_clicked({
        let turn = turn(1);
        move |_| turn()
    });

    // Scrolling turns the page (`BaseAppView._onScroll`).
    let scroll = gtk::EventControllerScroll::new(
        gtk::EventControllerScrollFlags::BOTH_AXES | gtk::EventControllerScrollFlags::DISCRETE,
    );
    scroll.connect_scroll({
        let (back, forward) = (turn(-1), turn(1));
        let last = Cell::new(None::<Instant>);
        move |_, dx, dy| {
            let by = if dy != 0.0 { dy } else { dx };
            if by == 0.0
                || last
                    .get()
                    .is_some_and(|last| last.elapsed() < SCROLL_INTERVAL)
            {
                return glib::Propagation::Stop;
            }
            last.set(Some(Instant::now()));
            if by > 0.0 {
                forward()
            } else {
                back()
            }
            glib::Propagation::Stop
        }
    });
    widget.add_controller(scroll);

    grid.imp().on_reshaped.replace(Some(Box::new({
        let inner = Rc::downgrade(&inner);
        move || {
            if let Some(inner) = inner.upgrade() {
                inner.pages_changed();
            }
        }
    })));

    let refresh = {
        let inner = Rc::downgrade(&inner);
        move || {
            if let Some(inner) = inner.upgrade() {
                inner.sync();
            }
        }
    };
    let subscriptions = vec![
        services.apps.state.select(|apps| apps.installed.clone(), {
            let refresh = refresh.clone();
            move |_| refresh()
        }),
        services.niri.select(|niri| niri.windows.clone(), {
            let refresh = refresh.clone();
            move |_| refresh()
        }),
        services.overview.select(|overview| overview.page, {
            let inner = Rc::downgrade(&inner);
            move |page| {
                if let Some(inner) = inner.upgrade() {
                    inner.show_page(*page);
                }
            }
        }),
        services.overview.select(
            |overview| {
                overview
                    .selected
                    .filter(|_| overview.apps && !overview.searching())
            },
            {
                let inner = Rc::downgrade(&inner);
                move |selected| {
                    if let Some(inner) = inner.upgrade() {
                        inner.select(*selected);
                    }
                }
            },
        ),
    ];

    AppDisplay {
        widget,
        inner,
        _subscriptions: subscriptions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The grid's lengths are in the code because GTK has no style
    /// properties for them; the theme lists the same ones, from the same
    /// variables as the rest of GNOME's stylesheet.
    #[test]
    fn the_grid_lengths_are_the_themes() {
        let lengths = |name: &str| -> Vec<f64> {
            gniri_theme::token(name)
                .unwrap_or_else(|| panic!("no token {name}"))
                .split_whitespace()
                .map(|length| {
                    length
                        .strip_suffix("px")
                        .and_then(|pixels| pixels.parse().ok())
                        .unwrap_or_else(|| panic!("token {name} is not pixels"))
                })
                .collect()
        };
        assert_eq!(lengths("icon-grid-row-spacing"), [STYLE.row_spacing]);
        assert_eq!(lengths("icon-grid-column-spacing"), [STYLE.column_spacing]);
        assert_eq!(
            lengths("icon-grid-max-row-spacing"),
            [STYLE.max_row_spacing]
        );
        assert_eq!(
            lengths("icon-grid-max-column-spacing"),
            [STYLE.max_column_spacing]
        );
        assert_eq!(
            lengths("icon-grid-page-padding"),
            [STYLE.page_padding_top, STYLE.page_padding_right]
        );
        assert_eq!(STYLE.page_padding_top, STYLE.page_padding_bottom);
        assert_eq!(STYLE.page_padding_left, STYLE.page_padding_right);
    }
}
