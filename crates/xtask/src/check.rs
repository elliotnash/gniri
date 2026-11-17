// SPDX-License-Identifier: GPL-3.0-or-later

//! `cargo xtask check`: behaviour checks in a live nested session.
//!
//! Where `diff` asks "does it look like GNOME", this asks "does it react":
//! the shell is driven through niri and through a virtual pointer, and its
//! widget dump and the screen are inspected after each step.

use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use niri_ipc::Action;
use serde_json::Value;

use crate::scene::{self, Scene};
use crate::session::{self, Session, SessionOptions, wait_until_within};
use crate::tree;

const WALLPAPER: &str = "#808080";
const WINDOW: &str = "#f6f5f4";
/// A pixel of the bar with nothing of the bar's on it, and in the overview
/// well clear of the workspace niri shows behind the bar and of its
/// shadow.
const BAR: (u32, u32) = (200, 20);

struct Checker {
    session: Session,
    passed: usize,
    failed: usize,
}

impl Checker {
    /// Records one check: polls `done` and reports what it was waiting for.
    fn expect(&mut self, what: &str, done: impl FnMut(&Session) -> Result<bool>) {
        self.expect_within(what, std::time::Duration::from_secs(3), done);
    }

    /// The same for something that takes its time, such as a banner
    /// leaving by itself.
    fn expect_within(
        &mut self,
        what: &str,
        limit: std::time::Duration,
        mut done: impl FnMut(&Session) -> Result<bool>,
    ) {
        let session = &self.session;
        match wait_until_within(what, limit, || done(session)) {
            Ok(()) => {
                println!("  ok   {what}");
                self.passed += 1;
            }
            Err(error) => {
                println!("  FAIL {what}: {error}");
                self.failed += 1;
            }
        }
    }
}

fn panel(session: &Session) -> Result<Value> {
    let mut panel =
        surface(session, "gniri-panel")?.ok_or_else(|| anyhow!("no panel surface is mapped"))?;
    Ok(panel["tree"].take())
}

/// One entry per workspace dot, true for the active one (the pill).
fn dots(session: &Session) -> Result<Vec<bool>> {
    let panel = panel(session)?;
    let mut dots = Vec::new();
    tree::walk(&panel, &mut |node| {
        if tree::classes(node).contains(&"workspace-dot")
            && let Ok([_, _, width, height]) = tree::rect(node, "box")
        {
            dots.push(width > height);
        }
    });
    Ok(dots)
}

fn has_state(node: &Value, state: &str) -> bool {
    node["states"]
        .as_array()
        .is_some_and(|states| states.iter().any(|other| other == state))
}

/// Whether the panel shows the overview look: transparent bar, activities
/// button checked.
fn panel_shows_overview(session: &Session) -> Result<bool> {
    let panel = panel(session)?;
    let bar = tree::select(&panel, "#panel")?;
    let activities = tree::select(&panel, "#panelActivities")?;
    Ok(tree::classes(bar).contains(&"overview") && has_state(activities, "checked"))
}

/// The colour of one screen pixel, as 8-bit RGB.
fn pixel(session: &Session, x: u32, y: u32) -> Result<[u8; 3]> {
    let shot = session::visual_dir()?.join("check.png");
    session.screenshot(&shot)?;
    let channel = |name: &str| format!("%[fx:round(255*u.p{{{x},{y}}}.{name})]");
    let format = format!("{},{},{}", channel("r"), channel("g"), channel("b"));
    let output = Command::new("magick")
        .arg(&shot)
        .args(["-format", &format, "info:"])
        .output()
        .context("running ImageMagick (`magick`)")?;
    let text = String::from_utf8_lossy(&output.stdout);
    let values: Vec<u8> = text
        .trim()
        .split(',')
        .filter_map(|value| value.parse().ok())
        .collect();
    match values[..] {
        [r, g, b] => Ok([r, g, b]),
        _ => bail!("unexpected output from magick: {text:?}"),
    }
}

fn near(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(a, b)| a.abs_diff(b) <= 2)
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

/// Moves the nested session's pointer to a logical position. `wlrctl` only
/// moves relatively, so go far past the bottom-right corner first, where
/// the compositor stops the pointer at the output's edge. (The top-left
/// corner would do too, but is niri's hot corner.)
fn pointer_to(session: &Session, x: i32, y: i32) -> Result<()> {
    let (width, height) = output_size(session)?;
    pointer(session, &["move", "10000", "10000"])?;
    pointer(
        session,
        &["move", &(x - width).to_string(), &(y - height).to_string()],
    )
}

/// The output's logical size: that of the wallpaper surface, which covers
/// it.
fn output_size(session: &Session) -> Result<(i32, i32)> {
    let wallpaper = surface(session, "gniri-wallpaper")?
        .ok_or_else(|| anyhow!("no wallpaper surface is mapped"))?;
    let size = |key: &str| {
        wallpaper[key]
            .as_i64()
            .map(|value| value as i32)
            .ok_or_else(|| anyhow!("the wallpaper surface has no {key}"))
    };
    Ok((size("width")?, size("height")?))
}

/// One of gniri's mapped surfaces, by namespace.
fn surface(session: &Session, namespace: &str) -> Result<Option<Value>> {
    let mut dump = session.dump_tree()?;
    let surfaces = dump["surfaces"]
        .as_array_mut()
        .ok_or_else(|| anyhow!("the dump has no surfaces"))?;
    Ok(surfaces
        .iter()
        .position(|surface| surface["namespace"] == namespace)
        .map(|index| surfaces.swap_remove(index)))
}

/// Clicks the middle of the widget `selector` picks on the stage.
fn click(session: &Session, selector: &str) -> Result<()> {
    click_on(session, "gniri-stage", selector)
}

/// Where a surface's top left corner is on the output. The stage covers
/// the output; a banner is centred under the panel.
fn surface_origin(session: &Session, surface: &Value) -> Result<(f64, f64)> {
    let panel_height = || -> Result<f64> {
        Ok(self::surface(session, "gniri-panel")?
            .and_then(|panel| panel["height"].as_f64())
            .unwrap_or(0.0))
    };
    // The overview's surface fills the output under the panel.
    if surface["namespace"] == "gniri-overview" {
        return Ok((0.0, panel_height()?));
    }
    if surface["namespace"] != "gniri-banner" {
        return Ok((0.0, 0.0));
    }
    let (output_width, _) = output_size(session)?;
    let width = surface["width"]
        .as_f64()
        .ok_or_else(|| anyhow!("the surface has no width"))?;
    Ok(((f64::from(output_width) - width) / 2.0, panel_height()?))
}

/// Clicks the middle of the widget `selector` picks on the surface called
/// `namespace`.
///
/// The click waits until the widget shows the pointer over it: each
/// `wlrctl` call is a client of its own, whose requests the compositor may
/// handle a moment later, and the surface may only just have been mapped.
fn click_on(session: &Session, namespace: &str, selector: &str) -> Result<()> {
    let target = || -> Result<(i32, i32)> {
        // A banner or a menu that was just asked for slides into place;
        // where its widgets are is only settled once it has been drawn
        // there.
        session.gniri_msg(&["debug", "sync"])?;
        let found =
            surface(session, namespace)?.ok_or_else(|| anyhow!("{namespace} is not mapped"))?;
        let (left, top) = surface_origin(session, &found)?;
        let [x, y, width, height] = tree::rect(tree::select(&found["tree"], selector)?, "box")?;
        Ok((
            (left + x + width / 2.0).round() as i32,
            (top + y + height / 2.0).round() as i32,
        ))
    };
    let (mut x, mut y) = target()?;
    let mut attempts = 0;
    loop {
        attempts += 1;
        let reached = hover_at(session, x, y, "the widget", || {
            Ok(on_surface(session, namespace, selector)?
                .is_some_and(|widget| has_state(&widget, "hover")))
        });
        match reached {
            Ok(()) => break,
            Err(error) if attempts == 2 => return Err(error),
            // A surface that was mapped a moment ago can still be laid
            // out anew when the compositor tells it its size: look again
            // where the widget is.
            Err(_) => (x, y) = target()?,
        }
    }
    pointer(session, &["click", "left"])
}

/// Moves the pointer to a place and waits until `hovered` says the widget
/// there has it. The compositor may apply the two moves that take the
/// pointer there in the wrong order, which leaves it somewhere else; then
/// they are made again.
fn hover_at(
    session: &Session,
    x: i32,
    y: i32,
    what: &str,
    mut hovered: impl FnMut() -> Result<bool>,
) -> Result<()> {
    let mut attempts = 0;
    loop {
        attempts += 1;
        pointer_to(session, x, y)?;
        let reached = wait_until_within(
            &format!("the pointer to reach {what}"),
            std::time::Duration::from_secs(1),
            || {
                nudge(session)?;
                hovered()
            },
        );
        match reached {
            Ok(()) => return Ok(()),
            Err(error) if attempts == 3 => return Err(error),
            Err(_) => {}
        }
    }
}

/// Moves the pointer a pixel and back. A surface that is mapped under a
/// resting pointer is not told the pointer is there until it moves.
fn nudge(session: &Session) -> Result<()> {
    pointer(session, &["move", "1", "0"])?;
    pointer(session, &["move", "-1", "0"])
}

/// Clicks the status button at the right end of the panel.
fn click_status_button(session: &Session) -> Result<()> {
    const BUTTON: &str = "#panelRight .panel-button:0";
    let panel = panel(session)?;
    let [x, y, width, height] = tree::rect(tree::select(&panel, BUTTON)?, "box")?;
    hover_at(
        session,
        (x + width / 2.0).round() as i32,
        (y + height / 2.0).round() as i32,
        "the status button",
        || {
            let panel = self::panel(session)?;
            Ok(has_state(tree::select(&panel, BUTTON)?, "hover"))
        },
    )?;
    pointer(session, &["click", "left"])
}

/// A node on the stage, if a menu is open and has it.
fn on_stage(session: &Session, selector: &str) -> Result<Option<Value>> {
    on_surface(session, "gniri-stage", selector)
}

/// A node on the surface called `namespace`, if it is mapped and has it.
fn on_surface(session: &Session, namespace: &str, selector: &str) -> Result<Option<Value>> {
    let Some(surface) = surface(session, namespace)? else {
        return Ok(None);
    };
    Ok(tree::select(&surface["tree"], selector).ok().cloned())
}

/// How many nodes of the stage carry `class`.
fn count_on_stage(session: &Session, class: &str) -> Result<usize> {
    count_on(session, "gniri-stage", "", class)
}

/// How many nodes carry `class` below the node `within` picks on the
/// surface called `namespace` (anywhere on it for an empty `within`).
fn count_on(session: &Session, namespace: &str, within: &str, class: &str) -> Result<usize> {
    let Some(surface) = surface(session, namespace)? else {
        return Ok(0);
    };
    let root = if within.is_empty() {
        &surface["tree"]
    } else {
        match tree::select(&surface["tree"], within) {
            Ok(root) => root,
            Err(_) => return Ok(0),
        }
    };
    let mut count = 0;
    tree::walk(root, &mut |node| {
        if tree::classes(node).contains(&class) {
            count += 1;
        }
    });
    Ok(count)
}

/// Presses and releases one key in the nested session.
fn key(session: &Session, name: &str) -> Result<()> {
    let status = session
        .client_command("wtype")
        .args(["-k", name])
        .status()
        .context("running wtype")?;
    if !status.success() {
        bail!("wtype failed");
    }
    Ok(())
}

/// Presses a key that a surface which was only just mapped is to get.
///
/// The compositor gives a new surface the keyboard a moment after it is
/// mapped, and a key sent before that goes elsewhere. So the key is
/// pressed again, a few times at most, until `done` shows it arrived.
fn key_until(
    session: &Session,
    name: &str,
    mut done: impl FnMut(&Session) -> Result<bool>,
) -> Result<()> {
    for _ in 0..4 {
        session.gniri_msg(&["debug", "sync"])?;
        key(session, name)?;
        let arrived = wait_until_within(
            "the key to arrive",
            std::time::Duration::from_millis(700),
            || done(session),
        );
        if arrived.is_ok() {
            break;
        }
    }
    Ok(())
}

/// Types `text` in the nested session, as a keyboard would.
fn type_text(session: &Session, text: &str) -> Result<()> {
    let status = session
        .client_command("wtype")
        .arg(text)
        .status()
        .context("running wtype")?;
    if !status.success() {
        bail!("wtype failed");
    }
    Ok(())
}

/// The text of every label below `node`, in order.
fn texts(node: &Value) -> Vec<String> {
    let mut texts = Vec::new();
    tree::walk(node, &mut |node| {
        if let Some(text) = node["text"].as_str().filter(|text| !text.is_empty()) {
            texts.push(text.to_owned());
        }
    });
    texts
}

/// Every command the shell has sent to a service so far.
fn commands(session: &Session) -> Result<Vec<String>> {
    let text = session.gniri_msg(&["debug", "commands"])?;
    serde_json::from_str(&text).context("gniri's command log is not a list of strings")
}

/// Whether a command starting with `prefix` has been sent.
fn sent(session: &Session, prefix: &str) -> Result<bool> {
    Ok(commands(session)?
        .iter()
        .any(|command| command.starts_with(prefix)))
}

fn text_of(node: &Value, selector: &str) -> Option<String> {
    tree::select(node, selector)
        .ok()
        .and_then(|label| label["text"].as_str())
        .map(str::to_owned)
}

/// Runs one `wlrctl pointer` action.
fn pointer(session: &Session, arguments: &[&str]) -> Result<()> {
    let status = session
        .client_command("wlrctl")
        .arg("pointer")
        .args(arguments)
        .status()
        .context("running wlrctl")?;
    if !status.success() {
        bail!("wlrctl pointer {} failed", arguments.join(" "));
    }
    // Every call is a Wayland client of its own, and the compositor reads
    // its clients in no promised order: without a pause, two moves in a row
    // can be applied the wrong way round.
    std::thread::sleep(std::time::Duration::from_millis(30));
    Ok(())
}

/// The panel: workspace dots, the overview, fullscreen windows and the
/// pointer, with real services behind it.
fn panel_and_overview() -> Result<(usize, usize)> {
    // An empty fixture: real clock and services, but animations off, so a
    // dump never catches a dot half way through growing.
    let fixture = session::visual_dir()?.join("logs/check.fixture.toml");
    fs::write(&fixture, "")?;
    let session = Session::start(&SessionOptions {
        name: "check".to_owned(),
        niri_config: None,
        renderer: "vulkan".to_owned(),
        gniri_args: scene::gniri_args(Some(Path::new(&fixture)), Some(WALLPAPER)),
    })?;
    let mut checker = Checker {
        session,
        passed: 0,
        failed: 0,
    };

    println!("workspace dots follow niri");
    checker.expect("an empty session has one dot, active", |session| {
        Ok(dots(session)? == [true])
    });
    checker.session.spawn_window(WINDOW)?;
    checker.expect(
        "a window on the first workspace adds a dot for the new empty one",
        |session| Ok(dots(session)? == [true, false]),
    );
    checker.session.action(Action::FocusWorkspaceDown {})?;
    checker.expect("focusing the workspace below moves the pill", |session| {
        Ok(dots(session)? == [false, true])
    });
    checker.session.action(Action::FocusWorkspaceUp {})?;
    checker.expect("and back", |session| Ok(dots(session)? == [true, false]));

    println!("overview");
    checker.session.action(Action::FocusWorkspaceDown {})?;
    checker.expect("the bar is tinted on the desktop", |session| {
        // 70% of rgb(21,21,24) over the grey wallpaper (docs/DECISIONS.md, S5).
        Ok(near(pixel(session, BAR.0, BAR.1)?, [53, 53, 55]))
    });
    checker.session.action(Action::OpenOverview {})?;
    checker.expect(
        "opening the overview checks the activities button",
        panel_shows_overview,
    );
    checker.expect(
        "and clears the bar, showing the dimmed backdrop",
        |session| Ok(near(pixel(session, BAR.0, BAR.1)?, [77, 77, 77])),
    );
    checker.session.action(Action::CloseOverview {})?;
    checker.expect("closing it restores the bar", |session| {
        Ok(!panel_shows_overview(session)? && near(pixel(session, BAR.0, BAR.1)?, [53, 53, 55]))
    });

    println!("fullscreen");
    checker.session.action(Action::FocusWorkspaceUp {})?;
    checker
        .session
        .action(Action::FullscreenWindow { id: None })?;
    checker.expect("a fullscreen window covers the panel", |session| {
        Ok(near(pixel(session, BAR.0, BAR.1)?, [0xf6, 0xf5, 0xf4]))
    });
    checker
        .session
        .action(Action::FullscreenWindow { id: None })?;
    checker.expect("and the panel is back afterwards", |session| {
        Ok(near(pixel(session, BAR.0, BAR.1)?, [53, 53, 55]))
    });

    println!("pointer");
    if on_path("wlrctl") {
        // Reaching the corner trips niri's hot corner; start from a closed
        // overview with the pointer resting on the activities button.
        pointer_to(&checker.session, 30, 16)?;
        checker.session.action(Action::CloseOverview {})?;
        checker.expect("(the overview is closed)", |session| {
            Ok(!session.overview_is_open()?)
        });
        checker.expect("hovering the activities button highlights it", |session| {
            let panel = panel(session)?;
            Ok(has_state(
                tree::select(&panel, "#panelActivities")?,
                "hover",
            ))
        });
        pointer(&checker.session, &["click", "left"])?;
        checker.expect("clicking it opens the overview", Session::overview_is_open);
        pointer(&checker.session, &["click", "left"])?;
        checker.expect("clicking again closes it", |session| {
            Ok(!session.overview_is_open()?)
        });
        pointer(&checker.session, &["scroll", "15", "0"])?;
        checker.expect(
            "scrolling down on it switches to the workspace below",
            |session| Ok(dots(session)? == [false, true]),
        );
        std::thread::sleep(std::time::Duration::from_millis(200));
        pointer(&checker.session, &["scroll", "-15", "0"])?;
        checker.expect("and scrolling up switches back", |session| {
            Ok(dots(session)? == [true, false])
        });
    } else {
        println!("  skip wlrctl is not installed");
    }

    let Checker {
        session,
        passed,
        failed,
    } = checker;
    session.quit()?;
    Ok((passed, failed))
}

/// A part of the checks: a session of its own.
type Part = fn() -> Result<(usize, usize)>;

/// The parts, by the names `cargo xtask check <part>` takes.
const PARTS: [(&str, Part); 13] = [
    ("panel", panel_and_overview),
    ("overview", overview),
    ("folders", folders_and_dragging),
    ("quick-settings", quick_settings),
    ("notifications", notifications),
    ("notifications-bus", notifications_over_the_bus),
    ("session", end_session_and_idle),
    ("lock", lock_screen),
    ("polkit", authentication_dialog),
    ("agents", agent_dialogs),
    ("config", config_reload),
    ("outputs", outputs_coming_and_going),
    ("language", another_language),
];

/// Runs every part, or only the one called `only`.
pub fn run(only: Option<&str>) -> Result<bool> {
    if let Some(name) = only
        && !PARTS.iter().any(|(part, _)| *part == name)
    {
        let names: Vec<&str> = PARTS.iter().map(|(part, _)| *part).collect();
        bail!("no part called {name:?}; there are: {}", names.join(", "));
    }
    let (mut passed, mut failed) = (0, 0);
    for (name, part) in PARTS {
        if only.is_some_and(|only| only != name) {
            continue;
        }
        let (part_passed, part_failed) = part()?;
        passed += part_passed;
        failed += part_failed;
    }
    println!("{passed} of {} checks passed", passed + failed);
    Ok(failed == 0)
}

/// The middle of the node called `name` below the node `within` picks on
/// the overview's surface, on the output.
fn overview_spot(session: &Session, within: &str, name: &str) -> Result<(f64, f64)> {
    let found =
        surface(session, "gniri-overview")?.ok_or_else(|| anyhow!("the overview is not mapped"))?;
    let (left, top) = surface_origin(session, &found)?;
    let root = tree::select(&found["tree"], within)?;
    let mut spot = None;
    tree::walk(root, &mut |node| {
        if spot.is_none() && node["name"] == name {
            spot = tree::rect(node, "box").ok();
        }
    });
    let [x, y, width, height] = spot.ok_or_else(|| anyhow!("nothing called {name:?} shows"))?;
    Ok((left + x + width / 2.0, top + y + height / 2.0))
}

/// The names of the tiles below the node `within` picks on the overview,
/// in their order.
fn tile_names(session: &Session, within: &str) -> Result<Vec<String>> {
    let Some(found) = surface(session, "gniri-overview")? else {
        return Ok(Vec::new());
    };
    let Ok(root) = tree::select(&found["tree"], within) else {
        return Ok(Vec::new());
    };
    let mut names = Vec::new();
    tree::walk(root, &mut |node| {
        if tree::classes(node).contains(&"overview-tile")
            && let Some(name) = node["name"].as_str()
        {
            names.push(name.to_owned());
        }
    });
    Ok(names)
}

/// App folders, and dragging icons in the overview, with the harness's own
/// pointer: GNOME's drags are a pointer held down and moved, which
/// `wlrctl` cannot do.
fn folders_and_dragging() -> Result<(usize, usize)> {
    const GRID: &str = ".app-display";
    const DIALOG: &str = ".app-folder-dialog";
    const DASH: &str = "#dash";
    /// Long enough for the icons to make room, or another to show a folder
    /// would be made, before letting go.
    const REST: std::time::Duration = std::time::Duration::from_millis(800);

    let fixture = Scene::load("app-grid-folders")?.write_fixture("check-folders", None)?;
    let session = Session::start(&SessionOptions {
        name: "check-folders".to_owned(),
        niri_config: None,
        renderer: "vulkan".to_owned(),
        gniri_args: scene::gniri_args(Some(&fixture), Some(WALLPAPER)),
    })?;
    let (width, height) = output_size(&session)?;
    let mut pointer =
        crate::pointer::Pointer::connect(&session.wayland_display, (width as u32, height as u32))?;
    let mut checker = Checker {
        session,
        passed: 0,
        failed: 0,
    };
    let grid = |session: &Session| tile_names(session, GRID);
    let dialog = |session: &Session| tile_names(session, DIALOG);
    let dash = |session: &Session| tile_names(session, DASH);
    let arranged = |session: &Session| -> Result<usize> {
        Ok(commands(session)?
            .iter()
            .filter(|command| command.starts_with("apps: Arrange"))
            .count())
    };
    let click = |pointer: &mut crate::pointer::Pointer, spot: (f64, f64)| -> Result<()> {
        pointer.move_to(spot)?;
        std::thread::sleep(std::time::Duration::from_millis(60));
        pointer.press()?;
        std::thread::sleep(std::time::Duration::from_millis(30));
        pointer.release()
    };

    checker.session.action(Action::OpenOverview {})?;
    checker.session.gniri_msg(&["debug", "show-apps"])?;
    println!("folders");
    checker.expect("folders are in the grid where they were put", |session| {
        Ok(grid(session)?.starts_with(&[
            "Maps".to_owned(),
            "Utilities".to_owned(),
            "Boxes".to_owned(),
            "Office".to_owned(),
        ]))
    });
    checker.expect("and what is in them is not beside them", |session| {
        Ok(!grid(session)?
            .iter()
            .any(|name| name == "Fonts" || name == "Writer"))
    });
    checker.session.gniri_msg(&["debug", "sync"])?;
    let spot = overview_spot(&checker.session, GRID, "Utilities")?;
    click(&mut pointer, spot)?;
    checker.expect("a click on a folder opens it", |session| {
        Ok(dialog(session)?
            == [
                "Document Viewer",
                "Fonts",
                "Image Viewer",
                "Passwords",
                "Zip Archives",
            ])
    });
    checker.expect("under its name, in the user's language", |session| {
        Ok(on_surface(session, "gniri-overview", ".folder-name-label")?
            .and_then(|label| label["text"].as_str().map(str::to_owned))
            .is_some_and(|name| !name.is_empty()))
    });
    click(&mut pointer, (20.0, f64::from(height) / 2.0))?;
    checker.expect("a click beside it closes it", |session| {
        Ok(dialog(session)?.is_empty())
    });

    println!("dragging in the grid");
    checker.session.gniri_msg(&["debug", "sync"])?;
    let (camera, characters) = (
        overview_spot(&checker.session, GRID, "Camera")?,
        overview_spot(&checker.session, GRID, "Characters")?,
    );
    // Held over the other one a moment: it shows the folder it would
    // make. The picture is for looking at
    // (target/visual/drag-folder-preview.png).
    pointer.move_to(camera)?;
    pointer.press()?;
    std::thread::sleep(std::time::Duration::from_millis(50));
    pointer.glide_to(characters)?;
    std::thread::sleep(REST);
    checker.expect("held over another, that one shows the folder", |session| {
        Ok(on_surface(session, "gniri-overview", ".overview-tile.drop")?.is_some())
    });
    checker
        .session
        .screenshot(&session::visual_dir()?.join("drag-folder-preview.png"))?;
    pointer.release()?;
    checker.expect(
        "an application dropped on another makes a folder of the two",
        |session| {
            let grid = grid(session)?;
            Ok(grid.get(4).is_some_and(|name| name == "Unnamed Folder")
                && !grid
                    .iter()
                    .any(|name| name == "Camera" || name == "Characters"))
        },
    );
    checker.expect("and the arrangement is the shell's to keep", |session| {
        Ok(arranged(session)? == 1)
    });

    checker.session.gniri_msg(&["debug", "sync"])?;
    let maps = overview_spot(&checker.session, GRID, "Maps")?;
    let boxes = overview_spot(&checker.session, GRID, "Boxes")?;
    // Beside Boxes, towards the icon after it: the place after it.
    let tile = 145.0_f64.min(f64::from(width) / 10.0);
    pointer.move_to(maps)?;
    pointer.press()?;
    std::thread::sleep(std::time::Duration::from_millis(50));
    pointer.glide_to((boxes.0 + tile * 0.45, boxes.1))?;
    std::thread::sleep(REST);
    // Room made where it would go (target/visual/drag-room.png).
    checker
        .session
        .screenshot(&session::visual_dir()?.join("drag-room.png"))?;
    pointer.release()?;
    checker.expect("one dropped beside another goes there", |session| {
        Ok(grid(session)?.starts_with(&[
            "Utilities".to_owned(),
            "Boxes".to_owned(),
            "Maps".to_owned(),
        ]))
    });

    checker.session.gniri_msg(&["debug", "sync"])?;
    let clocks = overview_spot(&checker.session, GRID, "Clocks")?;
    let office = overview_spot(&checker.session, GRID, "Office")?;
    pointer.drag(clocks, office, REST)?;
    checker.expect("one dropped on a folder goes into it", |session| {
        Ok(!grid(session)?.iter().any(|name| name == "Clocks"))
    });
    checker
        .session
        .gniri_msg(&["debug", "open-folder", "folder-1"])?;
    checker.expect("last, after what was there", |session| {
        Ok(dialog(session)? == ["Writer", "Spreadsheets", "Presentations", "Clocks"])
    });

    println!("a folder's dialog");
    checker.session.gniri_msg(&["debug", "sync"])?;
    let spreadsheets = overview_spot(&checker.session, DIALOG, "Spreadsheets")?;
    let writer = overview_spot(&checker.session, DIALOG, "Writer")?;
    pointer.drag(spreadsheets, (writer.0 - tile * 0.45, writer.1), REST)?;
    checker.expect("its applications are put in another order", |session| {
        Ok(dialog(session)? == ["Spreadsheets", "Writer", "Presentations", "Clocks"])
    });
    checker.session.gniri_msg(&["debug", "sync"])?;
    let presentations = overview_spot(&checker.session, DIALOG, "Presentations")?;
    // Out of the dialog, long enough for it to close, and into the grid.
    pointer.move_to(presentations)?;
    pointer.press()?;
    std::thread::sleep(std::time::Duration::from_millis(50));
    pointer.glide_to((30.0, f64::from(height) / 2.0))?;
    std::thread::sleep(std::time::Duration::from_millis(900));
    checker.expect("dragged out of it, it closes", |session| {
        Ok(dialog(session)?.is_empty())
    });
    pointer.release()?;
    checker.expect("and the application is in the grid again", |session| {
        Ok(grid(session)?.iter().any(|name| name == "Presentations"))
    });

    checker
        .session
        .gniri_msg(&["debug", "open-folder", "folder-1"])?;
    checker.session.gniri_msg(&["debug", "sync"])?;
    let edit = {
        let found = surface(&checker.session, "gniri-overview")?
            .ok_or_else(|| anyhow!("the overview is not mapped"))?;
        let (left, top) = surface_origin(&checker.session, &found)?;
        let [x, y, w, h] = tree::rect(
            tree::select(&found["tree"], ".folder-name-container .icon-button")?,
            "box",
        )?;
        (left + x + w / 2.0, top + y + h / 2.0)
    };
    click(&mut pointer, edit)?;
    checker.expect("its name can be changed", |session| {
        Ok(on_surface(session, "gniri-overview", ".folder-name-entry")?.is_some())
    });
    if on_path("wtype") {
        type_text(&checker.session, "Work")?;
        key(&checker.session, "Return")?;
        checker.expect("and the new name is the folder's", |session| {
            Ok(on_surface(session, "gniri-overview", ".folder-name-label")?
                .is_some_and(|label| label["text"] == "Work"))
        });
        key(&checker.session, "Escape")?;
        checker.expect("in the grid too", |session| {
            Ok(dialog(session)?.is_empty() && grid(session)?.iter().any(|name| name == "Work"))
        });
    } else {
        println!("  skip wtype is not installed");
        checker.session.gniri_msg(&["debug", "close-folder"])?;
    }

    println!("the dash");
    checker.session.gniri_msg(&["debug", "sync"])?;
    let favorites = dash(&checker.session)?.len();
    let help = overview_spot(&checker.session, GRID, "Help")?;
    let first = overview_spot(&checker.session, DASH, "Zen Browser")?;
    pointer.drag(help, (first.0 + 10.0, first.1), REST)?;
    checker.expect(
        "an application dragged to the dash is pinned there",
        |session| {
            let dash = dash(session)?;
            Ok(dash.len() == favorites + 1 && dash.get(1).is_some_and(|name| name == "Help"))
        },
    );
    checker.expect("and is no longer in the grid", |session| {
        Ok(!grid(session)?.iter().any(|name| name == "Help"))
    });
    checker.session.gniri_msg(&["debug", "sync"])?;
    let pinned = overview_spot(&checker.session, DASH, "Help")?;
    let show_apps = {
        let found = surface(&checker.session, "gniri-overview")?
            .ok_or_else(|| anyhow!("the overview is not mapped"))?;
        let (left, top) = surface_origin(&checker.session, &found)?;
        let [x, y, w, h] = tree::rect(tree::select(&found["tree"], ".show-apps")?, "box")?;
        (left + x + w / 2.0, top + y + h / 2.0)
    };
    pointer.drag(pinned, show_apps, REST)?;
    checker.expect("dropped on the apps button, it is unpinned", |session| {
        Ok(dash(session)?.len() == favorites && grid(session)?.iter().any(|name| name == "Help"))
    });

    println!("a drag let go of nowhere");
    checker.session.gniri_msg(&["debug", "sync"])?;
    let before = arranged(&checker.session)?;
    let order = grid(&checker.session)?;
    let boxes = overview_spot(&checker.session, GRID, "Boxes")?;
    // The search entry takes nothing.
    pointer.drag(boxes, (f64::from(width) / 2.0, 70.0), REST)?;
    checker.expect("changes nothing", |session| {
        Ok(arranged(session)? == before && grid(session)? == order)
    });

    let Checker {
        session,
        passed,
        failed,
    } = checker;
    drop(pointer);
    session.quit()?;
    Ok((passed, failed))
}

/// gniri in German: the texts are GNOME Shell's own translations, the
/// dates are written as GNOME's German formats say, and a text put
/// together from parts has them in the translation's order.
///
/// What a text should read is asked of gettext here, not written down: the
/// check is that gniri finds GNOME's translations, whatever they say this
/// year. It needs GNOME Shell's German catalog and the German locale, and
/// is skipped without them.
fn another_language() -> Result<(usize, usize)> {
    const LOCALE: &str = "de_DE.UTF-8";
    let german = |command: &mut Command| -> Option<String> {
        let output = command
            .env("LC_ALL", LOCALE)
            .env("LANGUAGE", "de")
            .output()
            .ok()?;
        Some(String::from_utf8_lossy(&output.stdout).into_owned())
    };
    let text = |original: &str| -> Option<String> {
        german(Command::new("gettext").args(["-d", "gnome-shell", original]))
            .filter(|text| text != original)
    };
    let installed = german(Command::new("locale").arg("-a")).is_some_and(|locales| {
        locales
            .lines()
            .any(|locale| locale.eq_ignore_ascii_case("de_DE.utf8"))
    });
    let (Some(no_events), Some(no_notifications), Some(dark_style), Some(today)) = (
        text("No Events"),
        text("No Notifications"),
        text("Dark Style"),
        text("Today"),
    ) else {
        println!("language: skipped (GNOME Shell's German translation is not installed)");
        return Ok((0, 0));
    };
    if !installed {
        println!("language: skipped (the locale {LOCALE} is not installed)");
        return Ok((0, 0));
    }
    let power_off = german(Command::new("ngettext").args([
        "-d",
        "gnome-shell",
        "The system will power off automatically in %d second",
        "The system will power off automatically in %d seconds",
        "60",
    ]))
    .unwrap_or_default()
    .replace("%d", "60");

    let path = session::visual_dir()?
        .join("logs")
        .join("check-language.fixture.toml");
    fs::write(
        &path,
        r#"
        time = "2026-11-14T08:28:00"
        session = { can_power_off = true }
        dark_style = { available = true }
        do_not_disturb = { available = true }
        "#,
    )?;
    let session = Session::start_in(
        &SessionOptions {
            name: "check-language".to_owned(),
            niri_config: None,
            renderer: "vulkan".to_owned(),
            gniri_args: scene::gniri_args(Some(&path), Some(WALLPAPER)),
        },
        session::Language {
            locale: LOCALE.to_owned(),
            languages: Some("de".to_owned()),
        },
    )?;
    let mut checker = Checker {
        session,
        passed: 0,
        failed: 0,
    };
    let stage_has = |session: &Session, wanted: &[&str]| -> Result<bool> {
        let Some(stage) = surface(session, "gniri-stage")? else {
            return Ok(false);
        };
        let texts = texts(&stage["tree"]);
        Ok(wanted
            .iter()
            .all(|wanted| texts.iter().any(|text| text == wanted)))
    };

    println!("another language");
    checker.expect("the clock writes the date as German does", |session| {
        // "%-e. %b_%l:%M %p": the day first, with a full stop.
        Ok(texts(&panel(session)?)
            .iter()
            .any(|text| text.starts_with("14. Nov")))
    });
    checker
        .session
        .gniri_msg(&["debug", "open-menu", "date-menu"])?;
    checker.expect(
        "the date menu's texts are GNOME's translations",
        |session| stage_has(session, &[&no_events, &no_notifications, &today]),
    );
    checker.expect("its date is written out in German", |session| {
        stage_has(session, &["Samstag", "14. November 2026"])
    });
    checker.expect("nothing of it is left in English", |session| {
        let Some(stage) = surface(session, "gniri-stage")? else {
            return Ok(false);
        };
        let texts = texts(&stage["tree"]);
        Ok(!["No Events", "No Notifications", "Today", "Saturday"]
            .iter()
            .any(|english| texts.iter().any(|text| text == english)))
    });
    checker
        .session
        .gniri_msg(&["debug", "open-menu", "quick-settings"])?;
    checker.expect("so are the quick settings' texts", |session| {
        stage_has(session, &[&dark_style])
    });
    checker.session.gniri_msg(&["debug", "close-menu"])?;
    checker.session.gniri_msg(&["power-off"])?;
    checker.expect(
        "a sentence with a number in it is the translation's, filled in",
        |session| {
            let Some(dialog) = surface(session, "gniri-dialog")? else {
                return Ok(false);
            };
            Ok(texts(&dialog["tree"]).contains(&power_off))
        },
    );
    checker.session.gniri_msg(&["debug", "close-dialog"])?;

    let Checker {
        session,
        passed,
        failed,
    } = checker;
    session.quit()?;
    Ok((passed, failed))
}

/// The quick settings menu and the on-screen display, in a session of
/// their own whose services are the `quick-settings` scene's fixture:
/// nothing a click does here reaches the system, and what it asked for can
/// be read back from the command log.
fn quick_settings() -> Result<(usize, usize)> {
    const WIFI: &str = ".quick-toggle-has-menu:0";
    const POWER_MODE: &str = ".quick-toggle-has-menu:2";
    const DARK_STYLE: &str = ".quick-toggle:5";

    let fixture = Scene::load("quick-settings")?.write_fixture("check-menu", None)?;
    let session = Session::start(&SessionOptions {
        name: "check-menu".to_owned(),
        niri_config: None,
        renderer: "vulkan".to_owned(),
        gniri_args: scene::gniri_args(Some(&fixture), Some(WALLPAPER)),
    })?;
    let mut checker = Checker {
        session,
        passed: 0,
        failed: 0,
    };
    let status_button_checked = |session: &Session| -> Result<bool> {
        let panel = panel(session)?;
        Ok(has_state(
            tree::select(&panel, "#panelRight .panel-button:0")?,
            "checked",
        ))
    };
    let menu_is_open = |session: &Session| Ok(on_stage(session, ".quick-settings")?.is_some());
    let menu_is_closed = |session: &Session| Ok(surface(session, "gniri-stage")?.is_none());

    println!("quick settings");
    checker
        .session
        .gniri_msg(&["debug", "open-menu", "quick-settings"])?;
    checker.expect("opening the menu maps the stage", menu_is_open);
    checker.expect("and shows the status button pressed", status_button_checked);
    checker
        .session
        .gniri_msg(&["debug", "open-submenu", "power-mode"])?;
    checker.expect(
        "a submenu opens under its toggle and dims the rest",
        |session| {
            let menu = on_stage(session, ".quick-toggle-menu-container")?;
            let bubble = on_stage(session, ".quick-settings")?;
            Ok(menu.is_some()
                && bubble.is_some_and(|bubble| tree::classes(&bubble).contains(&"shaded")))
        },
    );
    checker.session.action(Action::OpenOverview {})?;
    checker.expect("opening the overview closes the menu", menu_is_closed);
    checker.expect("and releases the status button", |session| {
        Ok(!status_button_checked(session)?)
    });
    checker.session.action(Action::CloseOverview {})?;

    println!("volume and brightness keys");
    checker.session.gniri_msg(&["volume", "up"])?;
    checker.expect("volume up asks for one step more", |session| {
        // The scene starts at 0.2545; one step is 0.06.
        sent(session, "audio: SetOutputVolume(0.31")
    });
    checker.expect("and shows the level on the display", |session| {
        Ok(surface(session, "gniri-osd")?.is_some_and(|osd| {
            tree::select(&osd["tree"], "GtkImage")
                .is_ok_and(|icon| icon["icon"] == "audio-volume-low-symbolic")
        }))
    });
    checker.session.gniri_msg(&["volume", "mute"])?;
    checker.expect("mute asks to mute and shows the muted icon", |session| {
        Ok(sent(session, "audio: SetOutputMuted(true)")?
            && surface(session, "gniri-osd")?.is_some_and(|osd| {
                tree::select(&osd["tree"], "GtkImage")
                    .is_ok_and(|icon| icon["icon"] == "audio-volume-muted-symbolic")
            }))
    });
    checker.session.gniri_msg(&["brightness", "down"])?;
    checker.expect("brightness down asks for one step less", |session| {
        sent(session, "brightness: Set(0.95")
    });
    checker.expect("the display goes away by itself", |session| {
        Ok(surface(session, "gniri-osd")?.is_none())
    });

    println!("quick settings with the pointer");
    if on_path("wlrctl") {
        click_status_button(&checker.session)?;
        checker.expect("clicking the status button opens the menu", menu_is_open);

        click(&checker.session, DARK_STYLE)?;
        checker.expect("clicking Dark Style asks to turn it off", |session| {
            sent(session, "dark-style: Set(false)")
        });
        checker.expect("and the toggle is no longer checked", |session| {
            Ok(on_stage(session, DARK_STYLE)?.is_some_and(|toggle| !has_state(&toggle, "checked")))
        });

        click(&checker.session, &format!("{WIFI} .quick-toggle"))?;
        checker.expect("clicking Wi-Fi asks to switch it off", |session| {
            sent(session, "network: SetWifiEnabled(false)")
        });

        click(
            &checker.session,
            &format!("{POWER_MODE} .quick-toggle-menu-button"),
        )?;
        checker.expect("clicking Power Mode's arrow opens its menu", |session| {
            Ok(on_stage(session, ".quick-toggle-menu")?
                .and_then(|menu| text_of(&menu, ".title"))
                .as_deref()
                == Some("Power Mode"))
        });
        click(
            &checker.session,
            ".quick-toggle-menu .popup-menu-section .popup-menu-item:1",
        )?;
        checker.expect("choosing Balanced asks for that profile", |session| {
            sent(session, "power: SetProfile(Balanced)")
        });
        checker.expect("closes the submenu and updates the toggle", |session| {
            let closed = on_stage(session, ".quick-toggle-menu")?.is_none();
            let toggle = on_stage(session, POWER_MODE)?;
            Ok(closed
                && toggle.is_some_and(|toggle| {
                    text_of(&toggle, ".quick-toggle-subtitle").as_deref() == Some("Balanced")
                        && !has_state(&toggle, "checked")
                }))
        });

        click(&checker.session, ".quick-slider:1 .slider")?;
        checker.expect(
            "clicking the middle of the brightness slider sets it to half",
            |session| {
                // The newest brightness command, such as "brightness: Set(0.5)".
                let level = commands(session)?.iter().rev().find_map(|command| {
                    command
                        .strip_prefix("brightness: Set(")?
                        .strip_suffix(')')?
                        .parse::<f64>()
                        .ok()
                });
                Ok(level.is_some_and(|level| (level - 0.5).abs() < 0.02))
            },
        );

        // Well away from the menu. Nothing there shows the pointer
        // arriving, so give the compositor a moment to move it.
        pointer_to(&checker.session, 200, 500)?;
        std::thread::sleep(std::time::Duration::from_millis(200));
        pointer(&checker.session, &["click", "left"])?;
        checker.expect("clicking outside the menu closes it", menu_is_closed);

        if on_path("wtype") {
            click_status_button(&checker.session)?;
            checker.expect("(the menu is open again)", menu_is_open);
            checker
                .session
                .gniri_msg(&["debug", "open-submenu", "wifi"])?;
            let escape = |session: &Session| -> Result<()> {
                let status = session
                    .client_command("wtype")
                    .args(["-k", "Escape"])
                    .status()
                    .context("running wtype")?;
                if !status.success() {
                    bail!("wtype failed");
                }
                Ok(())
            };
            escape(&checker.session)?;
            checker.expect("Escape closes the submenu first", |session| {
                Ok(on_stage(session, ".quick-toggle-menu")?.is_none()
                    && on_stage(session, ".quick-settings")?.is_some())
            });
            escape(&checker.session)?;
            checker.expect("and then the menu", menu_is_closed);
        } else {
            println!("  skip wtype is not installed");
        }

        click_status_button(&checker.session)?;
        checker.expect("(the menu is open again)", menu_is_open);
        click_status_button(&checker.session)?;
        checker.expect("clicking the status button again closes it", menu_is_closed);
    } else {
        println!("  skip wlrctl is not installed");
    }

    let Checker {
        session,
        passed,
        failed,
    } = checker;
    session.quit()?;
    Ok((passed, failed))
}

/// Longer than a banner stays up by itself.
const BANNER_TIME: std::time::Duration = std::time::Duration::from_secs(7);

/// Banners and the date menu, in a session whose notifications are the
/// `date-menu` scene's: nothing here needs a notification server, and what
/// a click asked for can be read back from the command log.
fn notifications() -> Result<(usize, usize)> {
    const BANNER: &str = "gniri-banner";
    const CLOCK: &str = "#panelCenter .panel-button";

    let fixture = Scene::load("date-menu")?.write_fixture("check-notifications", None)?;
    let session = Session::start(&SessionOptions {
        name: "check-notifications".to_owned(),
        niri_config: None,
        renderer: "vulkan".to_owned(),
        gniri_args: scene::gniri_args(Some(&fixture), Some(WALLPAPER)),
    })?;
    let mut checker = Checker {
        session,
        passed: 0,
        failed: 0,
    };
    let notify = |session: &Session, arguments: &[&str]| -> Result<String> {
        let mut all = vec!["debug", "notify"];
        all.extend(arguments);
        Ok(session.gniri_msg(&all)?.trim().to_owned())
    };
    let banner_title = |session: &Session| -> Result<Option<String>> {
        Ok(surface(session, BANNER)?.and_then(|banner| text_of(&banner["tree"], ".message-title")))
    };
    let no_banner = |session: &Session| Ok(surface(session, BANNER)?.is_none());
    let menu_is_open = |session: &Session| Ok(on_stage(session, ".datemenu-popover")?.is_some());
    let menu_is_closed = |session: &Session| Ok(surface(session, "gniri-stage")?.is_none());
    let group_titles = |session: &Session| -> Result<Vec<String>> {
        let Some(stage) = surface(session, "gniri-stage")? else {
            return Ok(Vec::new());
        };
        let mut titles = Vec::new();
        tree::walk(&stage["tree"], &mut |node| {
            if tree::classes(node).contains(&"message-notification-group")
                && let Some(title) = text_of(node, ".message:0 .message-title")
            {
                titles.push(title);
            }
        });
        Ok(titles)
    };

    println!("notification banners");
    notify(&checker.session, &["--app", "Tester", "Hello", "World"])?;
    checker.expect("a notification gets a banner under the panel", |session| {
        Ok(banner_title(session)?.as_deref() == Some("Hello")
            && sent(session, "notifications: ShowNextBanner")?)
    });
    notify(
        &checker.session,
        &["--app", "Tester", "Second", "Waits its turn"],
    )?;
    checker.expect("a second one waits for the first to leave", |session| {
        Ok(banner_title(session)?.as_deref() == Some("Hello"))
    });
    checker.expect_within(
        "the banner leaves by itself and the next one follows",
        BANNER_TIME,
        |session| {
            Ok(banner_title(session)?.as_deref() == Some("Second")
                && sent(session, "notifications: BannerHidden")?)
        },
    );
    checker.expect_within("and then none is left", BANNER_TIME, no_banner);

    let transient = notify(&checker.session, &["--transient", "Volume", "Passing"])?;
    checker.expect("a transient notification has its banner too", |session| {
        Ok(banner_title(session)?.as_deref() == Some("Volume"))
    });
    checker.expect_within("(its banner leaves)", BANNER_TIME, no_banner);

    let critical = notify(
        &checker.session,
        &[
            "--urgency",
            "critical",
            "--action",
            "fix=Fix It",
            "Battery",
            "Nearly empty",
        ],
    )?;
    checker.expect(
        "a critical one shows in full, with its buttons",
        |session| Ok(on_surface(session, BANNER, ".notification-button")?.is_some()),
    );
    // Longer than a banner's four seconds.
    std::thread::sleep(std::time::Duration::from_millis(4600));
    checker.expect("and stays until it is dealt with", |session| {
        Ok(banner_title(session)?.as_deref() == Some("Battery"))
    });
    if on_path("wlrctl") {
        click_on(&checker.session, BANNER, ".notification-button")?;
        checker.expect("clicking its button tells the application", |session| {
            sent(
                session,
                &format!("notifications: InvokeAction {{ id: {critical}, key: \"fix\""),
            )
        });
        checker.expect("and takes the banner down", no_banner);

        notify(
            &checker.session,
            &["--urgency", "critical", "Disk", "Nearly full"],
        )?;
        checker.expect("(another critical banner)", |session| {
            Ok(banner_title(session)?.as_deref() == Some("Disk"))
        });
        click_on(&checker.session, BANNER, ".message-close-button")?;
        checker.expect("its close button dismisses it", |session| {
            Ok(sent(session, "notifications: Dismiss(")? && surface(session, BANNER)?.is_none())
        });
    } else {
        println!("  skip wlrctl is not installed");
        checker
            .session
            .gniri_msg(&["debug", "open-menu", "date-menu"])?;
        checker.session.gniri_msg(&["debug", "close-menu"])?;
    }

    println!("date menu");
    checker
        .session
        .gniri_msg(&["debug", "open-menu", "date-menu"])?;
    checker.expect("opening it maps the stage", menu_is_open);
    checker.expect("and shows the clock pressed", |session| {
        let panel = panel(session)?;
        Ok(has_state(tree::select(&panel, CLOCK)?, "checked"))
    });
    checker.expect("the list has what arrived, newest first", |session| {
        let titles = group_titles(session)?;
        Ok(titles.first().map(String::as_str) == Some("Second")
            && titles.iter().any(|title| title == "Reference notification"))
    });
    checker.expect("less the transient one", |session| {
        Ok(
            !group_titles(session)?.iter().any(|title| title == "Volume")
                && sent(session, "notifications: BannerHidden")?
                && !transient.is_empty(),
        )
    });
    notify(&checker.session, &["--app", "Tester", "Quiet", "No banner"])?;
    checker.expect(
        "with the menu open a notification only joins the list",
        |session| {
            Ok(group_titles(session)?.iter().any(|title| title == "Quiet")
                && surface(session, BANNER)?.is_none())
        },
    );
    checker.expect(
        "today is selected and the today button has nothing to do",
        |session| {
            let today = on_stage(session, ".calendar-today")?;
            let button = on_stage(session, ".datemenu-today-button")?;
            Ok(
                today.is_some_and(|day| text_of(&day, "GtkLabel").as_deref() == Some("14"))
                    && button.is_some_and(|button| has_state(&button, "insensitive")),
            )
        },
    );

    if on_path("wlrctl") {
        const SYSTEM: &str = "#system";
        click(
            &checker.session,
            &format!("{SYSTEM} .message:0 .message-title"),
        )?;
        checker.expect(
            "clicking a stack of notifications opens the group",
            |session| {
                let header = on_stage(session, &format!("{SYSTEM} .message-group-header"))?;
                let third = on_stage(session, &format!("{SYSTEM} .message:2"))?;
                Ok(header.is_some()
                    && third.is_some_and(|card| !tree::classes(&card).contains(&"lower-in-stack")))
            },
        );
        checker.expect("and fades the other groups back", |session| {
            Ok(count_on_stage(session, "shaded")? > 0)
        });
        if on_path("wtype") {
            key(&checker.session, "Escape")?;
            checker.expect("Escape closes the group first", |session| {
                Ok(
                    on_stage(session, &format!("{SYSTEM} .message-group-header"))?.is_none()
                        && on_stage(session, ".datemenu-popover")?.is_some(),
                )
            });
        } else {
            println!("  skip wtype is not installed");
            click(
                &checker.session,
                &format!("{SYSTEM} .message-collapse-button"),
            )?;
        }
        click(
            &checker.session,
            &format!("{SYSTEM} .message:0 .message-close-button"),
        )?;
        checker.expect("closing a closed group dismisses all of it", |session| {
            Ok(sent(session, "notifications: DismissSource(\"system\")")?
                && on_stage(session, SYSTEM)?.is_none())
        });

        click(&checker.session, ".calendar-change-month-forward")?;
        checker.expect("the arrow shows the next month", |session| {
            Ok(on_stage(session, ".calendar")?
                .and_then(|calendar| text_of(&calendar, ".calendar-month-label"))
                .as_deref()
                == Some("December"))
        });
        checker.expect("and wakes the today button", |session| {
            Ok(on_stage(session, ".datemenu-today-button")?
                .is_some_and(|button| !has_state(&button, "insensitive")))
        });
        click(&checker.session, ".datemenu-today-button")?;
        checker.expect("which goes back to today", |session| {
            Ok(on_stage(session, ".calendar")?
                .and_then(|calendar| text_of(&calendar, ".calendar-month-label"))
                .as_deref()
                == Some("November"))
        });
        click(&checker.session, ".calendar-day:8")?;
        checker.expect("choosing a day names it on the events card", |session| {
            Ok(on_stage(session, ".events-title")?
                .and_then(|title| title["text"].as_str().map(str::to_owned))
                .as_deref()
                == Some("November 2"))
        });

        click(&checker.session, ".message-list-clear-button")?;
        checker.expect("Clear empties the list", |session| {
            Ok(sent(session, "notifications: Clear")?
                && count_on_stage(session, "message")? == 0
                && on_stage(session, ".message-list-placeholder")?.is_some())
        });

        click(&checker.session, ".world-clocks-button")?;
        checker.expect(
            "a card starts its application and closes the menu",
            |session| {
                Ok(sent(session, "launch-app: \"org.gnome.clocks\"")?
                    && surface(session, "gniri-stage")?.is_none())
            },
        );

        let click_clock = |session: &Session| -> Result<()> {
            let panel = panel(session)?;
            let [x, y, width, height] = tree::rect(tree::select(&panel, CLOCK)?, "box")?;
            hover_at(
                session,
                (x + width / 2.0).round() as i32,
                (y + height / 2.0).round() as i32,
                "the clock",
                || {
                    let panel = self::panel(session)?;
                    Ok(has_state(tree::select(&panel, CLOCK)?, "hover"))
                },
            )?;
            pointer(session, &["click", "left"])
        };
        click_clock(&checker.session)?;
        checker.expect("clicking the clock opens the menu", menu_is_open);
        click_status_button(&checker.session)?;
        checker.expect("the status button swaps it for quick settings", |session| {
            Ok(on_stage(session, ".quick-settings")?.is_some()
                && on_stage(session, ".datemenu-popover")?.is_none())
        });
        click_clock(&checker.session)?;
        checker.expect("and the clock swaps back", menu_is_open);
        click_clock(&checker.session)?;
        checker.expect("clicking the clock again closes it", menu_is_closed);
    } else {
        println!("  skip wlrctl is not installed");
        checker.session.gniri_msg(&["debug", "close-menu"])?;
    }

    println!("do not disturb");
    checker
        .session
        .gniri_msg(&["debug", "open-menu", "quick-settings"])?;
    checker.expect("(quick settings are open)", |session| {
        Ok(on_stage(session, ".quick-settings")?.is_some())
    });
    if on_path("wlrctl") {
        // The toggles that show depend on the scene; find this one by name.
        let stage = surface(&checker.session, "gniri-stage")?
            .ok_or_else(|| anyhow!("the quick settings menu did not open"))?;
        let mut titles = Vec::new();
        tree::walk(&stage["tree"], &mut |node| {
            if tree::classes(node).contains(&"quick-toggle") {
                titles.push(text_of(node, ".quick-toggle-title").unwrap_or_default());
            }
        });
        let index = titles
            .iter()
            .position(|title| title == "Do Not Disturb")
            .ok_or_else(|| anyhow!("no Do Not Disturb toggle among {titles:?}"))?;
        click(&checker.session, &format!(".quick-toggle:{index}"))?;
        checker.expect("the toggle asks for quiet", |session| {
            sent(session, "do-not-disturb: Set(true)")
        });
        checker.session.gniri_msg(&["debug", "close-menu"])?;
        checker.expect("(the menu is closed)", menu_is_closed);
        notify(&checker.session, &["--app", "Tester", "Hush", "Not now"])?;
        notify(&checker.session, &["--urgency", "critical", "Fire", "Now"])?;
        checker.expect(
            "then only a critical notification gets a banner",
            |session| Ok(banner_title(session)?.as_deref() == Some("Fire")),
        );
    } else {
        println!("  skip wlrctl is not installed");
    }

    let Checker {
        session,
        passed,
        failed,
    } = checker;
    session.quit()?;
    Ok((passed, failed))
}

/// The notification server itself: a session with no fixture, so gniri
/// serves `org.freedesktop.Notifications` on the session's private bus, and
/// `notify-send` as any application would use it.
fn notifications_over_the_bus() -> Result<(usize, usize)> {
    println!("notifications over the bus");
    if !on_path("notify-send") {
        println!("  skip notify-send is not installed");
        return Ok((0, 0));
    }
    let session = Session::start(&SessionOptions {
        name: "check-bus".to_owned(),
        niri_config: None,
        renderer: "vulkan".to_owned(),
        gniri_args: scene::gniri_args(None, Some(WALLPAPER)),
    })?;
    let mut checker = Checker {
        session,
        passed: 0,
        failed: 0,
    };
    let send = |session: &Session, arguments: &[&str]| -> Result<()> {
        let mut command = vec!["notify-send".to_owned()];
        command.extend(arguments.iter().map(|argument| (*argument).to_owned()));
        // Started by niri, so it talks to the session's own bus.
        session.action(Action::Spawn { command })
    };

    // The server takes a moment to claim its name; an application that
    // starts before then would have its notification go nowhere.
    std::thread::sleep(std::time::Duration::from_millis(500));
    send(
        &checker.session,
        &[
            "--app-name",
            "Tester",
            "Over the bus",
            "From <b>notify-send</b>",
        ],
    )?;
    checker.expect("notify-send gets a banner", |session| {
        let Some(banner) = surface(session, "gniri-banner")? else {
            return Ok(false);
        };
        Ok(
            text_of(&banner["tree"], ".message-title").as_deref() == Some("Over the bus")
                && text_of(&banner["tree"], ".message-source-title").as_deref() == Some("Tester")
                && text_of(&banner["tree"], ".message-body").as_deref() == Some("From notify-send"),
        )
    });
    checker.expect_within("which leaves by itself", BANNER_TIME, |session| {
        Ok(surface(session, "gniri-banner")?.is_none())
    });
    checker.expect(
        "and leaves the dot beside the clock gone, it having been seen",
        |session| {
            let panel = panel(session)?;
            Ok(tree::select(&panel, "#panelCenter .messages-indicator").is_err())
        },
    );
    send(
        &checker.session,
        &["--urgency", "low", "Quietly", "No banner for this"],
    )?;
    checker.expect("a low-urgency one only lights the dot", |session| {
        let panel = panel(session)?;
        Ok(
            tree::select(&panel, "#panelCenter .messages-indicator").is_ok()
                && surface(session, "gniri-banner")?.is_none(),
        )
    });
    checker
        .session
        .gniri_msg(&["debug", "open-menu", "date-menu"])?;
    checker.expect(
        "both are in the date menu, and the dot goes out",
        |session| {
            let panel = panel(session)?;
            Ok(count_on_stage(session, "message-notification-group")? == 2
                && tree::select(&panel, "#panelCenter .messages-indicator").is_err())
        },
    );

    let Checker {
        session,
        passed,
        failed,
    } = checker;
    session.quit()?;
    Ok((passed, failed))
}

/// Starts a session whose fixture is `fixture`, written to a file named
/// after `name`.
fn fixture_session(name: &str, fixture: &str) -> Result<Session> {
    let path = session::visual_dir()?
        .join("logs")
        .join(format!("{name}.fixture.toml"));
    fs::write(&path, fixture)?;
    Session::start(&SessionOptions {
        name: name.to_owned(),
        niri_config: None,
        renderer: "vulkan".to_owned(),
        gniri_args: scene::gniri_args(Some(&path), Some(WALLPAPER)),
    })
}

/// The dialog that asks before ending the session, and what idleness
/// does, with a fixture for services: what the shell asks of logind, niri
/// and the backlight is read back from the command log, and nothing is
/// powered off.
fn end_session_and_idle() -> Result<(usize, usize)> {
    const TITLE: &str = ".message-dialog-title";
    const DESCRIPTION: &str = ".message-dialog-description";

    let session = fixture_session(
        "check-session",
        r#"
        time = "2026-11-14T08:28:00"
        session = { can_suspend = true, can_restart = true, can_power_off = true }
        user = { name = "user", real_name = "Example User" }
        brightness = { level = 0.8 }
        idle = { available = true }
        "#,
    )?;
    let mut checker = Checker {
        session,
        passed: 0,
        failed: 0,
    };
    let dialog = |session: &Session, selector: &str| on_surface(session, "gniri-dialog", selector);
    let dialog_text = |session: &Session, selector: &str| -> Result<Option<String>> {
        Ok(dialog(session, selector)?.and_then(|node| node["text"].as_str().map(str::to_owned)))
    };

    println!("end-session dialog");
    checker
        .session
        .gniri_msg(&["debug", "open-menu", "quick-settings"])?;
    checker
        .session
        .gniri_msg(&["debug", "open-submenu", "shutdown"])?;
    checker.expect("the power menu offers what logind allows", |session| {
        let Some(menu) = on_stage(session, ".quick-toggle-menu")? else {
            return Ok(false);
        };
        let texts = texts(&menu);
        Ok(["Suspend", "Restart…", "Power Off…", "Log Out…"]
            .iter()
            .all(|entry| texts.iter().any(|text| text == entry)))
    });
    checker.session.gniri_msg(&["power-off"])?;
    checker.expect("asking to power off opens the dialog", |session| {
        Ok(dialog_text(session, TITLE)?.as_deref() == Some("Power Off"))
    });
    checker.expect("which gives a minute", |session| {
        Ok(dialog_text(session, DESCRIPTION)?.as_deref()
            == Some("The system will power off automatically in 60 seconds"))
    });
    checker.expect("and closes the menu behind it", |session| {
        Ok(surface(session, "gniri-stage")?.is_none())
    });
    if on_path("wtype") {
        key_until(&checker.session, "Escape", |session| {
            Ok(surface(session, "gniri-dialog")?.is_none())
        })?;
        checker.expect("Escape cancels", |session| {
            Ok(surface(session, "gniri-dialog")?.is_none())
        });
        checker.expect("and nothing was asked of logind", |session| {
            Ok(!sent(session, "session:")?)
        });
    } else {
        println!("  skip wtype is not installed");
        checker.session.gniri_msg(&["debug", "close-dialog"])?;
    }

    checker.session.gniri_msg(&["restart"])?;
    checker.expect("asking to restart opens the dialog", |session| {
        Ok(dialog_text(session, TITLE)?.as_deref() == Some("Restart"))
    });
    if on_path("wlrctl") {
        click_on(&checker.session, "gniri-dialog", ".modal-dialog-button:1")?;
        checker.expect("its second button restarts", |session| {
            sent(session, "session: Restart")
        });
        checker.expect("and closes the dialog", |session| {
            Ok(surface(session, "gniri-dialog")?.is_none())
        });
    } else {
        println!("  skip wlrctl is not installed");
        checker.session.gniri_msg(&["debug", "close-dialog"])?;
    }

    checker.session.gniri_msg(&["log-out"])?;
    checker.expect("the log-out dialog names the user", |session| {
        Ok(
            dialog_text(session, TITLE)?.as_deref() == Some("Log Out Example User")
                && dialog_text(session, DESCRIPTION)?.as_deref()
                    == Some("Example User will be logged out automatically in 60 seconds"),
        )
    });
    if on_path("wlrctl") {
        click_on(&checker.session, "gniri-dialog", ".modal-dialog-button:1")?;
        checker.expect("confirming asks niri to quit", |session| {
            sent(session, "niri: Quit")
        });
    } else {
        checker.session.gniri_msg(&["debug", "close-dialog"])?;
    }

    println!("idle");
    checker.session.gniri_msg(&["debug", "idle", "dim"])?;
    checker.expect("four fifths of the way the screen dims", |session| {
        sent(session, "brightness: Set(0.3)")
    });
    checker
        .session
        .gniri_msg(&["debug", "idle", "dim", "--resumed"])?;
    checker.expect("and is as bright as before at the next input", |session| {
        sent(session, "brightness: Set(0.8)")
    });
    checker.session.gniri_msg(&["debug", "idle", "dim"])?;
    checker.session.gniri_msg(&["debug", "idle", "blank"])?;
    checker.expect("then the screens turn off", |session| {
        sent(session, "niri: PowerOffMonitors")
    });
    checker.session.gniri_msg(&["debug", "idle", "lock"])?;
    checker.expect("and the session is asked to lock", |session| {
        sent(session, "session: Lock")
    });
    let locks = |session: &Session| -> Result<usize> {
        Ok(commands(session)?
            .iter()
            .filter(|command| command.starts_with("session: Lock"))
            .count())
    };
    checker.session.gniri_msg(&["lock"])?;
    checker.expect("`gniri msg lock` asks the same", |session| {
        Ok(locks(session)? == 2)
    });

    let Checker {
        session,
        passed,
        failed,
    } = checker;
    session.quit()?;
    Ok((passed, failed))
}

/// The lock screen, locking the nested session for real: the compositor
/// hides everything else, keys reach only the lock surface, and only the
/// fixture's password ends it. No PAM: a scene's lock screen never asks it.
fn lock_screen() -> Result<(usize, usize)> {
    const CLOCK: &str = ".unlock-dialog-clock";
    const PROMPT: &str = ".login-dialog-prompt-layout";
    const MESSAGE: &str = ".login-dialog-message";

    if !on_path("wtype") {
        println!("lock screen");
        println!("  skip wtype is not installed");
        return Ok((0, 0));
    }
    let session = fixture_session(
        "check-lock",
        r#"
        time = "2026-11-13T22:08:00"
        user = { name = "user", real_name = "Example User" }
        lock = { password = "sesame" }
        "#,
    )?;
    let mut checker = Checker {
        session,
        passed: 0,
        failed: 0,
    };
    let log = session::visual_dir()?.join("logs/check-lock-lock.log");
    let on_lock = |session: &Session, selector: &str| on_surface(session, "gniri-lock", selector);

    println!("lock screen");
    checker.expect("(the desktop shows before)", |session| {
        Ok(near(pixel(session, BAR.0, BAR.1)?, [53, 53, 55]))
    });
    checker.session.spawn_helper("lock")?;
    checker.expect("the locker says when the compositor has locked", |_| {
        Ok(fs::read_to_string(&log).is_ok_and(|log| log.lines().any(|line| line == "locked")))
    });
    checker.expect("the lock screen starts on the clock", |session| {
        let Some(clock) = on_lock(session, CLOCK)? else {
            return Ok(false);
        };
        Ok(texts(&clock)
            == [
                "10\u{2236}08 PM",
                "Friday November 13",
                "Click or press a key",
            ]
            && on_lock(session, PROMPT)?.is_none())
    });
    key_until(&checker.session, "Return", |session| {
        Ok(on_lock(session, PROMPT)?.is_some())
    })?;
    checker.expect("a key brings up the prompt for the user", |session| {
        let Some(prompt) = on_lock(session, PROMPT)? else {
            return Ok(false);
        };
        Ok(
            text_of(&prompt, ".user-widget-label").as_deref() == Some("Example User")
                && on_lock(session, CLOCK)?.is_none(),
        )
    });
    key(&checker.session, "Escape")?;
    checker.expect("Escape goes back to the clock", |session| {
        Ok(on_lock(session, CLOCK)?.is_some() && on_lock(session, PROMPT)?.is_none())
    });

    // Typed on the clock: the first letter brings the prompt up and is
    // not lost.
    type_text(&checker.session, "sesam")?;
    key(&checker.session, "Return")?;
    checker.expect("a wrong password is turned down", |session| {
        Ok(on_lock(session, MESSAGE)?
            .is_some_and(|message| message["text"] == "Sorry, that didn’t work. Please try again."))
    });
    checker.expect("and the session stays locked", |session| {
        Ok(session.helper_runs("lock") && on_lock(session, PROMPT)?.is_some())
    });
    type_text(&checker.session, "sesame")?;
    key(&checker.session, "Return")?;
    checker.expect("the right password ends the locker", |session| {
        Ok(!session.helper_runs("lock"))
    });
    checker.expect("and the desktop is back", |session| {
        Ok(near(pixel(session, BAR.0, BAR.1)?, [53, 53, 55]))
    });

    // The lock is the compositor's, not the locker's.
    println!("a lock screen that dies");
    let mut first = checker.session.start_helper("lock")?;
    checker.expect("(locked again)", |session| {
        Ok(on_lock(session, CLOCK)?.is_some())
    });
    first.kill()?;
    first.wait()?;
    checker.expect(
        "leaves the session locked: niri still shows nothing",
        |session| Ok(!session.helper_runs("lock") && pixel(session, BAR.0, BAR.1).is_err()),
    );
    let mut second = checker.session.start_helper("lock")?;
    checker.expect("another lock screen takes over", |session| {
        Ok(on_lock(session, CLOCK)?.is_some())
    });
    key_until(&checker.session, "Return", |session| {
        Ok(on_lock(session, PROMPT)?.is_some())
    })?;
    type_text(&checker.session, "sesame")?;
    key(&checker.session, "Return")?;
    checker.expect("and unlocks with the password", |session| {
        Ok(!session.helper_runs("lock") && near(pixel(session, BAR.0, BAR.1)?, [53, 53, 55]))
    });
    let status = second.wait()?;
    checker.expect("ending with the status that says \"unlocked\"", |_| {
        Ok(status.code() == Some(0))
    });

    let Checker {
        session,
        passed,
        failed,
    } = checker;
    session.quit()?;
    Ok((passed, failed))
}

/// The authentication dialog in the process that shows it, `gniri
/// polkit`, with the scene's request in place of polkit and the scene's
/// password in place of polkit's helper.
fn authentication_dialog() -> Result<(usize, usize)> {
    const AUTHENTICATE: &str = ".modal-dialog-button:1";
    const ENTRY: &str = ".prompt-dialog-password-entry";

    if !on_path("wtype") {
        println!("authentication dialog");
        println!("  skip wtype is not installed");
        return Ok((0, 0));
    }
    let session = fixture_session(
        "check-polkit",
        r#"
        time = "2026-11-14T08:28:00"
        user = { name = "user", real_name = "Example User" }
        lock = { password = "sesame" }

        [polkit.request]
        action_id = "org.freedesktop.policykit.exec"
        message = "Authentication is needed to run `/usr/bin/true' as the super user"
        user = "user"
        prompt = { text = "Password: " }
        "#,
    )?;
    let mut checker = Checker {
        session,
        passed: 0,
        failed: 0,
    };
    let dialog = |session: &Session, selector: &str| on_surface(session, "gniri-dialog", selector);

    println!("authentication dialog");
    checker.session.spawn_helper("polkit")?;
    checker.expect("a request opens the dialog for the user", |session| {
        let Some(dialog) = dialog(session, ".modal-dialog")? else {
            return Ok(false);
        };
        let texts = texts(&dialog);
        Ok(
            texts.first().map(String::as_str) == Some("Authentication Required")
                && texts.iter().any(|text| text == "Example User")
                && texts.iter().any(|text| text == "Password"),
        )
    });
    checker.expect("with nothing typed there is nothing to send", |session| {
        Ok(dialog(session, AUTHENTICATE)?.is_some_and(|button| has_state(&button, "insensitive")))
    });
    type_text(&checker.session, "wrong")?;
    checker.expect("typing makes Authenticate available", |session| {
        Ok(dialog(session, AUTHENTICATE)?.is_some_and(|button| !has_state(&button, "insensitive")))
    });
    key(&checker.session, "Return")?;
    checker.expect(
        "a wrong password is turned down, in the warning's look",
        |session| {
            Ok(dialog(session, ".prompt-dialog-error-label")?
                .is_some_and(|label| label["text"] == "Sorry, that didn’t work. Please try again."))
        },
    );
    checker.expect("and the entry is ready for another try", |session| {
        Ok(dialog(session, ENTRY)?.is_some_and(|entry| !has_state(&entry, "insensitive")))
    });
    type_text(&checker.session, "sesame")?;
    key(&checker.session, "Return")?;
    checker.expect("the right one closes the dialog", |session| {
        Ok(surface(session, "gniri-dialog")?.is_none())
    });
    checker.expect("and the agent stays for the next request", |session| {
        Ok(session.helper_runs("polkit"))
    });

    let Checker {
        session,
        passed,
        failed,
    } = checker;
    session.quit()?;
    Ok((passed, failed))
}

/// The dialogs of the two agents the shell is for the system: the one
/// NetworkManager asks for a Wi-Fi password, and the one BlueZ asks about a
/// pairing. A fixture stands in for both services, so what the dialogs
/// answer is read back from the command log.
fn agent_dialogs() -> Result<(usize, usize)> {
    const CANCEL: &str = ".modal-dialog-button:0";
    const CONFIRM: &str = ".modal-dialog-button:1";
    const TITLE: &str = ".message-dialog-title";

    let session = fixture_session(
        "check-agents",
        r#"
        time = "2026-11-14T08:28:00"
        user = { name = "user", real_name = "Example User" }
        rfkill = { available = true }
        bluetooth = { adapter = { powered = true } }

        [network]
        primary = { kind = "wifi", strength = 70 }
        wifi = { enabled = true, networks = [
            { ssid = "home", strength = 70, secure = true, known = true, status = "connected" },
            { ssid = "cafe", strength = 60, secure = true },
            { ssid = "campus", strength = 50, secure = true, enterprise = true },
        ] }

        [network_agent.request]
        title = "Authentication required"
        message = "Passwords or encryption keys are required to access the wireless network “cafe”"
        fields = [{ label = "Password", key = "psk", password = true, check = "wpa-psk" }]
        "#,
    )?;
    let mut checker = Checker {
        session,
        passed: 0,
        failed: 0,
    };
    let dialog = |session: &Session, selector: &str| on_surface(session, "gniri-dialog", selector);
    let button_is = |session: &Session, selector: &str, sensitive: bool| -> Result<bool> {
        Ok(dialog(session, selector)?
            .is_some_and(|button| has_state(&button, "insensitive") != sensitive))
    };
    let closed = |session: &Session| Ok(surface(session, "gniri-dialog")?.is_none());

    println!("Wi-Fi password dialog");
    checker.expect("a request opens the dialog", |session| {
        let Some(dialog) = dialog(session, ".modal-dialog")? else {
            return Ok(false);
        };
        let texts = texts(&dialog);
        Ok(
            texts.first().map(String::as_str) == Some("Authentication required")
                && texts.iter().any(|text| text.ends_with("network “cafe”"))
                && texts.iter().any(|text| text == "Password")
                && texts.iter().any(|text| text == "Connect"),
        )
    });
    checker.expect(
        "with nothing typed there is nothing to connect with",
        |session| button_is(session, CONFIRM, false),
    );
    if on_path("wtype") {
        // The dialog was mapped a moment ago; the first keys may come
        // before it has the keyboard, so one is pressed until it shows.
        key_until(&checker.session, "s", |session| {
            Ok(session.gniri_msg(&["debug", "dump-tree"]).is_ok())
        })?;
        type_text(&checker.session, "hort")?;
        std::thread::sleep(std::time::Duration::from_millis(300));
        checker.expect("nor with too short a password", |session| {
            button_is(session, CONFIRM, false)
        });
        type_text(&checker.session, " but long enough")?;
        checker.expect("eight characters make Connect available", |session| {
            button_is(session, CONFIRM, true)
        });
        key(&checker.session, "Return")?;
        checker.expect("Enter sends the answer and closes the dialog", |session| {
            Ok(sent(session, "network-agent: Respond")? && closed(session)?)
        });
        checker.expect("and the password is in no log", |session| {
            Ok(commands(session)?
                .iter()
                .all(|command| !command.contains("long enough")))
        });
    } else {
        println!("  skip wtype is not installed");
        checker.session.gniri_msg(&["debug", "close-dialog"])?;
    }

    if on_path("wlrctl") {
        const NETWORK: &str = ".quick-toggle-menu .popup-menu-section .popup-menu-item";
        let open_wifi_menu = |session: &Session| -> Result<()> {
            session.gniri_msg(&["debug", "open-menu", "quick-settings"])?;
            session
                .gniri_msg(&["debug", "open-submenu", "wifi"])
                .map(drop)
        };
        open_wifi_menu(&checker.session)?;
        checker.expect("(the Wi-Fi menu lists three networks)", |session| {
            count_on_stage(session, "popup-menu-badge").map(|locks| locks == 3)
        });
        click(&checker.session, &format!("{NETWORK}:1"))?;
        checker.expect(
            "a network that wants a password is joined from the menu",
            |session| sent(session, "network: ConnectWifi(\"cafe\")"),
        );
        checker.session.gniri_msg(&["debug", "close-menu"])?;
        open_wifi_menu(&checker.session)?;
        click(&checker.session, &format!("{NETWORK}:2"))?;
        checker.expect(
            "one that wants more is left to the settings application",
            |session| {
                Ok(
                    sent(session, "launch: [\"gnome-control-center\", \"wifi\"]")?
                        && !sent(session, "network: ConnectWifi(\"campus\")")?,
                )
            },
        );
        checker.session.gniri_msg(&["debug", "close-menu"])?;
    } else {
        println!("  skip wlrctl is not installed");
    }

    println!("Bluetooth pairing dialog");
    let ask = |session: &Session, arguments: &[&str]| -> Result<()> {
        let mut command = vec!["debug", "pairing"];
        command.extend(arguments);
        session.gniri_msg(&command).map(drop)
    };
    let title_is = |session: &Session, title: &str| -> Result<bool> {
        Ok(dialog(session, TITLE)?.is_some_and(|label| label["text"] == title))
    };
    let label_of = |session: &Session, selector: &str| -> Result<Option<String>> {
        Ok(dialog(session, selector)?.and_then(|button| texts(&button).into_iter().next()))
    };

    ask(
        &checker.session,
        &["confirm", "--device", "Example Phone", "--pin", "482193"],
    )?;
    checker.expect(
        "a number to compare is shown with the device's name",
        |session| {
            let Some(dialog) = dialog(session, ".modal-dialog")? else {
                return Ok(false);
            };
            let texts = texts(&dialog);
            Ok(title_is(session, "Confirm Bluetooth PIN")?
                && texts.iter().any(|text| text.ends_with("“Example Phone”."))
                && text_of(&dialog, ".pairing-dialog-pin").as_deref() == Some("482193"))
        },
    );
    if on_path("wlrctl") {
        click_on(&checker.session, "gniri-dialog", CONFIRM)?;
        checker.expect("Confirm says yes and closes the dialog", |session| {
            Ok(sent(session, "bluetooth-agent: Accept")? && closed(session)?)
        });
    } else {
        checker.session.gniri_msg(&["debug", "close-dialog"])?;
    }

    ask(&checker.session, &["pair", "--device", "Example Phone"])?;
    checker.expect(
        "a device that wants to pair is allowed or dismissed",
        |session| {
            Ok(title_is(session, "Bluetooth Pairing Request")?
                && label_of(session, CANCEL)?.as_deref() == Some("Dismiss")
                && label_of(session, CONFIRM)?.as_deref() == Some("Allow"))
        },
    );
    checker.session.gniri_msg(&["debug", "close-dialog"])?;
    checker.expect("(the dialog goes when the question is withdrawn)", closed);

    if on_path("wtype") {
        ask(&checker.session, &["enter-pin"])?;
        checker.expect("a PIN that is asked for has to be typed first", |session| {
            Ok(title_is(session, "Confirm Bluetooth PIN")? && button_is(session, CONFIRM, false)?)
        });
        key_until(&checker.session, "0", |session| {
            button_is(session, CONFIRM, true)
        })?;
        checker.expect("typing makes Confirm available", |session| {
            button_is(session, CONFIRM, true)
        });
        key(&checker.session, "Return")?;
        checker.expect("Enter sends what was typed", |session| {
            Ok(sent(session, "bluetooth-agent: Respond(\"0")? && closed(session)?)
        });

        ask(&checker.session, &["show-pin", "--keyboard"])?;
        checker.expect(
            "a PIN to type on a keyboard can only be given up on",
            |session| {
                let Some(dialog) = dialog(session, ".modal-dialog")? else {
                    return Ok(false);
                };
                Ok(title_is(session, "Pairing “Keyboard K380”")?
                    && texts(&dialog)
                        .iter()
                        .any(|text| text.ends_with("Then press “Return” on the keyboard."))
                    && count_on(session, "gniri-dialog", "", "modal-dialog-button")? == 1)
            },
        );
        key_until(&checker.session, "Escape", closed)?;
        checker.expect("Escape gives up", |session| {
            Ok(sent(session, "bluetooth-agent: Reject")? && closed(session)?)
        });
    } else {
        println!("  skip wtype is not installed");
    }

    let Checker {
        session,
        passed,
        failed,
    } = checker;
    session.quit()?;
    Ok((passed, failed))
}

/// The config file changing under a running shell: the file is the
/// check's own, and is written anew for each step. A fixture stands in for
/// the services, so what a backend was told is read from the command log.
fn config_reload() -> Result<(usize, usize)> {
    const SURFACE: &str = "gniri-overview";
    const CLOCK: &str = ".clock-display .clock";
    const PERCENTAGE: &str = "#panelRight .power-status GtkLabel";
    /// A pixel of the desktop with nothing of gniri's on it.
    const DESKTOP: (u32, u32) = (800, 600);
    /// What every step's file starts with.
    const BASE: &str = r##"
        [idle]
        blank_after = 0
        [lock]
        automatic = false
        on_suspend = false
    "##;

    let logs = session::visual_dir()?.join("logs");
    let config = logs.join("check-config.toml");
    let write_all = |text: &str| -> Result<()> {
        // Written beside it and moved into place, as an editor saves.
        let partial = config.with_extension("partial");
        fs::write(&partial, text)?;
        fs::rename(&partial, &config).context("writing the check's config file")
    };
    // Each step's file is the base with that step's settings after it.
    let write = |settings: &str| write_all(&format!("{BASE}\n{settings}"));
    const START: &str = r##"
        [wallpaper]
        color = "#808080"
        [clock]
        format = "12h"
        [panel]
        show_battery_percentage = true
        [dash]
        favorites = ["org.gnome.Nautilus", "org.gnome.Calculator"]
        icon_size = 64
    "##;
    write(START)?;

    let fixture = logs.join("check-config.fixture.toml");
    fs::write(
        &fixture,
        r#"
        time = "2026-11-14T08:28:00"
        power = { battery = { percentage = 91 } }
        idle = { available = true }

        [apps]
        installed = [
            { id = "org.gnome.Nautilus", name = "Files", icon = "org.gnome.Nautilus" },
            { id = "org.gnome.Calculator", name = "Calculator", icon = "org.gnome.Calculator" },
            { id = "org.gnome.TextEditor", name = "Text Editor", icon = "org.gnome.TextEditor" },
        ]
        "#,
    )?;
    let arguments = |fixture: Option<&Path>| -> Vec<String> {
        let mut arguments = vec!["--config".to_owned(), config.display().to_string()];
        if let Some(fixture) = fixture {
            arguments.extend(["--fixture".to_owned(), fixture.display().to_string()]);
        }
        arguments
    };
    let session = Session::start(&SessionOptions {
        name: "check-config".to_owned(),
        niri_config: None,
        renderer: "vulkan".to_owned(),
        gniri_args: arguments(Some(&fixture)),
    })?;
    let mut checker = Checker {
        session,
        passed: 0,
        failed: 0,
    };
    let clock =
        |session: &Session| -> Result<Option<String>> { Ok(text_of(&panel(session)?, CLOCK)) };
    let desktop = |session: &Session| pixel(session, DESKTOP.0, DESKTOP.1);
    let tiles = |session: &Session| count_on(session, SURFACE, "#dash", "overview-tile");

    println!("config file: what shows");
    checker.expect("the shell starts with the file's settings", |session| {
        let panel = panel(session)?;
        Ok(
            clock(session)?.is_some_and(|text| text.ends_with("8:28 AM"))
                && text_of(&panel, PERCENTAGE).as_deref() == Some("91%")
                && near(desktop(session)?, [128, 128, 128]),
        )
    });
    write(&START.replace("12h", "24h").replace(
        "show_battery_percentage = true",
        "show_battery_percentage = false",
    ))?;
    checker.expect("the clock takes a changed format at once", |session| {
        Ok(clock(session)?.is_some_and(|text| text.ends_with("08:28")))
    });
    checker.expect("and the battery's number goes", |session| {
        Ok(tree::select(&panel(session)?, PERCENTAGE).is_err())
    });
    write(&format!("{START}\n[appearance]\nstyle = \"light\""))?;
    checker.expect("the light style is taken at once", |session| {
        // The bar's light tint, folded as the dark one is: 75% of
        // rgb(198,198,198) over the grey wallpaper.
        Ok(near(pixel(session, BAR.0, BAR.1)?, [180, 180, 180]))
    });
    write(START)?;
    checker.expect("and the dark one again", |session| {
        Ok(near(pixel(session, BAR.0, BAR.1)?, [53, 53, 55]))
    });
    write(&START.replace("#808080", "#3584e4"))?;
    checker.expect("another wallpaper colour is drawn", |session| {
        Ok(near(desktop(session)?, [53, 132, 228]))
    });
    checker.expect("with the clock back as it was", |session| {
        Ok(clock(session)?.is_some_and(|text| text.ends_with("8:28 AM")))
    });

    write(&START.replace(
        "show_battery_percentage = true",
        "outputs = [\"some-other-output\"]",
    ))?;
    checker.expect(
        "an output taken off the panel's list loses its panel",
        |session| Ok(surface(session, "gniri-panel")?.is_none()),
    );
    checker.session.action(Action::OpenOverview {})?;
    std::thread::sleep(std::time::Duration::from_millis(300));
    checker.expect("and with it the dash", |session| {
        Ok(checker_overview_open(session)? && surface(session, SURFACE)?.is_none())
    });
    write(START)?;
    checker.expect("back on the list, it has both again", |session| {
        Ok(surface(session, "gniri-panel")?.is_some() && tiles(session)? == 2)
    });
    write(&START.replace(
        "\"org.gnome.Calculator\"]",
        "\"org.gnome.Calculator\", \"org.gnome.TextEditor\"]",
    ))?;
    checker.expect(
        "a third favourite joins the dash while it shows",
        |session| Ok(tiles(session)? == 3),
    );
    checker.session.action(Action::CloseOverview {})?;

    checker
        .session
        .gniri_msg(&["debug", "open-menu", "date-menu"])?;
    checker.expect("(the date menu offers world clocks)", |session| {
        Ok(on_stage(session, ".world-clocks-button")?.is_some())
    });
    write(&format!("{START}\n[date_menu]\nworld_clocks = false"))?;
    checker.expect("a menu closes when its settings change", |session| {
        Ok(surface(session, "gniri-stage")?.is_none())
    });
    checker
        .session
        .gniri_msg(&["debug", "open-menu", "date-menu"])?;
    checker.expect("and opens as the new ones have it", |session| {
        Ok(on_stage(session, ".datemenu-displays-box")?.is_some()
            && on_stage(session, ".world-clocks-button")?.is_none())
    });
    checker.session.gniri_msg(&["debug", "close-menu"])?;

    println!("config file: what the backends are told");
    checker.expect("(no backend was told anything so far)", |session| {
        Ok(commands(session)?.iter().all(|command| {
            ["idle:", "session:", "night-light:", "search:"]
                .iter()
                .all(|backend| !command.starts_with(backend))
        }))
    });
    write_all(
        &format!("{BASE}\n{START}\n[night_light]\ntemperature = 3000\n[search]\nproviders = false")
            .replace("blank_after = 0", "blank_after = 600")
            .replace("on_suspend = false", "on_suspend = true"),
    )?;
    checker.expect("the idle timers are set anew", |session| {
        sent(
            session,
            "idle: SetTimers([(BlankLocked, 15s), (Dim, 480s), (Blank, 600s)])",
        )
    });
    checker.expect("locking before sleep is switched on", |session| {
        sent(session, "session: SetLockOnSuspend(true)")
    });
    checker.expect("night light gets its new colour", |session| {
        sent(session, "night-light: SetTemperature(3000)")
    });
    checker.expect("and the search providers are asked no more", |session| {
        sent(session, "search: Configure(Options { enabled: false")
    });

    println!("config file: mistakes");
    let before = commands(&checker.session)?.len();
    write("[clock]\nformat = \"13h\"\n[dash]\nfavorites = []")?;
    std::thread::sleep(std::time::Duration::from_millis(700));
    checker.expect("a file with a mistake changes nothing", |session| {
        Ok(
            clock(session)?.is_some_and(|text| text.ends_with("8:28 AM"))
                && commands(session)?.len() == before,
        )
    });
    write(&START.replace("12h", "24h"))?;
    checker.expect("and the next good one is taken", |session| {
        Ok(clock(session)?.is_some_and(|text| text.ends_with("08:28")))
    });
    fs::remove_file(&config)?;
    checker.expect(
        "without the file the settings are the defaults",
        |session| {
            // No wallpaper of gniri's: niri's own backdrop shows.
            Ok(surface(session, "gniri-wallpaper")?.is_none()
                && tree::select(&panel(session)?, PERCENTAGE).is_err())
        },
    );
    write(START)?;
    checker.expect("and a file made later is found", |session| {
        Ok(surface(session, "gniri-wallpaper")?.is_some()
            && clock(session)?.is_some_and(|text| text.ends_with("8:28 AM")))
    });

    let Checker {
        session,
        passed,
        failed,
    } = checker;
    session.quit()?;
    let (mut passed, mut failed) = (passed, failed);

    // With real services, on the nested session's own bus: gniri is the
    // notification server there, and tells of a mistake as any program
    // would.
    write(START)?;
    let session = Session::start(&SessionOptions {
        name: "check-config-bus".to_owned(),
        niri_config: None,
        renderer: "vulkan".to_owned(),
        gniri_args: arguments(None),
    })?;
    let mut checker = Checker {
        session,
        passed: 0,
        failed: 0,
    };
    // The server takes a moment to claim its name.
    std::thread::sleep(std::time::Duration::from_millis(500));
    write("[clock]\nformat24 = true")?;
    checker.expect("a mistake is told of in a notification", |session| {
        let Some(banner) = surface(session, "gniri-banner")? else {
            return Ok(false);
        };
        Ok(text_of(&banner["tree"], ".message-title").as_deref()
            == Some("gniri’s settings were not changed")
            && text_of(&banner["tree"], ".message-body")
                .is_some_and(|body| body.contains("format24")))
    });
    write(START)?;
    checker.expect("which goes when the file is right again", |session| {
        Ok(surface(session, "gniri-banner")?.is_none())
    });
    let Checker {
        session,
        passed: bus_passed,
        failed: bus_failed,
    } = checker;
    session.quit()?;
    passed += bus_passed;
    failed += bus_failed;
    let _ = fs::remove_file(&config);
    Ok((passed, failed))
}

/// Outputs going away and coming back, and one changing its scale.
///
/// The nested compositor has one output, which cannot be unplugged. So
/// `gniri msg debug unplug` has the shell behave as if every output had
/// gone: it takes the same way through the shell that a real unplugging
/// does from the moment GTK reports it. The change of scale is the
/// compositor's own.
fn outputs_coming_and_going() -> Result<(usize, usize)> {
    let session = fixture_session(
        "check-outputs",
        r#"
        time = "2026-11-14T08:28:00"
        power = { battery = { percentage = 91 } }
        session = { can_power_off = true }
        network = { primary = { kind = "wifi", strength = 70 }, wifi = { enabled = true } }
        "#,
    )?;
    let mut checker = Checker {
        session,
        passed: 0,
        failed: 0,
    };
    let mapped = |session: &Session| -> Result<Vec<String>> {
        let dump = session.dump_tree()?;
        let surfaces = dump["surfaces"]
            .as_array()
            .ok_or_else(|| anyhow!("the dump has no surfaces"))?;
        Ok(surfaces
            .iter()
            .filter_map(|surface| surface["namespace"].as_str().map(str::to_owned))
            .collect())
    };
    let unplug = |session: &Session| session.gniri_msg(&["debug", "unplug"]).map(drop);
    let plug = |session: &Session| session.gniri_msg(&["debug", "unplug", "--back"]).map(drop);
    let desktop = ["gniri-backdrop", "gniri-panel", "gniri-wallpaper"];

    println!("outputs: unplugged and back");
    checker
        .session
        .gniri_msg(&["debug", "open-menu", "quick-settings"])?;
    checker.expect("(a menu is open)", |session| {
        Ok(surface(session, "gniri-stage")?.is_some())
    });
    unplug(&checker.session)?;
    checker.expect(
        "with no output left the shell shows nothing, and runs",
        |session| Ok(mapped(session)?.is_empty()),
    );
    checker.expect("and no menu is still taken to be open", |session| {
        Ok(session
            .gniri_msg(&["debug", "open-submenu", "wifi"])
            .is_err())
    });
    plug(&checker.session)?;
    checker.expect(
        "an output that comes back gets its panel and wallpaper",
        |session| Ok(mapped(session)? == desktop),
    );
    checker
        .session
        .gniri_msg(&["debug", "open-menu", "quick-settings"])?;
    checker.expect("and menus open on it", |session| {
        Ok(on_stage(session, ".quick-settings")?.is_some())
    });
    checker.session.gniri_msg(&["debug", "close-menu"])?;

    println!("outputs: banners");
    let notify = |session: &Session, arguments: &[&str]| -> Result<()> {
        let mut command = vec!["debug", "notify", "--app", "Tester"];
        command.extend(arguments);
        session.gniri_msg(&command).map(drop)
    };
    let banner_title = |session: &Session| -> Result<Option<String>> {
        Ok(surface(session, "gniri-banner")?
            .and_then(|banner| text_of(&banner["tree"], ".message-title")))
    };
    // A critical one: it would stay up for good.
    notify(&checker.session, &["--urgency", "critical", "Stays up"])?;
    checker.expect("(a banner is up)", |session| {
        Ok(banner_title(session)?.as_deref() == Some("Stays up"))
    });
    unplug(&checker.session)?;
    notify(&checker.session, &["Sent to nobody"])?;
    std::thread::sleep(std::time::Duration::from_millis(300));
    plug(&checker.session)?;
    checker.expect("a banner that lost its output is over", |session| {
        Ok(mapped(session)? == desktop)
    });
    notify(&checker.session, &["After the return"])?;
    checker.expect("and the next notification gets one again", |session| {
        Ok(banner_title(session)?.as_deref() == Some("After the return"))
    });
    checker
        .session
        .gniri_msg(&["debug", "open-menu", "date-menu"])?;
    checker.expect("all three are in the list", |session| {
        let Some(stage) = surface(session, "gniri-stage")? else {
            return Ok(false);
        };
        let texts = texts(&stage["tree"]);
        Ok(["Stays up", "Sent to nobody", "After the return"]
            .iter()
            .all(|title| texts.iter().any(|text| text == title)))
    });
    checker.session.gniri_msg(&["debug", "close-menu"])?;

    println!("outputs: dialogs");
    let dialog_title = |session: &Session| -> Result<Option<String>> {
        Ok(
            on_surface(session, "gniri-dialog", ".message-dialog-title")?
                .and_then(|title| title["text"].as_str().map(str::to_owned)),
        )
    };
    checker
        .session
        .gniri_msg(&["debug", "pairing", "pair", "--device", "Example Phone"])?;
    checker.expect("(a pairing question is asked)", |session| {
        Ok(dialog_title(session)?.as_deref() == Some("Bluetooth Pairing Request"))
    });
    unplug(&checker.session)?;
    checker.expect("(its dialog goes with the output)", |session| {
        Ok(mapped(session)?.is_empty())
    });
    plug(&checker.session)?;
    checker.expect("a question waits for an output to be asked on", |session| {
        Ok(dialog_title(session)?.as_deref() == Some("Bluetooth Pairing Request"))
    });
    checker.session.gniri_msg(&["debug", "close-dialog"])?;
    checker.session.gniri_msg(&["power-off"])?;
    checker.expect("(the power-off dialog counts down)", |session| {
        Ok(dialog_title(session)?.as_deref() == Some("Power Off"))
    });
    unplug(&checker.session)?;
    plug(&checker.session)?;
    checker.expect("powering off is not left counting down unseen", |session| {
        Ok(mapped(session)? == desktop && !sent(session, "session:")?)
    });

    println!("outputs: the overview");
    checker.session.action(Action::OpenOverview {})?;
    checker.expect("(the overview is dressed)", |session| {
        Ok(on_surface(session, "gniri-overview", "#dash")?.is_some())
    });
    unplug(&checker.session)?;
    plug(&checker.session)?;
    checker.expect(
        "an output that comes back into the overview is dressed too",
        |session| Ok(on_surface(session, "gniri-overview", "#dash")?.is_some()),
    );
    checker.session.action(Action::CloseOverview {})?;
    checker.expect("(and undressed with it)", |session| {
        Ok(surface(session, "gniri-overview")?.is_none())
    });

    println!("outputs: another scale");
    let geometry = |session: &Session, namespace: &str| -> Result<Option<(i64, i64, f64)>> {
        Ok(surface(session, namespace)?.and_then(|surface| {
            Some((
                surface["width"].as_i64()?,
                surface["height"].as_i64()?,
                surface["scale"].as_f64()?,
            ))
        }))
    };
    let set_scale = |session: &Session, scale: f64| -> Result<()> {
        session
            .request(niri_ipc::Request::Output {
                output: "winit".to_owned(),
                action: niri_ipc::OutputAction::Scale {
                    scale: niri_ipc::ScaleToSet::Specific(scale),
                },
            })
            .map(drop)
    };
    set_scale(&checker.session, 2.0)?;
    checker.expect(
        "the panel and the wallpaper follow the output's scale",
        |session| {
            Ok(geometry(session, "gniri-panel")? == Some((800, 32, 2.0))
                && geometry(session, "gniri-wallpaper")? == Some((800, 500, 2.0)))
        },
    );
    checker
        .session
        .gniri_msg(&["debug", "open-menu", "quick-settings"])?;
    checker.expect("and a menu fits the smaller screen", |session| {
        let Some(menu) = on_stage(session, ".quick-settings")? else {
            return Ok(false);
        };
        let [x, y, width, height] = tree::rect(&menu, "box")?;
        Ok(x >= 0.0 && y >= 32.0 && x + width <= 800.0 && y + height <= 500.0)
    });
    checker.session.gniri_msg(&["debug", "close-menu"])?;
    set_scale(&checker.session, 1.25)?;
    checker.expect("(and back)", |session| {
        Ok(geometry(session, "gniri-panel")? == Some((1280, 32, 1.25)))
    });

    let Checker {
        session,
        passed,
        failed,
    } = checker;
    session.quit()?;
    Ok((passed, failed))
}

/// Whether niri's overview is open.
fn checker_overview_open(session: &Session) -> Result<bool> {
    session.overview_is_open()
}

/// The overview's surface, with the `search` scene's fixture for
/// services: its applications, and two search providers that answer any
/// search with what the scene gives them. What a click or a key asked for
/// is read back from the command log; nothing is started.
fn overview() -> Result<(usize, usize)> {
    const SURFACE: &str = "gniri-overview";
    const ENTRY: &str = ".search-entry GtkText";

    let on = |session: &Session, selector: &str| on_surface(session, SURFACE, selector);
    let is_open = |session: &Session| Ok(surface(session, SURFACE)?.is_some());
    let is_closed = |session: &Session| {
        Ok(surface(session, SURFACE)?.is_none() && !session.overview_is_open()?)
    };
    let entry_text = |session: &Session| -> Result<Option<String>> {
        Ok(on(session, ENTRY)?.and_then(|entry| entry["text"].as_str().map(str::to_owned)))
    };
    // The text of the label in the first node that carries `class` and is
    // selected.
    let selected = |session: &Session| -> Result<Option<String>> {
        Ok(on(session, ".selected")?.and_then(|node| texts(&node).into_iter().next()))
    };
    // Types into the overview. The surface gets the keyboard a moment
    // after it is mapped, and what is typed before then goes nowhere: the
    // entry is emptied and the text typed again until it has all of it.
    let type_search = |session: &Session, text: &str| -> Result<()> {
        for _ in 0..4 {
            session.gniri_msg(&["debug", "sync"])?;
            type_text(session, text)?;
            let arrived = wait_until_within(
                "the text to arrive",
                std::time::Duration::from_millis(700),
                || Ok(entry_text(session)?.as_deref() == Some(text)),
            );
            if arrived.is_ok() {
                return Ok(());
            }
            session.gniri_msg(&["debug", "search", ""])?;
        }
        bail!("the overview never took {text:?} from the keyboard")
    };

    let fixture = Scene::load("search")?.write_fixture("check-overview", None)?;
    let session = Session::start(&SessionOptions {
        name: "check-overview".to_owned(),
        niri_config: None,
        renderer: "vulkan".to_owned(),
        gniri_args: scene::gniri_args(Some(&fixture), Some(WALLPAPER)),
    })?;
    let mut checker = Checker {
        session,
        passed: 0,
        failed: 0,
    };

    println!("overview: dash");
    checker.expect("nothing of the overview shows on the desktop", |session| {
        Ok(surface(session, SURFACE)?.is_none())
    });
    checker.session.action(Action::OpenOverview {})?;
    checker.expect("niri's overview brings the entry and the dash", |session| {
        Ok(on(session, ".search-entry")?.is_some() && on(session, "#dash")?.is_some())
    });
    checker.expect(
        "the dash has the six favourites and the apps button",
        |session| {
            Ok(count_on(session, SURFACE, "#dash", "overview-tile")? == 6
                && count_on(session, SURFACE, "#dash", "show-apps")? == 1)
        },
    );
    checker.expect("with nothing running: no dot, no separator", |session| {
        Ok(
            count_on(session, SURFACE, "#dash", "app-grid-running-dot")? == 0
                && count_on(session, SURFACE, "#dash", "dash-separator")? == 0,
        )
    });
    checker
        .session
        .spawn_app_window(WINDOW, Some("org.gnome.Settings"))?;
    checker.expect(
        "an application that starts joins it after a separator, with a dot",
        |session| {
            Ok(count_on(session, SURFACE, "#dash", "overview-tile")? == 7
                && count_on(session, SURFACE, "#dash", "dash-separator")? == 1
                && count_on(session, SURFACE, "#dash", "app-grid-running-dot")? == 1)
        },
    );
    checker
        .session
        .spawn_app_window(WINDOW, Some("org.gnome.Nautilus"))?;
    checker.expect("a favourite that starts only gets its dot", |session| {
        Ok(count_on(session, SURFACE, "#dash", "overview-tile")? == 7
            && count_on(session, SURFACE, "#dash", "app-grid-running-dot")? == 2)
    });

    println!("overview: search");
    if on_path("wtype") {
        // A window that was just opened has the focus; the overview is
        // opened afresh for the keyboard to be the entry's.
        checker.session.action(Action::CloseOverview {})?;
        checker.expect("(the overview is closed)", is_closed);
        checker.session.action(Action::OpenOverview {})?;
        checker.expect("(and open again)", is_open);
        type_search(&checker.session, "calc")?;
        checker.expect("typing fills the search entry", |session| {
            Ok(entry_text(session)?.as_deref() == Some("calc"))
        });
        checker.expect("and finds the applications by name", |session| {
            let Some(results) = on(session, ".grid-search-results")? else {
                return Ok(false);
            };
            Ok(texts(&results) == ["Calculator", "KiCad PCB Calculator"])
        });
        checker.expect("the providers are asked for the same", |session| {
            sent(session, "search: Search([\"calc\"])")
        });
        checker.expect("and what they found is listed under them", |session| {
            let Some(results) = on(session, "#searchResults")? else {
                return Ok(false);
            };
            let texts = texts(&results);
            Ok([
                "Characters",
                "Pocket Calculator",
                "Software",
                "15 more",
                "Uno Calculator",
            ]
            .iter()
            .all(|wanted| texts.iter().any(|text| text == wanted)))
        });
        checker.expect("the first result is the one Enter would take", |session| {
            Ok(selected(session)?.as_deref() == Some("Calculator"))
        });
        key(&checker.session, "Right")?;
        checker.expect(
            "Right belongs to the text until an arrow has gone down",
            |session| Ok(selected(session)?.as_deref() == Some("Calculator")),
        );
        key(&checker.session, "Down")?;
        checker.expect("Down goes to the first provider's result", |session| {
            Ok(selected(session)?.as_deref() == Some("Pocket Calculator"))
        });
        key(&checker.session, "Return")?;
        checker.expect("Enter there opens it in its application", |session| {
            sent(
                session,
                "search: Activate { provider: \"org.gnome.Characters\", \
                 result: \"pocket-calculator\", terms: [\"calc\"] }",
            )
        });
        checker.expect("and closes the overview", is_closed);

        checker.session.action(Action::OpenOverview {})?;
        checker.expect("(the overview is open)", is_open);
        checker.expect("a new overview has forgotten the search", |session| {
            Ok(entry_text(session)?.as_deref() == Some("")
                && on(session, "#searchResults")?.is_none())
        });
        type_search(&checker.session, "calc")?;
        checker.expect("(the search is back)", |session| {
            Ok(selected(session)?.as_deref() == Some("Calculator"))
        });
        key(&checker.session, "Return")?;
        checker.expect("Enter on an application starts it", |session| {
            sent(session, "launch-app: \"org.gnome.Calculator\"")
        });
        checker.expect("and closes the overview", is_closed);

        checker.session.action(Action::OpenOverview {})?;
        checker.expect("(the overview is open)", is_open);
        type_search(&checker.session, "pocket")?;
        checker.expect("(a search with only a provider's results)", |session| {
            Ok(selected(session)?.as_deref() == Some("Pocket Calculator"))
        });
        key(&checker.session, "Escape")?;
        checker.expect(
            "Escape ends the search and stays in the overview",
            |session| {
                Ok(entry_text(session)?.as_deref() == Some("")
                    && on(session, "#searchResults")?.is_none()
                    && session.overview_is_open()?)
            },
        );
        checker.session.gniri_msg(&["debug", "show-apps"])?;
        checker.expect("(the app grid shows)", |session| {
            Ok(on(session, ".icon-grid")?.is_some())
        });
        key(&checker.session, "Escape")?;
        checker.expect("Escape leaves the app grid for the windows", |session| {
            Ok(on(session, ".icon-grid")?.is_none() && session.overview_is_open()?)
        });
        key(&checker.session, "Escape")?;
        checker.expect("and then closes the overview", is_closed);
    } else {
        println!("  skip wtype is not installed");
    }

    println!("overview: pointer");
    if on_path("wlrctl") {
        // Away from niri's hot corner, which the pointer may be resting in.
        pointer_to(&checker.session, 400, 300)?;
        checker.session.action(Action::CloseOverview {})?;
        checker.expect("(the overview is closed)", is_closed);
        checker.session.action(Action::OpenOverview {})?;
        checker.expect("(the overview is open)", is_open);

        click_on(&checker.session, SURFACE, ".show-apps")?;
        checker.expect("the dash's last button shows the app grid", |session| {
            Ok(on(session, ".icon-grid")?.is_some()
                && on(session, ".show-apps")?.is_some_and(|button| has_state(&button, "checked")))
        });
        checker.expect(
            "Settings runs and is no favourite: in the grid with a dot",
            |session| {
                Ok(count_on(
                    session,
                    SURFACE,
                    ".icon-grid #Settings",
                    "app-grid-running-dot",
                )? == 1)
            },
        );
        click_on(&checker.session, SURFACE, ".icon-grid #Settings")?;
        checker.expect("a click on it goes to its window", |session| {
            Ok(
                session.focused_app()?.as_deref() == Some("org.gnome.Settings")
                    && sent(session, "niri: FocusWindow")?,
            )
        });
        checker.expect("and closes the overview", is_closed);

        checker.session.action(Action::OpenOverview {})?;
        checker.expect("(the overview is open)", is_open);
        click_on(&checker.session, SURFACE, "#dash #Files")?;
        checker.expect(
            "a click on a running favourite goes to its window",
            |session| Ok(session.focused_app()?.as_deref() == Some("org.gnome.Nautilus")),
        );
        checker.expect("and closes the overview", is_closed);

        checker.session.action(Action::OpenOverview {})?;
        checker.expect("(the overview is open)", is_open);
        click_on(&checker.session, SURFACE, "#dash #Calendar")?;
        checker.expect("a click on one that does not run starts it", |session| {
            sent(session, "launch-app: \"org.gnome.Calendar\"")
        });
        checker.expect("and closes the overview", is_closed);

        // The name of a dash item shows over it once the pointer has
        // rested there.
        checker.session.action(Action::OpenOverview {})?;
        checker.expect("(the overview is open)", is_open);
        let hover = |session: &Session, selector: &str| -> Result<()> {
            session.gniri_msg(&["debug", "sync"])?;
            let found = surface(session, SURFACE)?.ok_or_else(|| anyhow!("no overview"))?;
            let (left, top) = surface_origin(session, &found)?;
            let [x, y, width, height] = tree::rect(tree::select(&found["tree"], selector)?, "box")?;
            hover_at(
                session,
                (left + x + width / 2.0).round() as i32,
                (top + y + height / 2.0).round() as i32,
                "the item",
                || Ok(on(session, selector)?.is_some_and(|item| has_state(&item, "hover"))),
            )
        };
        let label = |session: &Session| -> Result<Option<String>> {
            Ok(on(session, ".dash-label")?
                .and_then(|label| label["text"].as_str().map(str::to_owned)))
        };
        hover(&checker.session, "#dash #Calendar")?;
        checker.expect("resting on a dash item shows its name", |session| {
            Ok(label(session)?.as_deref() == Some("Calendar"))
        });
        checker.expect(
            "over the middle of the item, a little above the dash",
            |session| {
                let (Some(label), Some(item), Some(dash)) = (
                    on(session, ".dash-label")?,
                    on(session, "#dash #Calendar")?,
                    on(session, "#dash")?,
                ) else {
                    return Ok(false);
                };
                let (label, item, dash) = (
                    tree::rect(&label, "box")?,
                    tree::rect(&item, "box")?,
                    tree::rect(&dash, "box")?,
                );
                let centre = |rect: [f64; 4]| rect[0] + rect[2] / 2.0;
                Ok((centre(label) - centre(item)).abs() <= 1.0
                    && dash[1] - (label[1] + label[3]) == 8.0)
            },
        );
        hover(&checker.session, "#dash #Files")?;
        checker.expect("the next item's name follows the pointer", |session| {
            Ok(label(session)?.as_deref() == Some("Files"))
        });
        let (width, height) = output_size(&checker.session)?;
        pointer_to(&checker.session, width / 2, height / 2)?;
        checker.expect("and goes when the pointer leaves the dash", |session| {
            Ok(label(session)?.is_none())
        });

        // Between the entry and the dash the surface lets the pointer
        // through to niri, whose overview is underneath.
        checker.session.action(Action::OpenOverview {})?;
        checker.expect("(the overview is open)", is_open);
        pointer(&checker.session, &["click", "left"])?;
        checker.expect(
            "a click on a window in niri's overview reaches niri, which closes it",
            is_closed,
        );
    } else {
        println!("  skip wlrctl is not installed");
    }

    println!("overview: gniri msg apps");
    checker.session.action(Action::CloseOverview {})?;
    checker.expect("(the overview is closed)", is_closed);
    checker.session.gniri_msg(&["apps"])?;
    checker.expect("opens the overview on the app grid", |session| {
        Ok(session.overview_is_open()? && on(session, ".icon-grid")?.is_some())
    });
    checker.session.gniri_msg(&["apps"])?;
    checker.expect("and closes it again", is_closed);

    let Checker {
        session,
        mut passed,
        mut failed,
    } = checker;
    session.quit()?;

    // The pages of the app grid, with the `app-grid` scene's applications:
    // more than one page of them.
    let fixture = Scene::load("app-grid")?.write_fixture("check-grid", None)?;
    let session = Session::start(&SessionOptions {
        name: "check-grid".to_owned(),
        niri_config: None,
        renderer: "vulkan".to_owned(),
        gniri_args: scene::gniri_args(Some(&fixture), Some(WALLPAPER)),
    })?;
    let mut checker = Checker {
        session,
        passed: 0,
        failed: 0,
    };
    let current_page = |session: &Session| -> Result<Option<usize>> {
        let Some(indicators) = on(session, ".page-indicators")? else {
            return Ok(None);
        };
        Ok(tree::children(&indicators)
            .iter()
            .position(|indicator| tree::classes(indicator).contains(&"current")))
    };
    let first_icon = |session: &Session| -> Result<Option<String>> {
        Ok(on(session, ".icon-grid .overview-tile")?
            .and_then(|tile| texts(&tile).into_iter().next()))
    };

    println!("overview: app grid");
    checker.session.gniri_msg(&["apps"])?;
    checker.expect("the grid shows a page of 24 icons", |session| {
        Ok(count_on(session, SURFACE, ".icon-grid", "overview-tile")? == 24)
    });
    checker.expect("of two, the first", |session| {
        Ok(count_on(session, SURFACE, "", "page-indicator")? == 2
            && current_page(session)? == Some(0))
    });
    checker.expect("in name order", |session| {
        Ok(first_icon(session)?.as_deref() == Some("Boxes"))
    });
    checker.expect("with an arrow to the next page only", |session| {
        Ok(count_on(session, SURFACE, "", "page-navigation-arrow")? == 1)
    });
    if on_path("wlrctl") {
        pointer_to(&checker.session, 400, 300)?;
        click_on(&checker.session, SURFACE, ".page-navigation-arrow")?;
        checker.expect("the arrow turns the page", |session| {
            Ok(
                current_page(session)? == Some(1)
                    && first_icon(session)?.as_deref() == Some("Tour"),
            )
        });
        checker.expect("to the last, which has an arrow back only", |session| {
            Ok(
                count_on(session, SURFACE, "", "page-navigation-arrow")? == 1
                    && count_on(session, SURFACE, ".icon-grid", "overview-tile")? == 6,
            )
        });
        // Over the icons, where the grid has the pointer.
        let (width, height) = output_size(&checker.session)?;
        pointer_to(&checker.session, width / 2, height / 2)?;
        pointer(&checker.session, &["scroll", "-15", "0"])?;
        checker.expect("scrolling up turns it back", |session| {
            Ok(current_page(session)? == Some(0))
        });
        click_on(&checker.session, SURFACE, ".page-indicator:1")?;
        checker.expect("a click on a page's dot goes to that page", |session| {
            Ok(current_page(session)? == Some(1))
        });
    } else {
        println!("  skip wlrctl is not installed");
        checker.session.gniri_msg(&["debug", "page", "1"])?;
    }
    if on_path("wtype") {
        key_until(&checker.session, "Page_Up", |session| {
            Ok(current_page(session)? == Some(0))
        })?;
        checker.expect("Page Up goes back a page", |session| {
            Ok(current_page(session)? == Some(0))
        });
        key(&checker.session, "Down")?;
        checker.expect("an arrow key picks the first icon", |session| {
            Ok(selected(session)?.as_deref() == Some("Boxes"))
        });
        key(&checker.session, "Left")?;
        key(&checker.session, "Down")?;
        key(&checker.session, "Right")?;
        checker.expect("and arrows move through the grid", |session| {
            // Second row, second column.
            Ok(selected(session)?.as_deref() == Some("Help"))
        });
        key(&checker.session, "Return")?;
        checker.expect("Enter starts what is picked", |session| {
            sent(session, "launch-app: \"org.example.Help\"")
        });
        checker.expect("and closes the overview", is_closed);

        // This scene has no search providers: only applications are found.
        checker.session.action(Action::OpenOverview {})?;
        checker.expect("(the overview is open)", is_open);
        type_search(&checker.session, "zzz")?;
        checker.expect("a search that finds nothing says so", |session| {
            Ok(on(session, ".search-statustext")?
                .is_some_and(|label| label["text"] == "No results"))
        });
    } else {
        println!("  skip wtype is not installed");
    }

    passed += checker.passed;
    failed += checker.failed;
    checker.session.quit()?;

    // The real thing: no fixture, so the applications are the ones
    // installed here and the surface comes and goes with its animations.
    println!("overview: installed applications");
    let session = Session::start(&SessionOptions {
        name: "check-apps".to_owned(),
        niri_config: None,
        renderer: "vulkan".to_owned(),
        gniri_args: scene::gniri_args(None, Some(WALLPAPER)),
    })?;
    let mut checker = Checker {
        session,
        passed: 0,
        failed: 0,
    };
    checker.session.gniri_msg(&["apps"])?;
    checker.expect("the app grid lists what is installed", |session| {
        Ok(count_on(session, SURFACE, ".icon-grid", "overview-tile")? > 0)
    });
    checker.expect("the dash and the entry have slid into place", |session| {
        Ok(on(session, ".controls-manager")?
            .is_some_and(|controls| tree::classes(&controls).contains(&"open")))
    });
    checker.session.action(Action::CloseOverview {})?;
    checker.expect("and the surface goes once they have slid away", |session| {
        Ok(surface(session, SURFACE)?.is_none())
    });

    passed += checker.passed;
    failed += checker.failed;
    checker.session.quit()?;
    Ok((passed, failed))
}
