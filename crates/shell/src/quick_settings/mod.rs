//! The quick settings menu. Port of `QuickSettings` in `js/ui/panel.js`
//! and the items under `js/ui/status/`.

mod grid;
mod menu;
mod slider;
mod system;
mod toggle;

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
        "Night Light",
        "night-light-symbolic",
        |services| &services.night_light,
        services,
        &mut subscriptions,
    );
    grid.add(&night_light.root, 1, None);
    let dark_style = simple_toggle(
        "Dark Style",
        "dark-mode-symbolic",
        |services| &services.dark_style,
        services,
        &mut subscriptions,
    );
    grid.add(&dark_style.root, 1, None);
    let do_not_disturb = simple_toggle(
        "Do Not Disturb",
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
                widget.add_css_class("dimmed");
            } else {
                widget.remove_css_class("dimmed");
            }
            grid.set_dimmed(*dimmed);
        }
    }));

    QuickSettings {
        widget,
        _subscriptions: subscriptions,
    }
}

/// `OutputStreamSlider` in `status/volume.js`.
fn output_slider(services: &Rc<Services>, grid: &QuickGrid, subscriptions: &mut Vec<Subscription>) {
    let row = SliderRow::new("audio-volume-muted-symbolic");
    row.slider.connect_changed({
        let services = Rc::clone(services);
        move |volume| {
            services.audio.send(audio::Command::SetOutputVolume(volume));
        }
    });
    row.connect_icon_clicked({
        let services = Rc::clone(services);
        move || {
            if let Some(command) = services.audio.state.with(audio::State::toggle_output_mute) {
                services.audio.send(command);
            }
        }
    });
    row.connect_menu_clicked(open_submenu(services, Submenu::SoundOutput));

    let menu = ToggleMenu::new("audio-headphones-symbolic", "Sound Output");
    add_settings_action(services, &menu, "Sound Settings", "sound");
    follow_submenu(services, Submenu::SoundOutput, &menu, subscriptions);

    subscriptions.push(services.audio.state.observe({
        let (row, menu, services) = (row.clone(), menu.clone(), Rc::clone(services));
        move |state| {
            row.root.set_visible(state.output.is_some());
            if let Some(output) = &state.output {
                row.set_icon(output.slider_icon_name());
                // A muted output shows an empty slider, as in GNOME.
                row.slider.set_value(if output.muted {
                    0.0
                } else {
                    output.volume.min(1.0)
                });
            }
            // GNOME offers the device menu only when there is a choice.
            row.set_menu_enabled(state.outputs.len() > 1);
            menu.set_entries(&device_entries(&services, &state.outputs, |name| {
                audio::Command::SelectOutput(name)
            }));
        }
    }));
    grid.add(&row.root, COLUMNS, Some(menu.root.upcast_ref()));
}

/// `InputStreamSlider` in `status/volume.js`: only while something records.
fn input_slider(services: &Rc<Services>, grid: &QuickGrid, subscriptions: &mut Vec<Subscription>) {
    let row = SliderRow::new("microphone-sensitivity-muted-symbolic");
    row.slider.connect_changed({
        let services = Rc::clone(services);
        move |volume| services.audio.send(audio::Command::SetInputVolume(volume))
    });
    row.connect_icon_clicked({
        let services = Rc::clone(services);
        move || {
            let muted = services
                .audio
                .state
                .with(|state| state.input.as_ref().map(|input| input.muted));
            if let Some(muted) = muted {
                services.audio.send(audio::Command::SetInputMuted(!muted));
            }
        }
    });
    row.connect_menu_clicked(open_submenu(services, Submenu::SoundInput));

    let menu = ToggleMenu::new("audio-input-microphone-symbolic", "Sound Input");
    add_settings_action(services, &menu, "Sound Settings", "sound");
    follow_submenu(services, Submenu::SoundInput, &menu, subscriptions);

    subscriptions.push(services.audio.state.observe({
        let (row, menu, services) = (row.clone(), menu.clone(), Rc::clone(services));
        move |state| {
            row.root
                .set_visible(state.recording && state.input.is_some());
            if let Some(input) = &state.input {
                row.set_icon(input.icon_name());
                row.slider.set_value(if input.muted {
                    0.0
                } else {
                    input.volume.min(1.0)
                });
            }
            row.set_menu_enabled(state.inputs.len() > 1);
            menu.set_entries(&device_entries(&services, &state.inputs, |name| {
                audio::Command::SelectInput(name)
            }));
        }
    }));
    grid.add(&row.root, COLUMNS, Some(menu.root.upcast_ref()));
}

fn device_entries(
    services: &Rc<Services>,
    devices: &[audio::Device],
    select: fn(String) -> audio::Command,
) -> Vec<Entry> {
    devices
        .iter()
        .map(|device| {
            let (services, name) = (Rc::clone(services), device.name.clone());
            Entry::new(&device.description, move || {
                services.audio.send(select(name.clone()));
                close_submenu(&services);
            })
            .checked(device.active)
        })
        .collect()
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
    let toggle = Toggle::new("Wired", "network-wired-symbolic");
    toggle.connect_clicked({
        let services = Rc::clone(services);
        move || {
            let connected = services.network.state.with(|state| {
                state
                    .wired
                    .as_ref()
                    .is_some_and(|wired| wired.status != network::Status::Idle)
            });
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
                toggle.set_checked(wired.status != network::Status::Idle);
            }
        }
    }));
    grid.add(&toggle.root, 1, None);
}

/// `NMWirelessToggle` in `status/network.js`.
fn wifi_toggle(services: &Rc<Services>, grid: &QuickGrid, subscriptions: &mut Vec<Subscription>) {
    let toggle = Toggle::with_menu("Wi-Fi", "network-wireless-disabled-symbolic");
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

    let menu = ToggleMenu::new("network-wireless-symbolic", "Wi-Fi");
    add_settings_action(services, &menu, "All Networks", "wifi");
    follow_submenu(services, Submenu::Wifi, &menu, subscriptions);

    subscriptions.push(services.network.state.select(|state| state.wifi.clone(), {
        let (toggle, menu, services) = (toggle.clone(), menu.clone(), Rc::clone(services));
        move |wifi: &Option<network::Wifi>| {
            toggle.set_visible(wifi.is_some());
            let Some(wifi) = wifi else { return };
            toggle.set_icon(wifi.icon_name());
            toggle.set_subtitle(wifi.subtitle());
            toggle.set_checked(wifi.enabled);
            menu.set_active(wifi.enabled);

            let entries: Vec<Entry> = wifi
                .networks
                .iter()
                .map(|network| {
                    let (services, ssid) = (Rc::clone(&services), network.ssid.clone());
                    let connected = network.status != network::Status::Idle;
                    let mut entry = Entry::new(&network.ssid, move || {
                        services.network.send(if connected {
                            network::Command::DisconnectWifi
                        } else {
                            network::Command::ConnectWifi(ssid.clone())
                        });
                        close_submenu(&services);
                    })
                    .icon(network.icon_name())
                    .checked(network.status == network::Status::Connected);
                    if network.secure {
                        entry.badge = Some("network-wireless-encrypted-symbolic");
                    }
                    if network.status == network::Status::Connecting {
                        entry.detail = Some("Connecting…".to_owned());
                    }
                    entry
                })
                .collect();
            menu.set_entries(&entries);
        }
    }));
    grid.add(&toggle.root, 1, Some(menu.root.upcast_ref()));
}

/// `BluetoothToggle` in `status/bluetooth.js`.
fn bluetooth_toggle(
    services: &Rc<Services>,
    grid: &QuickGrid,
    subscriptions: &mut Vec<Subscription>,
) {
    let toggle = Toggle::with_menu("Bluetooth", "bluetooth-disabled-symbolic");
    toggle.connect_clicked({
        let services = Rc::clone(services);
        move || {
            let powered = services.bluetooth.state.with(bluetooth::State::powered);
            services
                .bluetooth
                .send(bluetooth::Command::SetPowered(!powered));
        }
    });
    toggle.connect_menu_clicked(open_submenu(services, Submenu::Bluetooth));

    let menu = ToggleMenu::new("bluetooth-active-symbolic", "Bluetooth");
    add_settings_action(services, &menu, "Bluetooth Settings", "bluetooth");
    follow_submenu(services, Submenu::Bluetooth, &menu, subscriptions);

    subscriptions.push(services.bluetooth.state.observe({
        let (toggle, menu, services) = (toggle.clone(), menu.clone(), Rc::clone(services));
        move |state| {
            toggle.set_visible(state.adapter.is_some());
            toggle.set_icon(state.icon_name());
            toggle.set_subtitle(state.subtitle().as_deref());
            toggle.set_checked(state.powered());
            menu.set_active(state.powered());
            menu.set_placeholder(if state.powered() {
                "No available or connected devices"
            } else {
                "Turn on Bluetooth to connect to devices"
            });

            let entries: Vec<Entry> = state
                .devices
                .iter()
                .map(|device| {
                    let (services, address) = (Rc::clone(&services), device.address.clone());
                    let status = device.status;
                    let mut entry = Entry::new(&device.name, move || {
                        // `BluetoothDeviceItem`: a click connects or
                        // disconnects, and the menu stays open to show it.
                        match status {
                            bluetooth::Status::Disconnected => services
                                .bluetooth
                                .send(bluetooth::Command::Connect(address.clone())),
                            bluetooth::Status::Connected => services
                                .bluetooth
                                .send(bluetooth::Command::Disconnect(address.clone())),
                            bluetooth::Status::Connecting | bluetooth::Status::Disconnecting => {}
                        }
                    })
                    .icon(&device.icon_name());
                    entry.detail = match status {
                        bluetooth::Status::Connected => Some(match device.battery {
                            Some(level) => format!("{level}%"),
                            None => "Disconnect".to_owned(),
                        }),
                        bluetooth::Status::Connecting => Some("Connecting…".to_owned()),
                        bluetooth::Status::Disconnecting => Some("Disconnecting…".to_owned()),
                        bluetooth::Status::Disconnected => Some("Connect".to_owned()),
                    };
                    entry
                })
                .collect();
            menu.set_entries(&entries);
        }
    }));
    grid.add(&toggle.root, 1, Some(menu.root.upcast_ref()));
}

/// `PowerProfilesToggle` in `status/powerProfiles.js`.
fn power_toggle(services: &Rc<Services>, grid: &QuickGrid, subscriptions: &mut Vec<Subscription>) {
    let toggle = Toggle::with_menu("Power Mode", "power-profile-balanced-symbolic");
    toggle.connect_clicked({
        let services = Rc::clone(services);
        move || {
            let profile = services.power.state.with(power::State::toggled_profile);
            services.power.send(power::Command::SetProfile(profile));
        }
    });
    toggle.connect_menu_clicked(open_submenu(services, Submenu::PowerMode));

    let menu = ToggleMenu::new("power-profile-balanced-symbolic", "Power Mode");
    add_settings_action(services, &menu, "Power Settings", "power");
    follow_submenu(services, Submenu::PowerMode, &menu, subscriptions);

    subscriptions.push(services.power.state.select(
        |state| (state.profile, state.profiles.clone()),
        {
            let (toggle, menu, services) = (toggle.clone(), menu.clone(), Rc::clone(services));
            move |(active, profiles): &(Option<power::Profile>, Vec<power::Profile>)| {
                toggle.set_visible(active.is_some());
                let Some(active) = *active else { return };
                toggle.set_icon(active.icon_name());
                toggle.set_subtitle(Some(active.label()));
                toggle.set_checked(active != power::Profile::Balanced);
                toggle.set_menu_enabled(profiles.len() > 2);
                menu.set_active(active != power::Profile::Balanced);

                let entries: Vec<Entry> = profiles
                    .iter()
                    .map(|&profile| {
                        let services = Rc::clone(&services);
                        Entry::new(profile.label(), move || {
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
    let toggle = Toggle::new("Airplane Mode", "airplane-mode-symbolic");
    toggle.connect_clicked({
        let services = Rc::clone(services);
        move || {
            let on = services.rfkill.state.with(|state| state.airplane_mode);
            services.rfkill.send(rfkill::Command::SetAirplaneMode(!on));
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
