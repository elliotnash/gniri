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

use crate::scene;
use crate::session::{self, Session, SessionOptions, wait_until};
use crate::tree;

const WALLPAPER: &str = "#808080";
const WINDOW: &str = "#f6f5f4";

struct Checker {
    session: Session,
    passed: usize,
    failed: usize,
}

impl Checker {
    /// Records one check: polls `done` and reports what it was waiting for.
    fn expect(&mut self, what: &str, mut done: impl FnMut(&Session) -> Result<bool>) {
        let session = &self.session;
        match wait_until(what, || done(session)) {
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
    let mut dump = session.dump_tree()?;
    let surfaces = dump["surfaces"]
        .as_array_mut()
        .ok_or_else(|| anyhow!("the dump has no surfaces"))?;
    let index = surfaces
        .iter()
        .position(|surface| surface["namespace"] == "gniri-panel")
        .ok_or_else(|| anyhow!("no panel surface is mapped"))?;
    Ok(surfaces.swap_remove(index)["tree"].take())
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
/// moves relatively, so go far past the top-left corner first.
fn pointer_to(session: &Session, x: i32, y: i32) -> Result<()> {
    for (dx, dy) in [(-10000, -10000), (x, y)] {
        let status = session
            .client_command("wlrctl")
            .args(["pointer", "move", &dx.to_string(), &dy.to_string()])
            .status()
            .context("running wlrctl")?;
        if !status.success() {
            bail!("wlrctl pointer move failed");
        }
    }
    Ok(())
}

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
    Ok(())
}

pub fn run() -> Result<bool> {
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
        Ok(near(pixel(session, 300, 20)?, [53, 53, 55]))
    });
    checker.session.action(Action::OpenOverview {})?;
    checker.expect(
        "opening the overview checks the activities button",
        panel_shows_overview,
    );
    checker.expect(
        "and clears the bar, showing the dimmed backdrop",
        |session| Ok(near(pixel(session, 300, 20)?, [77, 77, 77])),
    );
    checker.session.action(Action::CloseOverview {})?;
    checker.expect("closing it restores the bar", |session| {
        Ok(!panel_shows_overview(session)? && near(pixel(session, 300, 20)?, [53, 53, 55]))
    });

    println!("fullscreen");
    checker.session.action(Action::FocusWorkspaceUp {})?;
    checker
        .session
        .action(Action::FullscreenWindow { id: None })?;
    checker.expect("a fullscreen window covers the panel", |session| {
        Ok(near(pixel(session, 300, 20)?, [0xf6, 0xf5, 0xf4]))
    });
    checker
        .session
        .action(Action::FullscreenWindow { id: None })?;
    checker.expect("and the panel is back afterwards", |session| {
        Ok(near(pixel(session, 300, 20)?, [53, 53, 55]))
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
    println!("{passed} of {} checks passed", passed + failed);
    Ok(failed == 0)
}
