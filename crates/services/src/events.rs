// SPDX-License-Identifier: GPL-3.0-or-later

//! Calendar events: what the date menu lists under the calendar, and which
//! days the calendar marks. Port of `DBusEventSource` (calendar.js).
//!
//! The events come from GNOME Shell's own calendar server
//! (`gnome-shell-calendar-server`), which reads the calendars Evolution
//! Data Server has: those set up in GNOME Calendar, Evolution or GNOME
//! Online Accounts. gniri talks to it as GNOME Shell does.
//!
//! Times are seconds since the epoch, as the server sends them. Which day
//! a time falls on is the shell's to say, where the time zone is known.

pub mod backend;

use serde::Deserialize;

use crate::model::Model;

/// One event, or one occurrence of a recurring one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CalendarEvent {
    /// The server's id: the calendar's, the event's and the occurrence's,
    /// each followed by a newline.
    pub id: String,
    pub summary: String,
    /// Seconds since the epoch.
    pub start: i64,
    pub end: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// There are calendars to show events of (`HasCalendars`). GNOME hides
    /// the events card while there are none.
    pub has_calendars: bool,
    /// Every event known in the range asked for, by id.
    pub events: Vec<CalendarEvent>,
    /// The span asked for: the days the calendar shows.
    pub range: Option<(i64, i64)>,
}

#[derive(Debug)]
pub enum Event {
    /// The server went away: nothing is known.
    Vanished,
    /// The events of a new span are on their way; those known so far are
    /// forgotten (`_loadEvents` with `forceReload`).
    Reloading,
    HasCalendars(bool),
    /// Events that are new or changed (`EventsAddedOrUpdated`).
    Added(Vec<CalendarEvent>),
    /// Events, or every occurrence of them, by id prefix
    /// (`EventsRemoved`).
    Removed(Vec<String>),
    /// A calendar went, and its events with it (`ClientDisappeared`).
    CalendarGone(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Asks for the events of a span (`SetTimeRange`), forgetting those
    /// known so far.
    SetRange(i64, i64),
}

impl State {
    fn remove_matching(&mut self, prefix: &str) -> bool {
        let before = self.events.len();
        self.events.retain(|event| !event.id.starts_with(prefix));
        self.events.len() != before
    }

    /// The events that overlap `[begin, end)`, in the order the menu lists
    /// them: by start, or by end for one that started before (`getEvents`
    /// and `_eventOverlapsInterval`, where an event of no length at the
    /// start counts).
    pub fn between(&self, begin: i64, end: i64) -> Vec<&CalendarEvent> {
        let overlaps = |event: &&CalendarEvent| {
            (event.start >= begin && event.end < end) || (event.end > begin && event.start < end)
        };
        let mut found: Vec<&CalendarEvent> = self.events.iter().filter(overlaps).collect();
        let key = |event: &CalendarEvent| {
            if event.start < begin && event.end <= end {
                event.end
            } else {
                event.start
            }
        };
        found.sort_by_key(|event| key(event));
        found
    }

    /// Whether anything happens in `[begin, end)` (`hasEvents`).
    pub fn any_between(&self, begin: i64, end: i64) -> bool {
        !self.between(begin, end).is_empty()
    }
}

impl Model for State {
    type Event = Event;
    type Command = Command;

    fn apply(&mut self, event: Event) {
        match event {
            Event::Vanished => {
                self.events.clear();
                self.has_calendars = false;
            }
            Event::Reloading => self.events.clear(),
            Event::HasCalendars(has) => self.has_calendars = has,
            Event::Added(events) => {
                // An occurrence replaces every other of the same event that
                // came before it in this batch, once.
                let mut handled = Vec::new();
                for event in events {
                    if !event.id.ends_with('\n')
                        && let Some(cut) = event.id.rfind('\n')
                    {
                        let parent = event.id[..=cut].to_owned();
                        if !handled.contains(&parent) {
                            self.remove_matching(&parent);
                            handled.push(parent);
                        }
                    }
                    self.events.retain(|other| other.id != event.id);
                    self.events.push(event);
                }
            }
            Event::Removed(ids) => {
                for id in ids {
                    self.remove_matching(&id);
                }
            }
            Event::CalendarGone(source) => {
                self.remove_matching(&format!("{source}\n"));
            }
        }
    }

    fn predict(&mut self, command: &Command) {
        match command {
            Command::SetRange(begin, end) => self.range = Some((*begin, *end)),
        }
    }
}

/// A scene's calendar: whether there are calendars, and events at local
/// times, which the shell turns into seconds in the session's time zone.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Fixture {
    pub has_calendars: bool,
    pub events: Vec<FixtureEvent>,
}

impl Default for Fixture {
    /// A scene has GNOME's calendar, with nothing in it: the captures show
    /// the card that says "No Events".
    fn default() -> Self {
        Self {
            has_calendars: true,
            events: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FixtureEvent {
    pub summary: String,
    /// `YYYY-MM-DDTHH:MM:SS`, local time.
    pub start: String,
    pub end: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(id: &str, summary: &str, start: i64, end: i64) -> CalendarEvent {
        CalendarEvent {
            id: id.to_owned(),
            summary: summary.to_owned(),
            start,
            end,
        }
    }

    #[test]
    fn events_come_and_go_by_their_ids() {
        let mut state = State::default();
        state.apply(Event::HasCalendars(true));
        state.apply(Event::Added(vec![
            event("cal\nlunch\n", "Lunch", 100, 200),
            event("cal\nstandup\n20261114", "Standup", 300, 310),
            event("cal\nstandup\n20261115", "Standup", 400, 410),
            event("other\nparty\n", "Party", 500, 600),
        ]));
        assert_eq!(state.events.len(), 4);
        // A later batch with one occurrence of the recurring one replaces
        // all of its occurrences.
        state.apply(Event::Added(vec![event(
            "cal\nstandup\n20261116",
            "Standup",
            500,
            510,
        )]));
        let ids: Vec<&str> = state.events.iter().map(|event| event.id.as_str()).collect();
        assert_eq!(
            ids,
            ["cal\nlunch\n", "other\nparty\n", "cal\nstandup\n20261116"]
        );
        // An update of one that is there replaces it.
        state.apply(Event::Added(vec![event(
            "cal\nlunch\n",
            "Late lunch",
            150,
            250,
        )]));
        assert!(
            state
                .events
                .iter()
                .any(|event| event.summary == "Late lunch")
        );
        assert_eq!(state.events.len(), 3);

        state.apply(Event::Removed(vec!["cal\nstandup\n".to_owned()]));
        assert_eq!(state.events.len(), 2);
        state.apply(Event::CalendarGone("other".to_owned()));
        assert_eq!(state.events.len(), 1);
        state.apply(Event::Vanished);
        assert_eq!(state, State::default());
    }

    #[test]
    fn a_new_range_forgets_what_was_known_once_the_server_reloads() {
        let mut state = State {
            events: vec![event("a\nb\n", "B", 1, 2)],
            ..State::default()
        };
        state.predict(&Command::SetRange(0, 10));
        assert_eq!(state.range, Some((0, 10)));
        assert_eq!(state.events.len(), 1, "a scene's events stay");
        state.apply(Event::Reloading);
        assert!(state.events.is_empty());
    }

    #[test]
    fn a_day_lists_what_overlaps_it() {
        let state = State {
            events: vec![
                event("c\nlate\n", "Late", 50, 60),
                event("c\nearly\n", "Early", 20, 30),
                // Began the day before, ends in it: sorted by its end.
                event("c\novernight\n", "Overnight", -10, 25),
                event("c\nnext\n", "Next day", 100, 110),
                // No length, at the very start: still today's.
                event("c\nmark\n", "Mark", 0, 0),
                // Ends as the day begins: yesterday's.
                event("c\nyesterday\n", "Yesterday", -20, 0),
            ],
            ..State::default()
        };
        let names: Vec<&str> = state
            .between(0, 100)
            .iter()
            .map(|event| event.summary.as_str())
            .collect();
        assert_eq!(names, ["Mark", "Early", "Overnight", "Late"]);
        assert!(state.any_between(100, 200));
        assert!(!state.any_between(200, 300));
    }
}
