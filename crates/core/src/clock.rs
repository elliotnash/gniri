//! Wall-clock time, behind a trait so tests and screenshots can freeze it.
//!
//! The shell supplies a clock backed by the system's local time; tests and
//! fixture scenes use [`FixedClock`].

use std::fmt;

/// A date and time of day in the user's time zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i32,
    /// 1 to 12.
    pub month: u8,
    /// 1 to 31.
    pub day: u8,
    /// 0 to 23.
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Weekday {
    Sunday,
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
}

/// A string that is not a `YYYY-MM-DDTHH:MM:SS` time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseTimeError(String);

impl fmt::Display for ParseTimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} is not a time like 2026-01-15T09:41:00", self.0)
    }
}

impl std::error::Error for ParseTimeError {}

fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 31,
    }
}

impl LocalTime {
    /// Parses `YYYY-MM-DDTHH:MM:SS` (a space instead of `T` also works).
    pub fn parse(text: &str) -> Result<Self, ParseTimeError> {
        let error = || ParseTimeError(text.to_owned());
        let (date, time) = text.split_once(['T', ' ']).ok_or_else(error)?;
        let number = |part: Option<&str>| -> Result<u32, ParseTimeError> {
            let part = part.ok_or_else(error)?;
            if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(error());
            }
            part.parse().map_err(|_| error())
        };

        let mut date = date.split('-');
        let (year, month, day) = (
            number(date.next())?,
            number(date.next())?,
            number(date.next())?,
        );
        let mut time = time.split(':');
        let (hour, minute, second) = (
            number(time.next())?,
            number(time.next())?,
            number(time.next())?,
        );
        if date.next().is_some() || time.next().is_some() {
            return Err(error());
        }

        let year = i32::try_from(year).map_err(|_| error())?;
        let in_range = (1..=12).contains(&month)
            && day >= 1
            && day <= u32::from(days_in_month(year, month as u8))
            && hour < 24
            && minute < 60
            && second < 60;
        if !in_range {
            return Err(error());
        }
        Ok(Self {
            year,
            month: month as u8,
            day: day as u8,
            hour: hour as u8,
            minute: minute as u8,
            second: second as u8,
        })
    }

    pub fn weekday(&self) -> Weekday {
        // Sakamoto's method.
        const OFFSETS: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
        let year = if self.month < 3 {
            self.year - 1
        } else {
            self.year
        };
        let index = (year + year / 4 - year / 100
            + year / 400
            + OFFSETS[usize::from(self.month) - 1]
            + i32::from(self.day))
        .rem_euclid(7);
        [
            Weekday::Sunday,
            Weekday::Monday,
            Weekday::Tuesday,
            Weekday::Wednesday,
            Weekday::Thursday,
            Weekday::Friday,
            Weekday::Saturday,
        ][index as usize]
    }
}

/// A source of the current local time.
pub trait Clock {
    fn now(&self) -> LocalTime;

    /// Whether time passes. A frozen clock needs no update timer.
    fn ticks(&self) -> bool {
        true
    }
}

/// A clock stopped at one moment.
#[derive(Clone, Copy, Debug)]
pub struct FixedClock(pub LocalTime);

impl Clock for FixedClock {
    fn now(&self) -> LocalTime {
        self.0
    }

    fn ticks(&self) -> bool {
        false
    }
}

/// What the panel clock shows. Mirrors GNOME's `clock-format`,
/// `clock-show-date`, `clock-show-weekday` and `clock-show-seconds` settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockFormat {
    pub hour12: bool,
    pub show_date: bool,
    pub show_weekday: bool,
    pub show_seconds: bool,
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn weekday_abbreviation(weekday: Weekday) -> &'static str {
    match weekday {
        Weekday::Sunday => "Sun",
        Weekday::Monday => "Mon",
        Weekday::Tuesday => "Tue",
        Weekday::Wednesday => "Wed",
        Weekday::Thursday => "Thu",
        Weekday::Friday => "Fri",
        Weekday::Saturday => "Sat",
    }
}

/// The panel clock text, as GNOME's wall clock (gnome-desktop's
/// `gnome-wall-clock.c`) writes it in English.
///
/// The formats are `[%a ][%b %-e]<en space>%l:%M[:%S] %p` and the same with
/// `%R` for 24 hours. Two details matter for the width staying put: the date
/// and the time are separated by an en space (U+2002), and a one-digit
/// 12-hour hour is padded with a figure space (U+2007), which is as wide as a
/// digit.
pub fn panel_clock_text(time: &LocalTime, format: &ClockFormat) -> String {
    const EN_SPACE: char = '\u{2002}';
    const FIGURE_SPACE: char = '\u{2007}';

    let mut text = String::new();
    if format.show_weekday {
        text.push_str(weekday_abbreviation(time.weekday()));
    }
    if format.show_date {
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(MONTHS[usize::from(time.month) - 1]);
        text.push(' ');
        text.push_str(&time.day.to_string());
    }
    if !text.is_empty() {
        // With only the weekday GNOME uses a plain space.
        text.push(if format.show_date { EN_SPACE } else { ' ' });
    }

    if format.hour12 {
        let hour = match time.hour % 12 {
            0 => 12,
            hour => hour,
        };
        if hour < 10 {
            text.push(FIGURE_SPACE);
        }
        text.push_str(&hour.to_string());
    } else {
        text.push_str(&format!("{:02}", time.hour));
    }
    text.push_str(&format!(":{:02}", time.minute));
    if format.show_seconds {
        text.push_str(&format!(":{:02}", time.second));
    }
    if format.hour12 {
        text.push_str(if time.hour < 12 { " AM" } else { " PM" });
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const DATE_ONLY: ClockFormat = ClockFormat {
        hour12: true,
        show_date: true,
        show_weekday: false,
        show_seconds: false,
    };

    fn time(text: &str) -> LocalTime {
        LocalTime::parse(text).unwrap()
    }

    #[test]
    fn parses_and_rejects() {
        assert_eq!(
            time("2026-11-14T08:28:05"),
            LocalTime {
                year: 2026,
                month: 11,
                day: 14,
                hour: 8,
                minute: 28,
                second: 5
            }
        );
        assert!(LocalTime::parse("2026-11-14 08:28:05").is_ok());
        for bad in [
            "",
            "2026-11-14",
            "2026-13-01T00:00:00",
            "2026-02-29T00:00:00",
            "2026-11-14T24:00:00",
            "2026-11-14T08:28",
            "2026-11-14T08:28:05:01",
            "2026-11-+4T08:28:05",
        ] {
            assert!(LocalTime::parse(bad).is_err(), "{bad:?} should be rejected");
        }
        assert!(LocalTime::parse("2028-02-29T00:00:00").is_ok());
    }

    #[test]
    fn weekdays() {
        assert_eq!(time("2026-11-14T00:00:00").weekday(), Weekday::Saturday);
        assert_eq!(time("2026-01-15T00:00:00").weekday(), Weekday::Thursday);
        assert_eq!(time("2000-02-29T00:00:00").weekday(), Weekday::Tuesday);
        assert_eq!(time("2024-01-01T00:00:00").weekday(), Weekday::Monday);
    }

    /// The exact strings GNOME Shell 51.1 showed in the reference captures
    /// (tests/visual/reference/gnome-51.1/metrics/panel.json).
    #[test]
    fn matches_gnome_reference_text() {
        assert_eq!(
            panel_clock_text(&time("2026-11-14T08:28:00"), &DATE_ONLY),
            "Nov 14\u{2002}\u{2007}8:28 AM"
        );
        assert_eq!(
            panel_clock_text(&time("2026-11-13T22:10:00"), &DATE_ONLY),
            "Nov 13\u{2002}10:10 PM"
        );
    }

    #[test]
    fn other_formats() {
        let moment = time("2026-01-05T00:07:09");
        let format = |hour12, show_date, show_weekday, show_seconds| ClockFormat {
            hour12,
            show_date,
            show_weekday,
            show_seconds,
        };
        assert_eq!(
            panel_clock_text(&moment, &format(false, true, true, false)),
            "Mon Jan 5\u{2002}00:07"
        );
        assert_eq!(
            panel_clock_text(&moment, &format(false, false, false, true)),
            "00:07:09"
        );
        assert_eq!(
            panel_clock_text(&moment, &format(true, false, true, false)),
            "Mon 12:07 AM"
        );
        assert_eq!(
            panel_clock_text(
                &time("2026-01-05T12:00:00"),
                &format(true, false, false, false)
            ),
            "12:00 PM"
        );
    }
}
