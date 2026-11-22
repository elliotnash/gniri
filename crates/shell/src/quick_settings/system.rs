// SPDX-License-Identifier: GPL-3.0-or-later

//! The first row of the quick settings menu: battery, screenshot, settings,
//! lock and the power-off menu. Port of `SystemItem` in
//! `js/ui/status/system.js`.

use gniri_core::{tr, tr_in};
use std::rc::Rc;

use gniri_core::Subscription;
use gniri_services::session;
use gtk::prelude::*;

use super::menu::ToggleMenu;
use super::toggle::set_checked;
use crate::dialogs::end_session;
use crate::menus::Submenu;
use crate::services::Services;

pub struct SystemItem {
    pub root: gtk::Box,
    /// The power-off menu, shown under this row.
    pub menu: ToggleMenu,
}

fn icon_button(icon_name: &str) -> gtk::Button {
    gtk::Button::builder()
        .css_classes(["icon-button"])
        .child(&gtk::Image::from_icon_name(icon_name))
        .build()
}

pub fn build(services: &Rc<Services>, subscriptions: &mut Vec<Subscription>) -> SystemItem {
    let row = gtk::Box::builder()
        .css_classes(["quick-settings-system-box"])
        .hexpand(true)
        .build();

    // `PowerToggle`: the charge level; opens the power settings.
    let battery_icon = gtk::Image::builder()
        .css_classes(["quick-toggle-icon"])
        .valign(gtk::Align::Center)
        .build();
    let battery_label = gtk::Label::builder()
        .css_classes(["quick-toggle-title"])
        .build();
    let battery_box = gtk::Box::builder()
        .css_classes(["quick-toggle-box"])
        .valign(gtk::Align::Center)
        .build();
    battery_box.append(&battery_icon);
    battery_box.append(&battery_label);
    let battery = gtk::Button::builder()
        .css_classes(["quick-toggle", "button", "power-item"])
        .child(&battery_box)
        .build();
    battery.connect_clicked({
        let services = Rc::clone(services);
        move |_| services.open_settings("power")
    });
    // Without a settings program the level is still shown, as a label.
    battery.set_can_target(services.has_settings());
    subscriptions.push(services.power.state.select(
        |state| {
            state
                .battery
                .as_ref()
                .map(|battery| (battery.icon_name(), battery.label()))
        },
        {
            let battery = battery.clone();
            move |state: &Option<(String, String)>| {
                battery.set_visible(state.is_some());
                if let Some((icon, label)) = state {
                    battery_icon.set_icon_name(Some(icon));
                    battery_label.set_label(label);
                }
            }
        },
    ));
    row.append(&battery);

    // `laptopSpacer`
    row.append(&gtk::Box::builder().hexpand(true).build());

    let screenshot = icon_button("screenshooter-symbolic");
    screenshot.connect_clicked({
        let services = Rc::clone(services);
        move |_| {
            services.menus.update(|menus| menus.close());
            services.niri_action(niri_ipc::Action::Screenshot {
                show_pointer: true,
                path: None,
            });
        }
    });
    row.append(&screenshot);

    let settings = icon_button("org.gnome.Settings-symbolic");
    settings.set_visible(services.has_settings());
    settings.connect_clicked({
        let services = Rc::clone(services);
        move |_| services.open_settings("")
    });
    row.append(&settings);

    let lock = icon_button("system-lock-screen-symbolic");
    lock.connect_clicked({
        let services = Rc::clone(services);
        move |_| {
            services.menus.update(|menus| menus.close());
            services.session.send(session::Command::Lock);
        }
    });
    row.append(&lock);

    let shutdown = icon_button("system-shutdown-symbolic");
    shutdown.connect_clicked({
        let services = Rc::clone(services);
        move |_| {
            services.menus.update(|menus| {
                menus.toggle_submenu(Submenu::Shutdown);
            });
        }
    });
    row.append(&shutdown);
    subscriptions.push(services.menus.select(
        |menus| menus.submenu() == Some(Submenu::Shutdown),
        {
            let shutdown = shutdown.clone();
            move |open| set_checked(&shutdown, *open)
        },
    ));

    // `ShutdownItem`'s menu. The entries with an ellipsis ask first
    // (crates/shell/src/dialogs/end_session.rs).
    let menu = ToggleMenu::new("system-shutdown-symbolic", &tr_in("title", "Power Off"));
    let suspend = menu.add_action(&tr("Suspend"), {
        let services = Rc::clone(services);
        move || {
            services.menus.update(|menus| menus.close());
            services.session.send(session::Command::Suspend);
        }
    });
    subscriptions.push(services.session.state.select(
        |state| state.can_suspend,
        move |can| suspend.set_visible(*can),
    ));
    let ask = |kind: end_session::Kind| {
        let services = Rc::clone(services);
        move || {
            // Opening the dialog closes the menu.
            services.end_session.update(|open| *open = Some(kind));
        }
    };
    let restart = menu.add_action(&tr("Restart…"), ask(end_session::Kind::Restart));
    subscriptions.push(services.session.state.select(
        |state| state.can_restart,
        move |can| restart.set_visible(*can),
    ));
    let power_off = menu.add_action(&tr("Power Off…"), ask(end_session::Kind::PowerOff));
    subscriptions.push(services.session.state.select(
        |state| state.can_power_off,
        move |can| power_off.set_visible(*can),
    ));
    menu.add_separator();
    menu.add_action(&tr("Log Out…"), ask(end_session::Kind::LogOut));

    let root = gtk::Box::builder()
        .css_classes(["quick-settings-system-item"])
        .build();
    root.append(&row);
    SystemItem { root, menu }
}
