//! The first row of the quick settings menu: battery, screenshot, settings,
//! lock and the power-off menu. Port of `SystemItem` in
//! `js/ui/status/system.js`.

use std::rc::Rc;

use gniri_core::Subscription;
use gniri_services::session;
use gtk::prelude::*;

use super::menu::ToggleMenu;
use super::toggle::set_checked;
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
            services
                .menus
                .update(|menus| menus.set_submenu(Some(Submenu::Shutdown)));
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

    // `ShutdownItem`'s menu. "Restart…" and "Power Off…" need the dialog
    // that asks first, which arrives with M4.
    let menu = ToggleMenu::new("system-shutdown-symbolic", "Power Off");
    let suspend = menu.add_action("Suspend", {
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
    menu.add_separator();
    menu.add_action("Log Out…", {
        let services = Rc::clone(services);
        move || {
            services.menus.update(|menus| menus.close());
            // niri asks for confirmation itself.
            services.niri_action(niri_ipc::Action::Quit {
                skip_confirmation: false,
            });
        }
    });

    let root = gtk::Box::builder()
        .css_classes(["quick-settings-system-item"])
        .build();
    root.append(&row);
    SystemItem { root, menu }
}
