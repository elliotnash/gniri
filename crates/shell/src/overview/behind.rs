// SPDX-License-Identifier: GPL-3.0-or-later

//! Whether what is behind the search entry is light.
//!
//! The entry's fill is 7% of white, with white text on it: over the
//! overview's dimmed background that is GNOME's entry, and over a white
//! window of the workspace above it is white on white. So the shell looks
//! at the screen where the entry is, takes the entry's own fill out of
//! what it sees there, and the theme darkens the fill while a good part
//! of the rest is light (`.light-behind`, _bms-overview.scss).
//!
//! niri does not say what is behind the entry. It names the active
//! workspace, but only once a scroll to another has ended, and not what
//! colour a window is. A picture is the one thing that follows the
//! screen.

use gniri_services::capture::{Format, Picture};
use gtk::gdk;

/// The share of the entry that has to be light for the entry to be over
/// something light. A window that reaches under a corner of it is not
/// enough; one under the half the text is not on is.
const SHARE: f32 = 0.25;

/// How bright that share has to be, from 0 to 1, for a fill that was
/// dark to go light, and how dark for it to go back. The wallpaper of a
/// workspace with no window there is between the two at its brightest,
/// and stays as it was.
const LIGHT_FROM: f32 = 0.5;
const DARK_FROM: f32 = 0.4;

/// A fill, as it adds to what is seen through it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fill {
    /// The brightness of its colour, from 0 to 1.
    luma: f32,
    alpha: f32,
}

impl Fill {
    /// The fill a design token names (`rgba(255, 255, 255, 0.07)`).
    pub fn parse(token: &str) -> Option<Self> {
        let colour = gdk::RGBA::parse(token).ok()?;
        Some(Self {
            luma: luma(colour.red(), colour.green(), colour.blue()),
            alpha: colour.alpha(),
        })
    }

    /// How bright what is behind the fill is where the two together are
    /// `seen`. A fill that lets nothing through says nothing about it.
    fn behind(self, seen: f32) -> Option<f32> {
        let through = 1.0 - self.alpha;
        (through > 0.05).then(|| ((seen - self.alpha * self.luma) / through).clamp(0.0, 1.0))
    }
}

/// Rec. 709's weights, on the values as they are stored: what is asked is
/// how light it looks beside the text, not how much light there is.
fn luma(red: f32, green: f32, blue: f32) -> f32 {
    0.2126 * red + 0.7152 * green + 0.0722 * blue
}

/// How bright what is behind `fill` is in a `picture` of it, from 0 to 1:
/// [`SHARE`] of it is at least as bright as that. The text on the fill is
/// too little of the picture to count.
pub fn brightness(picture: &Picture, fill: Fill) -> Option<f32> {
    let mut counts = [0u32; 256];
    let width = picture.width as usize * 4;
    for row in picture
        .pixels
        .chunks(picture.stride.max(1) as usize)
        .take(picture.height as usize)
    {
        for pixel in row.get(..width)?.as_chunks::<4>().0 {
            let (red, blue) = match picture.format {
                Format::Bgrx => (pixel[2], pixel[0]),
                Format::Rgbx => (pixel[0], pixel[2]),
            };
            let seen = luma(f32::from(red), f32::from(pixel[1]), f32::from(blue));
            counts[(seen.round() as usize).min(255)] += 1;
        }
    }
    let all: u32 = counts.iter().sum();
    if all == 0 {
        return None;
    }
    // From the brightest down, to where the share is reached.
    let wanted = (all as f32 * SHARE).ceil().max(1.0) as u32;
    let mut brighter = 0;
    let seen = (0..=255usize).rev().find(|&level| {
        brighter += counts[level];
        brighter >= wanted
    })?;
    fill.behind(seen as f32 / 255.0)
}

/// Whether something that bright is light, for an entry that `was` over
/// something light or was not: near the line it stays as it is.
pub fn is_light(brightness: f32, was: bool) -> bool {
    if was {
        brightness > DARK_FROM
    } else {
        brightness >= LIGHT_FROM
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fill(name: &str) -> Fill {
        let token = gniri_theme::token(name).unwrap_or_else(|| panic!("no token {name}"));
        Fill::parse(token).unwrap_or_else(|| panic!("{name} is no colour: {token}"))
    }

    /// A picture of `fill` over grey rows: `light` of every eight columns
    /// are over `bright`, the rest over `dark`.
    fn picture(fill: Fill, light: usize, bright: u8, dark: u8, format: Format) -> Picture {
        let (width, height) = (64usize, 8usize);
        let over = |behind: u8| {
            let seen = fill.alpha * fill.luma + (1.0 - fill.alpha) * f32::from(behind) / 255.0;
            (seen * 255.0).round() as u8
        };
        // A stride wider than the picture, as a compositor may give.
        let stride = width * 4 + 8;
        let mut pixels = vec![0xee; stride * height];
        for row in pixels.chunks_mut(stride) {
            for (column, pixel) in row[..width * 4].chunks_mut(4).enumerate() {
                let grey = over(if column % 8 < light { bright } else { dark });
                pixel.copy_from_slice(&[grey, grey, grey, 0]);
            }
        }
        Picture {
            width: width as u32,
            height: height as u32,
            stride: stride as u32,
            format,
            pixels,
        }
    }

    #[test]
    fn the_fill_is_taken_out_of_what_is_seen() {
        for name in ["bms-overview-fill", "bms-overview-fill-light-behind"] {
            let fill = fill(name);
            for behind in [0u8, 30, 77, 128, 255] {
                let found = brightness(&picture(fill, 8, behind, behind, Format::Bgrx), fill)
                    .expect("a picture with pixels");
                assert!(
                    (found - f32::from(behind) / 255.0).abs() < 0.02,
                    "{name} over {behind}: {found}"
                );
            }
        }
    }

    #[test]
    fn a_quarter_of_the_entry_over_white_is_enough() {
        let fill = fill("bms-overview-fill");
        let over = |light| brightness(&picture(fill, light, 255, 30, Format::Rgbx), fill).unwrap();
        assert!(is_light(over(2), false));
        assert!(is_light(over(8), false));
        // The text, or a window under one corner.
        assert!(!is_light(over(1), false));
        assert!(!is_light(over(0), false));
    }

    #[test]
    fn the_channels_are_read_in_the_pictures_order() {
        let fill = Fill {
            luma: 0.0,
            alpha: 0.0,
        };
        let blue = |format| Picture {
            width: 1,
            height: 1,
            stride: 4,
            format,
            pixels: vec![255, 0, 0, 0],
        };
        let as_blue = brightness(&blue(Format::Bgrx), fill).unwrap();
        let as_red = brightness(&blue(Format::Rgbx), fill).unwrap();
        assert!((as_blue - 0.0722).abs() < 0.01 && (as_red - 0.2126).abs() < 0.01);
    }

    #[test]
    fn near_the_line_the_entry_stays_as_it_is() {
        assert!(!is_light(0.45, false) && is_light(0.45, true));
        assert!(is_light(0.5, false) && !is_light(0.4, true));
    }

    #[test]
    fn the_backdrop_and_a_dark_window_are_dark_and_a_white_one_light() {
        let (plain, darkened) = (
            fill("bms-overview-fill"),
            fill("bms-overview-fill-light-behind"),
        );
        let over = |fill, behind| {
            brightness(&picture(fill, 8, behind, behind, Format::Bgrx), fill).unwrap()
        };
        // The reference backdrop, a terminal, and a browser's page.
        assert!(!is_light(over(plain, 77), false) && !is_light(over(plain, 30), false));
        assert!(is_light(over(plain, 255), false) && is_light(over(darkened, 255), true));
        // And back, from the darkened fill.
        assert!(!is_light(over(darkened, 30), true));
    }

    #[test]
    fn the_light_style_has_no_second_fill() {
        assert!(gniri_theme::light_token("bms-overview-fill-light-behind").is_none());
    }

    #[test]
    fn an_empty_picture_says_nothing() {
        let picture = Picture {
            width: 0,
            height: 0,
            stride: 0,
            format: Format::Bgrx,
            pixels: Vec::new(),
        };
        assert_eq!(brightness(&picture, fill("bms-overview-fill")), None);
    }
}
