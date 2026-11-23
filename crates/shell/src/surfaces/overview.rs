// SPDX-License-Identifier: GPL-3.0-or-later

//! What gniri shows on top of niri's overview: one surface on each output,
//! mapped while the overview is open.
//!
//! The surface is on the top layer and fills the output under the panel.
//! While the windows show, it only takes the pointer where the search
//! entry and the dash are: everywhere else niri's overview is underneath,
//! with its windows to click and drag. With the app grid or the search
//! results in front, it takes the pointer everywhere.
//!
//! What covers niri's workspaces behind the grid and the results goes on
//! behind the panel too: the part of it that is above this surface is
//! drawn on the panel's (surfaces/panel.rs), faded with the rest.
//!
//! It holds the keyboard for as long as it is mapped, so that typing
//! starts a search (docs/DECISIONS.md, S9). niri's own keys for the
//! overview therefore arrive here: Escape, Enter and the arrows are
//! handed on as the actions niri would have run.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gniri_core::Subscription;
use gniri_services::capture;
use gtk::{cairo, gdk, glib, graphene};
use gtk4_layer_shell::{KeyboardMode, Layer, LayerShell};
use niri_ipc::Action;

use super::layer::{self, Anchor, Zone};
use crate::overview::app_grid::{self, Shown, View};
use crate::overview::controls::{Controls, Parts};
use crate::overview::{
    self, Direction, behind, dash, drag, folder, icon, search_entry, search_results,
};
use crate::services::Services;
use crate::style;
use crate::widgets::frosted;
use crate::widgets::motion::Eased;

/// An opacity, eased.
fn opacity_of(widget: &impl IsA<gtk::Widget>) -> Eased {
    let target = widget.clone().upcast::<gtk::Widget>().downgrade();
    Eased::new(widget, 1.0, move |opacity| {
        if let Some(widget) = target.upgrade() {
            widget.set_opacity(opacity);
        }
    })
}

/// overviewControls.js `SIDE_CONTROLS_ANIMATION_TIME`: how long the dash
/// and the entry take to come and go. The theme's transitions last as
/// long.
const CONTROLS_ANIMATION: Duration = Duration::from_millis(250);

/// How often the shell looks at what is behind the entry while the
/// windows show. A picture of that little takes niri about 2 ms.
const BEHIND_INTERVAL: Duration = Duration::from_millis(100);
/// How long such a picture may take to come.
const BEHIND_PATIENCE: Duration = Duration::from_millis(200);

struct Inner {
    window: gtk::ApplicationWindow,
    controls: Controls,
    services: Rc<Services>,
    monitor: gdk::Monitor,
    entry: search_entry::SearchEntry,
    dash: Rc<dash::Dash>,
    app_display: Rc<app_grid::AppView>,
    folder: Rc<folder::FolderDialog>,
    dragging: drag::Dragging,
    search_results: search_results::SearchResults,
    cover: gtk::Widget,
    /// The cover's part behind the panel, on the panel's surface.
    panel_cover: gtk::Widget,
    /// Counts opens and closes, so a close that was overtaken by a new
    /// open does not unmap the surface.
    generation: Cell<u64>,
    /// The grid rising (`_stateAdjustment`), and the opacities of the
    /// cover, the grid and the search results (`_onSearchChanged`).
    rising: Eased,
    covering: Eased,
    grid_fade: Eased,
    search_fade: Eased,
}

/// One output's overview surface. Dropping it stops it following the
/// compositor; `window` must still be destroyed by whoever owns it.
pub struct Overview {
    pub window: gtk::ApplicationWindow,
    _subscriptions: Vec<Subscription>,
}

impl Inner {
    /// Where `widget` is on the surface, in whole pixels, grown to cover
    /// every pixel it touches.
    fn bounds(&self, widget: &impl IsA<gtk::Widget>) -> Option<cairo::RectangleInt> {
        let bounds: graphene::Rect = widget.as_ref().compute_bounds(&self.window)?;
        let (left, top) = (bounds.x().floor(), bounds.y().floor());
        let right = (bounds.x() + bounds.width()).ceil();
        let bottom = (bounds.y() + bounds.height()).ceil();
        Some(cairo::RectangleInt::new(
            left as i32,
            top as i32,
            (right - left) as i32,
            (bottom - top) as i32,
        ))
    }

    /// Takes the pointer where there is something of gniri's to point at.
    fn update_input_region(&self) {
        let Some(surface) = self.window.surface() else {
            return;
        };
        let region = cairo::Region::create();
        if !layer::pointer_ignored() {
            if self.services.overview.with(overview::Overview::covered) {
                let _ = region.union_rectangle(&cairo::RectangleInt::new(
                    0,
                    0,
                    self.window.width(),
                    self.window.height(),
                ));
            } else {
                for bounds in [
                    self.bounds(&self.entry.widget),
                    self.bounds(&self.dash.background()),
                ]
                .into_iter()
                .flatten()
                {
                    let _ = region.union_rectangle(&bounds);
                }
            }
        }
        surface.set_input_region(Some(&region));
    }

    /// What to ask for a picture of to see what is behind the entry, and
    /// the fill the entry has on it there. Nothing while the cover is
    /// behind the entry, where its fill is the theme's own whatever the
    /// picture, and nothing on the light style, whose entry reads over
    /// anything (_bms-overview.scss).
    fn behind_target(&self) -> Option<(String, capture::Rect, behind::Fill)> {
        if self.services.overview.with(overview::Overview::covered) {
            return None;
        }
        // Only the dark style has the second fill.
        let darkened = style::token("bms-overview-fill-light-behind")?;
        let fill = behind::Fill::parse(if self.entry.widget.has_css_class("light-behind") {
            darkened
        } else {
            style::token("bms-overview-fill")?
        })?;
        let output = self.monitor.connector()?;
        let bounds = self.bounds(&self.entry.widget)?;
        // Without the round ends, which what is beside the entry shows
        // through, and without the edge.
        let (ends, edge) = (bounds.height() / 2, 2);
        // The surface starts under whatever reserves the top of the
        // output, and the entry comes out from under that.
        let above = (self.monitor.geometry().height() - self.window.height()).max(0);
        let top = (above + bounds.y() + edge).max(0);
        let rect = capture::Rect {
            x: bounds.x() + ends,
            y: top,
            width: bounds.width() - 2 * ends,
            height: above + bounds.y() + bounds.height() - edge - top,
        };
        (rect.x >= 0 && rect.width > 0 && rect.height > 0).then(|| (output.into(), rect, fill))
    }

    /// Keeps the entry `.light-behind` while what is behind it is light,
    /// for as long as the opening that is number `generation` lasts.
    fn watch_behind(self: &Rc<Self>, generation: u64) {
        let inner = Rc::downgrade(self);
        let current = move || {
            inner
                .upgrade()
                .filter(|inner| inner.generation.get() == generation)
        };
        glib::spawn_future_local(async move {
            while let Some(inner) = current() {
                let asked = inner
                    .behind_target()
                    .map(|(output, rect, fill)| (frosted::capturer().capture(&output, rect), fill));
                drop(inner);
                if let Some((answer, fill)) = asked {
                    let picture = glib::future_with_timeout(BEHIND_PATIENCE, answer.recv())
                        .await
                        .ok()
                        .and_then(Result::ok)
                        .flatten();
                    let brightness = picture.and_then(|picture| behind::brightness(&picture, fill));
                    if let (Some(brightness), Some(inner)) = (brightness, current()) {
                        let entry = &inner.entry.widget;
                        let was = entry.has_css_class("light-behind");
                        icon::set_class(entry, "light-behind", behind::is_light(brightness, was));
                    }
                }
                glib::timeout_future(BEHIND_INTERVAL).await;
            }
        });
    }

    /// Shows or hides the cover. Its part behind the panel shows only
    /// while this surface is mapped: the panel's always is.
    fn set_cover_visible(&self, visible: bool) {
        self.cover.set_visible(visible);
        self.panel_cover
            .set_visible(visible && self.window.is_visible());
    }

    /// Shows what the state says: the windows, the app grid or the search
    /// results. The grid rises from below and goes back down
    /// (`_onShowAppsButtonToggled`), the results and the grid cross-fade
    /// (`_onSearchChanged`), and the cover over niri's workspaces fades in
    /// and out where GNOME's workspaces shrink and grow.
    fn show_view(self: &Rc<Self>) {
        let (apps, searching) = self
            .services
            .overview
            .with(|overview| (overview.apps, overview.searching()));
        let time = CONTROLS_ANIMATION.as_millis() as u32;
        let hidden_after = |hide: fn(&Self), shown: fn(&Self) -> bool| {
            let inner = Rc::downgrade(self);
            move || {
                if let Some(inner) = inner.upgrade()
                    && !shown(&inner)
                {
                    hide(&inner);
                    inner.update_input_region();
                }
            }
        };
        fn wants_cover(inner: &Inner) -> bool {
            inner
                .services
                .overview
                .with(|overview| overview.apps || overview.searching())
        }
        fn wants_grid(inner: &Inner) -> bool {
            inner
                .services
                .overview
                .with(|overview| overview.apps && !overview.searching())
        }
        fn wants_results(inner: &Inner) -> bool {
            inner.services.overview.with(overview::Overview::searching)
        }

        if apps || searching {
            self.set_cover_visible(true);
        }
        // The entry and the dash change their tint as the cover fades in
        // behind them or out (_bms-overview.scss).
        icon::set_class(&self.controls, "covered", apps || searching);
        self.covering.ease(
            if apps || searching { 1.0 } else { 0.0 },
            time,
            adw::Easing::EaseOutQuad,
            hidden_after(|inner| inner.set_cover_visible(false), wants_cover),
        );
        let grid = apps && !searching;
        if grid {
            self.app_display.widget.set_visible(true);
        }
        // Rising and falling only between the windows and the grid; a
        // search fades the grid in place.
        if !searching {
            self.rising.ease(
                if apps { 1.0 } else { 0.0 },
                time,
                adw::Easing::EaseOutSine,
                hidden_after(
                    |inner| inner.app_display.widget.set_visible(false),
                    wants_grid,
                ),
            );
        }
        self.grid_fade.ease(
            if searching { 0.0 } else { 1.0 },
            time,
            adw::Easing::EaseOutQuad,
            hidden_after(
                |inner| inner.app_display.widget.set_visible(false),
                wants_grid,
            ),
        );
        if searching {
            self.search_results.widget.set_visible(true);
        }
        self.search_fade.ease(
            if searching { 1.0 } else { 0.0 },
            time,
            adw::Easing::EaseOutQuad,
            hidden_after(
                |inner| inner.search_results.widget.set_visible(false),
                wants_results,
            ),
        );
        self.update_input_region();
    }

    fn open(self: &Rc<Self>) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        // The output's height less the surface's is what the panel takes.
        self.window.present();
        self.entry.grab_focus();
        self.show_view();
        self.watch_behind(generation);
        // The surface must be on screen with its parts in their closed
        // look before the class that starts the transitions is added.
        glib::idle_add_local_once({
            let inner = Rc::clone(self);
            move || {
                if inner.generation.get() == generation {
                    inner.controls.add_css_class("open");
                }
            }
        });
    }

    fn close(self: &Rc<Self>) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        if !self.window.is_visible() {
            return;
        }
        self.controls.remove_css_class("open");
        let unmap = {
            let inner = Rc::clone(self);
            move || {
                if inner.generation.get() == generation {
                    inner.window.set_visible(false);
                    // Whatever the state says, nothing of the overview
                    // stays on the panel.
                    inner.panel_cover.set_visible(false);
                }
            }
        };
        let animated =
            gtk::Settings::default().is_some_and(|settings| settings.is_gtk_enable_animations());
        if animated {
            glib::timeout_add_local_once(CONTROLS_ANIMATION, unmap);
        } else {
            unmap();
        }
    }

    /// The surface starts under whatever reserves the top of the output.
    fn update_above(&self) {
        let output_height = self.monitor.geometry().height();
        self.controls
            .set_above((output_height - self.window.height()).max(0));
    }

    /// Moves the selection with an arrow key. Returns whether the key was
    /// used up.
    fn arrow(&self, direction: Direction) -> bool {
        let state = self.services.overview.get();
        if state.searching() {
            // Left and right belong to the text until the selection has
            // been moved into the results.
            if state.selected.is_none() && matches!(direction, Direction::Left | Direction::Right) {
                return false;
            }
            let found = self.search_results.found();
            let selected = overview::move_among_results(&found, state.selected, direction);
            self.services
                .overview
                .update(|overview| overview.selected = selected);
        } else if state.folder.is_some() {
            let (count, shape) = self.folder.view.shape();
            let selected = overview::move_in_grid(count, shape, state.selected, direction);
            self.services.overview.update(|overview| {
                overview.selected = selected;
                if let Some(selected) = selected {
                    overview.folder_page = selected / (shape.0 * shape.1).max(1);
                }
            });
        } else if state.apps {
            let (count, shape) = self.app_display.shape();
            let selected = overview::move_in_grid(count, shape, state.selected, direction);
            self.services.overview.update(|overview| {
                overview.selected = selected;
                if let Some(selected) = selected {
                    overview.page = selected / (shape.0 * shape.1).max(1);
                }
            });
        } else {
            // The windows are niri's: what its own arrow keys do there.
            self.services.niri_action(match direction {
                Direction::Left => Action::FocusColumnLeft {},
                Direction::Right => Action::FocusColumnRight {},
                Direction::Up => Action::FocusWindowOrWorkspaceUp {},
                Direction::Down => Action::FocusWindowOrWorkspaceDown {},
            });
        }
        true
    }

    /// What Enter does.
    fn activate(&self) {
        let state = self.services.overview.get();
        if state.searching() {
            let found = self.search_results.found();
            if let Some(found) = found.get(state.selected.unwrap_or(0)) {
                overview::activate_found(&self.services, found, &state.terms());
            }
        } else if state.folder.is_some() || state.apps {
            let view = if state.folder.is_some() {
                &self.folder.view
            } else {
                &*self.app_display
            };
            match state.selected.and_then(|selected| view.item_at(selected)) {
                Some(Shown::App { id, .. }) => {
                    let app = self.services.apps.state.with(|apps| apps.app(&id).cloned());
                    if let Some(app) = app {
                        overview::activate_app(&self.services, &app);
                    }
                }
                Some(Shown::Folder { id, .. }) => self
                    .services
                    .overview
                    .update(|overview| overview.open_folder(&id)),
                None => {}
            }
        } else {
            // As in niri: back to the window that has the focus.
            self.services.close_overview();
        }
    }

    fn key_pressed(&self, key: gdk::Key, modifiers: gdk::ModifierType) -> glib::Propagation {
        let plain = !modifiers.intersects(
            gdk::ModifierType::CONTROL_MASK
                | gdk::ModifierType::ALT_MASK
                | gdk::ModifierType::SUPER_MASK,
        );
        // A drag ends where it began.
        if self.dragging.active() {
            if key == gdk::Key::Escape {
                self.dragging.cancel();
            }
            return glib::Propagation::Stop;
        }
        // The folder's name has the keyboard while it is changed.
        if self.folder.renaming() {
            if key == gdk::Key::Escape {
                self.folder.cancel_rename();
                return glib::Propagation::Stop;
            }
            return glib::Propagation::Proceed;
        }
        let used = match key {
            gdk::Key::Escape => {
                let mut handled = false;
                self.services
                    .overview
                    .update(|overview| handled = overview.back());
                if !handled {
                    self.services.close_overview();
                }
                true
            }
            gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter => {
                self.activate();
                true
            }
            gdk::Key::Left if plain => self.arrow(Direction::Left),
            gdk::Key::Right if plain => self.arrow(Direction::Right),
            gdk::Key::Up if plain => self.arrow(Direction::Up),
            gdk::Key::Down if plain => self.arrow(Direction::Down),
            gdk::Key::Page_Up | gdk::Key::Page_Down => {
                let state = self.services.overview.get();
                if state.apps && !state.searching() && state.folder.is_none() {
                    let forward = key == gdk::Key::Page_Down;
                    let (count, shape) = self.app_display.shape();
                    let pages = count.div_ceil((shape.0 * shape.1).max(1)).max(1);
                    self.services.overview.update(|overview| {
                        overview.selected = None;
                        overview.page = if forward {
                            (overview.page + 1).min(pages - 1)
                        } else {
                            overview.page.saturating_sub(1)
                        };
                    });
                }
                true
            }
            _ => false,
        };
        if used {
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    }
}

/// Whether niri has a workspace below the active workspace of `output`.
/// In its overview that one passes behind the dash; with none, only the
/// backdrop is there.
fn workspace_below(niri: &gniri_services::niri::State, output: Option<&str>) -> bool {
    let active: Vec<bool> = match output {
        Some(output) => niri
            .workspaces_on(output)
            .map(|workspace| workspace.active)
            .collect(),
        None => niri
            .workspaces
            .iter()
            .map(|workspace| workspace.active)
            .collect(),
    };
    active
        .iter()
        .position(|active| *active)
        .is_some_and(|index| index + 1 < active.len())
}

/// Builds the overview surface for `monitor`, unmapped. `cover` is the
/// blurred wallpaper to put in front of niri's workspaces while the app
/// grid or the search results show, and `panel_cover` another of it, for
/// the caller to put behind the panel (`Panel::set_cover`). The overview
/// shows and hides both.
pub fn build(
    app: &adw::Application,
    monitor: &gdk::Monitor,
    services: &Rc<Services>,
    cover: gtk::Widget,
    panel_cover: gtk::Widget,
) -> Overview {
    let window = layer::window(
        app,
        monitor,
        "overview",
        Layer::Top,
        Anchor::Fill,
        Zone::Respect,
    );
    window.set_keyboard_mode(KeyboardMode::Exclusive);
    // The connection for pictures of what is behind the entry, ahead of
    // the first one: the compositor names its outputs to it meanwhile.
    frosted::capturer();

    cover.add_css_class("overview-cover");
    cover.set_visible(false);
    // Nothing of the cover is to be pointed at.
    cover.set_can_target(false);
    panel_cover.set_visible(false);

    let entry = search_entry::build(services);
    let dash = Rc::new(dash::build(services));
    let app_display = Rc::new(app_grid::build(services, View::Apps));
    let search_results = search_results::build(services);
    let folder = Rc::new(folder::build(services));
    let drag_layer = gtk::Fixed::builder()
        .css_classes(["drag-layer"])
        .can_target(false)
        .build();
    // Behind the folder's dialog: what is drawn under it, blurred.
    folder.set_backdrop(vec![
        cover.clone(),
        entry.widget.clone().upcast(),
        app_display.widget.clone().upcast(),
        dash.widget.clone().upcast(),
    ]);
    folder.on_focus_back(entry.focuser());
    folder.on_source({
        let view = Rc::clone(&app_display);
        move |id| view.folder_icon(id)
    });
    let controls = Controls::new(
        Parts {
            cover: cover.clone(),
            entry: entry.widget.clone().upcast(),
            app_display: app_display.widget.clone().upcast(),
            search_results: search_results.widget.clone().upcast(),
            dash: dash.widget.clone().upcast(),
            dash_label: dash.label().upcast(),
            folder: folder.widget.clone(),
            drag_layer: drag_layer.clone().upcast(),
        },
        {
            let dash = Rc::downgrade(&dash);
            move |width, height| {
                if let Some(dash) = dash.upgrade() {
                    dash.set_max_size(width, height);
                }
            }
        },
        {
            let folder = Rc::downgrade(&folder);
            move |width, height| {
                if let Some(folder) = folder.upgrade() {
                    folder.fit(width, height);
                }
            }
        },
        {
            // The controls are the label's parent, which is what its
            // place is relative to.
            let dash = Rc::downgrade(&dash);
            move || {
                let dash = dash.upgrade()?;
                let controls = dash.label().parent()?;
                dash.label_anchor(&controls)
            }
        },
    );
    window.set_child(Some(&controls));

    let dragging = drag::Dragging::new(
        services,
        &controls,
        &drag_layer,
        &app_display,
        &folder,
        &dash,
    );
    let rising = Eased::new(&controls, 1.0, {
        let controls = controls.downgrade();
        move |grid| {
            if let Some(controls) = controls.upgrade() {
                controls.set_grid(grid);
            }
        }
    });
    // One opacity for the cover and its part behind the panel, so that
    // the two fade as one.
    let covering = Eased::new(&cover, 1.0, {
        let covers = [cover.downgrade(), panel_cover.downgrade()];
        move |opacity| {
            for cover in covers.iter().filter_map(glib::WeakRef::upgrade) {
                cover.set_opacity(opacity);
            }
        }
    });
    let grid_fade = opacity_of(&app_display.widget);
    let search_fade = opacity_of(&search_results.widget);
    let inner = Rc::new(Inner {
        window: window.clone(),
        controls,
        services: Rc::clone(services),
        monitor: monitor.clone(),
        entry,
        dash,
        app_display,
        folder,
        dragging,
        search_results,
        rising,
        covering,
        grid_fade,
        search_fade,
        cover,
        panel_cover,
        generation: Cell::new(0),
    });

    // The input region and the cover's place follow the layout.
    window.connect_map({
        let inner = Rc::downgrade(&inner);
        move |window| {
            let Some(surface) = window.surface() else {
                return;
            };
            let inner = inner.clone();
            surface.connect_layout(move |_, _, _| {
                if let Some(inner) = inner.upgrade() {
                    inner.update_above();
                    inner.update_input_region();
                }
            });
        }
    });
    // And the dash's own size, which the surface's layout does not see
    // change: an application was started or closed.
    let tick = {
        let inner = Rc::downgrade(&inner);
        move || {
            if let Some(inner) = inner.upgrade() {
                // After the layout that the change leads to.
                glib::idle_add_local_full(glib::Priority::LOW, move || {
                    inner.update_input_region();
                    glib::ControlFlow::Break
                });
            }
        }
    };

    // Keys, before the text field sees them.
    let keys = gtk::EventControllerKey::builder()
        .propagation_phase(gtk::PropagationPhase::Capture)
        .build();
    keys.connect_key_pressed({
        let inner = Rc::downgrade(&inner);
        move |_, key, _, modifiers| match inner.upgrade() {
            Some(inner) => inner.key_pressed(key, modifiers),
            None => glib::Propagation::Proceed,
        }
    });
    window.add_controller(keys);

    let subscriptions = vec![
        services.niri.select(|niri| niri.overview_open, {
            let inner = Rc::clone(&inner);
            move |open| {
                if *open {
                    inner.open();
                } else {
                    inner.close();
                }
            }
        }),
        services
            .overview
            .select(|overview| (overview.apps, overview.searching()), {
                let inner = Rc::clone(&inner);
                move |_| inner.show_view()
            }),
        // What passes behind the dash sets its tint (_bms-overview.scss).
        services.niri.select(
            {
                let output = monitor.connector().map(String::from);
                move |niri| workspace_below(niri, output.as_deref())
            },
            {
                let controls = inner.controls.clone();
                move |below| icon::set_class(&controls, "workspace-below", *below)
            },
        ),
        services.niri.select(|niri| niri.windows.len(), {
            let tick = tick.clone();
            move |_| tick()
        }),
        services
            .apps
            .state
            .select(|apps| apps.installed.len(), move |_| tick()),
    ];

    Overview {
        window,
        _subscriptions: subscriptions,
    }
}

#[cfg(test)]
mod tests {
    use gniri_services::niri::{State, Workspace};

    use super::workspace_below;

    fn workspace(output: &str, active: bool) -> Workspace {
        Workspace {
            output: Some(output.to_owned()),
            active,
            ..Workspace::default()
        }
    }

    #[test]
    fn the_workspace_below_is_that_of_the_active_one_on_the_output() {
        let niri = State {
            workspaces: vec![
                workspace("eDP-1", false),
                workspace("eDP-1", true),
                workspace("eDP-1", false),
                workspace("HDMI-A-1", true),
            ],
            ..State::default()
        };
        assert!(workspace_below(&niri, Some("eDP-1")));
        assert!(!workspace_below(&niri, Some("HDMI-A-1")));
        assert!(!workspace_below(&niri, Some("DP-2")));
    }

    #[test]
    fn the_last_workspace_has_none_below() {
        let first = State {
            workspaces: vec![workspace("eDP-1", true), workspace("eDP-1", false)],
            ..State::default()
        };
        assert!(workspace_below(&first, Some("eDP-1")));
        let last = State {
            workspaces: vec![workspace("eDP-1", false), workspace("eDP-1", true)],
            ..State::default()
        };
        assert!(!workspace_below(&last, Some("eDP-1")));
    }
}
