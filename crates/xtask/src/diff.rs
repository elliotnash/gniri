//! `cargo xtask diff <component>`: gniri against the GNOME reference.
//!
//! A component is described by `tests/visual/map/<component>.toml`: the
//! scenes that show it, which of our widgets corresponds to which GNOME
//! actor, and which strips of it to compare pixel by pixel. The checks:
//!
//! - **Layout** (tier 1): each widget's box against its actor's, in logical
//!   pixels.
//! - **Flat** (tier 2): over a flat wallpaper, the share of pixels that
//!   differ by more than a small fuzz, text excluded (GTK and St rasterise
//!   glyphs differently). See [`differing`] for what counts.
//! - **Real** (tier 3): over the real wallpaper, both sides low-passed, the
//!   RMS error and the mean brightness.
//!
//! Tier 0 (theme tokens) is a plain unit test in `crates/theme`.
//!
//! The nested output is smaller than the reference screen, so nothing is
//! compared in screen coordinates. A component has a root (the panel, a
//! menu, the OSD); boxes and pixels are compared relative to it, and where
//! the root sits is compared as its distance from the screen edges it is
//! attached to.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::Value;

use crate::scene::Scene;
use crate::session;
use crate::tree;

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
enum Anchor {
    Left,
    Center,
    Right,
    /// Spans the whole surface.
    Stretch,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
enum BoxKind {
    /// The widget's border box.
    #[default]
    Border,
    /// The extents of a label's text.
    Text,
}

/// Which screen edge a component hangs from, vertically.
#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "kebab-case")]
enum Vertical {
    #[default]
    Top,
    Bottom,
    /// In the middle of the screen (a dialog, the lock screen's clock).
    Center,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Map {
    /// Layer-shell namespace of our surface.
    surface: String,
    /// Selector of the widget that corresponds to the reference's root
    /// actor. The surface's own root if unset.
    root: Option<String>,
    /// Selector of a widget above the root that the pairs' selectors start
    /// from, for a component whose parts are not all inside its root: the
    /// dash's items are beside its background, not in it. The root if
    /// unset.
    tree: Option<String>,
    /// Whether the root's place on the screen is compared. False for a
    /// component that GNOME places by the size of the screen, which the
    /// nested output does not share with the reference: where such a
    /// component goes is arithmetic, tested where it is done.
    #[serde(default = "yes")]
    placement: bool,
    /// Where on the screen the component is attached: the edge its
    /// placement is measured from, and the part of the reference wallpaper
    /// the nested output is given.
    #[serde(default = "attach_left")]
    attach: Anchor,
    #[serde(default)]
    vertical: Vertical,
    /// Namespace of a surface that reserves the top of the screen, which
    /// the compositor therefore places ours below (the panel, for a
    /// surface that is anchored to the top and respects its exclusive
    /// zone). A widget dump cannot tell where its surface was put.
    below: Option<String>,
    /// Logical pixels around the root that are compared too (its shadow).
    #[serde(default)]
    margin: f64,
    /// The wallpaper image the real-wallpaper reference was taken over.
    wallpaper: PathBuf,
    /// Whether the reference has captures over flat wallpapers. GNOME's
    /// own lock screen was only captured over the real one.
    #[serde(default = "yes")]
    flat: bool,
    /// Tier 2: the share of differing pixels this component may have,
    /// where `flat_differing_percent` in thresholds.toml cannot hold. The
    /// map says why.
    flat_differing_percent: Option<f64>,
    states: Vec<State>,
    regions: Vec<Region>,
    pairs: Vec<Pair>,
}

fn attach_left() -> Anchor {
    Anchor::Left
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    scene: String,
    /// Name of the reference extract (tools/gnome-ref/extract.py).
    reference: String,
    /// Regions to compare in this state; all of them if unset.
    regions: Option<Vec<String>>,
    /// Pairs to compare in this state; all of them if unset.
    pairs: Option<Vec<String>>,
}

/// A strip of the root, full height, compared pixel by pixel.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Region {
    name: String,
    anchor: Anchor,
    /// Logical pixels. Unset (with `anchor = "stretch"`) for the whole root.
    width: Option<f64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pair {
    name: String,
    /// Selector in gniri's widget dump.
    ours: String,
    /// Selector in the GNOME actor dump.
    theirs: String,
    anchor: Anchor,
    #[serde(default)]
    ours_box: BoxKind,
    /// Allowed difference in logical pixels, where the default
    /// (`layout_px` in thresholds.toml) cannot hold.
    tolerance_px: Option<f64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Thresholds {
    /// Tier 1: largest allowed difference of any box edge, logical pixels.
    layout_px: f64,
    /// Tier 2: colour distance below which two pixels count as equal.
    flat_fuzz_percent: f64,
    /// Tier 2: largest allowed share of differing pixels.
    flat_differing_percent: f64,
    /// Tier 3: Gaussian sigma both images are blurred with first, pixels.
    real_lowpass_sigma: f64,
    /// Tier 3: largest allowed RMS error.
    real_rmse_percent: f64,
    /// Tier 3: largest allowed difference in mean brightness.
    real_brightness_percent: f64,
}

/// One comparison's outcome.
pub struct Check {
    pub tier: &'static str,
    pub subject: String,
    pub detail: String,
    pub pass: bool,
    /// A picture of the two sides, for the report.
    pub montage: Option<PathBuf>,
}

const FLAT_WALLPAPERS: [(&str, &str); 2] = [("flat-808080", "#808080"), ("flat-3584e4", "#3584e4")];
const SRGB_PROFILE: &str = "/usr/share/color/icc/colord/sRGB.icc";

fn reference_dir() -> PathBuf {
    session::workspace_root().join("tests/visual/reference/gnome-51.1")
}

fn magick(args: &[&str]) -> Result<std::process::Output> {
    Command::new("magick")
        .args(args)
        .output()
        .context("running ImageMagick (`magick`); is it installed?")
}

fn magick_ok(args: &[&str]) -> Result<String> {
    let output = magick(args)?;
    if !output.status.success() {
        bail!(
            "magick {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn path_str(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| anyhow!("non-UTF-8 path {}", path.display()))
}

/// A rectangle in whole physical pixels.
#[derive(Clone, Copy)]
struct PixelRect {
    x: i64,
    y: i64,
    width: i64,
    height: i64,
}

impl PixelRect {
    fn geometry(&self) -> String {
        format!("{}x{}+{}+{}", self.width, self.height, self.x, self.y)
    }
}

/// One side of a comparison: the root of a component, where it is on its
/// screen, and where it is in the picture it is cropped from.
struct Side<'a> {
    /// The root node. Boxes in it are in `tree` coordinates.
    tree: &'a Value,
    /// The root's box, in tree coordinates. Logical pixels, as all below.
    root: [f64; 4],
    /// What to add to a tree coordinate to get its place on the screen.
    on_screen: (f64, f64),
    /// What to add to a tree coordinate to get its place in the picture.
    in_picture: (f64, f64),
    /// The picture's size.
    picture: (f64, f64),
    screen: (f64, f64),
    /// Physical pixels per logical pixel.
    scale: f64,
    /// Key of the text rectangle: "text_box" in gniri's dump, "box" (of a
    /// `ClutterText`) in GNOME's.
    text_key: &'static str,
}

/// A rectangle relative to an anchor point on the root: the root's top
/// left, top centre or top right corner. Both sides of a comparison share
/// these coordinates, whatever the size of their screens.
#[derive(Clone, Copy)]
struct Area {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

impl Area {
    fn intersect(self, other: Self) -> Self {
        Self {
            left: self.left.max(other.left),
            top: self.top.max(other.top),
            right: self.right.min(other.right),
            bottom: self.bottom.min(other.bottom),
        }
    }
}

impl Side<'_> {
    /// The x coordinate, in tree coordinates, that `anchor` measures from.
    fn anchor_x(&self, anchor: Anchor) -> f64 {
        let [x, _, width, _] = self.root;
        match anchor {
            Anchor::Left | Anchor::Stretch => x,
            Anchor::Center => x + width / 2.0,
            Anchor::Right => x + width,
        }
    }

    /// The part of the picture there is, relative to `anchor`.
    fn available(&self, anchor: Anchor) -> Area {
        let left = -self.in_picture.0 - self.anchor_x(anchor);
        let top = -self.in_picture.1 - self.root[1];
        Area {
            left,
            top,
            right: left + self.picture.0,
            bottom: top + self.picture.1,
        }
    }

    /// `area` in the picture's physical pixels.
    fn pixels(&self, anchor: Anchor, area: &Area) -> PixelRect {
        let x = (area.left + self.anchor_x(anchor) + self.in_picture.0) * self.scale;
        let y = (area.top + self.root[1] + self.in_picture.1) * self.scale;
        // The two sides reach the same half pixel by different sums; the
        // nudge keeps a rounding error from sending them opposite ways.
        let pixel = |value: f64| (value + 1e-6).round() as i64;
        PixelRect {
            x: pixel(x),
            y: pixel(y),
            width: pixel((area.right - area.left) * self.scale),
            height: pixel((area.bottom - area.top) * self.scale),
        }
    }

    /// A tree-coordinate box in physical pixels relative to `crop`, grown
    /// by one pixel for antialiasing.
    fn mask(&self, [x, y, width, height]: [f64; 4], crop: &PixelRect) -> PixelRect {
        // From the pixel the box starts in to the pixel it ends in: a box
        // that starts part way into a pixel reaches a pixel further than
        // its size alone says.
        let low = |start: f64| (start * self.scale).floor() as i64 - 1;
        let high = |end: f64| (end * self.scale).ceil() as i64 + 1;
        let (left, top) = (low(x + self.in_picture.0), low(y + self.in_picture.1));
        PixelRect {
            x: left - crop.x,
            y: top - crop.y,
            width: high(x + width + self.in_picture.0) - left,
            height: high(y + height + self.in_picture.1) - top,
        }
    }

    /// Where text is drawn, relative to `crop`.
    fn text_masks(&self, crop: &PixelRect) -> Vec<PixelRect> {
        let mut masks = Vec::new();
        tree::walk(self.tree, &mut |node| {
            let is_text = match self.text_key {
                "text_box" => node["text"].as_str().is_some_and(|text| !text.is_empty()),
                _ => node["type"] == "ClutterText",
            };
            if is_text && let Ok(rect) = tree::rect(node, self.text_key) {
                masks.push(self.mask(rect, crop));
            }
        });
        masks
    }

    /// Boxes the reference extract blacked out (private text), relative to
    /// `crop`. They are never compared.
    fn redacted_masks(&self, crop: &PixelRect) -> Vec<PixelRect> {
        let mut masks = Vec::new();
        tree::walk(self.tree, &mut |node| {
            if node["redacted"] == true
                && let Ok(rect) = tree::rect(node, "box")
            {
                masks.push(self.mask(rect, crop));
            }
        });
        masks
    }
}

/// The area a region covers, relative to its anchor: the strip, grown by
/// the margin, and cut down to what both pictures hold.
fn region_area(map: &Map, region: &Region, ours: &Side, theirs: &Side) -> Area {
    let margin = map.margin;
    let width = region.width.unwrap_or(theirs.root[2]);
    let (left, right) = match region.anchor {
        Anchor::Left | Anchor::Stretch => (0.0, width),
        Anchor::Center => (-width / 2.0, width / 2.0),
        Anchor::Right => (-width, 0.0),
    };
    Area {
        left: left - margin,
        top: -margin,
        right: right + margin,
        bottom: theirs.root[3] + margin,
    }
    .intersect(ours.available(region.anchor))
    .intersect(theirs.available(region.anchor))
}

/// Cuts `rect` out of `source`, blacking out `masks` (relative to `rect`).
fn crop(source: &Path, rect: &PixelRect, masks: &[PixelRect], target: &Path) -> Result<()> {
    let mut args = vec![
        path_str(source)?.to_owned(),
        "-crop".to_owned(),
        rect.geometry(),
        "+repage".to_owned(),
        "-alpha".to_owned(),
        "off".to_owned(),
        "-fill".to_owned(),
        "black".to_owned(),
    ];
    for mask in masks {
        args.push("-draw".to_owned());
        args.push(format!(
            "rectangle {},{} {},{}",
            mask.x,
            mask.y,
            mask.x + mask.width - 1,
            mask.y + mask.height - 1
        ));
    }
    args.push(path_str(target)?.to_owned());
    magick_ok(&args.iter().map(String::as_str).collect::<Vec<_>>()).map(drop)
}

/// The normalised RMS error between two images, from ImageMagick's
/// `compare`.
fn rmse(a: &Path, b: &Path) -> Result<f64> {
    let output = magick(&[
        "compare",
        "-metric",
        "RMSE",
        path_str(a)?,
        path_str(b)?,
        "null:",
    ])?;
    // `compare` exits 1 when the images differ and 2 on a real error; the
    // number is on stderr either way, as "absolute (normalised)".
    if output.status.code() == Some(2) {
        bail!(
            "magick compare failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let text = String::from_utf8_lossy(&output.stderr);
    let normalised = match text.split_once('(') {
        Some((_, normalised)) => normalised.trim_end_matches([')', '\n', ' ']),
        None => text.trim(),
    };
    normalised
        .parse()
        .with_context(|| format!("unexpected output from magick compare: {text:?}"))
}

/// Marks the pixels where two images differ, white on black, in `mask`, and
/// returns how many there are.
///
/// GNOME Shell snaps every box to whole physical pixels; GTK draws a box
/// where it falls and antialiases its edges. At a fractional scale the two
/// therefore disagree along most edges by a blend of the colours either
/// side, which says nothing about the look. So a pixel only counts as
/// differing if it is outside the range of colours (per channel, widened by
/// the fuzz) that the other image has within one pixel of that place. Both
/// directions are checked: a thin line one image has and the other lacks
/// still counts, as does any wrong colour over an area. An edge that is a
/// whole pixel out of place does not; the layout tier catches that.
fn differing(a: &Path, b: &Path, fuzz_percent: f64, mask: &Path) -> Result<f64> {
    let fuzz = format!("{fuzz_percent}%");
    let (a, b) = (path_str(a)?, path_str(b)?);
    // "x minus y", black where y is at least x.
    let minus = |x: [&'static str; 2], y: [&'static str; 2]| -> Vec<String> {
        let side = |[image, morphology]: [&str; 2]| -> Vec<String> {
            let mut side = vec!["(".to_owned(), image.to_owned()];
            if !morphology.is_empty() {
                side.extend(["-morphology", morphology, "Square:1"].map(str::to_owned));
            }
            side.push(")".to_owned());
            side
        };
        let mut args = vec!["(".to_owned()];
        args.extend(side(x));
        args.extend(side(y));
        args.extend(
            [
                "-compose",
                "MinusSrc",
                "-composite",
                "-threshold",
                &fuzz,
                ")",
            ]
            .map(str::to_owned),
        );
        args
    };
    let mut args: Vec<String> = [
        a, "-write", "mpr:a", "+delete", b, "-write", "mpr:b", "+delete",
    ]
    .map(str::to_owned)
    .to_vec();
    // Darker than everything near it in the other image, or brighter.
    args.extend(minus(["mpr:b", "Erode"], ["mpr:a", ""]));
    args.extend(minus(["mpr:a", ""], ["mpr:b", "Dilate"]));
    args.extend(minus(["mpr:a", "Erode"], ["mpr:b", ""]));
    args.extend(minus(["mpr:b", ""], ["mpr:a", "Dilate"]));
    args.extend(
        [
            "-evaluate-sequence",
            "Max",
            // Any channel counts.
            "-separate",
            "-evaluate-sequence",
            "Max",
            "-write",
            path_str(mask)?,
            "-format",
            "%[fx:mean*w*h]",
            "info:",
        ]
        .map(str::to_owned),
    );
    let text = magick_ok(&args.iter().map(String::as_str).collect::<Vec<_>>())?;
    text.trim()
        .parse()
        .with_context(|| format!("unexpected output from magick: {text:?}"))
}

fn mean_brightness(image: &Path) -> Result<f64> {
    let text = magick_ok(&[
        path_str(image)?,
        "-colorspace",
        "Gray",
        "-format",
        "%[fx:mean]",
        "info:",
    ])?;
    text.trim()
        .parse()
        .with_context(|| format!("unexpected output from magick: {text:?}"))
}

/// Ours above theirs above where they differ (`mask`, from [`differing`]),
/// enlarged.
fn montage(ours: &Path, theirs: &Path, mask: &Path, target: &Path) -> Result<()> {
    magick_ok(&[
        path_str(ours)?,
        path_str(theirs)?,
        "(",
        path_str(mask)?,
        "-fill",
        "red",
        "-opaque",
        "white",
        ")",
        "-background",
        "#ff00ff",
        "-splice",
        "0x2",
        "-append",
        "-filter",
        "point",
        "-resize",
        "300%",
        path_str(target)?,
    ])?;
    Ok(())
}

/// The wallpaper as GNOME shows it on the reference screen ("zoom": scaled
/// to cover, centred), converted from its colour profile to sRGB.
fn reference_wallpaper(map: &Map, size: (i64, i64), work: &Path) -> Result<Option<PathBuf>> {
    if !map.wallpaper.is_file() || !Path::new(SRGB_PROFILE).is_file() {
        return Ok(None);
    }
    let target = work.join("wallpaper-reference.png");
    let side = size.0.max(size.1);
    magick_ok(&[
        path_str(&map.wallpaper)?,
        "-profile",
        SRGB_PROFILE,
        "-resize",
        &format!("{side}x{side}"),
        "-gravity",
        "center",
        "-crop",
        &format!("{}x{}+0+0", size.0, size.1),
        "+repage",
        "-strip",
        path_str(&target)?,
    ])?;
    Ok(Some(target))
}

fn our_surface<'a>(dump: &'a Value, namespace: &str) -> Result<&'a Value> {
    dump["surfaces"]
        .as_array()
        .and_then(|surfaces| {
            surfaces
                .iter()
                .find(|surface| surface["namespace"] == namespace)
        })
        .ok_or_else(|| anyhow!("gniri has no mapped surface {namespace:?}"))
}

fn number(value: &Value, what: &str) -> Result<f64> {
    value
        .as_f64()
        .ok_or_else(|| anyhow!("{what} is not a number: {value}"))
}

fn failed(tier: &'static str, subject: String, error: &anyhow::Error) -> Check {
    Check {
        tier,
        subject,
        detail: error.to_string(),
        pass: false,
        montage: None,
    }
}

/// Reports `differences` (name, ours minus GNOME's) against `tolerance`.
fn layout_check(
    subject: String,
    differences: &[(&str, f64)],
    tolerance: f64,
    boxes: ([f64; 4], [f64; 4]),
) -> Check {
    let worst = differences
        .iter()
        .map(|(_, difference)| difference.abs())
        .fold(0.0, f64::max);
    let pass = worst <= tolerance;
    let detail = if pass {
        format!("within {worst:.2} px")
    } else {
        let mut detail = format!("ours {:?}, GNOME {:?}; off by", boxes.0, boxes.1);
        for (name, difference) in differences {
            if difference.abs() > tolerance {
                let _ = write!(detail, " {name} {difference:+.2}");
            }
        }
        detail
    };
    Check {
        tier: "layout",
        subject,
        detail,
        pass,
        montage: None,
    }
}

/// Tier 1: where the root is on its screen, then every pair's boxes
/// relative to the root.
fn check_layout(
    map: &Map,
    state: &State,
    ours: &Side,
    theirs: &Side,
    tolerance: f64,
    checks: &mut Vec<Check>,
) {
    // The root's distance from the edges it is attached to.
    let placement = |side: &Side| {
        let [x, y, width, height] = side.root;
        let (x, y) = (x + side.on_screen.0, y + side.on_screen.1);
        let horizontal = match map.attach {
            Anchor::Left | Anchor::Stretch => x,
            Anchor::Center => x + width / 2.0 - side.screen.0 / 2.0,
            Anchor::Right => side.screen.0 - (x + width),
        };
        let vertical = match map.vertical {
            Vertical::Top => y,
            Vertical::Bottom => side.screen.1 - (y + height),
            Vertical::Center => y + height / 2.0 - side.screen.1 / 2.0,
        };
        (horizontal, vertical)
    };
    let (our_place, their_place) = (placement(ours), placement(theirs));
    let mut differences = vec![("height", ours.root[3] - theirs.root[3])];
    if map.placement {
        differences.push(("x", our_place.0 - their_place.0));
        differences.push(("y", our_place.1 - their_place.1));
    }
    if map.attach == Anchor::Stretch {
        // Each spans its own screen; the widths are not comparable.
        let fill = |side: &Side| side.root[2] - side.screen.0;
        differences.push(("fill", fill(ours) - fill(theirs)));
    } else {
        differences.push(("width", ours.root[2] - theirs.root[2]));
    }
    checks.push(layout_check(
        format!("{}: placement", state.scene),
        &differences,
        tolerance,
        (ours.root, theirs.root),
    ));

    let wanted = |pair: &&Pair| {
        state
            .pairs
            .as_ref()
            .is_none_or(|names| names.contains(&pair.name))
    };
    for pair in map.pairs.iter().filter(wanted) {
        let subject = format!("{}: {}", state.scene, pair.name);
        let key = match pair.ours_box {
            BoxKind::Border => "box",
            BoxKind::Text => "text_box",
        };
        let boxes = tree::select(ours.tree, &pair.ours)
            .and_then(|node| tree::rect(node, key))
            .and_then(|ours_box| {
                let theirs_box = tree::select(theirs.tree, &pair.theirs)
                    .and_then(|node| tree::rect(node, "box"))?;
                Ok((ours_box, theirs_box))
            });
        let (a, b) = match boxes {
            Ok(boxes) => boxes,
            Err(error) => {
                checks.push(failed("layout", subject, &error));
                continue;
            }
        };

        // Horizontal position as a distance from the pair's anchor on the
        // root; vertical position from the root's top.
        let offset = |side: &Side, [x, _, width, _]: [f64; 4]| match pair.anchor {
            Anchor::Left | Anchor::Stretch => x - side.anchor_x(Anchor::Left),
            Anchor::Center => x + width / 2.0 - side.anchor_x(Anchor::Center),
            Anchor::Right => side.anchor_x(Anchor::Right) - (x + width),
        };
        let mut differences = vec![
            ("x", offset(ours, a) - offset(theirs, b)),
            ("y", (a[1] - ours.root[1]) - (b[1] - theirs.root[1])),
            ("height", a[3] - b[3]),
        ];
        if pair.anchor == Anchor::Stretch {
            differences.push(("fill", (a[2] - ours.root[2]) - (b[2] - theirs.root[2])));
        } else {
            differences.push(("width", a[2] - b[2]));
        }
        checks.push(layout_check(
            subject,
            &differences,
            pair.tolerance_px.unwrap_or(tolerance),
            (a, b),
        ));
    }
}

struct Job<'a> {
    map: &'a Map,
    thresholds: &'a Thresholds,
    component: &'a str,
    work: &'a Path,
    report: &'a Path,
}

fn regions_of<'a>(map: &'a Map, state: &State) -> Vec<&'a Region> {
    map.regions
        .iter()
        .filter(|region| {
            state
                .regions
                .as_ref()
                .is_none_or(|names| names.contains(&region.name))
        })
        .collect()
}

/// The reference side of a state: GNOME's actor tree and where its root
/// is in the committed screenshot crops.
struct Reference {
    document: Value,
    name: String,
}

impl Reference {
    fn load(name: &str) -> Result<Self> {
        let path = reference_dir().join("metrics").join(format!("{name}.json"));
        let document = serde_json::from_str(
            &fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?,
        )?;
        Ok(Self {
            document,
            name: name.to_owned(),
        })
    }

    fn shot(&self, wallpaper: &str) -> PathBuf {
        reference_dir()
            .join("shots")
            .join(format!("{}-{wallpaper}.png", self.name))
    }

    fn side(&self) -> Result<Side<'_>> {
        let tree = &self.document["tree"];
        // The root actor, which the tree may start above, and the part of
        // the screen the crops hold: the root with a margin.
        let root = tree::rect(&self.document, "root_box")?;
        let shot_box = tree::rect(&self.document, "shot_box")?;
        Ok(Side {
            tree,
            root,
            on_screen: (0.0, 0.0),
            in_picture: (-shot_box[0], -shot_box[1]),
            picture: (shot_box[2], shot_box[3]),
            screen: (
                number(&self.document["stage"][0], "stage width")?,
                number(&self.document["stage"][1], "stage height")?,
            ),
            scale: number(&self.document["pixels_per_unit"], "pixels_per_unit")?,
            text_key: "box",
        })
    }
}

/// Our side of a comparison, from one captured run.
fn our_side<'a>(map: &Map, dump: &'a Value) -> Result<Side<'a>> {
    // The wallpaper surface covers the output: its size is the output's.
    let wallpaper = our_surface(dump, "gniri-wallpaper")?;
    let screen = (
        number(&wallpaper["width"], "output width")?,
        number(&wallpaper["height"], "output height")?,
    );
    let surface = our_surface(dump, &map.surface)?;
    let size = (
        number(&surface["width"], "surface width")?,
        number(&surface["height"], "surface height")?,
    );
    let anchored = |edge: &str| {
        surface["anchors"]
            .as_array()
            .is_some_and(|anchors| anchors.iter().any(|anchor| anchor == edge))
    };
    // A surface anchored to one edge of a pair sits against it; anchored
    // to both or neither it is centred (or fills the output).
    let place =
        |low: &str, high: &str, surface: f64, screen: f64| match (anchored(low), anchored(high)) {
            (true, false) => 0.0,
            (false, true) => screen - surface,
            _ => (screen - surface) / 2.0,
        };
    let reserved = match &map.below {
        Some(namespace) => number(
            &our_surface(dump, namespace)?["height"],
            "height of the surface above",
        )?,
        None => 0.0,
    };
    // What is reserved above belongs to neither a surface that hangs from
    // the top nor one that fills the output: both start under it.
    let position = (
        place("left", "right", size.0, screen.0),
        place("top", "bottom", size.1, screen.1 - reserved) + reserved,
    );
    let root = match &map.root {
        Some(selector) => tree::select(&surface["tree"], selector)?,
        None => &surface["tree"],
    };
    let tree = match &map.tree {
        Some(selector) => tree::select(&surface["tree"], selector)?,
        None => root,
    };
    Ok(Side {
        tree,
        root: tree::rect(root, "box")?,
        on_screen: position,
        // The picture is a screenshot of the whole output.
        in_picture: position,
        picture: screen,
        screen,
        scale: number(&surface["scale"], "surface scale")?,
        text_key: "text_box",
    })
}

fn check_state(context: &Job, state: &State, checks: &mut Vec<Check>) -> Result<()> {
    let Job {
        map,
        thresholds,
        component,
        work,
        report,
    } = *context;
    let scene = Scene::load(&state.scene)?;
    let reference = Reference::load(&state.reference)?;
    let theirs = reference.side()?;
    let regions = regions_of(map, state);

    // Tier 2: flat wallpapers, one run each.
    let mut output_size = None;
    // Where our root's top left corner is on the nested output, in logical
    // pixels: the same in every run.
    let mut our_origin = None;
    let origin = |ours: &Side| {
        (
            ours.root[0] + ours.on_screen.0,
            ours.root[1] + ours.on_screen.1,
        )
    };
    let flat_wallpapers: &[(&str, &str)] = if map.flat { &FLAT_WALLPAPERS } else { &[] };
    if !map.flat {
        // The size of the nested output is still needed below.
        let run = format!("{component}-{}-size", state.scene);
        let capture = scene.capture(&run, None, "#808080")?;
        let ours = our_side(map, &capture.dump)?;
        output_size = Some((
            (ours.screen.0 * ours.scale).round() as i64,
            (ours.screen.1 * ours.scale).round() as i64,
        ));
        our_origin = Some(origin(&ours));
    }
    for &(wallpaper, colour) in flat_wallpapers {
        let run = format!("{component}-{}-{wallpaper}", state.scene);
        let capture = scene.capture(&run, Some(wallpaper), colour)?;
        let ours = match our_side(map, &capture.dump) {
            Ok(ours) => ours,
            Err(error) => {
                checks.push(failed(
                    "flat",
                    format!("{}: on {wallpaper}", state.scene),
                    &error,
                ));
                continue;
            }
        };
        output_size = Some((
            (ours.screen.0 * ours.scale).round() as i64,
            (ours.screen.1 * ours.scale).round() as i64,
        ));
        our_origin = Some(origin(&ours));
        for region in &regions {
            let area = region_area(map, region, &ours, &theirs);
            let (our_rect, their_rect) = (
                ours.pixels(region.anchor, &area),
                theirs.pixels(region.anchor, &area),
            );
            let mut masks = ours.text_masks(&our_rect);
            masks.extend(theirs.text_masks(&their_rect));
            masks.extend(theirs.redacted_masks(&their_rect));
            let stem = format!("{}-{wallpaper}-{}", state.scene, region.name);
            let our_crop = work.join(format!("{stem}-ours.png"));
            let their_crop = work.join(format!("{stem}-gnome.png"));
            crop(&capture.shot, &our_rect, &masks, &our_crop)?;
            crop(&reference.shot(wallpaper), &their_rect, &masks, &their_crop)?;

            let mask = work.join(format!("{stem}-differing.png"));
            let count = differing(&our_crop, &their_crop, thresholds.flat_fuzz_percent, &mask)?;
            let share = 100.0 * count / (our_rect.width * our_rect.height) as f64;
            let picture = report.join(format!("{stem}.png"));
            montage(&our_crop, &their_crop, &mask, &picture)?;
            checks.push(Check {
                tier: "flat",
                subject: format!("{}: {} on {wallpaper}", state.scene, region.name),
                detail: format!("{share:.2}% of pixels differ"),
                pass: share
                    <= map
                        .flat_differing_percent
                        .unwrap_or(thresholds.flat_differing_percent),
                montage: Some(picture),
            });
        }
    }

    // Tiers 1 and 3: the real wallpaper. The nested output shows a window of
    // the reference screen, placed for each region so that the region lies
    // over the same wallpaper pixels on both sides.
    let reference_size = (
        (theirs.screen.0 * theirs.scale).round() as i64,
        (theirs.screen.1 * theirs.scale).round() as i64,
    );
    let wallpaper = reference_wallpaper(map, reference_size, work)?;
    let theirs_scale = theirs.scale;
    let mut layout_done = false;
    for region in &regions {
        let (Some(wallpaper), Some((output_width, output_height))) = (&wallpaper, output_size)
        else {
            continue;
        };
        let run = format!("{component}-{}-real-{}", state.scene, region.name);
        // A region that spans the root goes where the root is attached.
        let horizontal = match region.anchor {
            Anchor::Stretch => map.attach,
            anchor => anchor,
        };
        let window_x = match horizontal {
            Anchor::Left | Anchor::Stretch => 0,
            Anchor::Center => (reference_size.0 - output_width) / 2,
            Anchor::Right => reference_size.0 - output_width,
        };
        let window_y = match map.vertical {
            Vertical::Top => 0,
            Vertical::Bottom => reference_size.1 - output_height,
            Vertical::Center => (reference_size.1 - output_height) / 2,
        };
        // A component that is not where GNOME's is on its screen gets
        // the part of the wallpaper that puts the same pixels behind it,
        // as far as the reference screen reaches.
        let (window_x, window_y) = match our_origin {
            Some(ours) if !map.placement => {
                let offset = |theirs: f64, ours: f64, room: i64| {
                    (((theirs - ours) * theirs_scale).round() as i64).clamp(0, room.max(0))
                };
                (
                    offset(theirs.root[0], ours.0, reference_size.0 - output_width),
                    offset(theirs.root[1], ours.1, reference_size.1 - output_height),
                )
            }
            _ => (window_x, window_y),
        };
        let window = work.join(format!("wallpaper-{}.png", region.name));
        magick_ok(&[
            path_str(wallpaper)?,
            "-crop",
            &format!("{output_width}x{output_height}+{window_x}+{window_y}"),
            "+repage",
            path_str(&window)?,
        ])?;

        let capture = scene.capture(&run, None, path_str(&window)?)?;
        let ours = match our_side(map, &capture.dump) {
            Ok(ours) => ours,
            Err(error) => {
                checks.push(failed(
                    "real",
                    format!("{}: {}", state.scene, region.name),
                    &error,
                ));
                continue;
            }
        };
        if !layout_done {
            check_layout(map, state, &ours, &theirs, thresholds.layout_px, checks);
            layout_done = true;
        }

        let area = region_area(map, region, &ours, &theirs);
        let (our_rect, their_rect) = (
            ours.pixels(region.anchor, &area),
            theirs.pixels(region.anchor, &area),
        );
        // Only what the extract blacked out is left out here.
        let masks = theirs.redacted_masks(&their_rect);
        let stem = format!("{}-real-{}", state.scene, region.name);
        let our_crop = work.join(format!("{stem}-ours.png"));
        let their_crop = work.join(format!("{stem}-gnome.png"));
        crop(&capture.shot, &our_rect, &masks, &our_crop)?;
        crop(&reference.shot("real"), &their_rect, &masks, &their_crop)?;
        let mask = work.join(format!("{stem}-differing.png"));
        differing(&our_crop, &their_crop, thresholds.flat_fuzz_percent, &mask)?;
        let picture = report.join(format!("{stem}.png"));
        montage(&our_crop, &their_crop, &mask, &picture)?;

        // Low-pass both, so glyph and icon edges do not dominate.
        let blur = format!("0x{}", thresholds.real_lowpass_sigma);
        let (our_soft, their_soft) = (
            work.join(format!("{stem}-ours-soft.png")),
            work.join(format!("{stem}-gnome-soft.png")),
        );
        for (source, target) in [(&our_crop, &our_soft), (&their_crop, &their_soft)] {
            magick_ok(&[path_str(source)?, "-blur", &blur, path_str(target)?])?;
        }
        let rmse = 100.0 * rmse(&our_soft, &their_soft)?;
        let brightness =
            100.0 * (mean_brightness(&our_soft)? - mean_brightness(&their_soft)?).abs();
        checks.push(Check {
            tier: "real",
            subject: format!("{}: {}", state.scene, region.name),
            detail: format!("RMSE {rmse:.2}%, brightness off by {brightness:.2}%"),
            pass: rmse <= thresholds.real_rmse_percent
                && brightness <= thresholds.real_brightness_percent,
            montage: Some(picture),
        });
    }

    if !layout_done {
        // No real wallpaper on this machine: lay out over a flat colour.
        let run = format!("{component}-{}-layout", state.scene);
        let capture = scene.capture(&run, None, "#808080")?;
        match our_side(map, &capture.dump) {
            Ok(ours) => check_layout(map, state, &ours, &theirs, thresholds.layout_px, checks),
            Err(error) => checks.push(failed("layout", state.scene.clone(), &error)),
        }
        checks.push(Check {
            tier: "real",
            subject: format!("{}: all regions", state.scene),
            detail: format!(
                "skipped: {} or {SRGB_PROFILE} is not on this machine",
                map.wallpaper.display()
            ),
            pass: true,
            montage: None,
        });
    }
    Ok(())
}

/// Names of every component that has a map file.
pub fn components() -> Result<Vec<String>> {
    let dir = session::workspace_root().join("tests/visual/map");
    let mut names: Vec<String> = fs::read_dir(&dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            (path.extension()? == "toml").then(|| path.file_stem()?.to_str().map(str::to_owned))?
        })
        .collect();
    names.sort();
    Ok(names)
}

/// Runs every check for `component` and returns their outcomes.
pub fn run(component: &str) -> Result<Vec<Check>> {
    let root = session::workspace_root();
    let read = |path: PathBuf| {
        fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))
    };
    let map: Map = toml::from_str(&read(
        root.join(format!("tests/visual/map/{component}.toml")),
    )?)
    .with_context(|| format!("parsing the map of {component}"))?;
    let thresholds: Thresholds = toml::from_str(&read(root.join("tests/visual/thresholds.toml"))?)
        .context("parsing tests/visual/thresholds.toml")?;

    let visual = session::visual_dir()?;
    let work = visual.join("diff").join(component);
    let report = visual.join("report").join(component);
    for dir in [&work, &report] {
        fs::create_dir_all(dir)?;
    }

    let context = Job {
        map: &map,
        thresholds: &thresholds,
        component,
        work: &work,
        report: &report,
    };
    let mut checks = Vec::new();
    for state in &map.states {
        check_state(&context, state, &mut checks)?;
    }
    Ok(checks)
}

/// Prints the outcomes as a table and returns whether all passed.
pub fn print(component: &str, checks: &[Check]) -> bool {
    println!("{component}");
    for check in checks {
        println!(
            "  {} {:<6} {:<44} {}",
            if check.pass { "ok  " } else { "FAIL" },
            check.tier,
            check.subject,
            check.detail
        );
    }
    let failed = checks.iter().filter(|check| !check.pass).count();
    println!(
        "  {} of {} checks passed",
        checks.len() - failed,
        checks.len()
    );
    failed == 0
}
