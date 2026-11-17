// SPDX-License-Identifier: GPL-3.0-or-later

//! Dragging icons in the overview: to another place in the app grid, onto
//! another application to make a folder of the two, into a folder, out of
//! one, and to and from the dash. Port of what `AppDisplay`, `FolderIcon`,
//! `AppIcon`, `AppFolderDialog` and `Dash` do with drags (appDisplay.js,
//! dash.js), and of the parts of dnd.js they rely on.
//!
//! GNOME's drags never leave GNOME Shell, and neither do these: nothing
//! goes through the compositor. One gesture on the whole overview surface
//! finds the icon under the press, and once the pointer has moved far
//! enough takes it over from the icon's own click. An icon follows the
//! pointer; the grid, the folder and the dash show where it would go. On
//! letting go the applications' arrangement changes
//! ([`layout_after`]), and everything shows the new one.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gniri_services::apps::{self, Entry, Layout};
use gtk::{glib, graphene};

use super::app_grid::{AppView, Shown, Under};
use super::dash::Dash;
use super::folder::FolderDialog;
use super::layout::{DragLocation, reorder_target};
use crate::services::Services;

/// appDisplay.js `DELAYED_MOVE_TIMEOUT`: how long the pointer rests at a
/// place before the icons make room there.
const DELAYED_MOVE: Duration = Duration::from_millis(200);
/// How long it rests on another application before that one shows it
/// would become a folder (`AppIcon._setHoveringByDnd`).
const FOLDER_PREVIEW: Duration = Duration::from_millis(500);
/// `DRAG_PAGE_SWITCH_INITIAL_TIMEOUT` and `_REPEAT_TIMEOUT`: resting at
/// the side of the grid turns the page, and keeps turning it.
const PAGE_SWITCH: Duration = Duration::from_millis(1000);
/// `DRAG_PAGE_SWITCH_IMMEDIATELY_THRESHOLD_PX`: pushed against the edge of
/// the grid, the page turns at once.
const PAGE_SWITCH_EDGE: f64 = 20.0;
/// appDisplay.js `POPDOWN_DIALOG_TIMEOUT`: an application dragged out of
/// its folder's dialog closes it after this long outside.
const POPDOWN_DIALOG: Duration = Duration::from_millis(500);
/// dnd.js `SNAP_BACK_ANIMATION_TIME`: a drag let go of where nothing takes
/// it goes back.
const SNAP_BACK: Duration = Duration::from_millis(250);

/// Where a dragged icon came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// The app grid: an application or a folder.
    Grid(Entry),
    /// The dialog of folder `folder`: one of its applications.
    Folder { folder: String, app: String },
    /// The dash: a favourite, or another application that runs.
    Dash { app: String, favorite: bool },
}

impl Source {
    /// The application, unless a folder is dragged.
    fn app(&self) -> Option<&str> {
        match self {
            Self::Grid(Entry::App(app)) | Self::Folder { app, .. } | Self::Dash { app, .. } => {
                Some(app)
            }
            Self::Grid(Entry::Folder { .. }) => None,
        }
    }
}

/// What letting go would do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Drop {
    /// Put it in the app grid at this place, counted with it taken out.
    GridPlace(usize),
    /// Make a folder of it and this application.
    OntoApp(String),
    /// Put it into this folder.
    OntoFolder(String),
    /// Move it within its folder to this place.
    FolderPlace(usize),
    /// Pin it in the dash at this place of the row, as the row shows.
    DashPlace(usize),
    /// Take it out of the dash (on the show-apps button).
    Unpin,
}

/// The arrangement after `source` is let go of as `drop` says, or `None`
/// if that changes nothing. `config` is the config file's favourites;
/// `favorites` the dash's as they show.
pub fn layout_after(
    apps: &apps::State,
    source: &Source,
    drop: &Drop,
    config: &[String],
    favorites: &[String],
) -> Option<Layout> {
    let all_favorites = apps.favorites(config);
    let layout = match (source, drop) {
        (Source::Grid(entry), Drop::GridPlace(index)) => {
            apps.move_in_grid(entry, *index, &all_favorites)
        }
        (Source::Folder { folder, app }, Drop::GridPlace(index)) => {
            apps.remove_from_folder(app, folder, *index, &all_favorites)
        }
        (
            Source::Dash {
                app,
                favorite: true,
            },
            Drop::GridPlace(index),
        ) => apps.unpin(app, Some(*index), config),
        (Source::Grid(Entry::App(app)), Drop::OntoApp(onto)) => {
            apps.create_folder(app, onto, &all_favorites)
        }
        (Source::Grid(Entry::App(app)), Drop::OntoFolder(folder)) => {
            apps.add_to_folder(app, folder, &all_favorites)
        }
        (Source::Folder { folder, app }, Drop::FolderPlace(index)) => {
            apps.move_in_folder(app, folder, *index, &all_favorites)
        }
        (source, Drop::DashPlace(position)) => {
            let app = source.app()?;
            // Counted among the favourites as they show, without the one
            // that is dragged.
            let index = favorites
                .iter()
                .take(*position)
                .filter(|favorite| favorite.as_str() != app)
                .count();
            apps.pin(app, index, config)
        }
        (
            Source::Dash {
                app,
                favorite: true,
            },
            Drop::Unpin,
        ) => apps.unpin(app, None, config),
        _ => return None,
    };
    (layout != apps.layout).then_some(layout)
}

/// A press on something that can be dragged: what, where, its button, and
/// where it is in its grid.
type Press = (Source, (f64, f64), gtk::Widget, Option<usize>);

/// One drag under way.
struct Drag {
    source: Source,
    /// The icon that follows the pointer, and where in it the pointer is.
    icon: gtk::Widget,
    grab: (f64, f64),
    /// The icon dragged, made invisible meanwhile.
    hidden: Option<gtk::Widget>,
    /// Where the source is in the grid it came from, if it is in one.
    source_index: Option<usize>,
    /// What letting go would do now.
    drop: Option<Drop>,
    /// The place the grid shows the icon at, and the one it is about to.
    shown_at: Option<usize>,
    delayed: Option<(usize, glib::SourceId)>,
    /// The tile it is over, and whether that one shows the folder it would
    /// make yet.
    over_tile: Option<(usize, bool)>,
    folder_preview: Option<glib::SourceId>,
    page_switch: Option<glib::SourceId>,
    /// Pushed against the edge: where, until it moves away again.
    overshoot: Option<f64>,
    popdown: Option<glib::SourceId>,
    /// Whether it is in its folder's grid (else the app grid).
    in_folder: bool,
}

struct Inner {
    services: Rc<Services>,
    controls: gtk::Widget,
    layer: gtk::Fixed,
    apps: Rc<AppView>,
    folder: Rc<FolderDialog>,
    dash: Rc<Dash>,
    /// A press on something that can be dragged, and where.
    pending: RefCell<Option<Press>>,
    drag: RefCell<Option<Drag>>,
    /// Where the pointer is.
    pointer: Cell<(f64, f64)>,
}

/// Lets icons of one overview surface be dragged.
pub struct Dragging {
    inner: Rc<Inner>,
}

fn remove_source(source: &mut Option<glib::SourceId>) {
    if let Some(source) = source.take() {
        source.remove();
    }
}

impl Inner {
    /// What can be dragged at `(x, y)` of the controls: its source, its
    /// button, and where it is in its grid.
    fn source_at(&self, (x, y): (f64, f64)) -> Option<(Source, gtk::Widget, Option<usize>)> {
        let picked = self.controls.pick(x, y, gtk::PickFlags::DEFAULT)?;
        let overview = self.services.overview.get();
        if let Some(folder) = &overview.folder {
            if !self.folder.is_open() {
                return None;
            }
            let index = self.folder.view.tile_of(&picked)?;
            let Shown::App { id, .. } = self.folder.view.item_at(index)? else {
                return None;
            };
            let tile = self.folder.view.tile(index)?;
            return Some((
                Source::Folder {
                    folder: folder.clone(),
                    app: id,
                },
                tile.button.upcast(),
                Some(index),
            ));
        }
        if overview.apps
            && !overview.searching()
            && let Some(index) = self.apps.tile_of(&picked)
        {
            let item = self.apps.item_at(index)?;
            let tile = self.apps.tile(index)?;
            return Some((
                Source::Grid(item.entry()),
                tile.button.upcast(),
                Some(index),
            ));
        }
        let (app, favorite) = self.dash.item_of(&picked)?;
        let button = self.dash.button_of(&app)?;
        Some((Source::Dash { app, favorite }, button.upcast(), None))
    }

    /// Starts the drag the press was for.
    fn begin(self: &Rc<Self>, (source, start, widget, source_index): Press) {
        let size = self.dash.icon_size();
        let icon: gtk::Widget = match &source {
            Source::Grid(_) => self
                .apps
                .tile(source_index.unwrap_or_default())
                .map(|tile| tile.drag_image(size)),
            Source::Folder { .. } => self
                .folder
                .view
                .tile(source_index.unwrap_or_default())
                .map(|tile| tile.drag_image(size)),
            Source::Dash { app, .. } => {
                let icon = self
                    .services
                    .apps
                    .state
                    .with(|apps| apps.app(app).and_then(|app| app.icon.clone()));
                let image = super::icon::image(size);
                super::icon::set_app_icon(&image, icon.as_deref());
                Some(image.upcast())
            }
        }
        .unwrap_or_else(|| super::icon::image(size).upcast());
        icon.add_css_class("drag-icon");
        icon.set_can_target(false);

        // Where in the dragged button's icon the pointer is, kept for the
        // icon that follows it, at that icon's size.
        let grab = self
            .controls
            .compute_point(
                &widget,
                &graphene::Point::new(start.0 as f32, start.1 as f32),
            )
            .map_or((f64::from(size) / 2.0, f64::from(size) / 2.0), |point| {
                let (width, height) = (f64::from(widget.width()), f64::from(widget.height()));
                (
                    (f64::from(point.x()) / width.max(1.0)).clamp(0.0, 1.0) * f64::from(size),
                    (f64::from(point.y()) / height.max(1.0)).clamp(0.0, 1.0) * f64::from(size),
                )
            });
        self.layer.put(&icon, start.0 - grab.0, start.1 - grab.1);
        // `AppViewItem.scaleAndFade`: the icon leaves its place.
        widget.set_opacity(0.0);
        let in_folder = matches!(source, Source::Folder { .. });
        let from_dash = matches!(source, Source::Dash { favorite: true, .. });
        let mut drag = Drag {
            source,
            icon,
            grab,
            hidden: Some(widget),
            source_index,
            drop: None,
            shown_at: None,
            delayed: None,
            over_tile: None,
            folder_preview: None,
            page_switch: None,
            overshoot: None,
            popdown: None,
            in_folder,
        };
        let grid_shows = self
            .services
            .overview
            .with(|overview| overview.apps && !overview.searching() && overview.folder.is_none());
        if from_dash && grid_shows {
            self.make_room(&mut drag);
        }
        self.drag.replace(Some(drag));
    }

    /// The pointer moved to `(x, y)` of the controls during a drag.
    fn motion(self: &Rc<Self>, point: (f64, f64)) {
        self.pointer.set(point);
        let mut guard = self.drag.borrow_mut();
        let Some(drag) = guard.as_mut() else {
            return;
        };
        self.layer
            .move_(&drag.icon, point.0 - drag.grab.0, point.1 - drag.grab.1);
        drop(guard);
        self.retarget();
    }

    /// Works out what letting go would do where the pointer is now, and
    /// shows it.
    fn retarget(self: &Rc<Self>) {
        let point = self.pointer.get();
        let mut guard = self.drag.borrow_mut();
        let Some(drag) = guard.as_mut() else {
            return;
        };
        let overview = self.services.overview.get();

        // An application dragged within its folder's dialog.
        if drag.in_folder {
            if overview.folder.is_some() && self.folder.is_open() {
                if self.folder.contains(&self.controls, point) {
                    self.folder.set_lighter(false);
                    remove_source(&mut drag.popdown);
                    let grid = self.folder.view.grid();
                    let local = self.local(&grid, point);
                    let columns = self.folder.view.columns();
                    drag.drop = self.place_in(drag, &self.folder.view, local, columns, true);
                } else {
                    // `handleDragOver` outside the dialog: lighter, and
                    // closing unless it comes back in time.
                    self.folder.set_lighter(true);
                    drag.drop = Some(Drop::GridPlace(usize::MAX));
                    if drag.popdown.is_none() {
                        let inner = Rc::downgrade(self);
                        drag.popdown =
                            Some(glib::timeout_add_local_once(POPDOWN_DIALOG, move || {
                                if let Some(inner) = inner.upgrade() {
                                    inner.popdown();
                                }
                            }));
                    }
                }
                return;
            }
            // The dialog has closed: the drag goes on over the app grid,
            // which makes room for it.
            drag.in_folder = false;
            if let Some((_, source)) = drag.delayed.take() {
                source.remove();
            }
            self.make_room(drag);
        }

        // The dash, and its show-apps button.
        let app = drag.source.app().map(str::to_owned);
        let favorite = matches!(drag.source, Source::Dash { favorite: true, .. });
        if self.dash.over_show_apps(&self.controls, point) {
            self.dash.show_placeholder(None);
            self.dash.set_unpin(favorite);
            drag.drop = favorite.then_some(Drop::Unpin);
            self.leave_grid(drag);
            return;
        }
        self.dash.set_unpin(false);
        let installed = app.as_deref().is_some_and(|app| {
            self.services
                .apps
                .state
                .with(|apps| apps.app(app).is_some())
        });
        let favorites = self.dash_favorites();
        let own = app
            .as_deref()
            .and_then(|app| favorites.iter().position(|favorite| favorite == app));
        if let Some(place) = self.dash.place_at(&self.controls, point, own) {
            // Only an installed application can be pinned.
            let place = place.filter(|_| installed);
            self.dash.show_placeholder(place);
            drag.drop = place.map(Drop::DashPlace);
            self.leave_grid(drag);
            return;
        }
        self.dash.show_placeholder(None);

        // The app grid, while it shows. An application that runs and is no
        // favourite is in it already.
        let into_grid = !matches!(
            drag.source,
            Source::Dash {
                favorite: false,
                ..
            }
        );
        if into_grid && overview.apps && !overview.searching() && overview.folder.is_none() {
            let grid = self.apps.grid();
            let local = self.local(&grid, point);
            if self.apps.under(local) == Under::Nothing && !self.over(&self.apps.widget, point) {
                drag.drop = None;
                self.leave_grid(drag);
                return;
            }
            let columns = self.apps.columns();
            drag.drop = self.place_in(drag, &self.apps, local, columns, false);
            return;
        }
        drag.drop = None;
        self.leave_grid(drag);
    }

    /// `(x, y)` of the controls in `widget`'s coordinates.
    fn local(&self, widget: &impl IsA<gtk::Widget>, (x, y): (f64, f64)) -> (f64, f64) {
        self.controls
            .compute_point(widget, &graphene::Point::new(x as f32, y as f32))
            .map_or((-1.0, -1.0), |point| {
                (f64::from(point.x()), f64::from(point.y()))
            })
    }

    /// Whether `(x, y)` of the controls is on `widget`.
    fn over(&self, widget: &impl IsA<gtk::Widget>, point: (f64, f64)) -> bool {
        let (x, y) = self.local(widget, point);
        let widget = widget.as_ref();
        x >= 0.0 && y >= 0.0 && x <= f64::from(widget.width()) && y <= f64::from(widget.height())
    }

    /// The favourites as the dash shows them.
    fn dash_favorites(&self) -> Vec<String> {
        let favorites = self.services.favorites();
        self.services.apps.state.with(|apps| {
            favorites
                .iter()
                .filter_map(|favorite| apps.app(favorite).map(|app| app.id.clone()))
                .collect()
        })
    }

    /// What letting go at `local` of `view` would do, and the grid's room
    /// for it (`BaseAppView._onDragMotion` and `_maybeMoveItem`, and the
    /// icons' `handleDragOver`). Places are those of the grid as it shows,
    /// with the dragged icon where it was last moved to, as in GNOME,
    /// which moves the icon itself while it is dragged.
    fn place_in(
        self: &Rc<Self>,
        drag: &mut Drag,
        view: &AppView,
        local: (f64, f64),
        columns: usize,
        in_folder: bool,
    ) -> Option<Drop> {
        let under = view.under(local);
        // Where the dragged icon, or the room for it, is now.
        let current = drag.shown_at.or(drag.source_index);

        // On another application of the grid, or on a folder: something
        // to drop it onto (only an application from the grid itself).
        let mut on_tile = None;
        if !in_folder
            && let Source::Grid(Entry::App(_)) = &drag.source
            && let Under::Place {
                target: Some((position, DragLocation::OnIcon)),
                ..
            } = under
            && Some(position) != current
            && let Some(tile) = view.tile_at(position)
            && Some(tile) != drag.source_index
        {
            on_tile = view.item_at(tile).map(|item| (tile, item));
        }
        match on_tile {
            Some((tile, Shown::Folder { id, .. })) => {
                self.cancel_delayed_move(drag);
                self.set_over_tile(drag, view, Some((tile, false)));
                return Some(Drop::OntoFolder(id));
            }
            Some((tile, Shown::App { id, .. })) => {
                self.cancel_delayed_move(drag);
                if drag.over_tile.map(|(over, _)| over) != Some(tile) {
                    self.set_over_tile(drag, view, Some((tile, false)));
                    remove_source(&mut drag.folder_preview);
                    let inner = Rc::downgrade(self);
                    drag.folder_preview =
                        Some(glib::timeout_add_local_once(FOLDER_PREVIEW, move || {
                            let Some(inner) = inner.upgrade() else {
                                return;
                            };
                            let mut guard = inner.drag.borrow_mut();
                            if let Some(drag) = guard.as_mut() {
                                drag.folder_preview = None;
                                if let Some((tile, false)) = drag.over_tile {
                                    drag.over_tile = Some((tile, true));
                                    inner.apps.set_drop_on(Some(tile), true);
                                }
                            }
                        }));
                }
                return Some(Drop::OntoApp(id));
            }
            None => {
                self.set_over_tile(drag, view, None);
                remove_source(&mut drag.folder_preview);
            }
        }

        // The page turns at its edges.
        if !in_folder {
            self.maybe_switch_page(drag, under, local);
        }

        // Beside an icon, or after the last: a place to make room at.
        let target = match under {
            Under::Place {
                target: Some((position, location)),
                ..
            } if location != DragLocation::OnIcon => {
                Some(reorder_target(current, position, location, columns.max(1)))
            }
            Under::Place {
                index,
                target: None,
            } => Some(match current {
                Some(current) if current < index => index - 1,
                _ => index,
            }),
            _ => None,
        };
        match target {
            Some(target) if Some(target) != current => {
                if drag.delayed.as_ref().map(|(at, _)| *at) != Some(target) {
                    self.cancel_delayed_move(drag);
                    let inner = Rc::downgrade(self);
                    let original = drag.source_index;
                    drag.delayed = Some((
                        target,
                        glib::timeout_add_local_once(DELAYED_MOVE, move || {
                            let Some(inner) = inner.upgrade() else {
                                return;
                            };
                            let mut guard = inner.drag.borrow_mut();
                            let Some(drag) = guard.as_mut() else {
                                return;
                            };
                            drag.delayed = None;
                            drag.shown_at = Some(target);
                            let view = if in_folder {
                                &inner.folder.view
                            } else {
                                &*inner.apps
                            };
                            view.preview(Some((original, target)));
                        }),
                    ));
                }
            }
            // At its own place, or over nothing: no move.
            _ => self.cancel_delayed_move(drag),
        }
        // Let go of now, it goes where it was about to move to, else where
        // it shows.
        let place = drag
            .delayed
            .as_ref()
            .map(|(at, _)| *at)
            .or(current)
            .unwrap_or(usize::MAX);
        Some(if in_folder {
            Drop::FolderPlace(place)
        } else {
            Drop::GridPlace(place)
        })
    }

    /// Makes room in the app grid for an application from elsewhere: at
    /// the end of the page that shows, where GNOME puts its placeholder
    /// (`AppDisplay._ensurePlaceholder`).
    fn make_room(&self, drag: &mut Drag) {
        let (count, (columns, rows)) = self.apps.shape();
        let page = self.services.overview.with(|overview| overview.page);
        let place = ((page + 1) * columns * rows).min(count);
        drag.source_index = None;
        drag.shown_at = Some(place);
        self.apps.preview(Some((None, place)));
    }

    fn set_over_tile(&self, drag: &mut Drag, view: &AppView, over: Option<(usize, bool)>) {
        if drag.over_tile == over {
            return;
        }
        drag.over_tile = over;
        view.set_drop_on(
            over.map(|(tile, _)| tile),
            over.is_some_and(|(_, preview)| preview),
        );
    }

    fn cancel_delayed_move(&self, drag: &mut Drag) {
        if let Some((_, source)) = drag.delayed.take() {
            source.remove();
        }
    }

    /// `_dragMaybeSwitchPageImmediately` and the page indicators' timeout.
    fn maybe_switch_page(self: &Rc<Self>, drag: &mut Drag, under: Under, local: (f64, f64)) {
        let width = f64::from(self.apps.grid().width());
        let direction = if local.0 < PAGE_SWITCH_EDGE {
            Some(-1)
        } else if local.0 > width - PAGE_SWITCH_EDGE {
            Some(1)
        } else {
            None
        };
        match (direction, drag.overshoot) {
            (Some(direction), None) => {
                drag.overshoot = Some(local.0);
                self.apps.turn(direction);
                self.repeat_page_switch(drag, direction);
                return;
            }
            (Some(_), Some(_)) => return,
            (None, Some(at)) if (local.0 - at).abs() > PAGE_SWITCH_EDGE => {
                drag.overshoot = None;
                remove_source(&mut drag.page_switch);
            }
            _ => {}
        }
        match under {
            Under::PageEdge(direction) if drag.page_switch.is_none() => {
                let inner = Rc::downgrade(self);
                drag.page_switch = Some(glib::timeout_add_local_once(PAGE_SWITCH, move || {
                    let Some(inner) = inner.upgrade() else {
                        return;
                    };
                    inner.apps.turn(direction);
                    let mut guard = inner.drag.borrow_mut();
                    if let Some(drag) = guard.as_mut() {
                        drag.page_switch = None;
                        inner.repeat_page_switch(drag, direction);
                    }
                }));
            }
            Under::PageEdge(_) => {}
            _ if drag.overshoot.is_none() => remove_source(&mut drag.page_switch),
            _ => {}
        }
    }

    fn repeat_page_switch(self: &Rc<Self>, drag: &mut Drag, direction: isize) {
        remove_source(&mut drag.page_switch);
        let inner = Rc::downgrade(self);
        drag.page_switch = Some(glib::timeout_add_local(PAGE_SWITCH, move || {
            match inner.upgrade() {
                Some(inner) => {
                    inner.apps.turn(direction);
                    glib::ControlFlow::Continue
                }
                None => glib::ControlFlow::Break,
            }
        }));
    }

    /// The pointer left the grid for the dash or the show-apps button.
    fn leave_grid(&self, drag: &mut Drag) {
        self.cancel_delayed_move(drag);
        self.set_over_tile(drag, &self.apps, None);
        remove_source(&mut drag.folder_preview);
        remove_source(&mut drag.page_switch);
        drag.overshoot = None;
    }

    /// The folder's dialog closes under the drag (`popdown`).
    fn popdown(self: &Rc<Self>) {
        {
            let mut guard = self.drag.borrow_mut();
            let Some(drag) = guard.as_mut() else {
                return;
            };
            drag.popdown = None;
            self.cancel_delayed_move(drag);
            self.folder.view.preview(None);
        }
        self.services
            .overview
            .update(|overview| overview.close_folder());
        // The folder's grid empties after the state changes; the drag goes
        // on over the app grid once it has.
        let inner = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(inner) = inner.upgrade() {
                inner.retarget();
            }
        });
    }

    /// Lets go.
    fn end(self: &Rc<Self>, dropped: bool) {
        let Some(mut drag) = self.drag.take() else {
            return;
        };
        self.cancel_delayed_move(&mut drag);
        for source in [
            &mut drag.folder_preview,
            &mut drag.page_switch,
            &mut drag.popdown,
        ] {
            remove_source(source);
        }
        self.dash.show_placeholder(None);
        self.dash.set_unpin(false);
        self.apps.set_drop_on(None, false);
        self.folder.view.set_drop_on(None, false);
        self.folder.set_lighter(false);

        let layout = drag.drop.as_ref().filter(|_| dropped).and_then(|drop| {
            let config = self
                .services
                .config
                .with(|config| config.dash.favorites.clone());
            let favorites = self.dash_favorites();
            self.services
                .apps
                .state
                .with(|apps| layout_after(apps, &drag.source, drop, &config, &favorites))
        });
        let accepted = dropped && drag.drop.is_some();
        // Dropped outside its folder's dialog: the dialog closes as the
        // application goes into the grid (`AppFolderDialog.acceptDrop`).
        if accepted
            && matches!(drag.drop, Some(Drop::GridPlace(_)))
            && matches!(drag.source, Source::Folder { .. })
        {
            self.services
                .overview
                .update(|overview| overview.close_folder());
        }
        self.apps.preview(None);
        self.folder.view.preview(None);
        if let Some(layout) = layout {
            self.services.arrange(layout);
        }

        let hidden = drag.hidden.take();
        if accepted {
            self.layer.remove(&drag.icon);
            if let Some(hidden) = hidden {
                hidden.set_opacity(1.0);
            }
            return;
        }
        // Not taken: the icon goes back to where it came from, if that
        // still shows (`_cancelDrag`).
        let home = hidden
            .as_ref()
            .filter(|widget| widget.is_mapped())
            .and_then(|widget| widget.compute_point(&self.layer, &graphene::Point::new(0.0, 0.0)));
        let animated =
            gtk::Settings::default().is_some_and(|settings| settings.is_gtk_enable_animations());
        let (Some(home), true) = (home, animated) else {
            self.layer.remove(&drag.icon);
            if let Some(hidden) = hidden {
                hidden.set_opacity(1.0);
            }
            return;
        };
        let (x, y) = self.layer.child_position(&drag.icon);
        let (to_x, to_y) = (f64::from(home.x()), f64::from(home.y()));
        let (layer, icon) = (self.layer.clone(), drag.icon.clone());
        let target = adw::CallbackAnimationTarget::new({
            let (layer, icon) = (layer.clone(), icon.clone());
            move |progress| {
                layer.move_(&icon, x + (to_x - x) * progress, y + (to_y - y) * progress);
            }
        });
        let animation = adw::TimedAnimation::builder()
            .widget(&self.layer)
            .value_from(0.0)
            .value_to(1.0)
            .duration(SNAP_BACK.as_millis() as u32)
            .easing(adw::Easing::EaseOutQuad)
            .target(&target)
            .build();
        animation.connect_done(move |_| {
            layer.remove(&icon);
            if let Some(hidden) = &hidden {
                hidden.set_opacity(1.0);
            }
        });
        animation.play();
    }
}

impl Dragging {
    /// Lets the icons on `controls` be dragged: the app grid's, the open
    /// folder's and the dash's. `layer` covers the controls and carries
    /// the icon that follows the pointer.
    pub fn new(
        services: &Rc<Services>,
        controls: &impl IsA<gtk::Widget>,
        layer: &gtk::Fixed,
        apps: &Rc<AppView>,
        folder: &Rc<FolderDialog>,
        dash: &Rc<Dash>,
    ) -> Self {
        let inner = Rc::new(Inner {
            services: Rc::clone(services),
            controls: controls.clone().upcast(),
            layer: layer.clone(),
            apps: Rc::clone(apps),
            folder: Rc::clone(folder),
            dash: Rc::clone(dash),
            pending: RefCell::new(None),
            drag: RefCell::new(None),
            pointer: Cell::new((0.0, 0.0)),
        });

        let gesture = gtk::GestureDrag::builder()
            .button(gtk::gdk::BUTTON_PRIMARY)
            .propagation_phase(gtk::PropagationPhase::Capture)
            .build();
        gesture.connect_drag_begin({
            let inner = Rc::downgrade(&inner);
            move |gesture, x, y| {
                let Some(inner) = inner.upgrade() else {
                    return;
                };
                // No drag while one goes back, nor of anything but an icon.
                let source = inner
                    .drag
                    .borrow()
                    .is_none()
                    .then(|| inner.source_at((x, y)))
                    .flatten();
                match source {
                    Some((source, widget, index)) => {
                        inner.pending.replace(Some((source, (x, y), widget, index)));
                    }
                    None => {
                        gesture.set_state(gtk::EventSequenceState::Denied);
                    }
                }
            }
        });
        gesture.connect_drag_update({
            let inner = Rc::downgrade(&inner);
            move |gesture, dx, dy| {
                let Some(inner) = inner.upgrade() else {
                    return;
                };
                let Some((x, y)) = gesture.start_point() else {
                    return;
                };
                if inner.drag.borrow().is_none() {
                    // dnd.js: a pointer starts the drag once it has moved
                    // further than the drag threshold either way.
                    let threshold = gtk::Settings::default()
                        .map_or(8, |settings| settings.gtk_dnd_drag_threshold());
                    if dx.abs() <= f64::from(threshold) && dy.abs() <= f64::from(threshold) {
                        return;
                    }
                    let Some(pending) = inner.pending.take() else {
                        return;
                    };
                    // The icon's own click is over.
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    inner.begin(pending);
                }
                inner.motion((x + dx, y + dy));
            }
        });
        gesture.connect_drag_end({
            let inner = Rc::downgrade(&inner);
            move |_, _, _| {
                if let Some(inner) = inner.upgrade() {
                    inner.pending.take();
                    inner.end(true);
                }
            }
        });
        gesture.connect_cancel({
            let inner = Rc::downgrade(&inner);
            move |_, _| {
                if let Some(inner) = inner.upgrade() {
                    inner.pending.take();
                    inner.end(false);
                }
            }
        });
        controls.add_controller(gesture);
        Self { inner }
    }

    /// Whether something is being dragged.
    pub fn active(&self) -> bool {
        self.inner.drag.borrow().is_some()
    }

    /// Escape: the drag ends where it started (dnd.js `_onKeyPress`).
    pub fn cancel(&self) {
        self.inner.end(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gniri_services::Model;
    use gniri_services::apps::{App, Event, Folder};

    fn state(layout: Layout) -> apps::State {
        let app = |id: &str, name: &str| App {
            id: id.to_owned(),
            name: name.to_owned(),
            ..App::default()
        };
        let mut state = apps::State {
            layout,
            ..apps::State::default()
        };
        state.apply(Event::Installed(vec![
            app("a", "Alpha"),
            app("b", "Beta"),
            app("c", "Gamma"),
            app("d", "Delta"),
        ]));
        state
    }

    fn after(state: &apps::State, layout: Layout) -> apps::State {
        apps::State {
            layout,
            ..state.clone()
        }
    }

    fn grid(state: &apps::State, favorites: &[String]) -> Vec<String> {
        state
            .grid_items(favorites)
            .iter()
            .map(|item| item.name().to_owned())
            .collect()
    }

    fn app(id: &str) -> Entry {
        Entry::App(id.to_owned())
    }

    #[test]
    fn letting_go_in_the_grid_moves_it_there() {
        let state = state(Layout::none());
        assert_eq!(grid(&state, &[]), ["Alpha", "Beta", "Delta", "Gamma"]);
        let layout = layout_after(
            &state,
            &Source::Grid(app("c")),
            &Drop::GridPlace(0),
            &[],
            &[],
        )
        .unwrap();
        assert_eq!(
            grid(&after(&state, layout), &[]),
            ["Gamma", "Alpha", "Beta", "Delta"]
        );
        // Where it already was: nothing changes, nothing is written.
        let unchanged = layout_after(
            &state,
            &Source::Grid(app("c")),
            &Drop::GridPlace(3),
            &[],
            &[],
        );
        assert!(unchanged.is_some_and(|layout| grid(&after(&state, layout), &[])[3] == "Gamma"));
    }

    #[test]
    fn onto_another_application_it_makes_a_folder() {
        let state = state(Layout::none());
        let layout = layout_after(
            &state,
            &Source::Grid(app("a")),
            &Drop::OntoApp("d".to_owned()),
            &[],
            &[],
        )
        .unwrap();
        let state = after(&state, layout);
        assert_eq!(grid(&state, &[]), ["Beta", "Unnamed Folder", "Gamma"]);
        // A folder dragged onto an application makes nothing.
        let folder = Entry::Folder {
            folder: "folder-1".to_owned(),
        };
        assert_eq!(
            layout_after(
                &state,
                &Source::Grid(folder),
                &Drop::OntoApp("b".to_owned()),
                &[],
                &[]
            ),
            None
        );
        // Another goes into it.
        let layout = layout_after(
            &state,
            &Source::Grid(app("b")),
            &Drop::OntoFolder("folder-1".to_owned()),
            &[],
            &[],
        )
        .unwrap();
        let state = after(&state, layout);
        assert_eq!(state.layout.folders[0].apps, ["d", "a", "b"]);
        // And comes out of it to a place in the grid.
        let layout = layout_after(
            &state,
            &Source::Folder {
                folder: "folder-1".to_owned(),
                app: "a".to_owned(),
            },
            &Drop::GridPlace(usize::MAX),
            &[],
            &[],
        )
        .unwrap();
        assert_eq!(
            grid(&after(&state, layout), &[]),
            ["Unnamed Folder", "Gamma", "Alpha"]
        );
    }

    #[test]
    fn within_a_folder_it_moves_among_the_others() {
        let state = state(Layout {
            folders: vec![Folder {
                id: "f".to_owned(),
                name: "F".to_owned(),
                apps: vec!["a".to_owned(), "b".to_owned(), "c".to_owned()],
                ..Folder::default()
            }],
            ..Layout::none()
        });
        let source = Source::Folder {
            folder: "f".to_owned(),
            app: "a".to_owned(),
        };
        let layout = layout_after(&state, &source, &Drop::FolderPlace(2), &[], &[]).unwrap();
        assert_eq!(layout.folders[0].apps, ["b", "c", "a"]);
    }

    #[test]
    fn the_dash_takes_applications_and_gives_them_back() {
        let config = ["a".to_owned(), "b".to_owned()];
        let state = state(Layout::none());
        let shown = |state: &apps::State| state.favorites(&config);
        // From the grid to the place between the two.
        let layout = layout_after(
            &state,
            &Source::Grid(app("c")),
            &Drop::DashPlace(1),
            &config,
            &config,
        )
        .unwrap();
        let state = after(&state, layout);
        assert_eq!(shown(&state), ["a", "c", "b"]);
        // Within the dash: the place is counted in the row as it shows,
        // the dragged one in it.
        let row = shown(&state);
        let layout = layout_after(
            &state,
            &Source::Dash {
                app: "a".to_owned(),
                favorite: true,
            },
            &Drop::DashPlace(3),
            &config,
            &row,
        )
        .unwrap();
        let state = after(&state, layout);
        assert_eq!(shown(&state), ["c", "b", "a"]);
        // To the show-apps button, and out of the dash.
        let row = shown(&state);
        let layout = layout_after(
            &state,
            &Source::Dash {
                app: "b".to_owned(),
                favorite: true,
            },
            &Drop::Unpin,
            &config,
            &row,
        )
        .unwrap();
        let state = after(&state, layout);
        assert_eq!(shown(&state), ["c", "a"]);
        // To a place in the grid.
        let layout = layout_after(
            &state,
            &Source::Dash {
                app: "c".to_owned(),
                favorite: true,
            },
            &Drop::GridPlace(0),
            &config,
            &shown(&state),
        )
        .unwrap();
        let state = after(&state, layout);
        assert_eq!(shown(&state), ["a"]);
        assert_eq!(grid(&state, &shown(&state))[0], "Gamma");
        // A running application that is no favourite cannot be unpinned.
        assert_eq!(
            layout_after(
                &state,
                &Source::Dash {
                    app: "d".to_owned(),
                    favorite: false
                },
                &Drop::Unpin,
                &config,
                &shown(&state)
            ),
            None
        );
    }
}
