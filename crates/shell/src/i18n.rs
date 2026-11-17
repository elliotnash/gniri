// SPDX-License-Identifier: GPL-3.0-or-later

//! The user's language, from the catalogs GNOME itself installs.
//!
//! gniri's texts are GNOME's texts, word for word, so where GNOME Shell's
//! translations are installed gniri is translated: a text is looked up in
//! GNOME Shell's catalog, then in gnome-desktop's (the clock's formats),
//! gnome-bluetooth's (the pairing dialog) and NetworkManager's (the names
//! of the kinds of network, which GNOME Shell has from libnm). gniri has no
//! catalog of its own; the few texts that are only gniri's stay English.
//!
//! The names of months and weekdays are the C library's, which go by
//! `LC_TIME`; the texts go by `LC_MESSAGES` and `LANGUAGE`.

use gniri_core::i18n::{Catalog, Names};
use gtk::glib;

/// The catalogs, in the order they are asked.
const DOMAINS: [&str; 4] = [
    "gnome-shell",
    "gnome-desktop-3.0",
    "gnome-bluetooth-3.0",
    "NetworkManager",
];

struct Gettext;

impl Catalog for Gettext {
    fn text(&self, original: &str) -> Option<String> {
        DOMAINS.iter().find_map(|domain| {
            let text = glib::dgettext(Some(domain), original);
            (text != original).then(|| text.into())
        })
    }

    fn text_in(&self, context: &str, original: &str) -> Option<String> {
        DOMAINS.iter().find_map(|domain| {
            let text = glib::dpgettext2(Some(domain), context, original);
            (text != original).then(|| text.into())
        })
    }

    fn plural(&self, singular: &str, plural: &str, count: u64) -> Option<String> {
        DOMAINS.iter().find_map(|domain| {
            let text = glib::dngettext(Some(domain), singular, plural, count);
            // Without a translation gettext answers with one of the two.
            (text != singular && text != plural).then(|| text.into())
        })
    }
}

/// The names of the months and the weekdays as the C library has them for
/// the user's locale.
fn names() -> Names {
    let english = Names::default();
    let written = |month: i32, day: i32, hour: i32, format: &str, fallback: &str| {
        glib::DateTime::from_local(2023, month, day, hour, 0, 0.0)
            .ok()
            .and_then(|moment| moment.format(format).ok())
            .map(String::from)
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| fallback.to_owned())
    };
    let months = |format: &str, fallback: &[String; 12]| {
        std::array::from_fn(|index| written(index as i32 + 1, 1, 12, format, &fallback[index]))
    };
    // The first of January 2023 was a Sunday.
    let weekdays = |format: &str, fallback: &[String; 7]| {
        std::array::from_fn(|index| written(1, index as i32 + 1, 12, format, &fallback[index]))
    };
    Names {
        months: months("%B", &english.months),
        months_alone: months("%OB", &english.months_alone),
        months_short: months("%b", &english.months_short),
        weekdays: weekdays("%A", &english.weekdays),
        weekdays_short: weekdays("%a", &english.weekdays_short),
        // A locale that divides no day in two has no names for the halves.
        am: written(1, 1, 1, "%p", &english.am),
        pm: written(1, 1, 13, "%p", &english.pm),
    }
}

/// Takes the user's language into use. After GTK is initialised, which
/// sets the locale, and before anything is written.
pub fn init() {
    gniri_core::i18n::install(Box::new(Gettext), names());
}

/// The day the user's locale starts a week on, if it can be found out:
/// from the C library's locale data, which `locale` prints, else from
/// GTK's own translation, which says it for a language.
pub fn week_start() -> Option<gniri_core::Weekday> {
    thread_local! {
        /// Asked once: the answer does not change while the process runs.
        static WEEK_START: std::cell::OnceCell<Option<gniri_core::Weekday>> =
            const { std::cell::OnceCell::new() };
    }
    WEEK_START.with(|cell| *cell.get_or_init(find_week_start))
}

fn find_week_start() -> Option<gniri_core::Weekday> {
    use gniri_core::Weekday;
    const WEEK: [Weekday; 7] = [
        Weekday::Sunday,
        Weekday::Monday,
        Weekday::Tuesday,
        Weekday::Wednesday,
        Weekday::Thursday,
        Weekday::Friday,
        Weekday::Saturday,
    ];
    let from_locale = || {
        let output = std::process::Command::new("locale")
            .args(["first_weekday", "week-1stday"])
            .output()
            .ok()?;
        week_start_from_locale(&String::from_utf8_lossy(&output.stdout))
    };
    let from_gtk = || {
        // gtkcalendar.c: "calendar:week_start:0" for Sunday, ":1" for
        // Monday.
        let text = glib::dgettext(Some("gtk40"), "calendar:week_start:0");
        let day: usize = text.strip_prefix("calendar:week_start:")?.parse().ok()?;
        WEEK.get(day).copied()
    };
    from_locale().or_else(from_gtk)
}

/// What `locale first_weekday week-1stday` says: the number of the first
/// day of a week, counted from 1 at the day named by the second line, which
/// is a date: 19971130 was a Sunday, 19971201 a Monday (as GTK's calendar
/// reads the same two values).
fn week_start_from_locale(output: &str) -> Option<gniri_core::Weekday> {
    let mut lines = output.lines().map(str::trim);
    let first_weekday: i64 = lines.next()?.parse().ok()?;
    let origin = match lines.next()? {
        "19971130" => 0,
        "19971201" => 1,
        _ => return None,
    };
    if !(1..=7).contains(&first_weekday) {
        return None;
    }
    Some(gniri_core::Weekday::Sunday.plus(origin + first_weekday - 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gniri_core::Weekday;

    #[test]
    fn the_week_starts_where_the_locale_data_says() {
        // en_US, de_DE and what glibc has for the Emirates.
        assert_eq!(
            week_start_from_locale("1\n19971130\n"),
            Some(Weekday::Sunday)
        );
        assert_eq!(
            week_start_from_locale("2\n19971130\n"),
            Some(Weekday::Monday)
        );
        assert_eq!(
            week_start_from_locale("1\n19971201\n"),
            Some(Weekday::Monday)
        );
        assert_eq!(
            week_start_from_locale("7\n19971130\n"),
            Some(Weekday::Saturday)
        );
        assert_eq!(
            week_start_from_locale("7\n19971201\n"),
            Some(Weekday::Sunday)
        );
        assert_eq!(week_start_from_locale(""), None);
        assert_eq!(week_start_from_locale("1\n"), None);
        assert_eq!(week_start_from_locale("9\n19971130\n"), None);
        assert_eq!(week_start_from_locale("1\n20260101\n"), None);
    }
}
