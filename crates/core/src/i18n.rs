// SPDX-License-Identifier: GPL-3.0-or-later

//! Texts and dates in the user's language.
//!
//! gniri's texts are GNOME Shell's, so its translations are too: a text is
//! looked up by GNOME's English original in whatever catalogs the process
//! [`install`]s (the shell hands over gettext, which reads GNOME Shell's
//! own). Nothing here reads a file. Until something is installed, and in
//! every test, a text is its English original, and dates are written with
//! English names.
//!
//! The originals are C format strings, as in GNOME: [`fill`] puts values in
//! where they have `%s` and `%d`, and [`format_time`] writes a time the way
//! `strftime` would.

use std::fmt::Display;
use std::sync::OnceLock;

use crate::clock::LocalTime;

/// Where translations come from. Every method answers `None` for a text it
/// has no translation of.
pub trait Catalog: Send + Sync {
    fn text(&self, original: &str) -> Option<String>;

    /// A text that means different things in different places, with the
    /// place it is for (gettext's message context).
    fn text_in(&self, context: &str, original: &str) -> Option<String>;

    /// The form of a text for `count` things.
    fn plural(&self, singular: &str, plural: &str, count: u64) -> Option<String>;
}

/// The names dates are written with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Names {
    /// "November", as it is written in a date.
    pub months: [String; 12],
    /// "November", as it is written alone (`%OB`): some languages decline
    /// the month in a date.
    pub months_alone: [String; 12],
    /// "Nov".
    pub months_short: [String; 12],
    /// "Sunday" first.
    pub weekdays: [String; 7],
    /// "Sun" first.
    pub weekdays_short: [String; 7],
    pub am: String,
    pub pm: String,
}

impl Default for Names {
    /// English.
    fn default() -> Self {
        let months = [
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
        let weekdays = [
            "Sunday",
            "Monday",
            "Tuesday",
            "Wednesday",
            "Thursday",
            "Friday",
            "Saturday",
        ];
        let short = |name: &str| name[..3].to_owned();
        Self {
            months: months.map(str::to_owned),
            months_alone: months.map(str::to_owned),
            months_short: months.map(short),
            weekdays: weekdays.map(str::to_owned),
            weekdays_short: weekdays.map(short),
            am: "AM".to_owned(),
            pm: "PM".to_owned(),
        }
    }
}

struct Language {
    catalog: Box<dyn Catalog>,
    names: Names,
}

static LANGUAGE: OnceLock<Language> = OnceLock::new();
static ENGLISH: OnceLock<Names> = OnceLock::new();

/// Sets where this process's texts and date names come from. Only the
/// first call counts; it has to come before anything is written.
pub fn install(catalog: Box<dyn Catalog>, names: Names) {
    let _ = LANGUAGE.set(Language { catalog, names });
}

/// `original` in the user's language.
pub fn tr(original: &str) -> String {
    LANGUAGE
        .get()
        .and_then(|language| language.catalog.text(original))
        .unwrap_or_else(|| original.to_owned())
}

/// `original` in the user's language, as it is meant in `context`.
pub fn tr_in(context: &str, original: &str) -> String {
    LANGUAGE
        .get()
        .and_then(|language| language.catalog.text_in(context, original))
        .unwrap_or_else(|| original.to_owned())
}

/// The text for `count` things. Without a translation it is English's
/// rule: `singular` for one.
pub fn tr_n(singular: &str, plural: &str, count: u64) -> String {
    LANGUAGE
        .get()
        .and_then(|language| language.catalog.plural(singular, plural, count))
        .unwrap_or_else(|| if count == 1 { singular } else { plural }.to_owned())
}

/// The names dates are written with.
pub fn names() -> &'static Names {
    match LANGUAGE.get() {
        Some(language) => &language.names,
        None => ENGLISH.get_or_init(Names::default),
    }
}

/// Puts `values` into a C format string: the first where the first `%s` or
/// `%d` is, and so on; `%2$s` takes the second wherever it stands, which is
/// how a translation changes the order. `%%` is a percent sign. A
/// conversion there is no value for stays as it is written.
pub fn fill(template: &str, values: &[&dyn Display]) -> String {
    let mut filled = String::with_capacity(template.len());
    let mut next = 0;
    let mut rest = template;
    while let Some(at) = rest.find('%') {
        filled.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        if let Some(after) = after.strip_prefix('%') {
            filled.push('%');
            rest = after;
            continue;
        }
        // `%2$s`: a number, a dollar sign, the conversion.
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        let (position, conversion) = match after[digits..].strip_prefix('$') {
            Some(conversion) if digits > 0 => (after[..digits].parse::<usize>().ok(), conversion),
            _ => (None, after),
        };
        match conversion.chars().next() {
            Some('s' | 'd' | 'u') => {
                let index = match position {
                    Some(position) => position.wrapping_sub(1),
                    None => {
                        next += 1;
                        next - 1
                    }
                };
                match values.get(index) {
                    Some(value) => filled.push_str(&value.to_string()),
                    None => filled.push_str(&rest[at..rest.len() - conversion.len() + 1]),
                }
                rest = &conversion[1..];
            }
            _ => {
                filled.push('%');
                rest = after;
            }
        }
    }
    filled.push_str(rest);
    filled
}

/// A digit's width of nothing: what pads a one-digit hour, so that a clock
/// does not change its width at ten.
const FIGURE_SPACE: char = '\u{2007}';

/// Writes `time` as `strftime` would for `pattern`, with the conversions
/// GNOME's date formats and their translations use:
///
/// - `%a %A %b %h %B %OB`: the names of the weekday and the month;
/// - `%d %e %m %y %Y`: the date in numbers;
/// - `%H %k %I %l %M %S %p %P %R %T`: the time;
/// - `%-d` and the like: without padding.
///
/// `%e`, `%k` and `%l` pad with a figure space where C pads with a plain
/// one, as GNOME's clock ends up doing. Anything else is copied.
pub fn format_time(pattern: &str, time: &LocalTime) -> String {
    let names = names();
    let month = usize::from(time.month.clamp(1, 12)) - 1;
    let weekday = usize::from(time.weekday().index());
    let hour12 = match time.hour % 12 {
        0 => 12,
        hour => hour,
    };
    let two = |value: u32, unpadded: bool| {
        if unpadded {
            value.to_string()
        } else {
            format!("{value:02}")
        }
    };
    let spaced = |value: u32, unpadded: bool| {
        if unpadded || value >= 10 {
            value.to_string()
        } else {
            format!("{FIGURE_SPACE}{value}")
        }
    };

    let mut written = String::new();
    let mut characters = pattern.chars().peekable();
    while let Some(character) = characters.next() {
        if character != '%' {
            written.push(character);
            continue;
        }
        let mut unpadded = false;
        let mut alone = false;
        let mut raw = String::from('%');
        loop {
            match characters.peek() {
                Some('-') => unpadded = true,
                Some('O') => alone = true,
                _ => break,
            }
            raw.extend(characters.next());
        }
        let Some(conversion) = characters.next() else {
            written.push_str(&raw);
            break;
        };
        raw.push(conversion);
        match conversion {
            'a' => written.push_str(&names.weekdays_short[weekday]),
            'A' => written.push_str(&names.weekdays[weekday]),
            'b' | 'h' => written.push_str(&names.months_short[month]),
            'B' if alone => written.push_str(&names.months_alone[month]),
            'B' => written.push_str(&names.months[month]),
            'd' => written.push_str(&two(time.day.into(), unpadded)),
            'e' => written.push_str(&spaced(time.day.into(), unpadded)),
            'm' => written.push_str(&two(time.month.into(), unpadded)),
            'y' => written.push_str(&two(time.year.rem_euclid(100) as u32, unpadded)),
            'Y' => written.push_str(&time.year.to_string()),
            'H' => written.push_str(&two(time.hour.into(), unpadded)),
            'k' => written.push_str(&spaced(time.hour.into(), unpadded)),
            'I' => written.push_str(&two(hour12.into(), unpadded)),
            'l' => written.push_str(&spaced(hour12.into(), unpadded)),
            'M' => written.push_str(&two(time.minute.into(), unpadded)),
            'S' => written.push_str(&two(time.second.into(), unpadded)),
            'p' => written.push_str(if time.hour < 12 { &names.am } else { &names.pm }),
            'P' => {
                let half = if time.hour < 12 { &names.am } else { &names.pm };
                written.push_str(&half.to_lowercase());
            }
            'R' => written.push_str(&format!("{:02}:{:02}", time.hour, time.minute)),
            'T' => written.push_str(&format!(
                "{:02}:{:02}:{:02}",
                time.hour, time.minute, time.second
            )),
            '%' => written.push('%'),
            _ => written.push_str(&raw),
        }
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_installed_is_english() {
        assert_eq!(tr("Type to search"), "Type to search");
        assert_eq!(tr_in("grid sunday", "S"), "S");
        assert_eq!(tr_n("%d minute ago", "%d minutes ago", 1), "%d minute ago");
        assert_eq!(tr_n("%d minute ago", "%d minutes ago", 5), "%d minutes ago");
        assert_eq!(tr_n("%d minute ago", "%d minutes ago", 0), "%d minutes ago");
        assert_eq!(names().months[10], "November");
        assert_eq!(names().weekdays_short[0], "Sun");
    }

    #[test]
    fn values_go_where_the_format_says() {
        assert_eq!(fill("%d minutes ago", &[&5]), "5 minutes ago");
        assert_eq!(
            fill("%s is now known as %s", &[&"a", &"b"]),
            "a is now known as b"
        );
        // A translation that turns the order round.
        assert_eq!(fill("%2$s, früher %1$s", &[&"a", &"b"]), "b, früher a");
        assert_eq!(fill("%d\u{2009}%%", &[&91]), "91\u{2009}%");
        assert_eq!(fill("100%", &[]), "100%");
        assert_eq!(fill("%s and %s", &[&"one"]), "one and %s");
        assert_eq!(fill("%x %s", &[&"kept"]), "%x kept");
        assert_eq!(fill("", &[&1]), "");
        assert_eq!(fill("ends in %", &[]), "ends in %");
    }

    fn time(text: &str) -> LocalTime {
        LocalTime::parse(text).unwrap()
    }

    #[test]
    fn times_are_written_as_strftime_writes_them() {
        let morning = time("2026-11-04T08:05:09");
        assert_eq!(format_time("%A %B %-d", &morning), "Wednesday November 4");
        assert_eq!(format_time("%a %b %e", &morning), "Wed Nov \u{2007}4");
        assert_eq!(format_time("%OB %Y", &morning), "November 2026");
        assert_eq!(format_time("%d.%m.%y", &morning), "04.11.26");
        assert_eq!(format_time("%-d.%-m.", &morning), "4.11.");
        assert_eq!(format_time("%R %T", &morning), "08:05 08:05:09");
        assert_eq!(format_time("%l:%M %p", &morning), "\u{2007}8:05 AM");
        assert_eq!(format_time("%-l:%M %P", &morning), "8:05 am");
        assert_eq!(format_time("%I %k %H", &morning), "08 \u{2007}8 08");

        let night = time("2026-01-05T00:07:59");
        assert_eq!(format_time("%l:%M:%S %p", &night), "12:07:59 AM");
        let noon = time("2026-01-05T12:00:00");
        assert_eq!(format_time("%l %p, %h", &noon), "12 PM, Jan");
    }

    #[test]
    fn what_is_no_conversion_is_copied() {
        let moment = time("2026-11-04T08:05:09");
        assert_eq!(format_time("100%% at %H", &moment), "100% at 08");
        assert_eq!(format_time("%V %-", &moment), "%V %-");
        assert_eq!(format_time("", &moment), "");
    }
}
