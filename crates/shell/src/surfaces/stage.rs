//! The stage: a full-screen overlay surface that holds whichever menu is
//! open on its output, and is unmapped while none is.
//!
//! The menu is an ordinary widget on it; the compositor blurs behind the
//! menu alone because GTK reports the `backdrop-filter` region of that
//! widget (docs/DECISIONS.md, S2). The rest of the surface is transparent
//! and catches the click that closes the menu. The strip over the panel is
//! left out of the input region, so the panel's own buttons still work while
//! a menu is open (S12).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gniri_core::Subscription;
use gtk::{cairo, gdk, glib};
use gtk4_layer_shell::{KeyboardMode, Layer, LayerShell};

use super::layer::{self, Anchor, Zone};
use crate::date_menu::{self, DateMenu};
use crate::menus::Menu;
use crate::quick_settings::{self, QuickSettings};
use crate::services::Services;

/// boxpointer.js `POPUP_ANIMATION_TIME`: how long a menu takes to fade in or
/// out. The theme's transition on `.popup-menu-content` lasts as long.
const POPUP_ANIMATION: Duration = Duration::from_millis(150);
/// `.popup-menu-boxpointer { -arrow-rise }`: the gap between a menu and the
/// panel, and between a menu and the screen edge.
const ARROW_RISE: i32 = 6;

struct Inner {
    window: gtk::ApplicationWindow,
    /// The transparent, screen-sized child the menu is placed in.
    root: gtk::Box,
    services: Rc<Services>,
    /// Height of this output's panel right now, 0 without one.
    panel_height: Box<dyn Fn() -> i32>,
    /// Built when first opened, then kept.
    quick_settings: RefCell<Option<QuickSettings>>,
    date_menu: RefCell<Option<DateMenu>>,
    /// The menu that is showing.
    shown: Cell<Option<Menu>>,
    /// Counts opens and closes, so a close that was overtaken by a new open
    /// does not unmap the surface.
    generation: Cell<u64>,
}

/// One output's stage. Dropping it stops it following the menu state;
/// `window` must still be destroyed by whoever owns it.
pub struct Stage {
    pub window: gtk::ApplicationWindow,
    _subscription: Subscription,
}

impl Inner {
    /// Everything but the panel strip takes input.
    fn update_input_region(&self) {
        let Some(surface) = self.window.surface() else {
            return;
        };
        if layer::pointer_ignored() {
            surface.set_input_region(Some(&cairo::Region::create()));
            return;
        }
        let top = (self.panel_height)();
        let region = cairo::Region::create_rectangle(&cairo::RectangleInt::new(
            0,
            top,
            self.window.width(),
            (self.window.height() - top).max(0),
        ));
        surface.set_input_region(Some(&region));
    }

    /// The widget of `menu`, if it has been built.
    fn widget(&self, menu: Menu) -> Option<gtk::Box> {
        match menu {
            Menu::QuickSettings => self
                .quick_settings
                .borrow()
                .as_ref()
                .map(|menu| menu.widget.clone()),
            Menu::DateMenu => self
                .date_menu
                .borrow()
                .as_ref()
                .map(|menu| menu.widget.clone()),
        }
    }

    /// Tells `menu` it is no longer showing.
    fn closed(&self, menu: Menu) {
        if menu == Menu::DateMenu
            && let Some(date_menu) = &*self.date_menu.borrow()
        {
            date_menu.closed();
        }
    }

    /// What Escape does: close the innermost thing that is open.
    fn back(&self) {
        let handled = self.shown.get() == Some(Menu::DateMenu)
            && self
                .date_menu
                .borrow()
                .as_ref()
                .is_some_and(|date_menu| date_menu.back());
        if !handled {
            self.services.menus.update(|menus| menus.back());
        }
    }

    fn show(self: &Rc<Self>, menu: Menu) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);

        // Only one menu shows; the other, if it was open, goes at once.
        let before = self.shown.replace(Some(menu));
        for other in [Menu::QuickSettings, Menu::DateMenu] {
            if other == menu {
                continue;
            }
            if let Some(widget) = self.widget(other) {
                widget.remove_css_class("open");
                widget.set_visible(false);
            }
            if before == Some(other) {
                self.closed(other);
            }
        }

        let (widget, align) = match menu {
            Menu::QuickSettings => {
                // Something else may have changed the backlight since the
                // slider last heard of it.
                self.services
                    .brightness
                    .send(gniri_services::brightness::Command::Refresh);
                let mut quick_settings = self.quick_settings.borrow_mut();
                let quick_settings =
                    quick_settings.get_or_insert_with(|| quick_settings::build(&self.services));
                // It opens under the status area: the right end of the
                // panel.
                (quick_settings.widget.clone(), gtk::Align::End)
            }
            Menu::DateMenu => {
                let mut date_menu = self.date_menu.borrow_mut();
                let date_menu = date_menu.get_or_insert_with(|| date_menu::build(&self.services));
                date_menu.opened();
                // It opens under the clock: the middle of the panel.
                (date_menu.widget.clone(), gtk::Align::Center)
            }
        };
        if widget.parent().is_none() {
            widget.set_hexpand(true);
            widget.set_vexpand(true);
            widget.set_halign(align);
            widget.set_valign(gtk::Align::Start);
            if align == gtk::Align::End {
                widget.set_margin_end(ARROW_RISE);
            }
            self.root.append(&widget);
        }
        widget.set_visible(true);
        widget.set_margin_top((self.panel_height)() + ARROW_RISE);

        self.window.present();
        self.update_input_region();
        // The surface must be on screen with the menu in its closed look
        // before the class that starts the transition is added.
        glib::idle_add_local_once({
            let inner = Rc::clone(self);
            move || {
                if inner.generation.get() == generation {
                    widget.add_css_class("open");
                }
            }
        });
    }

    fn hide(self: &Rc<Self>) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        if !self.window.is_visible() {
            return;
        }
        let shown = self.shown.take();
        if let Some(widget) = shown.and_then(|menu| self.widget(menu)) {
            widget.remove_css_class("open");
        }
        if let Some(menu) = shown {
            self.closed(menu);
        }

        let unmap = {
            let inner = Rc::clone(self);
            move || {
                // A mapped surface with nothing to draw is never repainted
                // (S3), so it goes away with its menu.
                if inner.generation.get() == generation {
                    inner.window.set_visible(false);
                }
            }
        };
        let animated =
            gtk::Settings::default().is_some_and(|settings| settings.is_gtk_enable_animations());
        if animated {
            glib::timeout_add_local_once(POPUP_ANIMATION, unmap);
        } else {
            unmap();
        }
    }
}

/// Builds the stage for `monitor`, unmapped. `panel_height` is asked for the
/// height of the output's panel whenever a menu opens.
pub fn build(
    app: &adw::Application,
    monitor: &gdk::Monitor,
    services: &Rc<Services>,
    panel_height: impl Fn() -> i32 + 'static,
) -> Stage {
    let window = layer::window(
        app,
        monitor,
        "stage",
        Layer::Overlay,
        Anchor::Fill,
        Zone::Ignore,
    );
    // A menu takes the keyboard while it is open, as GNOME's grab does.
    window.set_keyboard_mode(KeyboardMode::Exclusive);

    let root = gtk::Box::builder().hexpand(true).vexpand(true).build();
    window.set_child(Some(&root));

    let inner = Rc::new(Inner {
        window: window.clone(),
        root: root.clone(),
        services: Rc::clone(services),
        panel_height: Box::new(panel_height),
        quick_settings: RefCell::new(None),
        date_menu: RefCell::new(None),
        shown: Cell::new(None),
        generation: Cell::new(0),
    });

    // GTK may replace the input region when the surface is resized.
    window.connect_map({
        let inner = Rc::downgrade(&inner);
        move |window| {
            let Some(surface) = window.surface() else {
                return;
            };
            let inner = inner.clone();
            surface.connect_layout(move |_, _, _| {
                if let Some(inner) = inner.upgrade() {
                    inner.update_input_region();
                }
            });
        }
    });

    // A click on the stage itself, outside the menu, closes the menu.
    let click = gtk::GestureClick::builder().button(0).build();
    click.connect_pressed({
        let (root, services) = (root.clone(), Rc::clone(services));
        move |_, _, x, y| {
            let on_menu = root
                .pick(x, y, gtk::PickFlags::DEFAULT)
                .is_some_and(|picked| picked != *root.upcast_ref::<gtk::Widget>());
            if !on_menu {
                services.menus.update(|menus| menus.close());
            }
        }
    });
    root.add_controller(click);

    // Escape closes the innermost open thing, before any widget sees it.
    let keys = gtk::EventControllerKey::builder()
        .propagation_phase(gtk::PropagationPhase::Capture)
        .build();
    keys.connect_key_pressed({
        let inner = Rc::downgrade(&inner);
        move |_, key, _, _| {
            if key == gdk::Key::Escape {
                if let Some(inner) = inner.upgrade() {
                    inner.back();
                }
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        }
    });
    window.add_controller(keys);

    let output: Option<String> = monitor.connector().map(Into::into);
    let subscription = services.menus.select(
        move |menus| {
            menus
                .open
                .as_ref()
                .filter(|open| open.output == output)
                .map(|open| open.menu)
        },
        move |menu| match menu {
            Some(menu) => inner.show(*menu),
            None => inner.hide(),
        },
    );

    Stage {
        window,
        _subscription: subscription,
    }
}
