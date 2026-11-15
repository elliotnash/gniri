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
    if surface["namespace"] != "gniri-banner" {
        return Ok((0.0, 0.0));
    }
    let (output_width, _) = output_size(session)?;
    let width = surface["width"]
        .as_f64()
        .ok_or_else(|| anyhow!("the surface has no width"))?;
    let panel = self::surface(session, "gniri-panel")?
        .and_then(|panel| panel["height"].as_f64())
        .unwrap_or(0.0);
    Ok(((f64::from(output_width) - width) / 2.0, panel))
}

/// Clicks the middle of the widget `selector` picks on the surface called
/// `namespace`.
///
/// The click waits until the widget shows the pointer over it: each
/// `wlrctl` call is a client of its own, whose requests the compositor may
/// handle a moment later, and the surface may only just have been mapped.
fn click_on(session: &Session, namespace: &str, selector: &str) -> Result<()> {
    // A banner or a menu that was just asked for slides into place; where
    // its widgets are is only settled once it has been drawn there.
    session.gniri_msg(&["debug", "sync"])?;
    let found = surface(session, namespace)?.ok_or_else(|| anyhow!("{namespace} is not mapped"))?;
    let (left, top) = surface_origin(session, &found)?;
    let [x, y, width, height] = tree::rect(tree::select(&found["tree"], selector)?, "box")?;
    hover_at(
        session,
        (left + x + width / 2.0).round() as i32,
        (top + y + height / 2.0).round() as i32,
        "the widget",
        || {
            Ok(on_surface(session, namespace, selector)?
                .is_some_and(|widget| has_state(&widget, "hover")))
        },
    )?;
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
    let Some(stage) = surface(session, "gniri-stage")? else {
        return Ok(0);
    };
    let mut count = 0;
    tree::walk(&stage["tree"], &mut |node| {
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

    let mut passed = passed;
    let mut failed = failed;
    for part in [quick_settings, notifications, notifications_over_the_bus] {
        let (part_passed, part_failed) = part()?;
        passed += part_passed;
        failed += part_failed;
    }
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
