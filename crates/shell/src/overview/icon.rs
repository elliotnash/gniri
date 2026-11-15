// SPDX-License-Identifier: GPL-3.0-or-later

//! The icons the overview is made of. Port of `BaseIcon` (iconGrid.js),
//! and of `AppIcon` and `AppViewItem` (appDisplay.js) without their menus
//! and without dragging.
//!
//! Widget trees, with the actors they stand for:
//!
//! ```text
//! button.overview-tile                AppIcon in the grid, GridSearchResult
//!   overlay
//!     GniriSquareBin.overview-icon.overview-icon-with-label   BaseIcon
//!       box
//!         image                         its StIcon
//!         label
//!     box.app-grid-running-dot
//!
//! button.overview-tile | button.show-apps     DashIcon, ShowAppsIcon's button
//!   overlay
//!     box.overview-icon                 BaseIcon without a label
//!       image
//!     box.app-grid-running-dot
//! ```

use gtk::prelude::*;
use gtk::{gdk, pango};

use crate::widgets::square::SquareBin;

/// What GNOME shows for an application without an icon of its own.
const FALLBACK_ICON: &str = "application-x-executable";

/// Shows an application's icon in `image`: `icon` is a name from the icon
/// theme or the path of a picture, as a desktop entry's `Icon` key is.
pub fn set_app_icon(image: &gtk::Image, icon: Option<&str>) {
    match icon {
        Some(path) if path.starts_with('/') => match gdk::Texture::from_filename(path) {
            Ok(texture) => image.set_paintable(Some(&texture)),
            Err(_) => image.set_icon_name(Some(FALLBACK_ICON)),
        },
        Some(name) if gtk::IconTheme::for_display(&image.display()).has_icon(name) => {
            image.set_icon_name(Some(name));
        }
        _ => image.set_icon_name(Some(FALLBACK_ICON)),
    }
}

/// An image `size` logical pixels wide and tall.
pub fn image(size: i32) -> gtk::Image {
    gtk::Image::builder()
        .pixel_size(size)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build()
}

/// The dot under the icon of an application that runs.
fn running_dot() -> gtk::Box {
    gtk::Box::builder()
        .css_classes(["app-grid-running-dot"])
        .halign(gtk::Align::Center)
        .valign(gtk::Align::End)
        .visible(false)
        .build()
}

/// An icon with its name under it: an application in the grid or among the
/// search results.
#[derive(Clone)]
pub struct Tile {
    pub button: gtk::Button,
    image: gtk::Image,
    label: gtk::Label,
    dot: gtk::Box,
}

impl Tile {
    pub fn new(icon_size: i32) -> Self {
        let image = image(icon_size);
        let label = gtk::Label::builder()
            .ellipsize(pango::EllipsizeMode::End)
            .justify(gtk::Justification::Center)
            // The square is as wide as its icon and label are tall; a long
            // name is cut short, it does not widen the tile.
            .max_width_chars(1)
            .build();
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        column.append(&image);
        column.append(&label);
        let icon = SquareBin::new(&column);
        icon.add_css_class("overview-icon");
        icon.add_css_class("overview-icon-with-label");

        let dot = running_dot();
        let overlay = gtk::Overlay::builder().child(&icon).build();
        overlay.add_overlay(&dot);
        let button = gtk::Button::builder()
            .css_classes(["overview-tile"])
            .child(&overlay)
            .focusable(false)
            .build();
        Self {
            button,
            image,
            label,
            dot,
        }
    }

    pub fn set(&self, name: &str, icon: Option<&str>) {
        self.label.set_label(name);
        set_app_icon(&self.image, icon);
        self.button.set_widget_name(name);
    }

    pub fn set_icon_size(&self, size: i32) {
        self.image.set_pixel_size(size);
    }

    pub fn set_running(&self, running: bool) {
        self.dot.set_visible(running);
    }

    /// The look of the item the keyboard is on (`:selected` in St).
    pub fn set_selected(&self, selected: bool) {
        set_class(&self.button, "selected", selected);
    }
}

/// An icon alone: an item of the dash.
#[derive(Clone)]
pub struct DashButton {
    pub button: gtk::Button,
    pub image: gtk::Image,
    dot: gtk::Box,
}

impl DashButton {
    /// `class` is `overview-tile` for an application and `show-apps` for
    /// the button that opens the app grid.
    pub fn new(class: &str, icon_size: i32) -> Self {
        let image = image(icon_size);
        let icon = gtk::Box::builder()
            .css_classes(["overview-icon"])
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        icon.append(&image);
        let dot = running_dot();
        let overlay = gtk::Overlay::builder().child(&icon).build();
        overlay.add_overlay(&dot);
        let button = gtk::Button::builder()
            .css_classes([class])
            .child(&overlay)
            .focusable(false)
            .build();
        Self { button, image, dot }
    }

    pub fn set_running(&self, running: bool) {
        self.dot.set_visible(running);
    }
}

/// Adds or removes a style class.
pub fn set_class(widget: &impl IsA<gtk::Widget>, class: &str, on: bool) {
    if on {
        widget.add_css_class(class);
    } else {
        widget.remove_css_class(class);
    }
}
