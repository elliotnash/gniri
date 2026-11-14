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
//!   glyphs differently).
//! - **Real** (tier 3): over the real wallpaper, both sides low-passed, the
//!   RMS error and the mean brightness.
//!
//! Tier 0 (theme tokens) is a plain unit test in `crates/theme`.
//!
//! The nested output is narrower than the reference screen, so everything is
//! compared relative to an anchor: the left edge, the centre or the right
//! edge of the surface.

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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Map {
    /// Layer-shell namespace of our surface.
    surface: String,
    /// The wallpaper image the real-wallpaper reference was taken over.
    wallpaper: PathBuf,
    states: Vec<State>,
    regions: Vec<Region>,
    pairs: Vec<Pair>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    scene: String,
    /// Name of the reference extract (tools/gnome-ref/extract.py).
    reference: String,
    /// Regions to compare in this state; all of them if unset.
    regions: Option<Vec<String>>,
}

/// A strip of the surface, full height, compared pixel by pixel.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Region {
    name: String,
    anchor: Anchor,
    /// Logical pixels.
    width: f64,
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

/// One side of a comparison: a tree, the surface it describes and a picture
/// whose origin is the surface's top-left corner.
struct Side<'a> {
    tree: &'a Value,
    /// Logical size of the surface (ours) or the reference actor (theirs).
    width: f64,
    height: f64,
    /// Physical pixels per logical pixel.
    scale: f64,
    /// Key of the text rectangle, and the selector step that finds text.
    text_key: &'static str,
}

impl Side<'_> {
    /// Left edge of a strip `width` wide at `anchor`, logical pixels.
    fn strip_x(&self, anchor: Anchor, width: f64) -> f64 {
        match anchor {
            Anchor::Left | Anchor::Stretch => 0.0,
            Anchor::Center => (self.width - width) / 2.0,
            Anchor::Right => self.width - width,
        }
    }

    fn region(&self, region: &Region) -> PixelRect {
        PixelRect {
            x: (self.strip_x(region.anchor, region.width) * self.scale).round() as i64,
            y: 0,
            width: (region.width * self.scale).round() as i64,
            height: (self.height * self.scale).round() as i64,
        }
    }

    /// Where text is drawn, in physical pixels relative to `region`.
    fn text_masks(&self, region: &PixelRect) -> Vec<PixelRect> {
        let mut masks = Vec::new();
        tree::walk(self.tree, &mut |node| {
            let is_text = match self.text_key {
                "text_box" => node["text"].as_str().is_some_and(|text| !text.is_empty()),
                _ => node["type"] == "ClutterText",
            };
            if !is_text {
                return;
            }
            if let Ok([x, y, width, height]) = tree::rect(node, self.text_key) {
                // One pixel of margin for antialiasing.
                masks.push(PixelRect {
                    x: (x * self.scale).floor() as i64 - region.x - 1,
                    y: (y * self.scale).floor() as i64 - region.y - 1,
                    width: (width * self.scale).ceil() as i64 + 2,
                    height: (height * self.scale).ceil() as i64 + 2,
                });
            }
        });
        masks
    }
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

/// The number ImageMagick's `compare` prints for `metric`: the pixel count
/// for AE, the normalised error for RMSE.
fn compare(metric: &str, fuzz_percent: Option<f64>, a: &Path, b: &Path) -> Result<f64> {
    let mut args = vec![
        "compare".to_owned(),
        "-metric".to_owned(),
        metric.to_owned(),
    ];
    if let Some(fuzz) = fuzz_percent {
        args.extend(["-fuzz".to_owned(), format!("{fuzz}%")]);
    }
    args.extend([
        path_str(a)?.to_owned(),
        path_str(b)?.to_owned(),
        "null:".to_owned(),
    ]);
    let output = magick(&args.iter().map(String::as_str).collect::<Vec<_>>())?;
    // `compare` exits 1 when the images differ and 2 on a real error; the
    // number is on stderr either way, as "absolute (normalised)" or alone.
    if output.status.code() == Some(2) {
        bail!(
            "magick compare failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let text = String::from_utf8_lossy(&output.stderr);
    let wanted = match (metric, text.split_once('(')) {
        ("RMSE", Some((_, normalised))) => normalised.trim_end_matches([')', '\n', ' ']),
        (_, Some((absolute, _))) => absolute.trim(),
        (_, None) => text.trim(),
    };
    wanted
        .parse()
        .with_context(|| format!("unexpected output from magick compare: {text:?}"))
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

/// Ours above theirs above where they differ, enlarged.
fn montage(ours: &Path, theirs: &Path, fuzz_percent: f64, target: &Path) -> Result<()> {
    let difference = target.with_extension("difference.png");
    // Exit status 1 only says the images differ.
    magick(&[
        "compare",
        "-fuzz",
        &format!("{fuzz_percent}%"),
        path_str(ours)?,
        path_str(theirs)?,
        path_str(&difference)?,
    ])?;
    magick_ok(&[
        path_str(ours)?,
        path_str(theirs)?,
        path_str(&difference)?,
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
    let _ = fs::remove_file(difference);
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

/// Tier 1: every pair's boxes, relative to the pair's anchor.
fn check_layout(
    map: &Map,
    state: &State,
    ours: &Side,
    theirs: &Side,
    tolerance: f64,
    checks: &mut Vec<Check>,
) {
    for pair in &map.pairs {
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
                checks.push(Check {
                    tier: "layout",
                    subject,
                    detail: error.to_string(),
                    pass: false,
                    montage: None,
                });
                continue;
            }
        };

        // Horizontal position as a distance from the anchor.
        let offset = |side: &Side, [x, _, width, _]: [f64; 4]| match pair.anchor {
            Anchor::Left | Anchor::Stretch => x,
            Anchor::Center => x + width / 2.0 - side.width / 2.0,
            Anchor::Right => side.width - (x + width),
        };
        let mut differences = vec![
            ("x", offset(ours, a) - offset(theirs, b)),
            ("y", a[1] - b[1]),
            ("height", a[3] - b[3]),
        ];
        if pair.anchor == Anchor::Stretch {
            // Each spans its own surface; the widths are not comparable.
            differences.push(("fill", (a[2] - ours.width) - (b[2] - theirs.width)));
        } else {
            differences.push(("width", a[2] - b[2]));
        }

        let tolerance = pair.tolerance_px.unwrap_or(tolerance);
        let worst = differences
            .iter()
            .map(|(_, difference)| difference.abs())
            .fold(0.0, f64::max);
        let pass = worst <= tolerance;
        let detail = if pass {
            format!("within {worst:.2} px")
        } else {
            let mut detail = format!("ours {a:?}, GNOME {b:?}; off by");
            for (name, difference) in differences {
                if difference.abs() > tolerance {
                    let _ = write!(detail, " {name} {difference:+.2}");
                }
            }
            detail
        };
        checks.push(Check {
            tier: "layout",
            subject,
            detail,
            pass,
            montage: None,
        });
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

fn check_state(context: &Job, state: &State, checks: &mut Vec<Check>) -> Result<()> {
    let Job {
        map,
        thresholds,
        component,
        work,
        report,
    } = *context;
    let scene = Scene::load(&state.scene)?;
    let reference_path = reference_dir()
        .join("metrics")
        .join(format!("{}.json", state.reference));
    let reference: Value = serde_json::from_str(
        &fs::read_to_string(&reference_path)
            .with_context(|| format!("reading {}", reference_path.display()))?,
    )?;
    let reference_shot = |wallpaper: &str| {
        reference_dir()
            .join("shots")
            .join(format!("{}-{wallpaper}.png", state.reference))
    };
    let [_, _, their_width, their_height] = tree::rect(&reference["tree"], "box")?;
    let theirs = Side {
        tree: &reference["tree"],
        width: their_width,
        height: their_height,
        scale: number(&reference["pixels_per_unit"], "pixels_per_unit")?,
        text_key: "box",
    };
    let regions = regions_of(map, state);

    // What both tiers below do with one captured run.
    let ours_of = |dump: &Value| -> Result<(f64, f64, f64)> {
        let surface = our_surface(dump, &map.surface)?;
        Ok((
            number(&surface["width"], "surface width")?,
            number(&surface["height"], "surface height")?,
            number(&surface["scale"], "surface scale")?,
        ))
    };

    // Tier 2: flat wallpapers, one run each.
    let mut output_size = None;
    for (wallpaper, colour) in FLAT_WALLPAPERS {
        let run = format!("{component}-{}-{wallpaper}", state.scene);
        let capture = scene.capture(&run, Some(wallpaper), colour)?;
        // The wallpaper surface covers the output: its size is the nested
        // output's, which the real-wallpaper runs below need.
        let backdrop = our_surface(&capture.dump, "gniri-wallpaper")?;
        let pixels = |key: &str| -> Result<i64> {
            let logical = number(&backdrop[key], key)?;
            Ok((logical * number(&backdrop["scale"], "scale")?).round() as i64)
        };
        output_size = Some((pixels("width")?, pixels("height")?));
        let (width, height, scale) = ours_of(&capture.dump)?;
        let surface = our_surface(&capture.dump, &map.surface)?;
        let ours = Side {
            tree: &surface["tree"],
            width,
            height,
            scale,
            text_key: "text_box",
        };
        for region in &regions {
            let (our_rect, their_rect) = (ours.region(region), theirs.region(region));
            let mut masks = ours.text_masks(&our_rect);
            masks.extend(theirs.text_masks(&their_rect));
            let stem = format!("{}-{wallpaper}-{}", state.scene, region.name);
            let our_crop = work.join(format!("{stem}-ours.png"));
            let their_crop = work.join(format!("{stem}-gnome.png"));
            crop(&capture.shot, &our_rect, &masks, &our_crop)?;
            crop(&reference_shot(wallpaper), &their_rect, &masks, &their_crop)?;

            let differing = compare(
                "AE",
                Some(thresholds.flat_fuzz_percent),
                &our_crop,
                &their_crop,
            )?;
            let share = 100.0 * differing / (our_rect.width * our_rect.height) as f64;
            let picture = report.join(format!("{stem}.png"));
            montage(
                &our_crop,
                &their_crop,
                thresholds.flat_fuzz_percent,
                &picture,
            )?;
            checks.push(Check {
                tier: "flat",
                subject: format!("{}: {} on {wallpaper}", state.scene, region.name),
                detail: format!("{share:.2}% of pixels differ"),
                pass: share <= thresholds.flat_differing_percent,
                montage: Some(picture),
            });
        }
    }

    // Tiers 1 and 3: the real wallpaper. The nested output shows a window of
    // the reference screen, moved for each region so that the region lies
    // over the same wallpaper pixels on both sides.
    let reference_size = (
        (theirs.width * theirs.scale).round() as i64,
        (number(&reference["stage"][1], "stage height")? * theirs.scale).round() as i64,
    );
    let wallpaper = reference_wallpaper(map, reference_size, work)?;
    let mut layout_done = false;
    for region in &regions {
        let (Some(wallpaper), Some((output_width, output_height))) = (&wallpaper, output_size)
        else {
            continue;
        };
        let run = format!("{component}-{}-real-{}", state.scene, region.name);
        let their_rect = theirs.region(region);
        let window_x = match region.anchor {
            Anchor::Left | Anchor::Stretch => 0,
            Anchor::Center => (reference_size.0 - output_width) / 2,
            Anchor::Right => reference_size.0 - output_width,
        };
        let window = work.join(format!("wallpaper-{}.png", region.name));
        magick_ok(&[
            path_str(wallpaper)?,
            "-crop",
            &format!("{output_width}x{output_height}+{window_x}+0"),
            "+repage",
            path_str(&window)?,
        ])?;

        let capture = scene.capture(&run, None, path_str(&window)?)?;
        let (width, height, scale) = ours_of(&capture.dump)?;
        let surface = our_surface(&capture.dump, &map.surface)?;
        let ours = Side {
            tree: &surface["tree"],
            width,
            height,
            scale,
            text_key: "text_box",
        };
        if !layout_done {
            check_layout(map, state, &ours, &theirs, thresholds.layout_px, checks);
            layout_done = true;
        }

        let stem = format!("{}-real-{}", state.scene, region.name);
        let our_crop = work.join(format!("{stem}-ours.png"));
        let their_crop = work.join(format!("{stem}-gnome.png"));
        crop(&capture.shot, &ours.region(region), &[], &our_crop)?;
        crop(&reference_shot("real"), &their_rect, &[], &their_crop)?;
        let picture = report.join(format!("{stem}.png"));
        montage(
            &our_crop,
            &their_crop,
            thresholds.flat_fuzz_percent,
            &picture,
        )?;

        // Low-pass both, so glyph and icon edges do not dominate.
        let blur = format!("0x{}", thresholds.real_lowpass_sigma);
        let (our_soft, their_soft) = (
            work.join(format!("{stem}-ours-soft.png")),
            work.join(format!("{stem}-gnome-soft.png")),
        );
        for (source, target) in [(&our_crop, &our_soft), (&their_crop, &their_soft)] {
            magick_ok(&[path_str(source)?, "-blur", &blur, path_str(target)?])?;
        }
        let rmse = 100.0 * compare("RMSE", None, &our_soft, &their_soft)?;
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
        let (width, height, scale) = ours_of(&capture.dump)?;
        let surface = our_surface(&capture.dump, &map.surface)?;
        let ours = Side {
            tree: &surface["tree"],
            width,
            height,
            scale,
            text_key: "text_box",
        };
        check_layout(map, state, &ours, &theirs, thresholds.layout_px, checks);
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
