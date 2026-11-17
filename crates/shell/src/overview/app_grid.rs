// SPDX-License-Identifier: GPL-3.0-or-later

//! The app grid: every installed application and folder, a page at a time,
//! and the same for the inside of a folder. Port of `AppDisplay`,
//! `FolderView`, `BaseAppView` and `AppGrid` (appDisplay.js), `IconGrid`
//! (iconGrid.js) and `PageIndicators` (pageIndicators.js), without the
//! icons' menus.
//!
//! Widget tree, with the actors it stands for:
//!
//! ```text
//! box.app-display                       AppDisplay, or a FolderView
//!   GniriIconGrid.icon-grid             the scroll view and its AppGrid
//!     button.overview-tile ...            AppIcon, FolderIcon (overview/icon.rs)
//!     button.page-navigation-arrow        the arrow to the page before
//!     button.page-navigation-arrow        and to the page after
//!   box.page-indicators                 PageIndicators
//!     button.page-indicator
//!       box.page-indicator-icon
//! ```
//!
//! What is shown comes from the applications' arrangement
//! (`gniri_services::apps::layout`); dragging an icon changes it
//! (`overview/drag.rs`), and while a drag is under way the grid shows the
//! icons where they would be after it ([`AppView::preview`]).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use gniri_core::Subscription;
use gniri_services::apps::{self, GridItem};
use gtk::glib;
use gtk::subclass::prelude::*;

use super::icon::{Tile, set_class};
use super::layout::{self, DropTarget, Grid, GridKind, GridStyle};
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
/// How long an icon takes to move to another place: Clutter's default
/// easing time, which `animateIconPosition` keeps.
const ICON_MOVE_TIME_MS: f64 = 250.0;
/// iconGrid.js `ICON_POSITION_DELAY`: each icon that moves starts this
/// much after the one before.
const ICON_POSITION_DELAY_MS: f64 = 10.0;

/// Where an icon that is moving set out from, and how many milliseconds
/// after the others it sets out.
type Departure = Option<((f64, f64), f64)>;

/// One place of the grid while it is laid out: an icon, or room for one
/// that is being dragged in from elsewhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Tile(usize),
    Gap,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct IconGrid {
        pub(super) tiles: RefCell<Vec<Tile>>,
        pub(super) arrows: RefCell<Option<[gtk::Button; 2]>>,
        pub(super) kind: Cell<GridKind>,
        /// The order the icons are laid out in, where it is not theirs: a
        /// drag shows where it would put one.
        pub(super) order: RefCell<Option<Vec<Slot>>>,
        /// The page that shows, or is being turned to.
        pub(super) page: Cell<usize>,
        /// Where the turning is: `page` once it has arrived.
        pub(super) position: Cell<f64>,
        pub(super) animation: RefCell<Option<adw::TimedAnimation>>,
        /// Where each icon was last put, through every page.
        pub(super) placed: RefCell<Vec<Option<(f64, f64)>>>,
        /// Where each icon that is moving set out from, and how long after
        /// the start it sets out.
        pub(super) moving: RefCell<Vec<Departure>>,
        /// Milliseconds since the icons started moving.
        pub(super) moved_for: Cell<f64>,
        pub(super) move_animation: RefCell<Option<adw::TimedAnimation>>,
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

        /// The places in the order they are laid out.
        pub(super) fn slots(&self) -> Vec<Slot> {
            self.order
                .borrow()
                .clone()
                .unwrap_or_else(|| (0..self.tiles.borrow().len()).map(Slot::Tile).collect())
        }
    }

    /// Ease-out-quad.
    fn ease(progress: f64) -> f64 {
        let progress = progress.clamp(0.0, 1.0);
        1.0 - (1.0 - progress) * (1.0 - progress)
    }

    impl WidgetImpl for IconGrid {
        fn measure(&self, _orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            // It takes the room it is given, like the scroll view it
            // stands for.
            (0, 0, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            let obj = self.obj();
            let kind = self.kind.get();
            let size = (f64::from(width), f64::from(height));
            let arrow = self.arrow_size();
            let icon = self.icon_size.get();
            let child = self.child_size().unwrap_or(icon);

            // `IconGridLayout._findBestIconSize`: the largest icons whose
            // items fit. An item is this much larger than its icon.
            let extra = child - icon;
            let best = layout::grid_icon_size(size, &STYLE, f64::from(arrow), kind, |icon| {
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

            let grid = layout::grid(size, &STYLE, f64::from(arrow), f64::from(child), kind);
            let tiles = self.tiles.borrow();
            let position = self.position.get();
            let moved_for = self.moved_for.get();
            let moving = self.moving.borrow();
            let mut placed = vec![None; tiles.len()];
            let mut laid_out = vec![false; tiles.len()];
            for (slot_index, slot) in self.slots().into_iter().enumerate() {
                let Slot::Tile(index) = slot else {
                    continue;
                };
                let Some(tile) = tiles.get(index) else {
                    continue;
                };
                laid_out[index] = true;
                let page = slot_index / grid.per_page();
                let (x, y) = grid.origin(slot_index % grid.per_page());
                // Through every page, as if they lay side by side.
                let target = (f64::from(x) + page as f64 * size.0, f64::from(y));
                placed[index] = Some(target);
                let (x, y) = match moving.get(index).copied().flatten() {
                    Some((from, delay)) => {
                        let progress = ease((moved_for - delay) / ICON_MOVE_TIME_MS);
                        (
                            from.0 + (target.0 - from.0) * progress,
                            from.1 + (target.1 - from.1) * progress,
                        )
                    }
                    None => target,
                };
                let x = x - position * size.0;
                // Only what is on the grid, of the page that shows or one
                // being turned from, is drawn or takes the pointer.
                tile.button
                    .set_child_visible(x + f64::from(child) > 0.0 && x < size.0);
                tile.button.size_allocate(
                    &gtk::Allocation::new(x.round() as i32, y.round() as i32, child, child),
                    -1,
                );
            }
            for (tile, laid_out) in tiles.iter().zip(laid_out) {
                if !laid_out {
                    tile.button.set_child_visible(false);
                }
            }
            drop(moving);
            self.placed.replace(placed);

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
    fn new(kind: GridKind, previous: &gtk::Button, next: &gtk::Button) -> Self {
        let grid: Self = glib::Object::new();
        grid.add_css_class("icon-grid");
        grid.imp().kind.set(kind);
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
        imp.order.replace(None);
        imp.placed.replace(Vec::new());
        imp.moving.replace(Vec::new());
        imp.tiles.replace(tiles);
        self.queue_resize();
    }

    /// Lays the icons out in `order` (`None`: their own), moving each that
    /// changes place to its new one (`animateIconPosition`).
    fn set_order(&self, order: Option<Vec<Slot>>) {
        let imp = self.imp();
        if *imp.order.borrow() == order {
            return;
        }
        let animated =
            gtk::Settings::default().is_some_and(|settings| settings.is_gtk_enable_animations());
        // Where each icon is drawn now, mid-move or not: where it sets out
        // from.
        let now: Vec<Option<(f64, f64)>> = {
            let placed = imp.placed.borrow();
            let moving = imp.moving.borrow();
            let moved_for = imp.moved_for.get();
            placed
                .iter()
                .enumerate()
                .map(|(index, target)| {
                    let target = (*target)?;
                    Some(match moving.get(index).copied().flatten() {
                        Some((from, delay)) => {
                            let progress = (moved_for - delay) / ICON_MOVE_TIME_MS;
                            let progress = 1.0 - (1.0 - progress.clamp(0.0, 1.0)).powi(2);
                            (
                                from.0 + (target.0 - from.0) * progress,
                                from.1 + (target.1 - from.1) * progress,
                            )
                        }
                        None => target,
                    })
                })
                .collect()
        };
        let old_slots = imp.slots();
        imp.order.replace(order);
        let new_slots = imp.slots();
        if let Some(animation) = imp.move_animation.take() {
            animation.pause();
        }
        if !animated || !self.is_mapped() {
            imp.moving.replace(Vec::new());
            self.queue_allocate();
            return;
        }
        // Those whose place changed set out, one a little after another.
        let place =
            |slots: &[Slot], index: usize| slots.iter().position(|slot| *slot == Slot::Tile(index));
        let mut moving = vec![None; now.len()];
        let mut changed = 0;
        for (index, start) in now.iter().enumerate() {
            let Some(start) = start else {
                continue;
            };
            if place(&old_slots, index) != place(&new_slots, index) {
                moving[index] = Some((*start, changed as f64 * ICON_POSITION_DELAY_MS));
                changed += 1;
            }
        }
        imp.moving.replace(moving);
        imp.moved_for.set(0.0);
        if changed == 0 {
            self.queue_allocate();
            return;
        }
        let total = ICON_MOVE_TIME_MS + (changed - 1) as f64 * ICON_POSITION_DELAY_MS;
        let grid = self.downgrade();
        let target = adw::CallbackAnimationTarget::new(move |value| {
            if let Some(grid) = grid.upgrade() {
                grid.imp().moved_for.set(value);
                grid.queue_allocate();
            }
        });
        let animation = adw::TimedAnimation::builder()
            .widget(self)
            .value_from(0.0)
            .value_to(total)
            .duration(total as u32)
            .easing(adw::Easing::Linear)
            .target(&target)
            .build();
        animation.connect_done({
            let grid = self.downgrade();
            move |_| {
                if let Some(grid) = grid.upgrade() {
                    grid.imp().moving.replace(Vec::new());
                    grid.imp().move_animation.take();
                    grid.queue_allocate();
                }
            }
        });
        animation.play();
        if animation.state() == adw::AnimationState::Playing {
            imp.move_animation.replace(Some(animation));
        }
    }

    /// The columns and rows of a page, once the grid has been laid out.
    fn shape(&self) -> Option<(usize, usize)> {
        self.imp()
            .geometry
            .get()
            .map(|grid| (grid.columns, grid.rows))
    }

    fn pages(&self) -> usize {
        let count = self.imp().slots().len();
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

    /// Whether a page is being turned.
    fn turning(&self) -> bool {
        self.imp().animation.borrow().is_some()
    }
}

/// Which grid a view is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    /// Every application and folder.
    Apps,
    /// The open folder's applications.
    Folder,
}

/// One item of a grid as it shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shown {
    App {
        id: String,
        name: String,
        icon: Option<String>,
        running: bool,
    },
    Folder {
        id: String,
        name: String,
        /// Of its first four applications.
        icons: Vec<Option<String>>,
    },
}

impl Shown {
    /// Its place in the arrangement.
    pub fn entry(&self) -> apps::Entry {
        match self {
            Self::App { id, .. } => apps::Entry::App(id.clone()),
            Self::Folder { id, .. } => apps::Entry::Folder { folder: id.clone() },
        }
    }

    /// The same tile, whether or not its application runs.
    fn same_tile(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::App { id, name, icon, .. },
                Self::App {
                    id: other_id,
                    name: other_name,
                    icon: other_icon,
                    ..
                },
            ) => (id, name, icon) == (other_id, other_name, other_icon),
            (a, b) => a == b,
        }
    }
}

struct Inner {
    services: Rc<Services>,
    view: View,
    grid: IconGrid,
    arrows: [gtk::Button; 2],
    indicators: gtk::Box,
    shown: RefCell<Vec<Shown>>,
    /// The tile something dragged would be dropped onto.
    drop_on: RefCell<Option<usize>>,
}

pub struct AppView {
    pub widget: gtk::Box,
    inner: Rc<Inner>,
    _subscriptions: Vec<Subscription>,
}

/// The applications of folder `id` as the grid has them.
fn folder_items<'a>(items: Vec<GridItem<'a>>, id: &str) -> Vec<&'a apps::App> {
    items
        .into_iter()
        .find_map(|item| match item {
            GridItem::Folder(folder) if folder.id == id => Some(folder.apps),
            _ => None,
        })
        .unwrap_or_default()
}

impl Inner {
    /// What the grid should show now.
    fn wanted(&self) -> Vec<Shown> {
        let favorites = self.services.favorites();
        let folder = self
            .services
            .overview
            .with(|overview| overview.folder.clone());
        self.services.apps.state.with(|apps| {
            let dash = self
                .services
                .niri
                .with(|niri| apps.dash(&favorites, &niri.windows));
            let running = |id: &str| dash.running.iter().any(|item| item.id == id);
            let app = |app: &apps::App| Shown::App {
                id: app.id.clone(),
                name: app.name.clone(),
                icon: app.icon.clone(),
                running: running(&app.id),
            };
            let items = apps.grid_items(&favorites);
            match self.view {
                View::Apps => items
                    .iter()
                    .map(|item| match item {
                        GridItem::App(found) => app(found),
                        GridItem::Folder(folder) => Shown::Folder {
                            id: folder.id.to_owned(),
                            name: folder.name.clone(),
                            icons: folder
                                .apps
                                .iter()
                                .take(4)
                                .map(|app| app.icon.clone())
                                .collect(),
                        },
                    })
                    .collect(),
                View::Folder => match &folder {
                    Some(id) => folder_items(items, id).into_iter().map(app).collect(),
                    // What the dialog showed, as it shrinks away.
                    None => self.shown.borrow().clone(),
                },
            }
        })
    }

    fn sync(self: &Rc<Self>) {
        let wanted = self.wanted();
        if *self.shown.borrow() == wanted {
            return;
        }
        let same_tiles = {
            let shown = self.shown.borrow();
            shown.len() == wanted.len()
                && shown
                    .iter()
                    .zip(&wanted)
                    .all(|(shown, wanted)| shown.same_tile(wanted))
        };
        if same_tiles {
            // Only the dots changed.
            for (tile, wanted) in self.grid.imp().tiles.borrow().iter().zip(&wanted) {
                if let Shown::App { running, .. } = wanted {
                    tile.set_running(*running);
                }
            }
        } else {
            self.drop_on.take();
            let tiles = wanted.iter().map(|item| self.build_tile(item)).collect();
            self.grid.set_tiles(tiles);
        }
        self.shown.replace(wanted);
        self.pages_changed();
        self.hide_open_folder();
    }

    /// A folder's icon is not there while its dialog is open
    /// (`FolderIcon._ensureFolderDialog`).
    fn hide_open_folder(&self) {
        if self.view != View::Apps {
            return;
        }
        let open = self
            .services
            .overview
            .with(|overview| overview.folder.clone());
        let shown = self.shown.borrow();
        for (tile, item) in self.grid.imp().tiles.borrow().iter().zip(shown.iter()) {
            if let Shown::Folder { id, .. } = item {
                // Faded out as the dialog opens, and back in as it
                // closes (the theme's transitions).
                let hidden = open.as_deref() == Some(id.as_str());
                super::icon::set_class(&tile.button, "folder-open", hidden);
            }
        }
    }

    fn build_tile(self: &Rc<Self>, item: &Shown) -> Tile {
        match item {
            Shown::App {
                id,
                name,
                icon,
                running,
            } => {
                let tile = Tile::new(LARGEST_ICON);
                tile.set(name, icon.as_deref());
                tile.set_running(*running);
                tile.button.connect_clicked({
                    let (services, id) = (Rc::clone(&self.services), id.clone());
                    move |_| {
                        let app = services.apps.state.with(|apps| apps.app(&id).cloned());
                        if let Some(app) = app {
                            super::activate_app(&services, &app);
                        }
                    }
                });
                tile
            }
            Shown::Folder { id, name, icons } => {
                let tile = Tile::new_folder(LARGEST_ICON);
                tile.set_folder(name, icons);
                tile.button.connect_clicked({
                    let (services, id) = (Rc::clone(&self.services), id.clone());
                    move |_| {
                        services
                            .overview
                            .update(|overview| overview.open_folder(&id))
                    }
                });
                tile
            }
        }
    }

    fn page(&self) -> usize {
        match self.view {
            View::Apps => self.services.overview.with(|overview| overview.page),
            View::Folder => self.services.overview.with(|overview| overview.folder_page),
        }
    }

    fn set_page(&self, page: usize) {
        match self.view {
            View::Apps => self
                .services
                .overview
                .update(|overview| overview.page = page),
            View::Folder => self
                .services
                .overview
                .update(|overview| overview.folder_page = page),
        }
    }

    /// Brings the page dots, the arrows and the page itself in line with
    /// how many pages there are.
    fn pages_changed(self: &Rc<Self>) {
        let pages = self.grid.pages();
        let page = self.page();
        if page >= pages {
            self.set_page(pages - 1);
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
                let (inner, page) = (Rc::downgrade(self), count);
                move |_| {
                    if let Some(inner) = inner.upgrade() {
                        inner.set_page(page);
                    }
                }
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

    /// Turns the page by `by`, within the pages there are.
    fn turn(&self, by: isize) {
        let pages = self.grid.pages();
        let page = self
            .page()
            .saturating_add_signed(by)
            .min(pages.saturating_sub(1));
        self.set_page(page);
    }
}

/// What is under a point of a grid, for a drag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Under {
    /// Nowhere to drop.
    Nothing,
    /// The band at the side of the page, where hovering turns it: -1 for
    /// the page before, 1 for the one after.
    PageEdge(isize),
    /// A place in the grid's order, through every page, and the icon
    /// there with where on it.
    Place {
        index: usize,
        target: Option<(usize, layout::DragLocation)>,
    },
}

impl AppView {
    /// How many icons there are and how a page holds them, for the arrow
    /// keys.
    pub fn shape(&self) -> (usize, (usize, usize)) {
        (
            self.inner.shown.borrow().len(),
            self.inner.grid.shape().unwrap_or((1, 1)),
        )
    }

    /// The item at `index`.
    pub fn item_at(&self, index: usize) -> Option<Shown> {
        self.inner.shown.borrow().get(index).cloned()
    }

    /// The grid widget, which drags' points are relative to.
    pub fn grid(&self) -> gtk::Widget {
        self.inner.grid.clone().upcast()
    }

    /// The tile at `index`.
    pub fn tile(&self, index: usize) -> Option<Tile> {
        self.inner.grid.imp().tiles.borrow().get(index).cloned()
    }

    /// The button of the folder `id`'s icon, if it shows.
    pub fn folder_icon(&self, id: &str) -> Option<gtk::Widget> {
        let index =
            self.inner.shown.borrow().iter().position(
                |shown| matches!(shown, Shown::Folder { id: other, .. } if other == id),
            )?;
        self.tile(index).map(|tile| tile.button.upcast())
    }

    /// The index of the tile whose button `widget` is or is inside.
    pub fn tile_of(&self, widget: &gtk::Widget) -> Option<usize> {
        let tiles = self.inner.grid.imp().tiles.borrow();
        let mut current = Some(widget.clone());
        while let Some(widget) = current {
            if let Some(index) = tiles
                .iter()
                .position(|tile| tile.button.upcast_ref::<gtk::Widget>() == &widget)
            {
                return Some(index);
            }
            current = widget.parent();
        }
        None
    }

    /// What is at `(x, y)` of the grid widget.
    pub fn under(&self, (x, y): (f64, f64)) -> Under {
        let grid = &self.inner.grid;
        let Some(geometry) = grid.imp().geometry.get() else {
            return Under::Nothing;
        };
        let (width, height) = (f64::from(grid.width()), f64::from(grid.height()));
        if x < 0.0 || y < 0.0 || x > width || y > height {
            return Under::Nothing;
        }
        if x < geometry.indicators_width {
            return Under::PageEdge(-1);
        }
        if x > width - geometry.indicators_width {
            return Under::PageEdge(1);
        }
        let page = self.inner.page();
        let per_page = geometry.per_page();
        let count = grid.imp().slots().len();
        let on_page = count.saturating_sub(page * per_page).min(per_page);
        match geometry.drop_target((x, y), on_page) {
            DropTarget::Invalid => Under::Nothing,
            DropTarget::Empty => Under::Place {
                index: page * per_page + on_page,
                target: None,
            },
            DropTarget::Item(position, location) => Under::Place {
                index: page * per_page + position,
                target: Some((page * per_page + position, location)),
            },
        }
    }

    /// The tile at `position` of the grid as it is laid out now, if there
    /// is one there and not the room for something dragged.
    pub fn tile_at(&self, position: usize) -> Option<usize> {
        match self.inner.grid.imp().slots().get(position) {
            Some(Slot::Tile(index)) => Some(*index),
            _ => None,
        }
    }

    /// The columns of a page.
    pub fn columns(&self) -> usize {
        self.inner.grid.shape().map_or(1, |(columns, _)| columns)
    }

    /// Lays the icons out as if the one at `from` (or one from elsewhere,
    /// for `None`) were at `to`; `None` puts them back.
    pub fn preview(&self, moved: Option<(Option<usize>, usize)>) {
        let order = moved.map(|(from, to)| {
            let count = self.inner.shown.borrow().len();
            let mut slots: Vec<Slot> = (0..count).map(Slot::Tile).collect();
            let moved = match from {
                Some(from) if from < count => slots.remove(from),
                _ => Slot::Gap,
            };
            slots.insert(to.min(slots.len()), moved);
            slots
        });
        self.inner.grid.set_order(order);
    }

    /// Marks the tile something would be dropped onto, or none.
    pub fn set_drop_on(&self, index: Option<usize>, folder_preview: bool) {
        let previous = self.inner.drop_on.replace(index);
        let tiles = self.inner.grid.imp().tiles.borrow();
        if let Some(previous) = previous
            && Some(previous) != index
            && let Some(tile) = tiles.get(previous)
        {
            tile.set_drop(false);
            tile.set_folder_preview(false);
        }
        if let Some(tile) = index.and_then(|index| tiles.get(index)) {
            tile.set_drop(true);
            tile.set_folder_preview(folder_preview);
        }
    }

    /// Turns the page by `by`, unless one is being turned.
    pub fn turn(&self, by: isize) -> bool {
        if self.inner.grid.turning() {
            return false;
        }
        self.inner.turn(by);
        true
    }
}

fn arrow(icon: &str) -> gtk::Button {
    gtk::Button::builder()
        .css_classes(["page-navigation-arrow"])
        .icon_name(icon)
        .focusable(false)
        .build()
}

pub fn build(services: &Rc<Services>, view: View) -> AppView {
    let arrows = [
        arrow("carousel-arrow-previous-symbolic"),
        arrow("carousel-arrow-next-symbolic"),
    ];
    let kind = match view {
        View::Apps => GridKind::Apps,
        View::Folder => GridKind::Folder,
    };
    let grid = IconGrid::new(kind, &arrows[0], &arrows[1]);
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
        view,
        grid: grid.clone(),
        arrows,
        indicators,
        shown: RefCell::new(Vec::new()),
        drop_on: RefCell::new(None),
    });

    let turn = |by: isize| {
        let inner = Rc::downgrade(&inner);
        move || {
            if let Some(inner) = inner.upgrade() {
                inner.turn(by);
            }
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
    let selected_here = move |overview: &super::Overview| {
        let shows = match view {
            View::Apps => overview.apps && !overview.searching() && overview.folder.is_none(),
            View::Folder => overview.folder.is_some(),
        };
        overview.selected.filter(|_| shows)
    };
    let subscriptions = vec![
        services
            .apps
            .state
            .select(|apps| (apps.installed.clone(), apps.layout.clone()), {
                let refresh = refresh.clone();
                move |_| refresh()
            }),
        services.niri.select(|niri| niri.windows.clone(), {
            let refresh = refresh.clone();
            move |_| refresh()
        }),
        services
            .config
            .select(|config| config.dash.favorites.clone(), {
                let refresh = refresh.clone();
                move |_| refresh()
            }),
        services
            .overview
            .select(|overview| overview.folder.clone(), {
                let inner = Rc::downgrade(&inner);
                move |_| {
                    if let Some(inner) = inner.upgrade() {
                        inner.sync();
                        inner.hide_open_folder();
                    }
                }
            }),
        services.overview.select(
            move |overview| match view {
                View::Apps => overview.page,
                View::Folder => overview.folder_page,
            },
            {
                let inner = Rc::downgrade(&inner);
                move |page| {
                    if let Some(inner) = inner.upgrade() {
                        inner.show_page(*page);
                    }
                }
            },
        ),
        services.overview.select(selected_here, {
            let inner = Rc::downgrade(&inner);
            move |selected| {
                if let Some(inner) = inner.upgrade() {
                    inner.select(*selected);
                }
            }
        }),
    ];

    AppView {
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
