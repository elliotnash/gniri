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
///
/// The click waits until the widget shows the pointer over it: each
/// `wlrctl` call is a client of its own, whose requests the compositor may
/// handle a moment later, and the menu may only just have been mapped.
fn click(session: &Session, selector: &str) -> Result<()> {
    let stage = surface(session, "gniri-stage")?.ok_or_else(|| anyhow!("no menu is open"))?;
    let [x, y, width, height] = tree::rect(tree::select(&stage["tree"], selector)?, "box")?;
    pointer_to(
        session,
        (x + width / 2.0).round() as i32,
        (y + height / 2.0).round() as i32,
    )?;
    wait_until("the pointer to reach the widget", || {
        nudge(session)?;
        Ok(on_stage(session, selector)?.is_some_and(|widget| has_state(&widget, "hover")))
    })?;
    pointer(session, &["click", "left"])
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
    pointer_to(
        session,
        (x + width / 2.0).round() as i32,
        (y + height / 2.0).round() as i32,
    )?;
    wait_until("the pointer to reach the status button", || {
        nudge(session)?;
        let panel = self::panel(session)?;
        Ok(has_state(tree::select(&panel, BUTTON)?, "hover"))
    })?;
    pointer(session, &["click", "left"])
}

/// A node on the stage, if a menu is open and has it.
fn on_stage(session: &Session, selector: &str) -> Result<Option<Value>> {
    let Some(stage) = surface(session, "gniri-stage")? else {
        return Ok(None);
    };
    Ok(tree::select(&stage["tree"], selector).ok().cloned())
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

    let (menu_passed, menu_failed) = quick_settings()?;
    let (passed, failed) = (passed + menu_passed, failed + menu_failed);
    println!("{passed} of {} checks passed", passed + failed);
    Ok(failed == 0)
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
