// SPDX-License-Identifier: GPL-3.0-or-later

//! The dialog an app folder opens in: its name, which can be changed, and
//! its applications, three by three. Port of `AppFolderDialog`
//! (appDisplay.js), with blur-my-shell's blur behind it.
//!
//! Widget tree, with the actors it stands for:
//!
//! ```text
//! box.app-folder-dialog-container       AppFolderDialog, and its shade
//!   GniriBlurBehind                     blur-my-shell's blur of the dialog
//!     box.app-folder-dialog             _viewBox
//!       box.folder-name-container         _entryBox
//!         box                               the empty actor that centres the name
//!         stack                             Shell.Stack
//!           label.folder-name-label
//!           entry.folder-name-entry
//!         togglebutton.icon-button          _editButton
//!       box.app-display                   the FolderView (overview/app_grid.rs)
//! ```
//!
//! blur-my-shell blurs whatever is behind the dialog, the app grid
//! included. The grid is on gniri's own surface, which the compositor
//! cannot blur under another part of the same surface, so
//! `GniriBlurBehind` draws the overview's parts again behind the dialog,
//! blurred and dimmed as the extension's blur would have them.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gniri_core::{Subscription, tr};
use gniri_services::apps::GridItem;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{glib, graphene, gsk};

use super::app_grid::{self, AppView, View};
use crate::services::Services;

/// appDisplay.js `FOLDER_DIALOG_ANIMATION_TIME`; the theme's transitions
/// last as long.
const ANIMATION_TIME: Duration = Duration::from_millis(200);
/// blur-my-shell's default blur as a GSK blur radius (twice the Gaussian
/// sigma of 7.5: docs/DECISIONS.md, S14), and its brightness.
const BLUR_RADIUS: f32 = 15.0;
const BLUR_BRIGHTNESS: f32 = 0.6;
/// `DIALOG_SHADE_NORMAL` and `DIALOG_SHADE_HIGHLIGHT`, black at 204 and 85
/// of 255, which blur-my-shell turns down to a quarter
/// (`FOLDER_DIALOG_SHADE_FACTOR`). The theme has the same two colours.
const SHADE: f32 = 51.0 / 255.0;
const SHADE_LIGHTER: f32 = 21.0 / 255.0;
/// `.app-folder-dialog`'s size and corners.
const DIALOG_SIZE: i32 = 720;
const DIALOG_RADIUS: f32 = 64.0;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct BlurBehind {
        /// What is behind the dialog, drawn again under it.
        pub(super) backdrop: RefCell<Vec<gtk::Widget>>,
        /// The shade over those is the lighter one.
        pub(super) lighter: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for BlurBehind {
        const NAME: &'static str = "GniriBlurBehind";
        type Type = super::BlurBehind;
        type ParentType = gtk::Widget;

        fn class_init(class: &mut Self::Class) {
            class.set_layout_manager_type::<gtk::BinLayout>();
        }
    }

    impl ObjectImpl for BlurBehind {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for BlurBehind {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let (width, height) = (obj.width() as f32, obj.height() as f32);
            if width > 0.0 && height > 0.0 {
                let bounds = graphene::Rect::new(0.0, 0.0, width, height);
                let corner = graphene::Size::new(DIALOG_RADIUS, DIALOG_RADIUS);
                let rounded = gsk::RoundedRect::new(bounds, corner, corner, corner, corner);
                snapshot.push_rounded_clip(&rounded);
                snapshot.push_blur(f64::from(BLUR_RADIUS));
                for widget in self.backdrop.borrow().iter() {
                    if !widget.is_drawable() {
                        continue;
                    }
                    let Some(origin) = widget.compute_point(&*obj, &graphene::Point::new(0.0, 0.0))
                    else {
                        continue;
                    };
                    let paintable = gtk::WidgetPaintable::new(Some(widget));
                    snapshot.save();
                    snapshot.translate(&origin);
                    paintable.snapshot(
                        snapshot,
                        f64::from(widget.width()),
                        f64::from(widget.height()),
                    );
                    snapshot.restore();
                }
                snapshot.pop();
                // The shade over the overview, then the blur's dimming.
                let shade = if self.lighter.get() {
                    SHADE_LIGHTER
                } else {
                    SHADE
                };
                let kept = (1.0 - shade) * BLUR_BRIGHTNESS;
                snapshot.append_color(&gtk::gdk::RGBA::new(0.0, 0.0, 0.0, 1.0 - kept), &bounds);
                snapshot.pop();
            }
            self.parent_snapshot(snapshot);
        }
    }
}

glib::wrapper! {
    pub struct BlurBehind(ObjectSubclass<imp::BlurBehind>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl BlurBehind {
    fn new(child: &impl IsA<gtk::Widget>) -> Self {
        let widget: Self = glib::Object::new();
        child.set_parent(&widget);
        widget
    }
}

struct Inner {
    services: Rc<Services>,
    container: gtk::Box,
    blur: BlurBehind,
    dialog: gtk::Box,
    label: gtk::Label,
    entry: gtk::Entry,
    stack: gtk::Stack,
    edit: gtk::ToggleButton,
    /// Counts opens and closes, so a close overtaken by an open does not
    /// hide the dialog.
    generation: Cell<u64>,
    /// Gives the keyboard back to the search entry.
    give_back_focus: RefCell<Option<Box<dyn Fn()>>>,
}

pub struct FolderDialog {
    /// The container, which covers the overview.
    pub widget: gtk::Widget,
    pub view: AppView,
    inner: Rc<Inner>,
    _subscriptions: Vec<Subscription>,
}

impl Inner {
    /// The open folder's id and name.
    fn folder(&self) -> Option<(String, String)> {
        let id = self
            .services
            .overview
            .with(|overview| overview.folder.clone())?;
        let favorites = self.services.favorites();
        let name = self.services.apps.state.with(|apps| {
            apps.grid_items(&favorites)
                .into_iter()
                .find_map(|item| match item {
                    GridItem::Folder(folder) if folder.id == id => Some(folder.name),
                    _ => None,
                })
        })?;
        Some((id, name))
    }

    fn open(self: &Rc<Self>) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        self.show_label(false);
        self.container.set_visible(true);
        self.container.set_can_target(true);
        // `popup` focuses the edit button: the keyboard is the dialog's,
        // and typing starts no search under it.
        self.edit.grab_focus();
        // On screen in its closed look first, then the transition.
        glib::idle_add_local_once({
            let inner = Rc::clone(self);
            move || {
                if inner.generation.get() == generation {
                    inner.container.add_css_class("open");
                }
            }
        });
    }

    fn close(self: &Rc<Self>) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        if !self.container.is_visible() {
            return;
        }
        self.set_lighter(false);
        self.container.remove_css_class("open");
        // Nothing of it takes the pointer while it fades.
        self.container.set_can_target(false);
        if let Some(give_back) = self.give_back_focus.borrow().as_ref() {
            give_back();
        }
        let hide = {
            let inner = Rc::clone(self);
            move || {
                if inner.generation.get() == generation {
                    inner.container.set_visible(false);
                }
            }
        };
        let animated =
            gtk::Settings::default().is_some_and(|settings| settings.is_gtk_enable_animations());
        if animated {
            glib::timeout_add_local_once(ANIMATION_TIME, hide);
        } else {
            hide();
        }
    }

    fn sync_name(&self) {
        if let Some((_, name)) = self.folder() {
            self.label.set_label(&name);
            if !self.edit.is_active() {
                self.entry.set_text(&name);
            }
        }
    }

    /// `_showFolderEntry`: the name can be changed.
    fn show_entry(&self) {
        self.stack.set_visible_child(&self.entry);
        self.entry.grab_focus();
        self.entry.select_region(0, -1);
    }

    /// `_showFolderLabel`, which takes a changed name into use.
    fn show_label(&self, apply: bool) {
        if self.edit.is_active() {
            // Comes back here through the toggle's signal.
            self.edit.set_active(false);
            return;
        }
        if apply && let Some((id, name)) = self.folder() {
            let new = self.entry.text().trim().to_owned();
            if !new.is_empty() && new != name {
                let layout = self
                    .services
                    .apps
                    .state
                    .with(|apps| apps.rename_folder(&id, &new));
                self.services.arrange(layout);
            }
        }
        self.stack.set_visible_child(&self.label);
        if self.container.is_visible() {
            self.edit.grab_focus();
        }
    }

    /// The shade is lighter while something dragged out of the dialog is
    /// outside it (`_setLighterBackground`).
    fn set_lighter(&self, lighter: bool) {
        if self.blur.imp().lighter.replace(lighter) != lighter {
            set_class(&self.container, "lighter", lighter);
            self.blur.queue_draw();
        }
    }
}

fn set_class(widget: &impl IsA<gtk::Widget>, class: &str, on: bool) {
    super::icon::set_class(widget, class, on);
}

impl FolderDialog {
    /// Whether the dialog's box is at `(x, y)` of `relative_to`.
    pub fn contains(&self, relative_to: &impl IsA<gtk::Widget>, (x, y): (f64, f64)) -> bool {
        let Some(point) = relative_to.as_ref().compute_point(
            &self.inner.dialog,
            &graphene::Point::new(x as f32, y as f32),
        ) else {
            return false;
        };
        let (x, y) = (f64::from(point.x()), f64::from(point.y()));
        x >= 0.0
            && y >= 0.0
            && x <= f64::from(self.inner.dialog.width())
            && y <= f64::from(self.inner.dialog.height())
    }

    /// Whether the dialog shows.
    pub fn is_open(&self) -> bool {
        self.inner.container.is_visible() && self.inner.container.has_css_class("open")
    }

    pub fn set_lighter(&self, lighter: bool) {
        self.inner.set_lighter(lighter);
    }

    /// Whether the folder's name is being changed: the keyboard is the
    /// name's then.
    pub fn renaming(&self) -> bool {
        self.inner.edit.is_active()
    }

    /// Stops changing the name, keeping it as it was.
    pub fn cancel_rename(&self) {
        self.inner.sync_name();
        if let Some((_, name)) = self.inner.folder() {
            self.inner.entry.set_text(&name);
        }
        self.inner.edit.set_active(false);
    }

    /// What is drawn behind the dialog, blurred: the overview's parts, in
    /// the order they are drawn.
    pub fn set_backdrop(&self, widgets: Vec<gtk::Widget>) {
        self.inner.blur.imp().backdrop.replace(widgets);
    }

    /// Run when the dialog gives the keyboard back.
    pub fn on_focus_back(&self, give_back: impl Fn() + 'static) {
        self.inner
            .give_back_focus
            .replace(Some(Box::new(give_back)));
    }

    /// Fits the dialog in `(width, height)`: 720 by 720 where there is
    /// room.
    pub fn fit(&self, width: i32, height: i32) {
        let side = |room: i32| DIALOG_SIZE.min(room).max(0);
        let (width, height) = (side(width), side(height));
        if self.inner.blur.size_request() != (width, height) {
            self.inner.blur.set_size_request(width, height);
        }
    }
}

pub fn build(services: &Rc<Services>) -> FolderDialog {
    let view = app_grid::build(services, View::Folder);
    view.widget.set_vexpand(true);

    let label = gtk::Label::builder()
        .css_classes(["folder-name-label"])
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    let entry = gtk::Entry::builder()
        .css_classes(["folder-name-entry"])
        .xalign(0.5)
        .build();
    entry.update_property(&[gtk::accessible::Property::Label(&tr("Folder name"))]);
    let stack = gtk::Stack::builder()
        .hexpand(true)
        .halign(gtk::Align::Center)
        .transition_type(gtk::StackTransitionType::Crossfade)
        .transition_duration(300)
        .build();
    stack.add_child(&label);
    stack.add_child(&entry);
    let edit = gtk::ToggleButton::builder()
        .css_classes(["icon-button"])
        .icon_name("document-edit-symbolic")
        .valign(gtk::Align::Center)
        .build();
    edit.update_property(&[gtk::accessible::Property::Label(&tr("Edit folder name"))]);
    // `ghostButton`: as large as the edit button, so the name is centred.
    let ghost = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let ghost_size = gtk::SizeGroup::new(gtk::SizeGroupMode::Both);
    ghost_size.add_widget(&ghost);
    ghost_size.add_widget(&edit);
    let name_row = gtk::Box::builder()
        .css_classes(["folder-name-container"])
        .build();
    name_row.append(&ghost);
    name_row.append(&stack);
    name_row.append(&edit);

    let dialog = gtk::Box::builder()
        .css_classes(["app-folder-dialog"])
        .orientation(gtk::Orientation::Vertical)
        .build();
    dialog.append(&name_row);
    dialog.append(&view.widget);
    let blur = BlurBehind::new(&dialog);
    blur.set_halign(gtk::Align::Center);
    blur.set_valign(gtk::Align::Center);
    blur.set_size_request(DIALOG_SIZE, DIALOG_SIZE);
    blur.set_overflow(gtk::Overflow::Hidden);

    let container = gtk::Box::builder()
        .css_classes(["app-folder-dialog-container"])
        .visible(false)
        .build();
    container.append(&blur);
    blur.set_hexpand(true);
    blur.set_vexpand(true);

    let inner = Rc::new(Inner {
        services: Rc::clone(services),
        container: container.clone(),
        blur,
        dialog: dialog.clone(),
        label,
        entry: entry.clone(),
        stack,
        edit: edit.clone(),
        generation: Cell::new(0),
        give_back_focus: RefCell::new(None),
    });

    edit.connect_toggled({
        let inner = Rc::downgrade(&inner);
        move |edit| {
            if let Some(inner) = inner.upgrade() {
                if edit.is_active() {
                    inner.show_entry();
                } else {
                    inner.show_label(true);
                }
            }
        }
    });
    entry.connect_activate({
        let inner = Rc::downgrade(&inner);
        move |_| {
            if let Some(inner) = inner.upgrade() {
                inner.show_label(true);
            }
        }
    });

    // A click beside the dialog closes it (`clickGesture`).
    let click = gtk::GestureClick::new();
    click.connect_released({
        let (inner, dialog) = (Rc::downgrade(&inner), dialog.clone());
        move |gesture, _, x, y| {
            let Some(inner) = inner.upgrade() else {
                return;
            };
            let container = &inner.container;
            let inside = container
                .compute_point(&dialog, &graphene::Point::new(x as f32, y as f32))
                .is_some_and(|point| {
                    point.x() >= 0.0
                        && point.y() >= 0.0
                        && point.x() <= dialog.width() as f32
                        && point.y() <= dialog.height() as f32
                });
            if !inside {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                inner
                    .services
                    .overview
                    .update(|overview| overview.close_folder());
            }
        }
    });
    container.add_controller(click);

    let subscriptions = vec![
        services
            .overview
            .select(|overview| overview.folder.clone(), {
                let inner = Rc::clone(&inner);
                move |folder: &Option<String>| {
                    if folder.is_some() {
                        inner.sync_name();
                        inner.open();
                    } else {
                        inner.close();
                    }
                }
            }),
        services
            .apps
            .state
            .select(|apps| (apps.installed.clone(), apps.layout.clone()), {
                let inner = Rc::downgrade(&inner);
                move |_| {
                    let Some(inner) = inner.upgrade() else {
                        return;
                    };
                    // A folder that has gone (its last application was
                    // taken out or uninstalled) closes.
                    if inner
                        .services
                        .overview
                        .with(|overview| overview.folder.is_some())
                        && inner.folder().is_none()
                    {
                        inner
                            .services
                            .overview
                            .update(|overview| overview.close_folder());
                    } else {
                        inner.sync_name();
                    }
                }
            }),
    ];

    FolderDialog {
        widget: container.upcast(),
        view,
        inner,
        _subscriptions: subscriptions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dialog's size and corners, and its shade, are in the code (it
    /// draws the blur behind the dialog itself); the theme has the same.
    #[test]
    fn the_dialog_is_drawn_as_the_theme_has_it() {
        assert_eq!(
            gniri_theme::token("app-folder-size"),
            Some(format!("{DIALOG_SIZE}px").as_str())
        );
        assert_eq!(
            gniri_theme::token("app-folder-radius"),
            Some(format!("{DIALOG_RADIUS}px").as_str())
        );
        let alpha = |name: &str| -> f32 {
            let token = gniri_theme::token(name).unwrap_or_else(|| panic!("no token {name}"));
            token
                .trim_end_matches(')')
                .rsplit(',')
                .next()
                .and_then(|alpha| alpha.trim().parse().ok())
                .unwrap_or_else(|| panic!("{name} is no rgba colour: {token}"))
        };
        assert!((alpha("bms-folder-shade") - SHADE).abs() < 1e-3);
        assert!((alpha("bms-folder-shade-lighter") - SHADE_LIGHTER).abs() < 1e-3);
    }
}
