// SPDX-License-Identifier: GPL-3.0-or-later

//! The screen-sharing indicator. Port of `ScreenSharingIndicator` in
//! `js/ui/status/remoteAccess.js`: an orange button, shown while something
//! is capturing the screen, that stops the capture when clicked.

use gniri_core::tr;
use std::rc::Rc;

use gniri_core::Subscription;
use gtk::prelude::*;

use super::button;
use crate::services::Services;

pub fn build(services: &Rc<Services>, subscriptions: &mut Vec<Subscription>) -> gtk::Button {
    let icons = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    icons.append(&gtk::Image::from_icon_name("screen-shared-symbolic"));
    icons.append(&gtk::Image::from_icon_name("screencast-stop-symbolic"));

    let button = button(&icons);
    button.add_css_class("screen-sharing-indicator");
    button.update_property(&[gtk::accessible::Property::Label(&tr("Stop Screen Sharing"))]);
    button.connect_clicked({
        let services = Rc::clone(services);
        move |_| {
            let Some(client) = &services.niri_client else {
                return;
            };
            for session in services.niri.with(|niri| niri.screencast_sessions.clone()) {
                if let Err(error) = client.stop_screencast(session) {
                    eprintln!("gniri: cannot stop screen sharing: {error}");
                }
            }
        }
    });

    subscriptions.push(
        services
            .niri
            .select(|niri| !niri.screencast_sessions.is_empty(), {
                let button = button.clone();
                move |sharing| button.set_visible(*sharing)
            }),
    );
    button
}
