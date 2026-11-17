// SPDX-License-Identifier: GPL-3.0-or-later

//! The date menu: notifications on the left, the calendar on the right.
//! Port of `DateMenuButton`'s menu in `js/ui/dateMenu.js`.

mod calendar;

use gniri_core::tr;
use std::rc::Rc;

use gniri_core::{Date, Subscription};
use gniri_services::notifications::Command;
use gtk::prelude::*;

use self::calendar::Calendar;
use crate::messages::MessageList;
use crate::services::Services;

/// The applications the cards under the calendar start, by desktop entry.
const CALENDAR_APP: &str = "org.gnome.Calendar";
const CLOCKS_APP: &str = "org.gnome.clocks";
const WEATHER_APP: &str = "org.gnome.Weather";

/// The menu and what keeps it up to date. Dropping it stops the updates.
pub struct DateMenu {
    /// `.popup-menu-content.datemenu-popover`
    pub widget: gtk::Box,
    services: Rc<Services>,
    calendar: Calendar,
    _messages: Rc<MessageList>,
    _subscriptions: Vec<Subscription>,
}

/// One of the cards under the calendar: a button that starts `app` and
/// closes the menu.
fn card(
    services: &Rc<Services>,
    class: &str,
    app: &'static str,
    child: &gtk::Widget,
) -> gtk::Button {
    let button = gtk::Button::builder()
        .css_classes([class])
        .hexpand(true)
        .child(child)
        .build();
    button.connect_clicked({
        let services = Rc::clone(services);
        move |_| {
            services.menus.update(|menus| menus.close());
            services.launch_app(app);
        }
    });
    button
}

fn header_label(classes: &[&str], text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .css_classes(classes)
        .xalign(0.0)
        .build()
}

pub fn build(services: &Rc<Services>) -> DateMenu {
    let mut subscriptions = Vec::new();
    let today = services.time.with(|time| time.date());

    // The menu is built anew when these settings change
    // (crates/shell/src/outputs.rs).
    let settings = services.config.with(|config| config.date_menu.clone());
    // The config file's day, else the locale's, else GNOME's own
    // fallback.
    let week_start = settings
        .week_start
        .map(gniri_config::WeekStart::weekday)
        .or_else(crate::i18n::week_start)
        .unwrap_or(gniri_core::Weekday::Sunday);
    let calendar = Calendar::new(today, week_start);

    // `TodayButton`: the date in words. It takes the calendar back to
    // today, so it only reacts while another day is selected.
    let day_label = gtk::Label::builder()
        .css_classes(["day-label"])
        .halign(gtk::Align::Start)
        .build();
    let date_label = gtk::Label::builder()
        .css_classes(["date-label"])
        .halign(gtk::Align::Start)
        .build();
    let today_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    today_box.append(&day_label);
    today_box.append(&date_label);
    let today_button = gtk::Button::builder()
        .css_classes(["datemenu-today-button"])
        .hexpand(true)
        .sensitive(false)
        .child(&today_box)
        .build();
    today_button.connect_clicked({
        let (services, calendar) = (Rc::clone(services), calendar.clone());
        move |_| calendar.set_date(services.time.with(|time| time.date()))
    });

    // `EventsSection`, as GNOME shows it with no events. Without a
    // calendar application to open it is only a label.
    let events_title = header_label(&["events-title"], &tr("Today"));
    let events_list = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .css_classes(["events-list"])
        .build();
    events_list.append(&header_label(&["event-placeholder"], &tr("No Events")));
    let events_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .css_classes(["events-box"])
        .build();
    events_box.append(&events_title);
    events_box.append(&events_list);
    let events = card(
        services,
        "events-button",
        CALENDAR_APP,
        events_box.upcast_ref(),
    );
    events.set_can_target(services.has_app(CALENDAR_APP));

    // `WorldClocksSection` and `WeatherSection`, as GNOME shows them before
    // anything is set up: they start the application that would do so.
    let displays = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .css_classes(["datemenu-displays-box"])
        .build();
    displays.append(&events);
    if settings.world_clocks && services.has_app(CLOCKS_APP) {
        let header = header_label(
            &["world-clocks-header", "no-world-clocks"],
            &tr("Add World Clocks…"),
        );
        displays.append(&card(
            services,
            "world-clocks-button",
            CLOCKS_APP,
            header.upcast_ref(),
        ));
    }
    if settings.weather && services.has_app(WEATHER_APP) {
        let header = header_label(
            &["weather-header", "no-location"],
            &tr("Select Weather Location…"),
        );
        displays.append(&card(
            services,
            "weather-button",
            WEATHER_APP,
            header.upcast_ref(),
        ));
    }

    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .css_classes(["datemenu-calendar-column"])
        .build();
    column.append(&today_button);
    column.append(&calendar.widget);
    column.append(&displays);

    let messages = Rc::new(MessageList::new(services));
    let area = gtk::Box::builder().name("calendarArea").build();
    area.append(&messages.root);
    area.append(&column);

    let widget = gtk::Box::builder()
        .css_classes(["popup-menu-content", "datemenu-popover"])
        .build();
    widget.append(&area);

    // The selected day decides the events card's title and whether the
    // today button has anything to do.
    calendar.connect_selected_date_changed({
        // Held weakly: the today button holds the calendar.
        let (services, events_title, today_button) = (
            Rc::clone(services),
            events_title.downgrade(),
            today_button.downgrade(),
        );
        move |selected: Date| {
            let today = services.time.with(|time| time.date());
            if let Some(events_title) = events_title.upgrade() {
                events_title.set_label(&selected.events_title(today));
            }
            if let Some(today_button) = today_button.upgrade() {
                today_button.set_sensitive(selected != today);
            }
        }
    });
    subscriptions.push(services.time.select(|time| time.date(), {
        let calendar = calendar.clone();
        move |today: &Date| {
            day_label.set_label(&today.weekday().name());
            date_label.set_label(&today.long_text());
            calendar.set_today(*today);
        }
    }));

    // `CalendarMessageList.setCaptureContainer`: with a group open, a click
    // anywhere else in the menu only closes the group.
    let click = gtk::GestureClick::builder()
        .button(0)
        .propagation_phase(gtk::PropagationPhase::Capture)
        .build();
    click.connect_pressed({
        let (widget, list) = (widget.downgrade(), Rc::downgrade(&messages));
        let services = Rc::clone(services);
        move |click, _, x, y| {
            let (Some(widget), Some(list)) = (widget.upgrade(), list.upgrade()) else {
                return;
            };
            let Some(group) = list.expanded_group() else {
                return;
            };
            let inside = widget
                .pick(x, y, gtk::PickFlags::DEFAULT)
                .is_some_and(|picked| picked == group || picked.is_ancestor(&group));
            if !inside {
                services.menus.update(|menus| menus.set_group(None));
                click.set_state(gtk::EventSequenceState::Claimed);
            }
        }
    });
    widget.add_controller(click);

    DateMenu {
        widget,
        services: Rc::clone(services),
        calendar,
        _messages: messages,
        _subscriptions: subscriptions,
    }
}

impl DateMenu {
    /// Called each time the menu opens (`open-state-changed`).
    pub fn opened(&self) {
        self.calendar
            .set_date(self.services.time.with(|time| time.date()));
        // `NotificationMessageGroup.vfunc_map`: what is listed is seen.
        let unseen = self.services.notifications.state.with(|state| {
            state
                .notifications()
                .any(|notification| !notification.acknowledged)
        });
        if unseen {
            self.services.notifications.send(Command::AcknowledgeAll);
        }
    }
}
