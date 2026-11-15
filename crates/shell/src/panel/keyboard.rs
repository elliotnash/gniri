// SPDX-License-Identifier: GPL-3.0-or-later

//! The keyboard layout indicator. Port of `InputSourceIndicator` in
//! `js/ui/status/keyboard.js`: shown only with more than one layout.
//!
//! GNOME opens a menu of layouts here. Until menus exist (M2) a click
//! switches to the next layout.

use std::rc::Rc;

use gniri_core::Subscription;
use gtk::prelude::*;

use super::button;
use crate::services::Services;

pub fn build(services: &Rc<Services>, subscriptions: &mut Vec<Subscription>) -> gtk::Button {
    let label = gtk::Label::builder()
        .css_classes(["system-status-icon", "keyboard-layout"])
        .build();
    let button = button(&label);
    button.connect_clicked({
        let services = Rc::clone(services);
        move |_| {
            services.niri_action(niri_ipc::Action::SwitchLayout {
                layout: niri_ipc::LayoutSwitchTarget::Next,
            });
        }
    });

    subscriptions.push(services.niri.select(
        |niri| (niri.keyboard_layouts.clone(), niri.keyboard_layout),
        {
            let button = button.clone();
            let services = Rc::clone(services);
            move |(layouts, current): &(Vec<String>, usize)| {
                button.set_visible(layouts.len() > 1);
                if layouts.len() > 1 {
                    let names = services.layout_names().short_names(layouts);
                    label.set_label(names.get(*current).map_or("", String::as_str));
                }
            }
        },
    ));
    button
}
