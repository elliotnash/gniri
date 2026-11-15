//! Dates for the date menu: the month grid, stepping between months and the
//! texts GNOME Shell writes dates and ages with, in English.
//!
//! Ports of the date arithmetic in gnome-shell's `js/ui/calendar.js`,
//! `js/ui/dateMenu.js` and `js/misc/dateUtils.js`.

use crate::clock::{LocalTime, Weekday};

/// A day in the proleptic Gregorian calendar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    pub year: i32,
    /// 1 to 12.
    pub month: u8,
    /// 1 to 31.
    pub day: u8,
}

const WEEKDAYS: [Weekday; 7] = [
    Weekday::Sunday,
    Weekday::Monday,
    Weekday::Tuesday,
    Weekday::Wednesday,
    Weekday::Thursday,
    Weekday::Friday,
    Weekday::Saturday,
];

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

impl Weekday {
    /// 0 for Sunday to 6 for Saturday, as JavaScript's `getDay`.
    pub fn index(self) -> u8 {
        WEEKDAYS
            .iter()
            .position(|other| *other == self)
            .expect("every weekday is listed") as u8
    }

    /// The weekday `days` after this one.
    pub fn plus(self, days: i64) -> Self {
        WEEKDAYS[(i64::from(self.index()) + days).rem_euclid(7) as usize]
    }

    /// "Saturday".
    pub fn name(self) -> &'static str {
        match self {
            Self::Sunday => "Sunday",
            Self::Monday => "Monday",
            Self::Tuesday => "Tuesday",
            Self::Wednesday => "Wednesday",
            Self::Thursday => "Thursday",
            Self::Friday => "Friday",
            Self::Saturday => "Saturday",
        }
    }

    /// The one-letter heading of a calendar column
    /// (`_getCalendarDayAbbreviation`).
    pub fn grid_abbreviation(self) -> &'static str {
        &self.name()[..1]
    }

    /// GNOME's `calendar-no-work` days in English: Saturday and Sunday.
    pub fn is_work_day(self) -> bool {
        !matches!(self, Self::Saturday | Self::Sunday)
    }
}

pub fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 31,
    }
}

impl Date {
    pub fn new(year: i32, month: u8, day: u8) -> Self {
        Self { year, month, day }
    }

    /// Days since 1970-01-01 (Howard Hinnant's `days_from_civil`).
    pub fn day_number(self) -> i64 {
        let year = i64::from(self.year) - i64::from(self.month <= 2);
        let era = year.div_euclid(400);
        let year_of_era = year.rem_euclid(400);
        let month = i64::from(self.month);
        let day_of_year =
            (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(self.day) - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        era * 146_097 + day_of_era - 719_468
    }

    /// The inverse of [`Date::day_number`].
    pub fn from_day_number(number: i64) -> Self {
        let shifted = number + 719_468;
        let era = shifted.div_euclid(146_097);
        let day_of_era = shifted.rem_euclid(146_097);
        let year_of_era =
            (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let shifted_month = (5 * day_of_year + 2) / 153;
        let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
        let month = if shifted_month < 10 {
            shifted_month + 3
        } else {
            shifted_month - 9
        };
        let year = year_of_era + era * 400 + i64::from(month <= 2);
        Self {
            year: year as i32,
            month: month as u8,
            day: day as u8,
        }
    }

    pub fn plus_days(self, days: i64) -> Self {
        Self::from_day_number(self.day_number() + days)
    }

    pub fn weekday(self) -> Weekday {
        // 1970-01-01 was a Thursday.
        Weekday::Thursday.plus(self.day_number())
    }

    /// The same day one month earlier or later, or that month's last day if
    /// it is shorter (`Calendar._onPrevMonthButtonClicked` and its twin).
    pub fn plus_months(self, months: i32) -> Self {
        let index = self.year * 12 + i32::from(self.month) - 1 + months;
        let (year, month) = (index.div_euclid(12), index.rem_euclid(12) as u8 + 1);
        Self {
            year,
            month,
            day: self.day.min(days_in_month(year, month)),
        }
    }

    pub fn month_name(self) -> &'static str {
        MONTHS[usize::from(self.month) - 1]
    }

    /// "November 14 2026": the date under the weekday at the top of the
    /// date menu (`TodayButton.setDate`, format `%B %-d %Y`).
    pub fn long_text(self) -> String {
        format!("{} {} {}", self.month_name(), self.day, self.year)
    }

    /// The calendar's heading: the month, with the year unless it is this
    /// year (`Calendar._update`).
    pub fn month_heading(self, today: Date) -> String {
        if self.year == today.year {
            self.month_name().to_owned()
        } else {
            format!("{} {}", self.month_name(), self.year)
        }
    }

    /// The title of the events card for this day
    /// (`EventsSection._updateTitle`).
    pub fn events_title(self, today: Date) -> String {
        match self.day_number() - today.day_number() {
            0 => "Today".to_owned(),
            -1 => "Yesterday".to_owned(),
            1 => "Tomorrow".to_owned(),
            _ if self.year == today.year => format!("{} {}", self.month_name(), self.day),
            _ => self.long_text(),
        }
    }
}

impl LocalTime {
    pub fn date(&self) -> Date {
        Date {
            year: self.year,
            month: self.month,
            day: self.day,
        }
    }

    /// Seconds from `earlier` to this moment, both read as the same offset
    /// from UTC. Across a daylight-saving change that is an hour out, which
    /// only ever shifts when "2 hours ago" turns into "3 hours ago".
    pub fn seconds_since(&self, earlier: &LocalTime) -> i64 {
        let seconds = |time: &LocalTime| {
            time.date().day_number() * 86_400
                + i64::from(time.hour) * 3600
                + i64::from(time.minute) * 60
                + i64::from(time.second)
        };
        seconds(self) - seconds(earlier)
    }
}

/// One cell of the month grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridDay {
    pub date: Date,
    /// Not in the month the grid shows.
    pub other_month: bool,
}

/// The six weeks the calendar shows for the month of `shown`, starting each
/// week on `week_start`. Port of `Calendar._rebuildCalendar`.
///
/// A month that begins on the first day of the week is preceded by a whole
/// week of the month before, so the grid never starts flush with the 1st.
pub fn month_grid(shown: Date, week_start: Weekday) -> Vec<GridDay> {
    let first = Date::new(shown.year, shown.month, 1);
    let days_to_week_start =
        (7 + i64::from(first.weekday().index()) - i64::from(week_start.index())) % 7;
    let padding = if days_to_week_start == 0 { 7 } else { 0 };
    let begin = first.plus_days(-(padding + days_to_week_start));
    (0..42)
        .map(|offset| {
            let date = begin.plus_days(offset);
            GridDay {
                date,
                other_month: date.month != shown.month || date.year != shown.year,
            }
        })
        .collect()
}

/// How long ago something happened, as GNOME writes it under a
/// notification. Port of `formatTimeSpan` in `js/misc/dateUtils.js`.
pub fn time_span_text(seconds_ago: i64) -> String {
    let seconds = seconds_ago.max(0) as f64;
    let minutes = seconds / 60.0;
    let hours = seconds / 3600.0;
    let days = seconds / 86_400.0;
    let weeks = days / 7.0;
    let months = days / 30.0;
    let years = weeks / 52.0;

    // `%d` of a fraction: the whole part.
    let count = |value: f64, unit: &str| {
        let value = value as i64;
        let plural = if value == 1 { "" } else { "s" };
        format!("{value} {unit}{plural} ago")
    };
    if minutes < 5.0 {
        "Just now".to_owned()
    } else if hours < 1.0 {
        count(minutes, "minute")
    } else if days < 1.0 {
        count(hours, "hour")
    } else if days < 2.0 {
        "Yesterday".to_owned()
    } else if days < 15.0 {
        count(days, "day")
    } else if weeks < 8.0 {
        count(weeks, "week")
    } else if years < 1.0 {
        count(months, "month")
    } else {
        count(years, "year")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u8, day: u8) -> Date {
        Date::new(year, month, day)
    }

    #[test]
    fn day_numbers_round_trip() {
        assert_eq!(date(1970, 1, 1).day_number(), 0);
        assert_eq!(date(2026, 11, 14).day_number(), 20_771);
        for number in [-800_000, -1, 0, 59, 60, 11_016, 20_771, 2_000_000] {
            assert_eq!(Date::from_day_number(number).day_number(), number);
        }
        assert_eq!(date(2024, 2, 28).plus_days(2), date(2024, 3, 1));
        assert_eq!(date(2026, 1, 1).plus_days(-1), date(2025, 12, 31));
    }

    #[test]
    fn weekdays_agree_with_the_clock() {
        for (year, month, day) in [(2026, 11, 14), (2026, 1, 15), (2000, 2, 29), (1969, 7, 20)] {
            let time = LocalTime {
                year,
                month,
                day,
                hour: 0,
                minute: 0,
                second: 0,
            };
            assert_eq!(date(year, month, day).weekday(), time.weekday());
        }
        assert_eq!(Weekday::Saturday.plus(1), Weekday::Sunday);
        assert_eq!(Weekday::Sunday.plus(-1), Weekday::Saturday);
    }

    /// The grid GNOME Shell 51.1 showed on 2026-11-14 with the week starting
    /// on Sunday (tests/visual/reference/gnome-51.1): November begins on a
    /// Sunday, so the first row is the last week of October.
    #[test]
    fn grid_matches_the_reference_capture() {
        let grid = month_grid(date(2026, 11, 14), Weekday::Sunday);
        assert_eq!(grid.len(), 42);
        assert_eq!(grid[0].date, date(2026, 10, 25));
        assert!(grid[..7].iter().all(|day| day.other_month));
        assert_eq!(grid[7].date, date(2026, 11, 1));
        assert!(!grid[7].other_month);
        assert_eq!(grid[36].date, date(2026, 11, 30));
        assert_eq!(grid[37].date, date(2026, 12, 1));
        assert!(grid[37].other_month);
        assert_eq!(grid[41].date, date(2026, 12, 5));
    }

    #[test]
    fn grid_with_other_week_starts_and_months() {
        // Monday start: November 2026 begins six days into the week.
        let grid = month_grid(date(2026, 11, 1), Weekday::Monday);
        assert_eq!(grid[0].date, date(2026, 10, 26));
        assert_eq!(grid[6].date, date(2026, 11, 1));
        // February 2026 begins on a Sunday too.
        let grid = month_grid(date(2026, 2, 10), Weekday::Sunday);
        assert_eq!(grid[0].date, date(2026, 1, 25));
        assert_eq!(grid[41].date, date(2026, 3, 7));
        // Every row starts on the week's first day.
        for start in [Weekday::Sunday, Weekday::Monday, Weekday::Saturday] {
            let grid = month_grid(date(2027, 5, 31), start);
            assert!(grid.chunks(7).all(|week| week[0].date.weekday() == start));
            assert!(grid.iter().any(|day| day.date == date(2027, 5, 1)));
            assert!(grid.iter().any(|day| day.date == date(2027, 5, 31)));
        }
    }

    #[test]
    fn stepping_months_keeps_the_day_where_it_can() {
        assert_eq!(date(2026, 11, 14).plus_months(1), date(2026, 12, 14));
        assert_eq!(date(2026, 12, 14).plus_months(1), date(2027, 1, 14));
        assert_eq!(date(2026, 1, 31).plus_months(1), date(2026, 2, 28));
        assert_eq!(date(2024, 3, 31).plus_months(-1), date(2024, 2, 29));
        assert_eq!(date(2026, 1, 15).plus_months(-1), date(2025, 12, 15));
    }

    #[test]
    fn texts() {
        let today = date(2026, 11, 14);
        assert_eq!(today.long_text(), "November 14 2026");
        assert_eq!(today.weekday().name(), "Saturday");
        assert_eq!(today.month_heading(today), "November");
        assert_eq!(date(2027, 1, 3).month_heading(today), "January 2027");
        assert_eq!(today.events_title(today), "Today");
        assert_eq!(date(2026, 11, 13).events_title(today), "Yesterday");
        assert_eq!(date(2026, 11, 15).events_title(today), "Tomorrow");
        assert_eq!(date(2026, 11, 20).events_title(today), "November 20");
        assert_eq!(date(2027, 2, 3).events_title(today), "February 3 2027");
        let headings: Vec<&str> = (0..7)
            .map(|day| Weekday::Sunday.plus(day).grid_abbreviation())
            .collect();
        assert_eq!(headings, ["S", "M", "T", "W", "T", "F", "S"]);
    }

    /// The labels GNOME showed in the reference capture, taken at 22:10 and
    /// at 08:28 the next morning, for a notification from 22:09.
    #[test]
    fn time_spans() {
        let time = |text: &str| LocalTime::parse(text).unwrap();
        let sent = time("2026-11-13T22:09:30");
        assert_eq!(
            time_span_text(time("2026-11-13T22:10:00").seconds_since(&sent)),
            "Just now"
        );
        assert_eq!(
            time_span_text(time("2026-11-14T08:28:00").seconds_since(&sent)),
            "10 hours ago"
        );

        assert_eq!(time_span_text(-5), "Just now");
        assert_eq!(time_span_text(5 * 60), "5 minutes ago");
        assert_eq!(time_span_text(59 * 60 + 59), "59 minutes ago");
        assert_eq!(time_span_text(3600), "1 hour ago");
        assert_eq!(time_span_text(86_400), "Yesterday");
        assert_eq!(time_span_text(2 * 86_400), "2 days ago");
        assert_eq!(time_span_text(15 * 86_400), "2 weeks ago");
        assert_eq!(time_span_text(60 * 86_400), "2 months ago");
        assert_eq!(time_span_text(400 * 86_400), "1 year ago");
    }
}
