// SPDX-License-Identifier: GPL-3.0-or-later

//! The top bar's contents. Port of `js/ui/panel.js`.
//!
//! Layout as in GNOME's user session: activities on the left, the clock in
//! the centre, and on the right the screen-sharing indicator, the keyboard
//! layout and the system status button.

mod activities;
mod clock;
mod keyboard;
mod screencast;
mod status;

use std::rc::Rc;

use gniri_core::Subscription;
use gtk::prelude::*;

use crate::services::Services;

/// The bar and what keeps it up to date. Dropping it stops the updates.
pub struct Bar {
    pub widget: gtk::CenterBox,
    _subscriptions: Vec<Subscription>,
}

/// Builds the bar for the output called `output` (its connector name).
pub fn build(services: &Rc<Services>, output: Option<&str>) -> Bar {
    let mut subscriptions = Vec::new();

    let side = |name: &str| gtk::Box::builder().name(name).build();
    let (left, centre, right) = (side("panelLeft"), side("panelCenter"), side("panelRight"));

    left.append(&activities::build(services, output, &mut subscriptions));
    centre.append(&clock::build(services, output, &mut subscriptions));
    right.append(&screencast::build(services, &mut subscriptions));
    right.append(&keyboard::build(services, &mut subscriptions));
    right.append(&status::build(services, output, &mut subscriptions));

    let bar = gtk::CenterBox::builder()
        .name("panel")
        .css_classes(["gniri-panel"])
        .start_widget(&left)
        .center_widget(&centre)
        .end_widget(&right)
        .build();

    // Transparent while the overview is open, like `#panel:overview`.
    subscriptions.push(services.niri.select(|niri| niri.overview_open, {
        let bar = bar.clone();
        move |open| {
            if *open {
                bar.add_css_class("overview");
            } else {
                bar.remove_css_class("overview");
            }
        }
    }));

    Bar {
        widget: bar,
        _subscriptions: subscriptions,
    }
}

/// A `.panel-button`: the pill-shaped button every panel item is.
fn button(child: &impl IsA<gtk::Widget>) -> gtk::Button {
    gtk::Button::builder()
        .css_classes(["panel-button"])
        .child(child)
        // The panel takes no keyboard focus yet.
        .focusable(false)
        .build()
}

/// Sets or clears the `:checked` look GNOME gives a panel button whose menu
/// (or the overview) is open.
fn set_checked(button: &gtk::Button, checked: bool) {
    if checked {
        button.set_state_flags(gtk::StateFlags::CHECKED, false);
    } else {
        button.unset_state_flags(gtk::StateFlags::CHECKED);
    }
}
