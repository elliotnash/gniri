// SPDX-License-Identifier: GPL-3.0-or-later

//! Night light: a warmer screen in the evening, set through the
//! compositor's gamma tables.
//!
//! The state is a plain on/off toggle ([`crate::toggles`]). GNOME's night
//! light also follows a schedule (sunset to sunrise by default); gniri's is
//! on while the toggle is, at the colour temperature from the config file.

pub mod backend;

/// GNOME's default `night-light-temperature`, in kelvin.
pub const DEFAULT_TEMPERATURE: u32 = 2700;
/// The ends of the slider in GNOME Settings.
pub const TEMPERATURE_RANGE: std::ops::RangeInclusive<u32> = 1700..=4700;

/// How much of each of red, green and blue a black body of `kelvin` emits,
/// 0.0 to 1.0, with 6600 K as white.
///
/// Tanner Helland's fit to Mitchell Charity's black-body table, which is
/// accurate to about 1% between 1000 K and 40000 K.
pub fn white_point(kelvin: u32) -> [f64; 3] {
    let t = f64::from(kelvin.clamp(1000, 40000)) / 100.0;
    let red = if t <= 66.0 {
        255.0
    } else {
        329.698_727_446 * (t - 60.0).powf(-0.133_204_759_2)
    };
    let green = if t <= 66.0 {
        99.470_802_586_1 * t.ln() - 161.119_568_166_1
    } else {
        288.122_169_528_3 * (t - 60.0).powf(-0.075_514_849_2)
    };
    let blue = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        138.517_731_223_1 * (t - 10.0).ln() - 305.044_792_730_7
    };
    [red, green, blue].map(|channel| channel.clamp(0.0, 255.0) / 255.0)
}

/// A gamma table for the compositor: `size` 16-bit entries each for red,
/// green and blue, one channel after the other, every channel a straight
/// line from black up to its share of `white`.
pub fn gamma_table(size: usize, white: [f64; 3]) -> Vec<u16> {
    let last = size.saturating_sub(1).max(1) as f64;
    white
        .iter()
        .flat_map(|&share| {
            (0..size).map(move |index| {
                let level = index as f64 / last * share.clamp(0.0, 1.0);
                (level * f64::from(u16::MAX)).round() as u16
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warmer_light_has_less_blue_then_less_green() {
        let [red, green, blue] = white_point(2700);
        assert_eq!(red, 1.0);
        assert!((0.60..0.70).contains(&green), "{green}");
        assert!((0.30..0.40).contains(&blue), "{blue}");

        let warmest = white_point(1700);
        assert!(warmest[1] < green && warmest[2] < blue);
        // Daylight is white, near enough.
        for channel in white_point(6600) {
            assert!(channel > 0.97, "{channel}");
        }
    }

    #[test]
    fn a_table_is_three_ramps_from_black() {
        let table = gamma_table(4, [1.0, 0.5, 0.0]);
        assert_eq!(table.len(), 12);
        assert_eq!(table[..4], [0, 21845, 43690, 65535]);
        assert_eq!(table[4..8], [0, 10923, 21845, 32768]);
        assert_eq!(table[8..], [0, 0, 0, 0]);
        // The neutral table changes nothing.
        let neutral = gamma_table(256, [1.0; 3]);
        assert_eq!(neutral[255], u16::MAX);
        assert_eq!(neutral[256], 0);
        assert!(gamma_table(0, [1.0; 3]).is_empty());
    }
}
