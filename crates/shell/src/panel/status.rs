//! The system status button at the right end of the panel: network, volume,
//! power profile and battery. Port of `QuickSettings` in `js/ui/panel.js`
//! and the indicators under `js/ui/status/`; its menu arrives in M2.

use std::rc::Rc;

use gniri_core::Subscription;
use gtk::prelude::*;

use super::button;
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

pub fn build(services: &Rc<Services>, subscriptions: &mut Vec<Subscription>) -> gtk::Button {
    let indicators = gtk::Box::builder()
        .css_classes(["panel-status-indicators-box"])
        .build();

    let (network, network_icon) = indicator();
    subscriptions.push(services.network.select(
        |state| {
            state
                .primary
                .as_ref()
                .map(|connection| connection.icon_name())
        },
        {
            let network = network.clone();
            move |name| show(&network, &network_icon, *name)
        },
    ));

    let (volume, volume_icon) = indicator();
    subscriptions.push(services.audio.select(
        |state| state.output.as_ref().map(|output| output.icon_name()),
        {
            let volume = volume.clone();
            move |name| show(&volume, &volume_icon, *name)
        },
    ));

    let (profile, profile_icon) = indicator();
    subscriptions.push(services.power.select(
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
    subscriptions.push(services.power.select(
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

    for item in [&network, &volume, &profile, &system] {
        indicators.append(item);
    }
    button(&indicators)
}
