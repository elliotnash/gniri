// SPDX-License-Identifier: GPL-3.0-or-later

//! World clocks: the places GNOME Clocks keeps clocks for, which the date
//! menu shows the time of. Port of the data side of `WorldClocksSection`
//! (dateMenu.js).
//!
//! GNOME Clocks keeps its places as libgweather serializes them: a name
//! and coordinates. GNOME Shell asks libgweather's database for each
//! place's time zone; gniri has no libgweather, and takes the zone whose
//! principal city is nearest, from the time zone database's own table
//! (`zone1970.tab`). For the cities clocks are kept for that is their own
//! zone.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// A place there is a clock for.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Clock {
    /// The city, as libgweather names it.
    pub city: String,
    /// Its time zone: "Asia/Tokyo".
    pub zone: String,
}

/// A time zone and where its principal city is, in degrees.
#[derive(Clone, Debug, PartialEq)]
pub struct Zone {
    pub id: String,
    pub latitude: f64,
    pub longitude: f64,
}

/// An ISO 6709 coordinate as the zone table writes it: a sign, then
/// degrees, minutes and maybe seconds, `degree_digits` digits of degrees.
fn coordinate(text: &str, degree_digits: usize) -> Option<f64> {
    let sign = match text.as_bytes().first()? {
        b'+' => 1.0,
        b'-' => -1.0,
        _ => return None,
    };
    let digits = &text[1..];
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) || digits.len() < degree_digits + 2 {
        return None;
    }
    let number = |range: std::ops::Range<usize>| digits.get(range)?.parse::<f64>().ok();
    let degrees = number(0..degree_digits)?;
    let minutes = number(degree_digits..degree_digits + 2)?;
    let seconds = match digits.len() - degree_digits {
        2 => 0.0,
        4 => number(degree_digits + 2..degree_digits + 4)?,
        _ => return None,
    };
    Some(sign * (degrees + minutes / 60.0 + seconds / 3600.0))
}

/// The zones of a zone table (`zone1970.tab`, or `zone.tab`): a line per
/// zone of country codes, coordinates and the zone's name.
pub fn parse_zone_table(text: &str) -> Vec<Zone> {
    text.lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let (_, place, id) = (fields.next()?, fields.next()?, fields.next()?);
            // The longitude's sign starts it.
            let split = place[1..].find(['+', '-'])? + 1;
            Some(Zone {
                id: id.to_owned(),
                latitude: coordinate(&place[..split], 2)?,
                longitude: coordinate(&place[split..], 3)?,
            })
        })
        .collect()
}

/// The zone table of the system's time zone database.
pub fn zone_table() -> Vec<Zone> {
    let directory = std::env::var_os("TZDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/usr/share/zoneinfo"));
    ["zone1970.tab", "zone.tab"]
        .iter()
        .find_map(|name| std::fs::read_to_string(Path::new(&directory).join(name)).ok())
        .map(|text| parse_zone_table(&text))
        .unwrap_or_default()
}

/// The zone whose principal city is nearest to a place, in degrees.
pub fn nearest_zone(zones: &[Zone], latitude: f64, longitude: f64) -> Option<&str> {
    let radians = |degrees: f64| degrees.to_radians();
    // The angle between two places on a sphere: the haversine.
    let distance = |zone: &Zone| {
        let (a, b) = (radians(latitude), radians(zone.latitude));
        let (delta_latitude, delta_longitude) = (b - a, radians(zone.longitude - longitude));
        let h = (delta_latitude / 2.0).sin().powi(2)
            + a.cos() * b.cos() * (delta_longitude / 2.0).sin().powi(2);
        h.sqrt().asin()
    };
    zones
        .iter()
        .min_by(|a, b| distance(a).total_cmp(&distance(b)))
        .map(|zone| zone.id.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &str = "\
# A comment
JP\t+353916+1394441\tAsia/Tokyo
RU\t+554521+0373704\tEurope/Moscow\tMSK+00 - Moscow area
AU\t-3352+15113\tAustralia/Sydney\tNew South Wales (most areas)
US\t+404251-0740023\tAmerica/New_York\tEastern (most areas)
US\t+415100-0873900\tAmerica/Chicago\tCentral (most areas)
IN\t+2232+08822\tAsia/Kolkata
";

    #[test]
    fn the_zone_table_is_read() {
        let zones = parse_zone_table(TABLE);
        assert_eq!(zones.len(), 6);
        assert_eq!(zones[0].id, "Asia/Tokyo");
        assert!((zones[0].latitude - 35.654_444).abs() < 1e-5);
        assert!((zones[0].longitude - 139.744_722).abs() < 1e-5);
        assert!((zones[2].latitude + 33.866_666).abs() < 1e-5);
        assert!((zones[3].longitude + 74.006_388).abs() < 1e-5);
        assert_eq!(coordinate("+12", 2), None);
        assert_eq!(coordinate("x1234", 2), None);
    }

    /// The places as libgweather has them (`find_nearest_city`), and the
    /// zones it gives them.
    #[test]
    fn a_city_is_in_the_zone_whose_city_is_nearest() {
        let zones = parse_zone_table(TABLE);
        for (latitude, longitude, zone) in [
            (
                0.971_275_728_734_844_3,
                0.650_426_040_394_317_6,
                "Europe/Moscow",
            ),
            (0.621_918_984_309_548_6, 2.440_842_958_914_07, "Asia/Tokyo"),
            (
                -0.592_539_281_052_075,
                2.638_646_934_988_996,
                "Australia/Sydney",
            ),
            (
                0.711_803_440_787_256_4,
                -1.290_961_875_876_236_7,
                "America/New_York",
            ),
        ] {
            let (latitude, longitude) = (f64::to_degrees(latitude), f64::to_degrees(longitude));
            assert_eq!(nearest_zone(&zones, latitude, longitude), Some(zone));
        }
        assert_eq!(nearest_zone(&[], 0.0, 0.0), None);
    }

    #[test]
    fn the_systems_table_is_there() {
        // Where the time zone database is installed, every zone of it.
        let zones = zone_table();
        if !zones.is_empty() {
            assert!(zones.iter().any(|zone| zone.id == "Europe/Berlin"));
        }
    }
}
