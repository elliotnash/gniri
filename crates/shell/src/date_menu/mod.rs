// SPDX-License-Identifier: GPL-3.0-or-later

//! The date menu: notifications on the left, the calendar on the right.
//! Port of `DateMenuButton`'s menu in `js/ui/dateMenu.js`.

mod calendar;

use std::rc::Rc;

use gniri_core::{Date, Subscription, tr};
use gniri_services::notifications::Command;
use gtk::prelude::*;

use self::calendar::Calendar;
use crate::messages::MessageList;
use crate::services::{Services, SystemClock};

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

/// The world clocks, nearest offset first (`WorldClocksSection._clocksChanged`
/// and `_updateTimeLabels`): a city, its time and how far ahead of the
/// local time it is. Without any, an offer to add some.
fn fill_world_clocks(services: &Services, grid: &gtk::Grid) {
    while let Some(child) = grid.first_child() {
        grid.remove(&child);
    }
    let hour12 = services
        .config
        .with(|config| config.clock.format == gniri_config::HourFormat::H12);
    let now = gtk::glib::DateTime::now_local().expect("the local time is representable");
    let local_offset = now.utc_offset().as_seconds();
    let mut clocks: Vec<(i64, String, gniri_core::LocalTime, i64)> =
        services.world_clocks.with(|clocks| {
            clocks
                .iter()
                .filter_map(|clock| {
                    // An unknown name gives UTC, whose name is "UTC".
                    let zone = gtk::glib::TimeZone::new(Some(&clock.zone));
                    if zone.identifier() != clock.zone.as_str() {
                        return None;
                    }
                    let there = now.to_timezone(&zone).ok()?;
                    let interval = zone.find_interval(gtk::glib::TimeType::Standard, now.to_unix());
                    let standard = i64::from(zone.offset(interval));
                    let time = gniri_core::LocalTime {
                        year: there.year(),
                        month: there.month() as u8,
                        day: there.day_of_month() as u8,
                        hour: there.hour() as u8,
                        minute: there.minute() as u8,
                        second: there.second() as u8,
                    };
                    let ahead = there.utc_offset().as_seconds() - local_offset;
                    Some((standard, clock.city.clone(), time, ahead))
                })
                .collect()
        });
    // Under a scene the clock stands still at the scene's time.
    if services.sandboxed {
        let scene = services.time.get();
        for (_, _, time, ahead) in &mut clocks {
            let unix = SystemClock::unix_of(&scene) + *ahead;
            *time = SystemClock::local_of(unix);
        }
    }
    clocks.sort_by_key(|(standard, ..)| *standard);

    let header = header_label(
        &["world-clocks-header"],
        &if clocks.is_empty() {
            tr("Add World Clocks…")
        } else {
            tr("World Clocks")
        },
    );
    if clocks.is_empty() {
        header.add_css_class("no-world-clocks");
    }
    grid.attach(&header, 0, 0, 2, 1);
    for (row, (_, city, time, ahead)) in clocks.iter().enumerate() {
        let row = row as i32 + 1;
        let city = header_label(&["world-clocks-city"], city);
        city.set_hexpand(true);
        city.set_ellipsize(gtk::pango::EllipsizeMode::End);
        city.set_max_width_chars(1);
        let time = gtk::Label::builder()
            .label(gniri_core::calendar::time_of_day_text(time, hour12))
            .css_classes(["world-clocks-time"])
            .xalign(1.0)
            .build();
        let zone = gtk::Label::builder()
            .label(gniri_core::offset_text(*ahead))
            .css_classes(["world-clocks-timezone"])
            .xalign(1.0)
            .build();
        grid.attach(&city, 0, row, 1, 1);
        grid.attach(&time, 1, row, 1, 1);
        grid.attach(&zone, 2, row, 1, 1);
    }
}

/// Lists `day`'s events in `list` (`EventsSection._reloadEvents`).
fn fill_events(services: &Services, list: &gtk::Box, day: Date) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    let (begin, end) = (
        calendar::day_start(day),
        calendar::day_start(day.plus_days(1)),
    );
    let hour12 = services
        .config
        .with(|config| config.clock.format == gniri_config::HourFormat::H12);
    let this_year = services.time.with(|time| time.year);
    let shown = services.events.state.with(|state| {
        state
            .between(begin, end)
            .into_iter()
            .map(|event| {
                let (start, end) = (
                    SystemClock::local_of(event.start),
                    SystemClock::local_of(event.end),
                );
                (
                    event.summary.clone(),
                    gniri_core::event_time_text(&start, &end, day, this_year, hour12),
                )
            })
            .collect::<Vec<_>>()
    });
    for (summary, time) in &shown {
        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["event-box"])
            .build();
        let summary = header_label(&["event-summary"], summary);
        summary.set_ellipsize(gtk::pango::EllipsizeMode::End);
        summary.set_max_width_chars(1);
        row.append(&summary);
        row.append(&header_label(&["event-time"], time));
        list.append(&row);
    }
    if shown.is_empty() {
        list.append(&header_label(&["event-placeholder"], &tr("No Events")));
    }
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
    let calendar = Calendar::new(services, today, week_start);

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

    // `EventsSection`: the selected day's events. Without a calendar
    // application to open it is only a label; without calendars it does
    // not show.
    let events_title = header_label(&["events-title"], &tr("Today"));
    let events_list = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .css_classes(["events-list"])
        .build();
    let selected_day = Rc::new(std::cell::Cell::new(today));
    let list_events = {
        let (services, list, selected_day) = (
            Rc::clone(services),
            events_list.downgrade(),
            Rc::clone(&selected_day),
        );
        move || {
            let Some(list) = list.upgrade() else {
                return;
            };
            fill_events(&services, &list, selected_day.get());
        }
    };
    list_events();
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
    subscriptions.push(services.events.state.select(|state| state.has_calendars, {
        let events = events.downgrade();
        move |has| {
            if let Some(events) = events.upgrade() {
                events.set_visible(*has);
            }
        }
    }));
    subscriptions.push(services.events.state.select(|state| state.events.clone(), {
        let list_events = list_events.clone();
        move |_| list_events()
    }));
    subscriptions.push(services.config.select(|config| config.clock.format, {
        let list_events = list_events.clone();
        move |_| list_events()
    }));

    // `WorldClocksSection` and `WeatherSection`, as GNOME shows them before
    // anything is set up: they start the application that would do so.
    let displays = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .css_classes(["datemenu-displays-box"])
        .build();
    displays.append(&events);
    if settings.world_clocks && services.has_app(CLOCKS_APP) {
        let grid = gtk::Grid::builder()
            .css_classes(["world-clocks-grid"])
            .hexpand(true)
            .build();
        displays.append(&card(
            services,
            "world-clocks-button",
            CLOCKS_APP,
            grid.upcast_ref(),
        ));
        let fill = {
            let (services, grid) = (Rc::clone(services), grid.downgrade());
            move || {
                if let Some(grid) = grid.upgrade() {
                    fill_world_clocks(&services, &grid);
                }
            }
        };
        fill();
        subscriptions.push(services.world_clocks.subscribe({
            let fill = fill.clone();
            move |_| fill()
        }));
        // Each minute, and with another clock format.
        subscriptions.push(services.time.select(|time| (time.hour, time.minute), {
            let fill = fill.clone();
            move |_| fill()
        }));
        subscriptions.push(
            services
                .config
                .select(|config| config.clock.format, move |_| fill()),
        );
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
            selected_day.set(selected);
            list_events();
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
