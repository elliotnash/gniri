// SPDX-License-Identifier: GPL-3.0-or-later

//! The icons the overview is made of. Port of `BaseIcon` (iconGrid.js),
//! and of `AppIcon`, `FolderIcon` and `AppViewItem` (appDisplay.js)
//! without their menus. Dragging them is `overview/drag.rs`.
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
//! button.overview-tile.app-folder     FolderIcon
//!   overlay
//!     GniriSquareBin.overview-icon.overview-icon-with-label
//!       box
//!         grid.folder-icon              FolderView.createFolderIcon
//!           image ...                     up to four of its applications
//!         label
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

/// appDisplay.js `FOLDER_SUBICON_FRACTION`: how large the icons in a
/// folder's icon are, of the folder's.
const FOLDER_SUBICON_FRACTION: f64 = 0.4;

/// A folder's icon: four of its applications' icons, two by two, in a
/// square as large as an application's icon.
#[derive(Clone)]
pub struct FolderIcon {
    pub grid: gtk::Grid,
    images: [gtk::Image; 4],
}

impl FolderIcon {
    fn new(size: i32) -> Self {
        let grid = gtk::Grid::builder()
            .css_classes(["folder-icon"])
            .row_homogeneous(true)
            .column_homogeneous(true)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        let images: [gtk::Image; 4] = std::array::from_fn(|_| image(0));
        for (index, image) in images.iter().enumerate() {
            let rtl = grid.direction() == gtk::TextDirection::Rtl;
            let column = if rtl { (index + 1) % 2 } else { index % 2 };
            grid.attach(image, column as i32, (index / 2) as i32, 1, 1);
        }
        let icon = Self { grid, images };
        icon.set_size(size);
        icon
    }

    fn set_size(&self, size: i32) {
        self.grid.set_size_request(size, size);
        let sub_size = (FOLDER_SUBICON_FRACTION * f64::from(size)).floor() as i32;
        for image in &self.images {
            image.set_pixel_size(sub_size);
        }
    }

    /// Shows the first four of these icons; the rest of the square stays
    /// empty.
    pub fn set_icons(&self, icons: &[Option<String>]) {
        for (index, image) in self.images.iter().enumerate() {
            match icons.get(index) {
                Some(icon) => {
                    set_app_icon(image, icon.as_deref());
                    image.set_child_visible(true);
                }
                None => image.set_child_visible(false),
            }
        }
    }
}

/// What a tile shows above its name.
#[derive(Clone)]
enum TileIcon {
    App(gtk::Image),
    Folder(FolderIcon),
}

/// An icon with its name under it: an application in the grid or among the
/// search results, or a folder.
#[derive(Clone)]
pub struct Tile {
    pub button: gtk::Button,
    icon: TileIcon,
    label: gtk::Label,
    dot: gtk::Box,
}

impl Tile {
    pub fn new(icon_size: i32) -> Self {
        let image = image(icon_size);
        Self::with_icon(TileIcon::App(image))
    }

    /// A folder's tile.
    pub fn new_folder(icon_size: i32) -> Self {
        let tile = Self::with_icon(TileIcon::Folder(FolderIcon::new(icon_size)));
        tile.button.add_css_class("app-folder");
        tile
    }

    fn with_icon(icon: TileIcon) -> Self {
        let widget: gtk::Widget = match &icon {
            TileIcon::App(image) => image.clone().upcast(),
            TileIcon::Folder(folder) => folder.grid.clone().upcast(),
        };
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
        column.append(&widget);
        column.append(&label);
        let square = SquareBin::new(&column);
        square.add_css_class("overview-icon");
        square.add_css_class("overview-icon-with-label");

        let dot = running_dot();
        let overlay = gtk::Overlay::builder().child(&square).build();
        overlay.add_overlay(&dot);
        let button = gtk::Button::builder()
            .css_classes(["overview-tile"])
            .child(&overlay)
            .focusable(false)
            .build();
        Self {
            button,
            icon,
            label,
            dot,
        }
    }

    pub fn set(&self, name: &str, icon: Option<&str>) {
        self.label.set_label(name);
        if let TileIcon::App(image) = &self.icon {
            set_app_icon(image, icon);
        }
        self.button.set_widget_name(name);
    }

    /// A folder's name and the icons of its first applications.
    pub fn set_folder(&self, name: &str, icons: &[Option<String>]) {
        self.label.set_label(name);
        if let TileIcon::Folder(folder) = &self.icon {
            folder.set_icons(icons);
        }
        self.button.set_widget_name(name);
    }

    pub fn set_icon_size(&self, size: i32) {
        match &self.icon {
            TileIcon::App(image) => image.set_pixel_size(size),
            TileIcon::Folder(folder) => folder.set_size(size),
        }
    }

    /// What follows the pointer while the tile is dragged: its icon, at
    /// `size`.
    pub fn drag_image(&self, size: i32) -> gtk::Widget {
        match &self.icon {
            TileIcon::App(image) => {
                let copy = super::icon::image(size);
                match image.paintable() {
                    Some(paintable) if image.icon_name().is_none() => {
                        copy.set_paintable(Some(&paintable));
                    }
                    _ => copy.set_icon_name(image.icon_name().as_deref()),
                }
                copy.upcast()
            }
            TileIcon::Folder(folder) => {
                let copy = FolderIcon::new(size);
                let icons: Vec<Option<String>> = folder
                    .images
                    .iter()
                    .filter(|image| image.is_child_visible())
                    .map(|image| image.icon_name().map(String::from))
                    .collect();
                copy.set_icons(&icons);
                copy.grid.upcast()
            }
        }
    }

    /// The icon is drawn small and the name hidden: what another
    /// application looks like when something dragged onto it would make a
    /// folder of the two (`AppIcon._showFolderPreview`).
    pub fn set_folder_preview(&self, on: bool) {
        self.label.set_opacity(if on { 0.0 } else { 1.0 });
        if let TileIcon::App(image) = &self.icon {
            set_class(image, "folder-preview", on);
        }
    }

    /// Something dragged would be dropped onto this tile (`:drop`).
    pub fn set_drop(&self, on: bool) {
        set_class(&self.button, "drop", on);
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
