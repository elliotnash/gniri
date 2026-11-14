//! The system status button at the right end of the panel: network, volume,
//! power profile and battery. Port of `QuickSettings` in `js/ui/panel.js`
//! and the indicators under `js/ui/status/`. Clicking it opens the quick
//! settings menu (crates/shell/src/quick_settings).

use std::rc::Rc;

use gniri_core::Subscription;
use gtk::prelude::*;

use super::{button, set_checked};
use crate::menus::Menu;
use crate::services::Services;

/// One indicator: GNOME's `SystemIndicator`, a box holding its icon.
fn indicator() -> (gtk::Box, gtk::Image) {
    let icon = gtk::Image::builder()
        .css_classes(["system-status-icon"])
        .build();
    let container = gtk::Box::builder()
        .css_classes(["panel-status-indicators-box"])
        .build();
    container.append(&icon);
    (container, icon)
}

/// Shows `name` in `icon`, or hides the whole indicator when there is none.
fn show(container: &gtk::Box, icon: &gtk::Image, name: Option<&str>) {
    container.set_visible(name.is_some());
    if let Some(name) = name {
        icon.set_icon_name(Some(name));
    }
}

/// An indicator that is one fixed icon, shown or hidden.
fn flag(icon_name: &str) -> gtk::Box {
    let (container, icon) = indicator();
    icon.set_icon_name(Some(icon_name));
    container.set_visible(false);
    container
}

pub fn build(
    services: &Rc<Services>,
    output: Option<&str>,
    subscriptions: &mut Vec<Subscription>,
) -> gtk::Button {
    let indicators = gtk::Box::builder()
        .css_classes(["panel-status-indicators-box"])
        .build();

    let (network, network_icon) = indicator();
    subscriptions.push(
        services
            .network
            .state
            .select(|state| state.panel_icon_name(), {
                let network = network.clone();
                move |name| show(&network, &network_icon, *name)
            }),
    );

    let (volume, volume_icon) = indicator();
    subscriptions.push(services.audio.state.select(
        |state| state.output.as_ref().map(|output| output.icon_name()),
        {
            let volume = volume.clone();
            move |name| show(&volume, &volume_icon, *name)
        },
    ));

    let (profile, profile_icon) = indicator();
    subscriptions.push(services.power.state.select(
        |state| {
            state
                .profile
                .filter(|profile| profile.shows_indicator())
                .map(|profile| profile.icon_name())
        },
        {
            let profile = profile.clone();
            move |name| show(&profile, &profile_icon, *name)
        },
    ));

    // The battery, or a power icon on a machine without one
    // (`status/system.js`, `Indicator._sync`).
    let (system, system_icon) = indicator();
    system.add_css_class("power-status");
    let percentage = gtk::Label::new(None);
    system.append(&percentage);
    let show_percentage = services.config.panel.show_battery_percentage;
    subscriptions.push(services.power.state.select(
        |state| {
            state
                .battery
                .as_ref()
                .map(|battery| (battery.icon_name(), battery.label()))
        },
        move |battery| match battery {
            Some((icon, label)) => {
                system_icon.set_icon_name(Some(icon));
                percentage.set_label(label);
                percentage.set_visible(show_percentage);
            }
            None => {
                system_icon.set_icon_name(Some("system-shutdown-symbolic"));
                percentage.set_visible(false);
            }
        },
    ));

    // Indicators that only say something is on.
    let airplane = flag("airplane-mode-symbolic");
    subscriptions.push(services.rfkill.state.select(|state| state.airplane_mode, {
        let airplane = airplane.clone();
        move |on| airplane.set_visible(*on)
    }));
    let bluetooth = flag("bluetooth-active-symbolic");
    subscriptions.push(
        services
            .bluetooth
            .state
            .select(|state| state.shows_indicator(), {
                let bluetooth = bluetooth.clone();
                move |on| bluetooth.set_visible(*on)
            }),
    );
    let night_light = flag("night-light-symbolic");
    subscriptions.push(services.night_light.state.select(|state| state.on, {
        let night_light = night_light.clone();
        move |on| night_light.set_visible(*on)
    }));
    let do_not_disturb = flag("notifications-disabled-symbolic");
    subscriptions.push(services.do_not_disturb.state.select(|state| state.on, {
        let do_not_disturb = do_not_disturb.clone();
        move |on| do_not_disturb.set_visible(*on)
    }));

    // `InputIndicator` in `status/volume.js`: the microphone, while
    // something records.
    let (microphone, microphone_icon) = indicator();
    microphone_icon.add_css_class("privacy-indicator");
    subscriptions.push(services.audio.state.select(
        |state| {
            state
                .input
                .as_ref()
                .filter(|_| state.recording)
                .map(|input| input.icon_name())
        },
        {
            let microphone = microphone.clone();
            move |name| show(&microphone, &microphone_icon, *name)
        },
    ));

    // The order `QuickSettings._setupIndicators` adds them in.
    for item in [
        &microphone,
        &night_light,
        &network,
        &do_not_disturb,
        &bluetooth,
        &airplane,
        &volume,
        &profile,
        &system,
    ] {
        indicators.append(item);
    }

    let button = button(&indicators);
    let output: Option<String> = output.map(str::to_owned);
    button.connect_clicked({
        let (services, output) = (Rc::clone(services), output.clone());
        move |_| {
            services
                .menus
                .update(|menus| menus.toggle(Menu::QuickSettings, output.as_deref()));
        }
    });
    subscriptions.push(services.menus.select(
        move |menus| menus.is_open(Menu::QuickSettings, output.as_deref()),
        {
            let button = button.clone();
            move |open| set_checked(&button, *open)
        },
    ));
    button
}
