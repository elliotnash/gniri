// SPDX-License-Identifier: GPL-3.0-or-later

//! The dash: the favourite and the running applications in a row at the
//! bottom of the overview, and the button that opens the app grid. Port of
//! `Dash` and `ShowAppsIcon` in `js/ui/dash.js`, without the items' menus.
//! Dragging an item, or an application to the dash, is `overview/drag.rs`;
//! this module only shows where it would go.
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
//!         box.dash-item-container         DragPlaceholderItem, during a drag
//!           box.placeholder
//!         box.dash-separator
//!       box.dash-item-container         ShowAppsIcon
//!         button.show-apps
//! label.dash-label                      an item's name, while pointed at
//! ```
//!
//! The label is not in the dash: it floats above it, and whoever lays the
//! overview out places it ([`Dash::label_anchor`]).
//!
//! An item reaches from the top of the dash to the bottom of the screen,
//! so that it is hit by a pointer pushed against the edge; the background
//! ends above the edge. St sizes the background from the container with a
//! constraint; here the two share a `GtkOverlay` that is measured by the
//! container.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gniri_core::{Subscription, tr};
use gniri_services::apps::{self, DashItem};
use gtk::prelude::*;
use gtk::{glib, graphene};

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

/// dash.js `DASH_ITEM_HOVER_TIMEOUT`: how long the pointer rests on an
/// item before its name shows, and how long after leaving one the next
/// item's name still shows at once.
const HOVER_TIMEOUT: Duration = Duration::from_millis(300);
/// dash.js `DASH_ITEM_LABEL_HIDE_TIME`; the theme's transition lasts as
/// long.
const LABEL_HIDE_TIME: Duration = Duration::from_millis(100);

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
    label: gtk::Label,
    /// The item the pointer is on, once its name shows.
    labelled: RefCell<Option<gtk::Box>>,
    /// Runs out when the name of the item under the pointer is to show.
    label_timer: RefCell<Option<glib::SourceId>>,
    /// A name showed a moment ago: the next shows without waiting
    /// (`Dash._labelShowing`).
    label_showing: Cell<bool>,
    /// Counts shows and hides, so one that was overtaken does nothing.
    label_generation: Cell<u64>,
    /// Where something dragged would go among the favourites, with the room
    /// made for it there.
    placeholder: RefCell<Option<(usize, gtk::Box)>>,
    /// An application that is a favourite is over the show-apps button:
    /// letting go of it unpins it.
    unpin: Cell<bool>,
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
    /// Shows `name` above `container` while the pointer is on `button`
    /// (`Dash._hookUpLabel` and `_syncLabel`).
    fn hook_label(self: &Rc<Self>, container: &gtk::Box, button: &gtk::Button, name: &str) {
        let motion = gtk::EventControllerMotion::new();
        motion.connect_enter({
            let (inner, container, name) =
                (Rc::downgrade(self), container.downgrade(), name.to_owned());
            move |_, _, _| {
                let (Some(inner), Some(container)) = (inner.upgrade(), container.upgrade()) else {
                    return;
                };
                let delay = if inner.label_showing.get() {
                    Duration::ZERO
                } else {
                    HOVER_TIMEOUT
                };
                let name = name.clone();
                let timer = glib::timeout_add_local_once(delay, {
                    let inner = Rc::clone(&inner);
                    move || {
                        inner.label_timer.take();
                        inner.show_label(&container, &name);
                    }
                });
                if let Some(old) = inner.label_timer.replace(Some(timer)) {
                    old.remove();
                }
            }
        });
        let leave = {
            let inner = Rc::downgrade(self);
            move || {
                if let Some(inner) = inner.upgrade() {
                    inner.hide_label();
                }
            }
        };
        motion.connect_leave({
            let leave = leave.clone();
            move |_| leave()
        });
        button.add_controller(motion);
        // A click is an answer; the name has done its work.
        button.connect_clicked(move |_| leave());
    }

    fn show_label(&self, container: &gtk::Box, name: &str) {
        self.label_generation.set(self.label_generation.get() + 1);
        self.label_showing.set(true);
        self.labelled.replace(Some(container.clone()));
        self.label.set_label(name);
        self.label.set_visible(true);
        self.label.add_css_class("shown");
        // It may be showing already, over another item.
        if let Some(parent) = self.label.parent() {
            parent.queue_allocate();
        }
    }

    fn hide_label(self: &Rc<Self>) {
        if let Some(timer) = self.label_timer.take() {
            timer.remove();
        }
        if self.labelled.borrow().is_none() {
            return;
        }
        let generation = self.label_generation.get() + 1;
        self.label_generation.set(generation);
        self.label.remove_css_class("shown");
        let overtaken = {
            let inner = Rc::downgrade(self);
            move || {
                inner
                    .upgrade()
                    .filter(|inner| inner.label_generation.get() == generation)
            }
        };
        // Gone once it has faded.
        let hide = {
            let overtaken = overtaken.clone();
            move || {
                if let Some(inner) = overtaken() {
                    inner.label.set_visible(false);
                    inner.labelled.take();
                }
            }
        };
        let animated =
            gtk::Settings::default().is_some_and(|settings| settings.is_gtk_enable_animations());
        if animated {
            glib::timeout_add_local_once(LABEL_HIDE_TIME, hide);
        } else {
            hide();
        }
        glib::timeout_add_local_once(HOVER_TIMEOUT, move || {
            if let Some(inner) = overtaken() {
                inner.label_showing.set(false);
            }
        });
    }

    /// The dash as it should be now.
    fn dash(&self) -> apps::Dash {
        let favorites = self.services.favorites();
        self.services.niri.with(|niri| {
            self.services
                .apps
                .state
                .with(|apps| apps.dash(&favorites, &niri.windows))
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

    /// Makes room for something dragged at `position` among the items of
    /// the dash, or takes the room away (`DragPlaceholderItem`).
    fn set_placeholder(&self, position: Option<usize>) {
        if self.placeholder.borrow().as_ref().map(|(at, _)| *at) == position {
            return;
        }
        if let Some((_, old)) = self.placeholder.take() {
            self.apps.remove(&old);
        }
        let Some(position) = position else {
            return;
        };
        let size = self.icon_size.get();
        let gap = gtk::Box::builder()
            .css_classes(["placeholder"])
            .width_request(size)
            .height_request(size / 2)
            .valign(gtk::Align::Center)
            .build();
        let holder = container(&gap);
        // After the item that comes before it, the separator not counted.
        let items = self.items.borrow();
        match position.checked_sub(1).and_then(|before| items.get(before)) {
            Some(before) => self
                .apps
                .insert_child_after(&holder, Some(&before.container)),
            None => self.apps.prepend(&holder),
        }
        drop(items);
        self.placeholder.replace(Some((position, holder)));
    }

    /// Makes the row show `dash` (`Dash._redisplay`). The items of
    /// applications that stay are kept, so a pointer over one stays over
    /// it.
    fn sync(self: &Rc<Self>, dash: &apps::Dash) {
        self.set_placeholder(None);
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
            // The item a name shows over may be about to go.
            self.hide_label();
            let mut old: Vec<Item> = std::mem::take(&mut items);
            for item in &old {
                self.apps.remove(&item.container);
            }
            if self.separator.parent().is_some() {
                self.apps.remove(&self.separator);
            }
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
        let container = container(&button.button);
        self.hook_label(&container, &button.button, &item.name);
        Item {
            shown: (item.id.clone(), item.name.clone(), item.icon.clone()),
            container,
            button,
        }
    }

    /// Sizes the icons for a dash that may be `max_width` by `max_height`
    /// (`Dash.setMaxSize` and `_adjustIconSize`).
    fn set_max_size(self: &Rc<Self>, max_width: i32, max_height: i32) {
        self.sized_for.set((max_width, max_height));
        let size = match self.services.config.with(|config| config.dash.icon_size) {
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

    /// The label that names the item under the pointer. It is laid out by
    /// the overview, over the dash.
    pub fn label(&self) -> gtk::Label {
        self.inner.label.clone()
    }

    /// Where the item is whose name the label shows, in `relative_to`'s
    /// coordinates: its left edge, its width and its top
    /// (`DashItemContainer.showLabel` centres the label over these).
    pub fn label_anchor(&self, relative_to: &impl IsA<gtk::Widget>) -> Option<(i32, i32, i32)> {
        let labelled = self.inner.labelled.borrow();
        let bounds = labelled.as_ref()?.compute_bounds(relative_to)?;
        Some((
            bounds.x().floor() as i32,
            bounds.width().round() as i32,
            bounds.y().floor() as i32,
        ))
    }

    /// The application of the item whose button `widget` is or is inside,
    /// and whether it is a favourite.
    pub fn item_of(&self, widget: &gtk::Widget) -> Option<(String, bool)> {
        let items = self.inner.items.borrow();
        let favorites = self.inner.favorites.get();
        let mut current = Some(widget.clone());
        while let Some(widget) = current {
            if let Some(index) = items
                .iter()
                .position(|item| item.button.button.upcast_ref::<gtk::Widget>() == &widget)
            {
                return Some((items[index].shown.0.clone(), index < favorites));
            }
            current = widget.parent();
        }
        None
    }

    /// The button of the item of `app`, to hide while it is dragged.
    pub fn button_of(&self, app: &str) -> Option<gtk::Button> {
        self.inner
            .items
            .borrow()
            .iter()
            .find(|item| item.shown.0 == app)
            .map(|item| item.button.button.clone())
    }

    /// The size of the dash's icons.
    pub fn icon_size(&self) -> i32 {
        self.inner.icon_size.get()
    }

    /// Where among the favourites something dragged to `(x, y)` of
    /// `relative_to` would go, if it is over the dash's items
    /// (`Dash.handleDragOver`): a place in the row as it shows, at most
    /// after the last favourite. `own` is where the dragged application is
    /// among the favourites, if it is one: just before or after it is
    /// where it already is, and no place.
    pub fn place_at(
        &self,
        relative_to: &impl IsA<gtk::Widget>,
        (x, y): (f64, f64),
        own: Option<usize>,
    ) -> Option<Option<usize>> {
        let inner = &self.inner;
        let background = self.background();
        let point = relative_to
            .as_ref()
            .compute_point(&background, &graphene::Point::new(x as f32, y as f32))?;
        let inside = point.x() >= 0.0
            && point.y() >= 0.0
            && point.x() <= background.width() as f32
            && point.y() <= background.height() as f32;
        if !inside {
            return None;
        }
        // Over the show-apps button: that is no place in the row.
        let over_show_apps = relative_to
            .as_ref()
            .compute_point(
                &inner.show_apps.button,
                &graphene::Point::new(x as f32, y as f32),
            )
            .is_some_and(|point| {
                point.x() >= 0.0 && point.x() <= inner.show_apps.button.width() as f32
            });
        if over_show_apps {
            return None;
        }
        let box_x = relative_to
            .as_ref()
            .compute_point(&inner.apps, &graphene::Point::new(x as f32, y as f32))?
            .x();
        let items = inner.items.borrow();
        let count = items.len();
        // The room for the placeholder and the separator left out.
        let width: i32 = items.iter().map(|item| item.container.width()).sum();
        let favorites = inner.favorites.get();
        let position = if count == 0 || width <= 0 {
            0
        } else {
            let position = (f64::from(box_x) * count as f64 / f64::from(width)).floor();
            (position.max(0.0) as usize).min(favorites)
        };
        if own.is_some_and(|own| position == own || position == own + 1) {
            return Some(None);
        }
        Some(Some(position))
    }

    /// Shows the room for something dragged at `position`, or none.
    pub fn show_placeholder(&self, position: Option<usize>) {
        self.inner.set_placeholder(position);
    }

    /// Whether `(x, y)` of `relative_to` is on the show-apps button.
    pub fn over_show_apps(&self, relative_to: &impl IsA<gtk::Widget>, (x, y): (f64, f64)) -> bool {
        let button = &self.inner.show_apps.button;
        relative_to
            .as_ref()
            .compute_point(button, &graphene::Point::new(x as f32, y as f32))
            .is_some_and(|point| {
                point.x() >= 0.0
                    && point.y() >= 0.0
                    && point.x() <= button.width() as f32
                    && point.y() <= button.height() as f32
            })
    }

    /// A favourite dragged over the show-apps button would be unpinned
    /// there: the button is drawn hovered and named "Unpin"
    /// (`ShowAppsIcon.setDragApp`).
    pub fn set_unpin(&self, unpin: bool) {
        let inner = &self.inner;
        if inner.unpin.replace(unpin) == unpin {
            return;
        }
        let button = &inner.show_apps.button;
        if unpin {
            button.set_state_flags(gtk::StateFlags::PRELIGHT, false);
        } else {
            button.unset_state_flags(gtk::StateFlags::PRELIGHT);
        }
        let container = button
            .parent()
            .and_then(|parent| parent.downcast::<gtk::Box>().ok());
        if let Some(container) = container {
            if unpin {
                inner.show_label(&container, &tr("Unpin"));
            } else {
                inner.hide_label();
            }
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
        .with(|config| config.dash.icon_size)
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
        .update_property(&[gtk::accessible::Property::Label(&tr("Show Apps"))]);
    show_apps.button.connect_clicked({
        let services = Rc::clone(services);
        move |_| services.overview.update(|overview| overview.toggle_apps())
    });

    let show_apps_container = container(&show_apps.button);
    let label = gtk::Label::builder()
        .css_classes(["dash-label"])
        .visible(false)
        .can_target(false)
        .build();

    let row = gtk::Box::builder()
        .css_classes(["dash-container"])
        .halign(gtk::Align::Center)
        .build();
    row.append(&apps);
    row.append(&show_apps_container);

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
        label,
        labelled: RefCell::new(None),
        label_timer: RefCell::new(None),
        label_showing: Cell::new(false),
        label_generation: Cell::new(0),
        placeholder: RefCell::new(None),
        unpin: Cell::new(false),
    });
    inner.hook_label(
        &show_apps_container,
        &inner.show_apps.button,
        &tr("Show Apps"),
    );

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
        services.apps.state.select(
            |apps| (apps.installed.clone(), apps.layout.favorites.clone()),
            {
                let refresh = refresh.clone();
                move |_| refresh()
            },
        ),
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
