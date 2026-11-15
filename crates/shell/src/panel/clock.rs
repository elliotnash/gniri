//! The clock in the centre of the panel. Port of `DateMenuButton` and
//! `MessagesIndicator` in `js/ui/dateMenu.js`. Clicking it opens the date
//! menu (crates/shell/src/date_menu).

use std::rc::Rc;

use gniri_core::{Subscription, panel_clock_text};
use gtk::prelude::*;

use super::{button, set_checked};
use crate::menus::Menu;
use crate::services::Services;

pub fn build(
    services: &Rc<Services>,
    output: Option<&str>,
    subscriptions: &mut Vec<Subscription>,
) -> gtk::Button {
    let label = gtk::Label::builder().css_classes(["clock"]).build();
    // The dot that says something has not been seen, and an empty box as
    // wide on the other side, so the clock stays in the middle.
    let indicator = gtk::Image::builder()
        .icon_name("message-indicator-symbolic")
        .css_classes(["messages-indicator"])
        .valign(gtk::Align::Center)
        .visible(false)
        .build();
    let indicator_pad = gtk::Image::builder()
        .css_classes(["messages-indicator"])
        .visible(false)
        .build();
    let container = gtk::Box::builder()
        .css_classes(["clock-display-box"])
        .build();
    container.append(&indicator_pad);
    container.append(&label);
    container.append(&indicator);

    let button = button(&container);
    button.add_css_class("clock-display");

    // What it says changes with the time and with the format, which is a
    // setting.
    let show = {
        let services = Rc::downgrade(services);
        move || {
            let Some(services) = services.upgrade() else {
                return;
            };
            let format = services.config.with(|config| config.clock.format());
            let text = services.time.with(|time| panel_clock_text(time, &format));
            label.set_label(&text);
        }
    };
    subscriptions.push(services.time.select(|time| *time, {
        let show = show.clone();
        move |_| show()
    }));
    subscriptions.push(
        services
            .config
            .select(|config| config.clock.clone(), move |_| show()),
    );
    subscriptions.push(services.notifications.state.select(
        |state| state.shows_indicator(),
        move |shown| {
            indicator.set_visible(*shown);
            indicator_pad.set_visible(*shown);
        },
    ));

    let output: Option<String> = output.map(str::to_owned);
    button.connect_clicked({
        let (services, output) = (Rc::clone(services), output.clone());
        move |_| {
            services
                .menus
                .update(|menus| menus.toggle(Menu::DateMenu, output.as_deref()));
        }
    });
    subscriptions.push(services.menus.select(
        move |menus| menus.is_open(Menu::DateMenu, output.as_deref()),
        {
            let button = button.clone();
            move |open| set_checked(&button, *open)
        },
    ));
    button
}
