// SPDX-License-Identifier: GPL-3.0-or-later

//! `cargo xtask`: development and visual-test driver for gniri.

mod check;
mod diff;
mod install;
mod report;
mod scene;
mod session;
mod tree;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use scene::Scene;
use session::{Session, SessionOptions};

#[derive(Parser)]
#[command(name = "xtask", about = "Development and visual-test driver for gniri")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run gniri in a nested niri window until Ctrl-C.
    Nest {
        #[command(flatten)]
        session: SessionArgs,
    },
    /// Run gniri nested, screenshot the output, and exit.
    Shot {
        /// Name of the screenshot: written to target/visual/<NAME>.png.
        name: String,
        /// Open niri's overview before the screenshot.
        #[arg(long)]
        overview: bool,
        #[command(flatten)]
        session: SessionArgs,
    },
    /// Compare a component with the GNOME reference
    /// (tests/visual/map/<COMPONENT>.toml).
    Diff { component: String },
    /// Diff every component and write target/visual/report/index.html.
    Report,
    /// Build gniri for release and install it with its niri fragment,
    /// systemd unit and documentation. Below a home directory (the
    /// default, ~/.local) that needs no privileges.
    Install {
        /// Where to install. For the system, give /usr or /usr/local
        /// together with --destdir, and copy the result as root.
        #[arg(long)]
        prefix: Option<PathBuf>,
        /// Install into this directory as if it were `/`.
        #[arg(long)]
        destdir: Option<PathBuf>,
        /// Install the release build there is instead of building.
        #[arg(long)]
        no_build: bool,
    },
    /// Take away what `install` put there, given the same options.
    Uninstall {
        #[arg(long)]
        prefix: Option<PathBuf>,
        #[arg(long)]
        destdir: Option<PathBuf>,
    },
    /// Check how the shell behaves in a live nested session.
    Check {
        /// Only this part: panel, overview, quick-settings, notifications,
        /// notifications-bus, session, lock, polkit, agents, config or
        /// outputs.
        part: Option<String>,
    },
}

#[derive(clap::Args)]
struct SessionArgs {
    /// Wallpaper for gniri: an image file or a colour such as "#808080".
    #[arg(long)]
    wallpaper: Option<String>,
    /// Scene to set up first (tests/visual/scenes/<SCENE>.toml): its fixture
    /// is given to gniri and its steps are played.
    #[arg(long)]
    scene: Option<String>,
    /// niri config to use instead of tests/visual/niri/test.kdl.
    #[arg(long)]
    niri_config: Option<PathBuf>,
    /// GTK renderer (GSK_RENDERER).
    #[arg(long, default_value = "vulkan")]
    renderer: String,
    /// Show the scene in the shell's light style.
    #[arg(long, requires = "scene")]
    light: bool,
}

impl SessionArgs {
    /// Starts the session, with the scene (if any) played.
    fn start(&self, name: &str) -> Result<Session> {
        let scene = self.scene.as_deref().map(Scene::load).transpose()?;
        let scene = match scene {
            Some(scene) if self.light => Some(scene.in_light_style()),
            scene => scene,
        };
        let fixture = match &scene {
            Some(scene) => Some(scene.write_fixture(name, None)?),
            None => None,
        };
        let session = Session::start(&SessionOptions {
            name: name.to_owned(),
            niri_config: self.niri_config.clone(),
            renderer: self.renderer.clone(),
            gniri_args: scene::gniri_args(fixture.as_deref(), self.wallpaper.as_deref()),
        })?;
        if let Some(scene) = &scene {
            scene.play(&session)?;
        }
        Ok(session)
    }
}

fn run() -> Result<bool> {
    match Cli::parse().command {
        Command::Nest { session } => {
            let session = session.start("nest")?;
            println!("NIRI_SOCKET={}", session.niri_socket.display());
            println!("WAYLAND_DISPLAY={}", session.wayland_display);
            println!("Running. Press Ctrl-C to stop.");
            session.wait()?;
            Ok(true)
        }
        Command::Shot {
            name,
            overview,
            session,
        } => {
            let session = session.start(&name)?;
            if overview {
                session.action(niri_ipc::Action::OpenOverview {})?;
            }
            session.settle()?;
            let path = session::visual_dir()?.join(format!("{name}.png"));
            session.screenshot(&path).context("taking the screenshot")?;
            session.quit()?;
            println!("{}", path.display());
            Ok(true)
        }
        Command::Diff { component } => {
            let checks = diff::run(&component)?;
            Ok(diff::print(&component, &checks))
        }
        Command::Report => report::run(),
        Command::Install {
            prefix,
            destdir,
            no_build,
        } => install::run(&install::Options {
            prefix,
            destdir,
            no_build,
            uninstall: false,
        }),
        Command::Uninstall { prefix, destdir } => install::run(&install::Options {
            prefix,
            destdir,
            no_build: true,
            uninstall: true,
        }),
        Command::Check { part } => check::run(part.as_deref()),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Command;

    /// Every source file says what licence it is under, in its first
    /// lines. The files copied unchanged from gnome-shell are left as they
    /// came; the README beside them says where from and under what.
    #[test]
    fn every_source_file_names_its_licence() {
        const LICENCE: &str = "SPDX-License-Identifier: GPL-3.0-or-later";
        const UNCHANGED: &str = "crates/theme/scss/upstream";

        fn visit(directory: &Path, root: &Path, missing: &mut Vec<String>) {
            for entry in std::fs::read_dir(directory).expect("the directory can be read") {
                let path = entry.expect("its entries too").path();
                let relative = path.strip_prefix(root).expect("below the root");
                if path.is_dir() {
                    let skipped = relative == Path::new(UNCHANGED)
                        || path.file_name().is_some_and(|name| name == "__pycache__");
                    if !skipped {
                        visit(&path, root, missing);
                    }
                    continue;
                }
                let source = path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| ["rs", "scss", "py", "js"].contains(&extension));
                if !source {
                    continue;
                }
                let text = std::fs::read_to_string(&path).expect("a source file is text");
                // After a `#!` line, where there is one.
                if !text.lines().take(2).any(|line| line.ends_with(LICENCE)) {
                    missing.push(relative.display().to_string());
                }
            }
        }

        let root = crate::session::workspace_root();
        let mut missing = Vec::new();
        for directory in ["crates", "tools", "spikes"] {
            visit(&root.join(directory), &root, &mut missing);
        }
        assert!(
            missing.is_empty(),
            "no licence line at the top of: {missing:#?}"
        );
    }

    /// The originals of a compiled gettext catalog (a `.mo` file), each
    /// with its context, if it has one, before a `\u{4}`; of a text with a
    /// plural, the singular.
    fn originals(catalog: &[u8]) -> Vec<String> {
        let word = |at: usize| -> usize {
            let bytes: [u8; 4] = catalog[at..at + 4].try_into().expect("four bytes");
            // Written in the byte order of the machine that compiled it.
            if catalog[..4] == [0xde, 0x12, 0x04, 0x95] {
                u32::from_le_bytes(bytes) as usize
            } else {
                u32::from_be_bytes(bytes) as usize
            }
        };
        let (count, table) = (word(8), word(12));
        (0..count)
            .map(|index| {
                let (length, at) = (word(table + 8 * index), word(table + 8 * index + 4));
                let original = &catalog[at..at + length];
                let singular = original.split(|byte| *byte == 0).next().unwrap_or_default();
                String::from_utf8_lossy(singular).into_owned()
            })
            .collect()
    }

    /// Every text gniri asks a translation for is one GNOME has: written
    /// as GNOME writes it, to the ellipsis and the kind of dash, and asked
    /// for with GNOME's context. A text that is a little off is simply
    /// never translated, which nobody reading English would notice.
    ///
    /// The catalogs of a few languages are read, since each only has the
    /// texts its translators got to. Skipped where GNOME Shell's are not
    /// installed.
    #[test]
    fn every_translated_text_is_one_of_gnomes() {
        const DOMAINS: [&str; 4] = [
            "gnome-shell",
            "gnome-desktop-3.0",
            "gnome-bluetooth-3.0",
            "NetworkManager",
        ];
        /// gniri's own, which stay English.
        const OWN: [&str; 1] = ["Disconnecting…"];

        let mut known = std::collections::HashSet::new();
        for language in ["de", "fr", "es", "sv", "it"] {
            for domain in DOMAINS {
                let path = format!("/usr/share/locale/{language}/LC_MESSAGES/{domain}.mo");
                if let Ok(catalog) = std::fs::read(path) {
                    known.extend(originals(&catalog));
                }
            }
        }
        if !known.contains("Type to search") {
            eprintln!("skipped: GNOME Shell's translations are not installed");
            return;
        }

        /// The string literals `call(` is given at the start of its
        /// arguments, wherever it is called in `source`.
        fn literals(source: &str, call: &str, wanted: usize) -> Vec<Vec<String>> {
            let mut found = Vec::new();
            let mut rest = source;
            while let Some(at) = rest.find(call) {
                let before = rest[..at].chars().next_back();
                rest = &rest[at + call.len()..];
                if before.is_some_and(|before| before.is_alphanumeric() || before == '_') {
                    continue;
                }
                let mut arguments = Vec::new();
                let mut cursor = rest;
                while arguments.len() < wanted {
                    cursor = cursor.trim_start().trim_start_matches('&').trim_start();
                    let Some(literal) = cursor.strip_prefix('"') else {
                        break;
                    };
                    let mut text = String::new();
                    let mut characters = literal.char_indices();
                    let mut end = None;
                    while let Some((index, character)) = characters.next() {
                        match character {
                            '"' => {
                                end = Some(index);
                                break;
                            }
                            '\\' => match characters.next() {
                                Some((_, 'u')) => {
                                    let code: String = characters
                                        .by_ref()
                                        .map(|(_, digit)| digit)
                                        .skip(1)
                                        .take_while(|digit| *digit != '}')
                                        .collect();
                                    let code = u32::from_str_radix(&code, 16).expect("hex");
                                    text.push(char::from_u32(code).expect("a character"));
                                }
                                Some((_, other)) => text.push(other),
                                None => {}
                            },
                            other => text.push(other),
                        }
                    }
                    let Some(end) = end else {
                        break;
                    };
                    arguments.push(text);
                    cursor = literal[end + 1..].trim_start().trim_start_matches(',');
                }
                if arguments.len() == wanted {
                    found.push(arguments);
                }
            }
            found
        }

        fn visit(directory: &Path, sources: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(directory).expect("the directory can be read") {
                let path = entry.expect("its entries too").path();
                if path.is_dir() {
                    visit(&path, sources);
                } else if path.extension().is_some_and(|extension| extension == "rs") {
                    sources.push(path);
                }
            }
        }

        let root = crate::session::workspace_root();
        let mut sources = Vec::new();
        for member in ["core", "services", "shell"] {
            visit(&root.join("crates").join(member).join("src"), &mut sources);
        }
        let mut unknown = Vec::new();
        let mut checked = 0;
        for path in sources {
            let source = std::fs::read_to_string(&path).expect("a source file is text");
            let mut asked: Vec<String> = Vec::new();
            asked.extend(
                literals(&source, "tr(", 1)
                    .into_iter()
                    .map(|a| a[0].clone()),
            );
            asked.extend(
                literals(&source, "tr_in(", 2)
                    .into_iter()
                    .map(|a| format!("{}\u{4}{}", a[0], a[1])),
            );
            asked.extend(
                literals(&source, "tr_n(", 1)
                    .into_iter()
                    .map(|a| a[0].clone()),
            );
            for text in asked {
                checked += 1;
                if !known.contains(&text) && !OWN.contains(&text.as_str()) {
                    unknown.push(format!("{}: {text:?}", path.display()));
                }
            }
        }
        assert!(checked > 100, "only {checked} texts were found to check");
        assert!(
            unknown.is_empty(),
            "GNOME has no such text (a \\u{{4}} follows a context): {unknown:#?}"
        );
    }

    /// gniri's own code runs on the GLib main loop and a few plain
    /// threads; zbus is used with its default runtime. A crate that pulls
    /// tokio in would start a second runtime beside them.
    #[test]
    fn nothing_depends_on_tokio() {
        let output = Command::new(env!("CARGO"))
            .args(["tree", "--workspace", "--invert", "tokio"])
            .current_dir(crate::session::workspace_root())
            .output()
            .expect("cargo runs");
        let listing = String::from_utf8_lossy(&output.stdout);
        assert!(
            listing.trim().is_empty(),
            "tokio is in the dependency tree:\n{listing}"
        );
    }
}
