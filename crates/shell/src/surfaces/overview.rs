//! What gniri shows on top of niri's overview: one surface on each output,
//! mapped while the overview is open.
//!
//! The surface is on the top layer and fills the output under the panel.
//! While the windows show, it only takes the pointer where the search
//! entry and the dash are: everywhere else niri's overview is underneath,
//! with its windows to click and drag. With the app grid or the search
//! results in front, it takes the pointer everywhere.
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
use gtk::{cairo, gdk, glib, graphene};
use gtk4_layer_shell::{KeyboardMode, Layer, LayerShell};
use niri_ipc::Action;

use super::layer::{self, Anchor, Zone};
use crate::overview::controls::{Controls, Parts};
use crate::overview::{self, Direction, app_grid, dash, search_entry, search_results};
use crate::services::Services;

/// overviewControls.js `SIDE_CONTROLS_ANIMATION_TIME`: how long the dash
/// and the entry take to come and go. The theme's transitions last as
/// long.
const CONTROLS_ANIMATION: Duration = Duration::from_millis(250);

struct Inner {
    window: gtk::ApplicationWindow,
    controls: Controls,
    services: Rc<Services>,
    monitor: gdk::Monitor,
    entry: search_entry::SearchEntry,
    dash: Rc<dash::Dash>,
    app_display: app_grid::AppDisplay,
    search_results: search_results::SearchResults,
    cover: gtk::Widget,
    /// Counts opens and closes, so a close that was overtaken by a new
    /// open does not unmap the surface.
    generation: Cell<u64>,
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

    /// Shows what the state says: the windows, the app grid or the search
    /// results.
    fn show_view(&self) {
        let (apps, searching) = self
            .services
            .overview
            .with(|overview| (overview.apps, overview.searching()));
        self.cover.set_visible(apps || searching);
        self.app_display.widget.set_visible(apps && !searching);
        self.search_results.widget.set_visible(searching);
        self.update_input_region();
    }

    fn open(self: &Rc<Self>) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        // The output's height less the surface's is what the panel takes.
        self.window.present();
        self.entry.grab_focus();
        self.show_view();
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
        } else if state.apps {
            let app = state
                .selected
                .and_then(|selected| self.app_display.app_at(selected))
                .and_then(|id| self.services.apps.state.with(|apps| apps.app(&id).cloned()));
            if let Some(app) = app {
                overview::activate_app(&self.services, &app);
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
                if state.apps && !state.searching() {
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

/// Builds the overview surface for `monitor`, unmapped. `cover` is the
/// blurred wallpaper to put in front of niri's workspaces while the app
/// grid or the search results show.
pub fn build(
    app: &adw::Application,
    monitor: &gdk::Monitor,
    services: &Rc<Services>,
    cover: gtk::Widget,
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

    cover.add_css_class("overview-cover");
    cover.set_visible(false);
    // Nothing of the cover is to be pointed at.
    cover.set_can_target(false);

    let entry = search_entry::build(services);
    let dash = Rc::new(dash::build(services));
    let app_display = app_grid::build(services);
    let search_results = search_results::build(services);
    let controls = Controls::new(
        Parts {
            cover: cover.clone(),
            entry: entry.widget.clone().upcast(),
            app_display: app_display.widget.clone().upcast(),
            search_results: search_results.widget.clone().upcast(),
            dash: dash.widget.clone().upcast(),
        },
        {
            let dash = Rc::downgrade(&dash);
            move |width, height| {
                if let Some(dash) = dash.upgrade() {
                    dash.set_max_size(width, height);
                }
            }
        },
    );
    window.set_child(Some(&controls));

    let inner = Rc::new(Inner {
        window: window.clone(),
        controls,
        services: Rc::clone(services),
        monitor: monitor.clone(),
        entry,
        dash,
        app_display,
        search_results,
        cover,
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
