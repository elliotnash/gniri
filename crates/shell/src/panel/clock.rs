//! The clock in the centre of the panel. Port of `DateMenuButton` in
//! `js/ui/dateMenu.js`; the menu itself arrives in M3.

use std::rc::Rc;

use gniri_core::{Subscription, panel_clock_text};
use gtk::prelude::*;

use super::button;
use crate::services::Services;

pub fn build(services: &Rc<Services>, subscriptions: &mut Vec<Subscription>) -> gtk::Button {
    let label = gtk::Label::builder().css_classes(["clock"]).build();
    let container = gtk::Box::builder()
        .css_classes(["clock-display-box"])
        .build();
    container.append(&label);

    let button = button(&container);
    button.add_css_class("clock-display");

    let format = services.config.clock.format();
    subscriptions.push(services.time.select(
        move |time| panel_clock_text(time, &format),
        move |text| label.set_label(text),
    ));
    button
}
