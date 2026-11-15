//! The dash: the favourite and the running applications in a row at the
//! bottom of the overview, and the button that opens the app grid. Port of
//! `Dash` and `ShowAppsIcon` in `js/ui/dash.js`, without dragging and
//! without the items' menus.
//!
//! Widget tree, with the actors it stands for:
//!
//! ```text
//! box#dash                              Dash
//!   overlay
//!     box.dash-background               its background, behind the items
//!     box.dash-container                the St.BoxLayout of both parts
//!       box.dash-apps                   the box of application items
//!         box.dash-item-container         DashItemContainer
//!           button.overview-tile            DashIcon (overview/icon.rs)
//!         box.dash-separator
//!       box.dash-item-container         ShowAppsIcon
//!         button.show-apps
//! ```
//!
//! An item reaches from the top of the dash to the bottom of the screen,
//! so that it is hit by a pointer pushed against the edge; the background
//! ends above the edge. St sizes the background from the container with a
//! constraint; here the two share a `GtkOverlay` that is measured by the
//! container.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gniri_core::Subscription;
use gniri_services::apps::{self, DashItem};
use gtk::glib;
use gtk::prelude::*;

use super::icon::{self, DashButton};
use super::layout::{self, DashMetrics};
use crate::services::Services;

/// The dash's own lengths in the theme (crates/theme/scss/widgets/_dash.scss),
/// which `Dash._adjustIconSize` reads from the theme nodes.
const METRICS: DashMetrics = DashMetrics {
    // `#dash { padding: 0 6px }`
    horizontal_padding: 12,
    // `.dash-background { padding: 12px ... }`
    background_vertical_padding: 24,
    // `.overview-icon { padding: 6px }`
    button_extra_width: 12,
    // The same, the button's 12px above the screen edge, and the room the
    // background's padding leaves around the icon.
    button_extra_height: 50,
};
/// The size the icons have until the screen is known: the largest.
const DEFAULT_ICON_SIZE: i32 = 64;
/// `.overview-icon { padding }`: an icon's box is this much larger than
/// the icon on every side.
const ICON_PADDING: i32 = 6;

/// One application's item.
struct Item {
    /// What it shows; rebuilt when this changes.
    shown: (String, String, Option<String>),
    container: gtk::Box,
    button: DashButton,
}

struct Inner {
    services: Rc<Services>,
    apps: gtk::Box,
    separator: gtk::Box,
    /// Makes the separator as tall as an icon.
    separator_sizer: gtk::Box,
    /// Makes the background as tall as an icon's box (the actor GNOME
    /// binds to the height of the show-apps icon).
    sizer: gtk::Box,
    show_apps: DashButton,
    items: RefCell<Vec<Item>>,
    /// How many of `items` are favourites; the separator follows them.
    favorites: Cell<usize>,
    icon_size: Cell<i32>,
    /// The room there was when the icons were last sized.
    sized_for: Cell<(i32, i32)>,
}

pub struct Dash {
    pub widget: gtk::Box,
    inner: Rc<Inner>,
    _subscriptions: Vec<Subscription>,
}

fn container(child: &impl IsA<gtk::Widget>) -> gtk::Box {
    let container = gtk::Box::builder()
        .css_classes(["dash-item-container"])
        .build();
    container.append(child);
    container
}

impl Inner {
    /// The dash as it should be now.
    fn dash(&self) -> apps::Dash {
        let favorites = &self.services.config.dash.favorites;
        self.services.niri.with(|niri| {
            self.services
                .apps
                .state
                .with(|apps| apps.dash(favorites, &niri.windows))
        })
    }

    /// The item called `id`, as it is now.
    fn item(&self, id: &str) -> Option<DashItem> {
        let dash = self.dash();
        dash.favorites
            .into_iter()
            .chain(dash.running)
            .find(|item| item.id == id)
    }

    /// Makes the row show `dash` (`Dash._redisplay`). The items of
    /// applications that stay are kept, so a pointer over one stays over
    /// it.
    fn sync(self: &Rc<Self>, dash: &apps::Dash) {
        let wanted: Vec<&DashItem> = dash.favorites.iter().chain(&dash.running).collect();
        let shown = |item: &DashItem| (item.id.clone(), item.name.clone(), item.icon.clone());
        let mut items = self.items.borrow_mut();
        let unchanged = items.len() == wanted.len()
            && self.favorites.get() == dash.favorites.len()
            && items
                .iter()
                .zip(&wanted)
                .all(|(item, wanted)| item.shown == shown(wanted));
        if !unchanged {
            let mut old: Vec<Item> = std::mem::take(&mut items);
            for item in &old {
                self.apps.remove(&item.container);
            }
            self.apps.remove(&self.separator);
            for (index, wanted) in wanted.iter().enumerate() {
                if index == dash.favorites.len() {
                    self.apps.append(&self.separator);
                }
                let item = match old.iter().position(|item| item.shown == shown(wanted)) {
                    Some(index) => old.swap_remove(index),
                    None => self.build_item(wanted),
                };
                self.apps.append(&item.container);
                items.push(item);
            }
            self.favorites.set(dash.favorites.len());
        }
        // The separator only shows between two parts that both have
        // something in them.
        self.separator
            .set_visible(!dash.favorites.is_empty() && !dash.running.is_empty());
        for (item, wanted) in items.iter().zip(&wanted) {
            item.button.set_running(wanted.running());
        }
        drop(items);
        if !unchanged {
            // More or fewer items may want another icon size.
            let (width, height) = self.sized_for.get();
            self.set_max_size(width, height);
        }
    }

    fn build_item(self: &Rc<Self>, item: &DashItem) -> Item {
        let button = DashButton::new("overview-tile", self.icon_size.get());
        icon::set_app_icon(&button.image, item.icon.as_deref());
        button.button.set_widget_name(&item.name);
        button
            .button
            .update_property(&[gtk::accessible::Property::Label(&item.name)]);
        button.button.connect_clicked({
            let inner = Rc::downgrade(self);
            let id = item.id.clone();
            move |_| {
                let Some(inner) = inner.upgrade() else {
                    return;
                };
                // What the item stands for now: its windows may have
                // changed since it was built.
                if let Some(item) = inner.item(&id) {
                    super::activate_dash_item(&inner.services, &item);
                }
            }
        });
        Item {
            shown: (item.id.clone(), item.name.clone(), item.icon.clone()),
            container: container(&button.button),
            button,
        }
    }

    /// Sizes the icons for a dash that may be `max_width` by `max_height`
    /// (`Dash.setMaxSize` and `_adjustIconSize`).
    fn set_max_size(self: &Rc<Self>, max_width: i32, max_height: i32) {
        self.sized_for.set((max_width, max_height));
        let size = match self.services.config.dash.icon_size {
            Some(size) => size as i32,
            None if max_width <= 0 || max_height <= 0 => return,
            None => layout::dash_icon_size(
                max_width,
                max_height,
                // The button that opens the app grid counts.
                self.items.borrow().len() + 1,
                METRICS,
            ),
        };
        if self.icon_size.replace(size) == size {
            return;
        }
        // This is reached from the middle of a layout; what the layout
        // measured must not change under it.
        glib::idle_add_local_once({
            let inner = Rc::clone(self);
            move || inner.apply_icon_size()
        });
    }

    fn apply_icon_size(&self) {
        let size = self.icon_size.get();
        for item in self.items.borrow().iter() {
            item.button.image.set_pixel_size(size);
        }
        self.show_apps.image.set_pixel_size(size);
        self.separator_sizer.set_size_request(-1, size);
        self.sizer.set_size_request(-1, size + 2 * ICON_PADDING);
    }
}

impl Dash {
    /// Tells the dash how much room it may take.
    pub fn set_max_size(&self, max_width: i32, max_height: i32) {
        if self.inner.sized_for.get() != (max_width, max_height) {
            self.inner.set_max_size(max_width, max_height);
        }
    }

    /// The part that is drawn: what takes pointer input while the windows
    /// show around it.
    pub fn background(&self) -> gtk::Widget {
        self.inner
            .apps
            .ancestor(gtk::Overlay::static_type())
            .expect("the dash's items are in its overlay")
    }
}

pub fn build(services: &Rc<Services>) -> Dash {
    let icon_size = services
        .config
        .dash
        .icon_size
        .map_or(DEFAULT_ICON_SIZE, |size| size as i32);

    let apps = gtk::Box::builder().css_classes(["dash-apps"]).build();
    // A size asked of the separator itself would count its margins in; it
    // is asked of something inside it.
    let separator_sizer = gtk::Box::builder().height_request(icon_size).build();
    let separator = gtk::Box::builder()
        .css_classes(["dash-separator"])
        .valign(gtk::Align::Center)
        .build();
    separator.append(&separator_sizer);

    let show_apps = DashButton::new("show-apps", icon_size);
    show_apps
        .image
        .set_icon_name(Some("view-app-grid-symbolic"));
    show_apps.image.add_css_class("show-apps-icon");
    show_apps
        .button
        .update_property(&[gtk::accessible::Property::Label("Show Apps")]);
    show_apps.button.connect_clicked({
        let services = Rc::clone(services);
        move |_| services.overview.update(|overview| overview.toggle_apps())
    });

    let row = gtk::Box::builder()
        .css_classes(["dash-container"])
        .halign(gtk::Align::Center)
        .build();
    row.append(&apps);
    row.append(&container(&show_apps.button));

    let sizer = gtk::Box::builder()
        .height_request(icon_size + 2 * ICON_PADDING)
        .build();
    let background = gtk::Box::builder().css_classes(["dash-background"]).build();
    background.append(&sizer);
    let overlay = gtk::Overlay::builder()
        .child(&background)
        .halign(gtk::Align::Center)
        .hexpand(true)
        .build();
    overlay.add_overlay(&row);
    overlay.set_measure_overlay(&row, true);

    let widget = gtk::Box::builder().name("dash").build();
    widget.append(&overlay);

    let inner = Rc::new(Inner {
        services: Rc::clone(services),
        apps,
        separator,
        separator_sizer,
        sizer,
        show_apps,
        items: RefCell::new(Vec::new()),
        favorites: Cell::new(0),
        icon_size: Cell::new(icon_size),
        sized_for: Cell::new((0, 0)),
    });

    // The row follows the installed applications and the open windows.
    let refresh = {
        let inner = Rc::downgrade(&inner);
        move || {
            if let Some(inner) = inner.upgrade() {
                inner.sync(&inner.dash());
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
        // Checked while the app grid shows.
        services
            .overview
            .select(|overview| overview.apps && !overview.searching(), {
                let button = inner.show_apps.button.clone();
                move |checked| {
                    if *checked {
                        button.set_state_flags(gtk::StateFlags::CHECKED, false);
                    } else {
                        button.unset_state_flags(gtk::StateFlags::CHECKED);
                    }
                }
            }),
    ];

    Dash {
        widget,
        inner,
        _subscriptions: subscriptions,
    }
}
