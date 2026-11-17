// SPDX-License-Identifier: GPL-3.0-or-later

//! The month calendar. Port of `Calendar` in `js/ui/calendar.js`. It asks
//! the calendar's events for the days it shows, and marks those that have
//! any.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gniri_core::{Date, Subscription, Weekday, month_grid};
use gniri_services::events;
use gtk::prelude::*;

use crate::services::{Services, SystemClock};

/// Seconds since the epoch at the start of `date`, in the local time zone.
pub fn day_start(date: Date) -> i64 {
    SystemClock::unix_of(&date.midnight())
}

type DateCallback = Rc<dyn Fn(Date)>;

struct Inner {
    services: Rc<Services>,
    grid: gtk::Grid,
    month_label: gtk::Label,
    /// The 42 day buttons with the date each shows.
    days: RefCell<Vec<(gtk::Button, Date)>>,
    week_start: Weekday,
    today: Cell<Date>,
    selected: Cell<Date>,
    /// The month the day buttons were built for, and the "today" they
    /// marked.
    built: Cell<Option<(Date, Date)>>,
    selected_changed: RefCell<Option<DateCallback>>,
    events: RefCell<Option<Subscription>>,
}

#[derive(Clone)]
pub struct Calendar {
    /// `.calendar`
    pub widget: gtk::Grid,
    inner: Rc<Inner>,
}

/// Row of the first week; the heading and the weekday letters are above.
const FIRST_WEEK_ROW: i32 = 2;

fn pager_button(class: &str, icon_name: &str) -> gtk::Button {
    gtk::Button::builder()
        .css_classes([class, "pager-button"])
        .child(
            &gtk::Image::builder()
                .icon_name(icon_name)
                .halign(gtk::Align::Center)
                .valign(gtk::Align::Center)
                .build(),
        )
        .build()
}

impl Inner {
    /// `Calendar.setDate`
    fn set_date(self: &Rc<Self>, date: Date) {
        if self.selected.replace(date) == date {
            return;
        }
        self.update();
        let changed = self.selected_changed.borrow().clone();
        if let Some(changed) = changed {
            changed(date);
        }
    }

    /// `Calendar._rebuildCalendar`
    fn rebuild(self: &Rc<Self>) {
        for (button, _) in self.days.take() {
            self.grid.remove(&button);
        }
        let (selected, today) = (self.selected.get(), self.today.get());
        let mut days = Vec::new();
        for (index, day) in month_grid(selected, self.week_start)
            .into_iter()
            .enumerate()
        {
            let (row, column) = ((index / 7) as i32, (index % 7) as i32);
            let button = gtk::Button::builder()
                .label(format!("{:02}", day.date.day))
                .css_classes(["calendar-day"])
                .build();
            button.add_css_class(if day.date.weekday().is_work_day() {
                "calendar-weekday"
            } else {
                "calendar-weekend"
            });
            if row == 0 {
                button.add_css_class("calendar-day-top");
            }
            if column == 0 {
                button.add_css_class("calendar-day-left");
            }
            if day.date == today {
                button.add_css_class("calendar-today");
            } else if day.other_month {
                button.add_css_class("calendar-other-month");
            }
            button.connect_clicked({
                let inner = Rc::downgrade(self);
                move |_| {
                    if let Some(inner) = inner.upgrade() {
                        inner.set_date(day.date);
                    }
                }
            });
            self.grid
                .attach(&button, column, FIRST_WEEK_ROW + row, 1, 1);
            days.push((button, day.date));
        }
        self.days.replace(days);
        self.built
            .set(Some((Date::new(selected.year, selected.month, 1), today)));

        // `requestRange`: the events of the days that show.
        let days = self.days.borrow();
        if let (Some((_, first)), Some((_, last))) = (days.first(), days.last()) {
            let range = (day_start(*first), day_start(last.plus_days(1)));
            if self.services.events.state.with(|state| state.range) != Some(range) {
                self.services
                    .events
                    .send(events::Command::SetRange(range.0, range.1));
            }
        }
        drop(days);
        self.mark_events();
    }

    /// Marks the days that have events (`calendar-day-with-events`).
    fn mark_events(&self) {
        self.services.events.state.with(|state| {
            for (button, date) in self.days.borrow().iter() {
                let has = state.any_between(day_start(*date), day_start(date.plus_days(1)));
                if has {
                    button.add_css_class("calendar-day-with-events");
                } else {
                    button.remove_css_class("calendar-day-with-events");
                }
            }
        });
    }

    /// `Calendar._update`
    fn update(self: &Rc<Self>) {
        let (selected, today) = (self.selected.get(), self.today.get());
        self.month_label.set_label(&selected.month_heading(today));
        let month = Date::new(selected.year, selected.month, 1);
        if self.built.get() != Some((month, today)) {
            self.rebuild();
        }
        for (button, date) in self.days.borrow().iter() {
            if *date == selected {
                button.set_state_flags(gtk::StateFlags::SELECTED, false);
            } else {
                button.unset_state_flags(gtk::StateFlags::SELECTED);
            }
        }
    }
}

impl Calendar {
    /// A calendar showing `today`'s month, with weeks starting on
    /// `week_start`.
    pub fn new(services: &Rc<Services>, today: Date, week_start: Weekday) -> Self {
        let grid = gtk::Grid::builder().css_classes(["calendar"]).build();

        // `Calendar._buildHeader`
        let back = pager_button("calendar-change-month-back", "pan-start-symbolic");
        let forward = pager_button("calendar-change-month-forward", "pan-end-symbolic");
        let month_label = gtk::Label::builder()
            .css_classes(["calendar-month-label"])
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .hexpand(true)
            .build();
        let header = gtk::Box::builder()
            .css_classes(["calendar-month-header"])
            .build();
        header.append(&back);
        header.append(&month_label);
        header.append(&forward);
        grid.attach(&header, 0, 0, 7, 1);
        for column in 0..7 {
            let heading = gtk::Label::builder()
                .label(week_start.plus(column).grid_abbreviation())
                .css_classes(["calendar-day-heading"])
                .build();
            grid.attach(&heading, column as i32, 1, 1, 1);
        }

        let inner = Rc::new(Inner {
            services: Rc::clone(services),
            grid: grid.clone(),
            month_label,
            days: RefCell::default(),
            week_start,
            today: Cell::new(today),
            selected: Cell::new(today),
            built: Cell::new(None),
            selected_changed: RefCell::default(),
            events: RefCell::default(),
        });
        inner.update();
        inner.events.replace(Some(services.events.state.subscribe({
            let inner = Rc::downgrade(&inner);
            move |_| {
                if let Some(inner) = inner.upgrade() {
                    inner.mark_events();
                }
            }
        })));

        let step = |months: i32| {
            let inner = Rc::downgrade(&inner);
            move || {
                if let Some(inner) = inner.upgrade() {
                    inner.set_date(inner.selected.get().plus_months(months));
                }
            }
        };
        back.connect_clicked({
            let step = step(-1);
            move |_| step()
        });
        forward.connect_clicked({
            let step = step(1);
            move |_| step()
        });
        // `Calendar._onScroll`
        let scroll = gtk::EventControllerScroll::new(
            gtk::EventControllerScrollFlags::BOTH_AXES | gtk::EventControllerScrollFlags::DISCRETE,
        );
        scroll.connect_scroll({
            let (previous, next) = (step(-1), step(1));
            move |_, dx, dy| {
                if dx < 0.0 || dy < 0.0 {
                    previous();
                } else {
                    next();
                }
                gtk::glib::Propagation::Stop
            }
        });
        grid.add_controller(scroll);

        Self {
            widget: grid,
            inner,
        }
    }

    /// Selects `date` and shows its month.
    pub fn set_date(&self, date: Date) {
        self.inner.set_date(date);
    }

    /// Tells the calendar which day it is.
    pub fn set_today(&self, today: Date) {
        if self.inner.today.replace(today) != today {
            self.inner.update();
        }
    }

    /// Calls `changed` whenever another date is selected.
    pub fn connect_selected_date_changed(&self, changed: impl Fn(Date) + 'static) {
        self.inner.selected_changed.replace(Some(Rc::new(changed)));
    }
}
