// SPDX-License-Identifier: GPL-3.0-or-later

//! The menu a quick settings item opens under its row. Port of
//! `QuickToggleMenu` in `js/ui/quickSettings.js`, with the few kinds of item
//! from `js/ui/popupMenu.js` those menus use.

use std::rc::Rc;

use gtk::prelude::*;

/// One row of a menu's list.
#[derive(Clone)]
pub struct Entry {
    /// Icon before the label (`PopupImageMenuItem`); `None` for none.
    pub icon: Option<String>,
    pub label: String,
    /// A second, dimmer text after the label, such as a battery level.
    pub detail: Option<String>,
    /// Small icon after the label, such as a network's padlock.
    pub badge: Option<&'static str>,
    /// Draw the check mark ornament.
    pub checked: bool,
    pub activate: Rc<dyn Fn()>,
}

impl Entry {
    pub fn new(label: &str, activate: impl Fn() + 'static) -> Self {
        Self {
            icon: None,
            label: label.to_owned(),
            detail: None,
            badge: None,
            checked: false,
            activate: Rc::new(activate),
        }
    }

    pub fn icon(mut self, icon: &str) -> Self {
        self.icon = Some(icon.to_owned());
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }
}

#[derive(Clone)]
pub struct ToggleMenu {
    /// `.quick-toggle-menu-container`: what the grid places under the row.
    pub root: gtk::Box,
    header_icon: gtk::Image,
    /// The list of entries, rebuilt by [`ToggleMenu::set_entries`].
    section: gtk::Box,
    /// Shown instead of the list when it is empty.
    placeholder: gtk::Label,
    /// Separator and fixed entries below the list.
    footer: gtk::Box,
}

/// A `.popup-menu-item` row with its contents in a box.
fn item(children: &[gtk::Widget]) -> gtk::Button {
    let row = gtk::Box::builder().css_classes(["popup-menu-box"]).build();
    for child in children {
        row.append(child);
    }
    gtk::Button::builder()
        .css_classes(["popup-menu-item"])
        .child(&row)
        .build()
}

fn entry_item(entry: &Entry) -> gtk::Button {
    let mut children: Vec<gtk::Widget> = Vec::new();
    if let Some(icon) = &entry.icon {
        children.push(
            gtk::Image::builder()
                .icon_name(icon)
                .css_classes(["popup-menu-icon"])
                .valign(gtk::Align::Center)
                .build()
                .upcast(),
        );
    }
    children.push(
        gtk::Label::builder()
            .label(&entry.label)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build()
            .upcast(),
    );
    if let Some(badge) = entry.badge {
        children.push(
            gtk::Image::builder()
                .icon_name(badge)
                .css_classes(["popup-menu-badge"])
                .valign(gtk::Align::Center)
                .build()
                .upcast(),
        );
    }
    // `PopupMenu.Ornament.CHECK`: after the label, and keeping its place
    // when not shown.
    let ornament = gtk::Image::builder()
        .css_classes(["popup-menu-ornament"])
        .valign(gtk::Align::Center)
        .build();
    if entry.checked {
        ornament.set_icon_name(Some("ornament-check-symbolic"));
    }
    children.push(ornament.upcast());
    if let Some(detail) = &entry.detail {
        children.push(
            gtk::Label::builder()
                .label(detail)
                .css_classes(["device-subtitle"])
                .hexpand(true)
                .halign(gtk::Align::End)
                .build()
                .upcast(),
        );
    }

    let button = item(&children);
    let activate = Rc::clone(&entry.activate);
    button.connect_clicked(move |_| activate());
    button
}

impl ToggleMenu {
    /// A menu with `setHeader(icon, title)` called on it.
    pub fn new(icon_name: &str, title: &str) -> Self {
        let header_icon = gtk::Image::builder()
            .icon_name(icon_name)
            .css_classes(["icon"])
            .valign(gtk::Align::Center)
            .build();
        let header_title = gtk::Label::builder()
            .label(title)
            .css_classes(["title"])
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Center)
            .hexpand(true)
            .build();
        let header = gtk::Box::builder().css_classes(["header"]).build();
        header.append(&header_icon);
        header.append(&header_title);

        let section = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["popup-menu-section"])
            .build();
        let placeholder = gtk::Label::builder()
            .css_classes(["popup-menu-item", "bt-menu-placeholder"])
            .wrap(true)
            .justify(gtk::Justification::Center)
            .visible(false)
            .build();
        let footer = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();

        let menu = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["quick-toggle-menu"])
            .build();
        menu.append(&header);
        menu.append(&section);
        menu.append(&placeholder);
        menu.append(&footer);

        let root = gtk::Box::builder()
            .css_classes(["quick-toggle-menu-container"])
            .visible(false)
            .build();
        menu.set_hexpand(true);
        root.append(&menu);
        Self {
            root,
            header_icon,
            section,
            placeholder,
            footer,
        }
    }

    /// The header icon is drawn on the accent colour while the menu's item
    /// is switched on (`_syncChecked`).
    pub fn set_active(&self, active: bool) {
        if active {
            self.header_icon.add_css_class("active");
        } else {
            self.header_icon.remove_css_class("active");
        }
    }

    /// Replaces the list of entries.
    pub fn set_entries(&self, entries: &[Entry]) {
        while let Some(child) = self.section.first_child() {
            self.section.remove(&child);
        }
        for entry in entries {
            self.section.append(&entry_item(entry));
        }
        self.section.set_visible(!entries.is_empty());
        self.placeholder
            .set_visible(entries.is_empty() && !self.placeholder.label().is_empty());
    }

    /// The text shown while the list is empty.
    pub fn set_placeholder(&self, text: &str) {
        self.placeholder.set_label(text);
        self.placeholder
            .set_visible(self.section.first_child().is_none() && !text.is_empty());
    }

    /// `PopupSeparatorMenuItem`, below the list.
    pub fn add_separator(&self) {
        let line = gtk::Box::builder()
            .css_classes(["popup-separator-menu-item-separator"])
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        let row = gtk::Box::builder()
            .css_classes(["popup-menu-item", "popup-separator-menu-item"])
            .build();
        row.append(&line);
        self.footer.append(&row);
    }

    /// A plain entry below the list (`addAction`, `addSettingsAction`).
    /// Returns its button so the caller can hide or disable it.
    pub fn add_action(&self, label: &str, activate: impl Fn() + 'static) -> gtk::Button {
        let button = item(&[gtk::Label::new(Some(label)).upcast()]);
        button.connect_clicked(move |_| activate());
        self.footer.append(&button);
        button
    }
}
