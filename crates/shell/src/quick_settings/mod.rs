// SPDX-License-Identifier: GPL-3.0-or-later

//! The quick settings menu. Port of `QuickSettings` in `js/ui/panel.js`
//! and the items under `js/ui/status/`.

mod grid;
mod menu;
mod slider;
mod system;
mod toggle;

use gniri_core::{tr, tr_in};
use std::rc::Rc;

use gniri_core::Subscription;
use gniri_services::{audio, bluetooth, brightness, network, power, rfkill, toggles};
use gtk::prelude::*;

use self::grid::QuickGrid;
use self::menu::{Entry, ToggleMenu};
use self::slider::SliderRow;
use self::toggle::Toggle;
use crate::menus::Submenu;
use crate::services::{Service, Services};

/// `.quick-settings-grid`: `spacing-rows` and `spacing-columns`,
/// `$base_padding * 2`.
const GRID_SPACING: i32 = 12;
/// `QuickSettings`: two columns.
const COLUMNS: usize = 2;

/// The menu and what keeps it up to date. Dropping it stops the updates.
pub struct QuickSettings {
    /// `.popup-menu-content.quick-settings`
    pub widget: gtk::Box,
    _subscriptions: Vec<Subscription>,
}

/// Shows `menu` while `submenu` is the open one, and dims the rest of the
/// grid then.
fn follow_submenu(
    services: &Rc<Services>,
    submenu: Submenu,
    menu: &ToggleMenu,
    subscriptions: &mut Vec<Subscription>,
) {
    let root = menu.root.clone();
    subscriptions.push(services.menus.select(
        move |menus| menus.submenu() == Some(submenu),
        move |open| root.set_visible(*open),
    ));
}

fn open_submenu(services: &Rc<Services>, submenu: Submenu) -> impl Fn() + 'static {
    let services = Rc::clone(services);
    move || {
        services
            .menus
            .update(|menus| menus.set_submenu(Some(submenu)));
    }
}

/// Closes the submenu after one of its entries was chosen
/// (`PopupMenuBase.itemActivated`).
fn close_submenu(services: &Rc<Services>) {
    services.menus.update(|menus| menus.set_submenu(None));
}

/// An entry that opens a panel of the settings application and closes the
/// whole menu (`addSettingsAction`). Nothing is added without a settings
/// program.
fn add_settings_action(
    services: &Rc<Services>,
    menu: &ToggleMenu,
    label: &str,
    panel: &'static str,
) {
    if services.has_settings() {
        menu.add_separator();
        let services = Rc::clone(services);
        menu.add_action(label, move || services.open_settings(panel));
    }
}

/// A toggle for one on/off setting (`status/darkMode.js` and the like).
fn simple_toggle(
    title: &str,
    icon: &str,
    service: impl Fn(&Services) -> &Service<toggles::State> + Copy + 'static,
    services: &Rc<Services>,
    subscriptions: &mut Vec<Subscription>,
) -> Toggle {
    let toggle = Toggle::new(title, icon);
    toggle.connect_clicked({
        let services = Rc::clone(services);
        move || {
            let service = service(&services);
            let on = service.state.with(|state| state.on);
            service.send(toggles::Command::Set(!on));
        }
    });
    subscriptions.push(service(services).state.observe({
        let toggle = toggle.clone();
        move |state| {
            toggle.set_visible(state.available);
            toggle.set_checked(state.on);
        }
    }));
    toggle
}

pub fn build(services: &Rc<Services>) -> QuickSettings {
    let mut subscriptions = Vec::new();
    let grid = QuickGrid::new(COLUMNS, GRID_SPACING, GRID_SPACING);

    // The system row and its power-off menu.
    let system = system::build(services, &mut subscriptions);
    follow_submenu(
        services,
        Submenu::Shutdown,
        &system.menu,
        &mut subscriptions,
    );
    grid.add(&system.root, COLUMNS, Some(system.menu.root.upcast_ref()));

    output_slider(services, &grid, &mut subscriptions);
    input_slider(services, &grid, &mut subscriptions);
    brightness_slider(services, &grid, &mut subscriptions);

    wired_toggle(services, &grid, &mut subscriptions);
    wifi_toggle(services, &grid, &mut subscriptions);
    bluetooth_toggle(services, &grid, &mut subscriptions);
    power_toggle(services, &grid, &mut subscriptions);

    let night_light = simple_toggle(
        &tr("Night Light"),
        "night-light-symbolic",
        |services| &services.night_light,
        services,
        &mut subscriptions,
    );
    grid.add(&night_light.root, 1, None);
    let dark_style = simple_toggle(
        &tr("Dark Style"),
        "dark-mode-symbolic",
        |services| &services.dark_style,
        services,
        &mut subscriptions,
    );
    grid.add(&dark_style.root, 1, None);
    let do_not_disturb = simple_toggle(
        &tr("Do Not Disturb"),
        "notifications-disabled-symbolic",
        |services| &services.do_not_disturb,
        services,
        &mut subscriptions,
    );
    grid.add(&do_not_disturb.root, 1, None);

    airplane_toggle(services, &grid, &mut subscriptions);

    // While a submenu is open everything else in the menu is dimmed
    // (`QuickSettingsMenu._setDimmed`).
    let widget = gtk::Box::builder()
        .css_classes(["popup-menu-content", "quick-settings"])
        .build();
    widget.append(&grid);
    subscriptions.push(services.menus.select(|menus| menus.submenu().is_some(), {
        let (widget, grid) = (widget.clone(), grid.clone());
        move |dimmed| {
            if *dimmed {
                widget.add_css_class("shaded");
            } else {
                widget.remove_css_class("shaded");
            }
            grid.set_dimmed(*dimmed);
        }
    }));

    QuickSettings {
        widget,
        _subscriptions: subscriptions,
    }
}

/// One of the two volume sliders of `status/volume.js`, with its device
/// menu.
struct VolumeSlider {
    icon: &'static str,
    submenu: Submenu,
    menu_icon: &'static str,
    menu_title: String,
    /// The commands for the slider moving to a value.
    slide: fn(&audio::State, f64) -> Vec<audio::Command>,
    /// The commands for a click on the icon.
    click_icon: fn(&audio::State) -> Vec<audio::Command>,
    select: fn(String) -> audio::Command,
    /// Whether the row shows, its icon and where its handle is.
    row: fn(&audio::State) -> Option<(&'static str, f64)>,
    devices: fn(&audio::State) -> &[audio::Device],
}

fn volume_slider(
    slider: VolumeSlider,
    services: &Rc<Services>,
    grid: &QuickGrid,
    subscriptions: &mut Vec<Subscription>,
) {
    let row = SliderRow::new(slider.icon);
    row.slider.connect_changed({
        let services = Rc::clone(services);
        move |value| {
            let commands = services
                .audio
                .state
                .with(|state| (slider.slide)(state, value));
            for command in commands {
                services.audio.send(command);
            }
        }
    });
    row.connect_icon_clicked({
        let services = Rc::clone(services);
        move || {
            for command in services.audio.state.with(slider.click_icon) {
                services.audio.send(command);
            }
        }
    });
    row.connect_menu_clicked(open_submenu(services, slider.submenu));

    let menu = ToggleMenu::new(slider.menu_icon, &slider.menu_title);
    add_settings_action(services, &menu, &tr("Sound Settings"), "sound");
    follow_submenu(services, slider.submenu, &menu, subscriptions);

    subscriptions.push(services.audio.state.observe({
        let (row, menu, services) = (row.clone(), menu.clone(), Rc::clone(services));
        move |state| {
            let shown = (slider.row)(state);
            row.root.set_visible(shown.is_some());
            if let Some((icon, value)) = shown {
                row.set_icon(icon);
                // The slider ends at 100% (over-amplification is a setting
                // gniri does not have).
                row.slider.set_value(value.min(1.0));
            }
            let devices = (slider.devices)(state);
            // A device menu is offered only when there is a choice.
            row.set_menu_enabled(devices.len() > 1);
            let entries: Vec<Entry> = devices
                .iter()
                .map(|device| {
                    let (services, name) = (Rc::clone(&services), device.name.clone());
                    Entry::new(&device.description, move || {
                        services.audio.send((slider.select)(name.clone()));
                        close_submenu(&services);
                    })
                    .checked(device.active)
                })
                .collect();
            menu.set_entries(&entries);
        }
    }));
    grid.add(&row.root, COLUMNS, Some(menu.root.upcast_ref()));
}

/// `OutputStreamSlider` in `status/volume.js`.
fn output_slider(services: &Rc<Services>, grid: &QuickGrid, subscriptions: &mut Vec<Subscription>) {
    let slider = VolumeSlider {
        icon: "audio-volume-muted-symbolic",
        submenu: Submenu::SoundOutput,
        menu_icon: "audio-headphones-symbolic",
        menu_title: tr("Sound Output"),
        slide: audio::State::slide_output,
        click_icon: audio::State::click_output_icon,
        select: audio::Command::SelectOutput,
        row: |state| {
            let output = state.output.as_ref()?;
            Some((output.slider_icon_name(), output.slider_value()))
        },
        devices: |state| &state.outputs,
    };
    volume_slider(slider, services, grid, subscriptions);
}

/// `InputStreamSlider` in `status/volume.js`: only while something records.
fn input_slider(services: &Rc<Services>, grid: &QuickGrid, subscriptions: &mut Vec<Subscription>) {
    let slider = VolumeSlider {
        icon: "microphone-sensitivity-muted-symbolic",
        submenu: Submenu::SoundInput,
        menu_icon: "audio-input-microphone-symbolic",
        menu_title: tr("Sound Input"),
        slide: audio::State::slide_input,
        click_icon: audio::State::click_input_icon,
        select: audio::Command::SelectInput,
        row: |state| {
            let input = state.input.as_ref().filter(|_| state.shows_input())?;
            Some((input.icon_name(), input.slider_value()))
        },
        devices: |state| &state.inputs,
    };
    volume_slider(slider, services, grid, subscriptions);
}

/// `BrightnessItem` in `status/brightness.js`.
fn brightness_slider(
    services: &Rc<Services>,
    grid: &QuickGrid,
    subscriptions: &mut Vec<Subscription>,
) {
    let row = SliderRow::new("display-brightness-symbolic");
    row.slider.connect_changed({
        let services = Rc::clone(services);
        move |level| services.brightness.send(brightness::Command::Set(level))
    });
    subscriptions.push(services.brightness.state.select(|state| state.level, {
        let row = row.clone();
        move |level: &Option<f64>| {
            row.root.set_visible(level.is_some());
            if let Some(level) = level {
                row.slider.set_value(*level);
            }
        }
    }));
    grid.add(&row.root, COLUMNS, None);
}

/// `NMWiredToggle` in `status/network.js`.
fn wired_toggle(services: &Rc<Services>, grid: &QuickGrid, subscriptions: &mut Vec<Subscription>) {
    let toggle = Toggle::new(&tr("Wired"), "network-wired-symbolic");
    toggle.connect_clicked({
        let services = Rc::clone(services);
        move || {
            let connected = services
                .network
                .state
                .with(|state| state.wired.as_ref().is_some_and(network::Wired::is_active));
            services
                .network
                .send(network::Command::SetWiredConnected(!connected));
        }
    });
    subscriptions.push(services.network.state.select(|state| state.wired.clone(), {
        let toggle = toggle.clone();
        move |wired: &Option<network::Wired>| {
            toggle.set_visible(wired.is_some());
            if let Some(wired) = wired {
                toggle.set_icon(wired.icon_name());
                toggle.set_checked(wired.is_active());
            }
        }
    }));
    grid.add(&toggle.root, 1, None);
}

/// One row of the Wi-Fi menu. `NMWirelessNetworkItem` in
/// `status/network.js`.
fn network_entry(services: &Rc<Services>, network: &network::Network) -> Entry {
    let mut entry = Entry::new(&network.ssid, {
        let (services, network) = (Rc::clone(services), network.clone());
        move || {
            if network.is_active() {
                services.network.send(network::Command::DisconnectWifi);
            } else if network.can_connect() {
                services
                    .network
                    .send(network::Command::ConnectWifi(network.ssid.clone()));
            } else {
                // It wants more than a password (a user name, a
                // certificate): only the settings application can set
                // that up, and GNOME opens it too.
                services.open_settings("wifi");
                return;
            }
            close_submenu(&services);
        }
    })
    .icon(network.icon_name())
    .checked(network.status == network::Status::Connected);
    if network.secure {
        entry.badge = Some("network-wireless-encrypted-symbolic");
    }
    if network.status == network::Status::Connecting {
        entry.detail = Some(tr("Connecting…"));
    }
    entry
}

/// `NMWirelessToggle` in `status/network.js`.
fn wifi_toggle(services: &Rc<Services>, grid: &QuickGrid, subscriptions: &mut Vec<Subscription>) {
    let toggle = Toggle::with_menu(&tr("Wi-Fi"), "network-wireless-disabled-symbolic");
    toggle.connect_clicked({
        let services = Rc::clone(services);
        move || {
            let enabled = services
                .network
                .state
                .with(|state| state.wifi.as_ref().is_some_and(|wifi| wifi.enabled));
            services
                .network
                .send(network::Command::SetWifiEnabled(!enabled));
        }
    });
    toggle.connect_menu_clicked({
        let services = Rc::clone(services);
        move || {
            // The list in the menu should be fresh.
            services.network.send(network::Command::ScanWifi);
            services
                .menus
                .update(|menus| menus.set_submenu(Some(Submenu::Wifi)));
        }
    });

    let menu = ToggleMenu::new("network-wireless-symbolic", &tr("Wi–Fi"));
    add_settings_action(services, &menu, &tr("All Networks"), "wifi");
    follow_submenu(services, Submenu::Wifi, &menu, subscriptions);

    subscriptions.push(services.network.state.select(|state| state.wifi.clone(), {
        let (toggle, menu, services) = (toggle.clone(), menu.clone(), Rc::clone(services));
        move |wifi: &Option<network::Wifi>| {
            toggle.set_visible(wifi.is_some());
            let Some(wifi) = wifi else { return };
            toggle.set_icon(wifi.icon_name());
            toggle.set_subtitle(wifi.subtitle().as_deref());
            toggle.set_checked(wifi.enabled);
            menu.set_active(wifi.enabled);
            let entries: Vec<Entry> = wifi
                .visible_networks()
                .iter()
                .map(|network| network_entry(&services, network))
                .collect();
            menu.set_entries(&entries);
        }
    }));
    grid.add(&toggle.root, 1, Some(menu.root.upcast_ref()));
}

/// One row of the Bluetooth menu. `BluetoothDeviceItem` in
/// `status/bluetooth.js`: a click connects or disconnects, and the menu
/// stays open to show it happen.
fn device_entry(services: &Rc<Services>, device: &bluetooth::Device) -> Entry {
    let mut entry = Entry::new(&device.name, {
        let (services, activated) = (Rc::clone(services), device.activated());
        move || {
            if let Some(command) = activated.clone() {
                services.bluetooth.send(command);
            }
        }
    })
    .icon(&device.icon_name());
    // GNOME shows a spinner while a request is under way.
    entry.detail = Some(match device.status {
        bluetooth::Status::Connecting => tr("Connecting…"),
        bluetooth::Status::Disconnecting => tr("Disconnecting…"),
        _ => device.action_label(),
    });
    entry
}

/// `BluetoothToggle` in `status/bluetooth.js`.
fn bluetooth_toggle(
    services: &Rc<Services>,
    grid: &QuickGrid,
    subscriptions: &mut Vec<Subscription>,
) {
    let toggle = Toggle::with_menu(&tr("Bluetooth"), "bluetooth-disabled-symbolic");
    toggle.connect_clicked({
        let services = Rc::clone(services);
        move || {
            let command = services.bluetooth.state.with(bluetooth::State::toggled);
            services.bluetooth.send(command);
        }
    });
    toggle.connect_menu_clicked(open_submenu(services, Submenu::Bluetooth));

    let menu = ToggleMenu::new("bluetooth-active-symbolic", &tr("Bluetooth"));
    add_settings_action(services, &menu, &tr("Bluetooth Settings"), "bluetooth");
    follow_submenu(services, Submenu::Bluetooth, &menu, subscriptions);

    // GNOME shows the toggle by the Bluetooth kill switch, not by the
    // adapter: on some laptops the adapter goes away while it is switched
    // off, and the toggle is what switches it back on.
    let update_visible = {
        let (toggle, services) = (toggle.clone(), Rc::clone(services));
        move || {
            let adapter = services
                .bluetooth
                .state
                .with(|state| state.adapter.is_some());
            let switch = services
                .rfkill
                .state
                .with(rfkill::State::bluetooth_available);
            toggle.set_visible(adapter || switch);
        }
    };
    subscriptions.push(services.rfkill.state.observe({
        let update_visible = update_visible.clone();
        move |_| update_visible()
    }));
    subscriptions.push(services.bluetooth.state.observe({
        let (toggle, menu, services) = (toggle.clone(), menu.clone(), Rc::clone(services));
        move |state| {
            update_visible();
            toggle.set_icon(state.icon_name());
            toggle.set_subtitle(state.subtitle().as_deref());
            toggle.set_checked(state.powered());
            menu.set_active(state.powered());
            menu.set_placeholder(&state.placeholder().unwrap_or_default());
            let entries: Vec<Entry> = state
                .menu_devices()
                .map(|device| device_entry(&services, device))
                .collect();
            menu.set_entries(&entries);
        }
    }));
    grid.add(&toggle.root, 1, Some(menu.root.upcast_ref()));
}

/// `PowerProfilesToggle` in `status/powerProfiles.js`.
fn power_toggle(services: &Rc<Services>, grid: &QuickGrid, subscriptions: &mut Vec<Subscription>) {
    let toggle = Toggle::with_menu(
        &tr_in("Quick settings button title", "Power Mode"),
        "power-profile-balanced-symbolic",
    );
    toggle.connect_clicked({
        let services = Rc::clone(services);
        move || {
            let profile = services.power.state.with(power::State::toggled_profile);
            services.power.send(power::Command::SetProfile(profile));
        }
    });
    toggle.connect_menu_clicked(open_submenu(services, Submenu::PowerMode));

    let menu = ToggleMenu::new(
        "power-profile-balanced-symbolic",
        &tr_in("Quick settings menu header", "Power Mode"),
    );
    add_settings_action(services, &menu, &tr("Power Settings"), "power");
    follow_submenu(services, Submenu::PowerMode, &menu, subscriptions);

    subscriptions.push(services.power.state.select(
        |state| (state.profile, state.profiles.clone()),
        {
            let (toggle, menu, services) = (toggle.clone(), menu.clone(), Rc::clone(services));
            move |(active, profiles): &(Option<power::Profile>, Vec<power::Profile>)| {
                toggle.set_visible(active.is_some());
                let Some(active) = *active else { return };
                toggle.set_icon(active.icon_name());
                toggle.set_subtitle(Some(&active.label()));
                toggle.set_checked(active != power::Profile::Balanced);
                toggle.set_menu_enabled(profiles.len() > 2);
                menu.set_active(active != power::Profile::Balanced);

                let entries: Vec<Entry> = profiles
                    .iter()
                    .map(|&profile| {
                        let services = Rc::clone(&services);
                        Entry::new(&profile.label(), move || {
                            services.power.send(power::Command::SetProfile(profile));
                            close_submenu(&services);
                        })
                        .icon(profile.icon_name())
                        .checked(profile == active)
                    })
                    .collect();
                menu.set_entries(&entries);
            }
        },
    ));
    grid.add(&toggle.root, 1, Some(menu.root.upcast_ref()));
}

/// `RfkillToggle` in `status/rfkill.js`.
fn airplane_toggle(
    services: &Rc<Services>,
    grid: &QuickGrid,
    subscriptions: &mut Vec<Subscription>,
) {
    let toggle = Toggle::new(&tr("Airplane Mode"), "airplane-mode-symbolic");
    toggle.connect_clicked({
        let services = Rc::clone(services);
        move || {
            let command = services.rfkill.state.with(rfkill::State::toggled);
            services.rfkill.send(command);
        }
    });
    subscriptions.push(services.rfkill.state.observe({
        let toggle = toggle.clone();
        move |state| {
            toggle.set_visible(state.available);
            toggle.set_checked(state.airplane_mode);
        }
    }));
    grid.add(&toggle.root, 1, None);
}
