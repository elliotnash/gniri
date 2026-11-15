// SPDX-License-Identifier: GPL-3.0-or-later

//! A slider row: an icon button, the slider and optionally a menu button.
//! Port of `QuickSlider` in `js/ui/quickSettings.js`.

use gtk::prelude::*;

use crate::widgets::slider::Slider;

#[derive(Clone)]
pub struct SliderRow {
    pub root: gtk::Box,
    pub slider: Slider,
    icon: gtk::Image,
    icon_button: gtk::Button,
    menu_button: gtk::Button,
}

impl SliderRow {
    pub fn new(icon_name: &str) -> Self {
        let icon = gtk::Image::from_icon_name(icon_name);
        let icon_button = gtk::Button::builder()
            .css_classes(["icon-button", "flat"])
            .child(&icon)
            // `icon-reactive` is off until someone listens for clicks. St
            // shows the icon as usual then, not greyed out, so the button
            // stays sensitive and only ignores the pointer.
            .can_target(false)
            .focusable(false)
            .build();

        let slider = Slider::new();
        let bin = gtk::Box::builder()
            .css_classes(["slider-bin"])
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        bin.append(&slider.widget);

        let menu_button = gtk::Button::builder()
            .css_classes(["icon-button", "flat"])
            .child(&gtk::Image::from_icon_name("go-next-symbolic"))
            .visible(false)
            .build();

        let root = gtk::Box::builder().css_classes(["quick-slider"]).build();
        root.append(&icon_button);
        root.append(&bin);
        root.append(&menu_button);
        Self {
            root,
            slider,
            icon,
            icon_button,
            menu_button,
        }
    }

    pub fn set_icon(&self, icon_name: &str) {
        self.icon.set_icon_name(Some(icon_name));
    }

    /// Makes the icon a button (`icon-reactive`).
    pub fn connect_icon_clicked(&self, clicked: impl Fn() + 'static) {
        self.icon_button.set_can_target(true);
        self.icon_button.set_focusable(true);
        self.icon_button.connect_clicked(move |_| clicked());
    }

    /// Shows or hides the menu button (`menu-enabled`).
    pub fn set_menu_enabled(&self, enabled: bool) {
        self.menu_button.set_visible(enabled);
    }

    pub fn connect_menu_clicked(&self, clicked: impl Fn() + 'static) {
        self.menu_button.connect_clicked(move |_| clicked());
    }
}
