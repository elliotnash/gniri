//! Quick toggles. Port of `QuickToggle` and `QuickMenuToggle` in
//! `js/ui/quickSettings.js`.

use gtk::prelude::*;

/// Sets or clears the `:checked` state St gives a toggled button.
pub fn set_checked(widget: &impl IsA<gtk::Widget>, checked: bool) {
    if checked {
        widget.set_state_flags(gtk::StateFlags::CHECKED, false);
    } else {
        widget.unset_state_flags(gtk::StateFlags::CHECKED);
    }
}

/// A pill with an icon, a title and an optional second line; optionally with
/// an arrow button at its end that opens a menu.
#[derive(Clone)]
pub struct Toggle {
    /// What goes into the grid: the button itself, or the box holding it and
    /// the menu button.
    pub root: gtk::Widget,
    button: gtk::Button,
    icon: gtk::Image,
    subtitle: gtk::Label,
    /// The separator and the arrow button of a toggle with a menu.
    menu: Option<(gtk::Box, gtk::Button)>,
}

/// The `.quick-toggle` button on its own.
fn contents(title: &str) -> (gtk::Button, gtk::Image, gtk::Label) {
    let icon = gtk::Image::builder()
        .css_classes(["quick-toggle-icon"])
        .build();
    let title = gtk::Label::builder()
        .label(title)
        .css_classes(["quick-toggle-title"])
        .halign(gtk::Align::Start)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    let subtitle = gtk::Label::builder()
        .css_classes(["quick-toggle-subtitle"])
        .halign(gtk::Align::Start)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .visible(false)
        .build();
    let titles = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .valign(gtk::Align::Center)
        .hexpand(true)
        .build();
    titles.append(&title);
    titles.append(&subtitle);

    let row = gtk::Box::builder()
        .css_classes(["quick-toggle-box"])
        .build();
    row.append(&icon);
    row.append(&titles);

    let button = gtk::Button::builder()
        .css_classes(["quick-toggle", "button"])
        .child(&row)
        .build();
    (button, icon, subtitle)
}

impl Toggle {
    pub fn new(title: &str, icon_name: &str) -> Self {
        let (button, icon, subtitle) = contents(title);
        icon.set_icon_name(Some(icon_name));
        Self {
            root: button.clone().upcast(),
            button,
            icon,
            subtitle,
            menu: None,
        }
    }

    /// A toggle with a menu button. `QuickMenuToggle`.
    pub fn with_menu(title: &str, icon_name: &str) -> Self {
        let (button, icon, subtitle) = contents(title);
        icon.set_icon_name(Some(icon_name));
        button.set_hexpand(true);

        let separator = gtk::Box::builder()
            .css_classes(["quick-toggle-separator"])
            .build();
        let arrow = gtk::Button::builder()
            .css_classes(["quick-toggle-menu-button", "icon-button"])
            .child(&gtk::Image::from_icon_name("go-next-symbolic"))
            .build();

        let root = gtk::Box::builder()
            .css_classes(["quick-toggle-has-menu"])
            .build();
        root.append(&button);
        root.append(&separator);
        root.append(&arrow);
        Self {
            root: root.upcast(),
            button,
            icon,
            subtitle,
            menu: Some((separator, arrow)),
        }
    }

    pub fn set_icon(&self, icon_name: &str) {
        self.icon.set_icon_name(Some(icon_name));
    }

    /// The second line; `None` or empty hides it.
    pub fn set_subtitle(&self, subtitle: Option<&str>) {
        let subtitle = subtitle.filter(|text| !text.is_empty());
        self.subtitle.set_visible(subtitle.is_some());
        self.subtitle.set_label(subtitle.unwrap_or_default());
    }

    pub fn set_checked(&self, checked: bool) {
        set_checked(&self.button, checked);
        if let Some((_, arrow)) = &self.menu {
            set_checked(arrow, checked);
            // The separator's colour depends on it.
            set_checked(&self.root, checked);
        }
    }

    pub fn set_visible(&self, visible: bool) {
        self.root.set_visible(visible);
    }

    /// Shows or hides the menu button (`menu-enabled`).
    pub fn set_menu_enabled(&self, enabled: bool) {
        if let Some((separator, arrow)) = &self.menu {
            separator.set_visible(enabled);
            arrow.set_visible(enabled);
        }
    }

    pub fn connect_clicked(&self, clicked: impl Fn() + 'static) {
        self.button.connect_clicked(move |_| clicked());
    }

    pub fn connect_menu_clicked(&self, clicked: impl Fn() + 'static) {
        if let Some((_, arrow)) = &self.menu {
            arrow.connect_clicked(move |_| clicked());
        }
    }
}
