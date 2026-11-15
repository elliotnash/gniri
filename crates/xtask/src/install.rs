// SPDX-License-Identifier: GPL-3.0-or-later

//! `cargo xtask install`: puts gniri and what it comes with where a system
//! expects them.
//!
//! ```text
//! <prefix>/bin/gniri
//! <prefix>/share/gniri/gniri.kdl             the fragment niri's config includes
//! <prefix>/share/doc/gniri/                  README, CONFIG, FIDELITY, the example config
//! <prefix>/share/licenses/gniri/COPYING
//! <prefix>/lib/systemd/user/gniri.service    (share/systemd/user below a home directory)
//! /etc/pam.d/gniri, gniri-fingerprint        (not below a home directory)
//! ```
//!
//! The default prefix is `~/.local`, which needs no privileges. For the
//! system, stage it and copy the result as root, so that cargo never runs
//! as root:
//!
//! ```text
//! cargo xtask install --prefix /usr --destdir target/stage
//! sudo cp -a target/stage/. /
//! ```
//!
//! The fragment and the unit are installed with the binary's path written
//! in, so neither depends on what `PATH` is when niri or systemd start.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::session::workspace_root;

/// Where the PAM service files go. PAM reads no other place.
const PAM_DIR: &str = "/etc/pam.d";

/// What one installed file is made from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The release build of gniri.
    Binary,
    /// A file of the repository, as it is.
    Copy(&'static str),
    /// The niri fragment, with the installed paths written in.
    Fragment,
    /// The systemd unit, likewise.
    Unit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub source: Source,
    /// Where it goes on the installed system.
    pub target: PathBuf,
}

/// Where everything goes.
pub struct Layout {
    pub items: Vec<Item>,
    pub binary: PathBuf,
    pub fragment: PathBuf,
    pub unit: PathBuf,
    /// Below the user's home directory: nothing there needs privileges,
    /// and nothing outside it is touched.
    pub personal: bool,
}

/// The files of an installation under `prefix`. `home` is the home
/// directory of whoever installs.
pub fn layout(prefix: &Path, home: Option<&Path>) -> Layout {
    let personal = home.is_some_and(|home| prefix.starts_with(home));
    let binary = prefix.join("bin/gniri");
    let fragment = prefix.join("share/gniri/gniri.kdl");
    // systemd looks for a user's own units under its data directory, and
    // for everybody's under lib.
    let unit = prefix
        .join(if personal {
            "share/systemd/user"
        } else {
            "lib/systemd/user"
        })
        .join("gniri.service");
    let docs = prefix.join("share/doc/gniri");

    let mut items = vec![
        Item {
            source: Source::Binary,
            target: binary.clone(),
        },
        Item {
            source: Source::Fragment,
            target: fragment.clone(),
        },
        Item {
            source: Source::Unit,
            target: unit.clone(),
        },
        Item {
            source: Source::Copy("COPYING"),
            target: prefix.join("share/licenses/gniri/COPYING"),
        },
    ];
    for (file, name) in [
        ("README.md", "README.md"),
        ("docs/CONFIG.md", "CONFIG.md"),
        ("docs/FIDELITY.md", "FIDELITY.md"),
        ("data/config.example.toml", "config.example.toml"),
    ] {
        items.push(Item {
            source: Source::Copy(file),
            target: docs.join(name),
        });
    }
    if !personal {
        for file in ["data/pam/gniri", "data/pam/gniri-fingerprint"] {
            let name = Path::new(file).file_name().expect("the path names a file");
            items.push(Item {
                source: Source::Copy(file),
                target: Path::new(PAM_DIR).join(name),
            });
        }
    }
    Layout {
        items,
        binary,
        fragment,
        unit,
        personal,
    }
}

/// The niri fragment as it is installed: its binds start gniri by its
/// installed path, and its first lines name the fragment's own.
pub fn render_fragment(text: &str, layout: &Layout) -> String {
    text.replace(
        "spawn \"gniri\"",
        &format!("spawn \"{}\"", layout.binary.display()),
    )
    .replace(
        "/absolute/path/to/gniri.kdl",
        &layout.fragment.display().to_string(),
    )
}

/// The systemd unit as it is installed: it starts gniri by its installed
/// path.
pub fn render_unit(text: &str, layout: &Layout) -> String {
    text.replace(
        "ExecStart=gniri",
        &format!("ExecStart={}", layout.binary.display()),
    )
}

pub struct Options {
    pub prefix: Option<PathBuf>,
    /// A directory to install into as if it were `/`.
    pub destdir: Option<PathBuf>,
    /// Use the release build there is instead of building.
    pub no_build: bool,
    /// Take away what an installation put there.
    pub uninstall: bool,
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

/// `target` below `destdir`, if there is one.
fn staged(destdir: Option<&Path>, target: &Path) -> PathBuf {
    match destdir {
        Some(destdir) => destdir.join(target.strip_prefix("/").unwrap_or(target)),
        None => target.to_owned(),
    }
}

fn build_release(root: &Path) -> Result<()> {
    let status = Command::new(env!("CARGO"))
        .args(["build", "--release", "--package", "gniri"])
        .current_dir(root)
        .status()
        .context("running cargo build")?;
    if !status.success() {
        bail!("building gniri failed");
    }
    Ok(())
}

pub fn run(options: &Options) -> Result<bool> {
    let root = workspace_root();
    let home = home();
    let prefix = match (&options.prefix, &home) {
        (Some(prefix), _) => prefix.clone(),
        (None, Some(home)) => home.join(".local"),
        (None, None) => bail!("HOME is not set; say where to install with --prefix"),
    };
    if !prefix.is_absolute() {
        bail!("--prefix must be an absolute path");
    }
    let layout = layout(&prefix, home.as_deref());
    let destdir = options.destdir.as_deref();

    if options.uninstall {
        for item in &layout.items {
            let path = staged(destdir, &item.target);
            match fs::remove_file(&path) {
                Ok(()) => println!("removed {}", path.display()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => bail!("cannot remove {}: {error}", path.display()),
            }
            // The directories that were gniri's alone go too; one that
            // holds anything else stays, since removing it fails.
            if let Some(parent) = path.parent()
                && parent.file_name().is_some_and(|name| name == "gniri")
            {
                let _ = fs::remove_dir(parent);
            }
        }
        return Ok(true);
    }

    let binary = root.join("target/release/gniri");
    if !options.no_build {
        build_release(&root)?;
    }
    if !binary.is_file() {
        bail!("{} is not there; run without --no-build", binary.display());
    }

    for item in &layout.items {
        let path = staged(destdir, &item.target);
        let parent = path.parent().expect("an installed file is in a directory");
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        let read = |file: &str| {
            fs::read_to_string(root.join(file)).with_context(|| format!("reading {file}"))
        };
        let (written, mode) = match item.source {
            Source::Binary => {
                // Removed first: a running gniri keeps the old file, where
                // writing into it would fail.
                let _ = fs::remove_file(&path);
                (fs::copy(&binary, &path).map(drop), 0o755)
            }
            Source::Copy(file) => (fs::copy(root.join(file), &path).map(drop), 0o644),
            Source::Fragment => (
                fs::write(
                    &path,
                    render_fragment(&read("data/niri/gniri.kdl")?, &layout),
                ),
                0o644,
            ),
            Source::Unit => (
                fs::write(
                    &path,
                    render_unit(&read("data/systemd/gniri.service")?, &layout),
                ),
                0o644,
            ),
        };
        written.with_context(|| {
            let hint = if destdir.is_none() && !layout.personal {
                " (to install for the system, stage it with --destdir and copy that as root)"
            } else {
                ""
            };
            format!("writing {}{hint}", path.display())
        })?;
        fs::set_permissions(&path, fs::Permissions::from_mode(mode))
            .with_context(|| format!("setting the mode of {}", path.display()))?;
        println!("installed {}", path.display());
    }

    println!();
    if let Some(destdir) = destdir {
        println!(
            "Staged under {0}. To install: sudo cp -a {0}/. /",
            destdir.display()
        );
    }
    println!("To use it, add to ~/.config/niri/config.kdl, after your own binds:");
    println!("    include \"{}\"", layout.fragment.display());
    println!("and start gniri with niri, as a service that is started again if it fails");
    println!("({}):", layout.unit.display());
    println!("    systemctl --user daemon-reload");
    println!("    systemctl --user add-wants niri.service gniri.service");
    println!(
        "or, instead, with a line in niri's config: spawn-at-startup \"{}\"",
        layout.binary.display()
    );
    if layout.personal {
        println!();
        println!("The lock screen's own PAM services are not installed below a home");
        println!("directory; without them it uses the system's (see the README):");
        println!("    sudo cp data/pam/gniri data/pam/gniri-fingerprint {PAM_DIR}/");
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets(layout: &Layout) -> Vec<String> {
        layout
            .items
            .iter()
            .map(|item| item.target.display().to_string())
            .collect()
    }

    #[test]
    fn a_system_installation_has_the_unit_under_lib_and_the_pam_services() {
        let layout = layout(Path::new("/usr"), Some(Path::new("/home/someone")));
        assert!(!layout.personal);
        assert_eq!(
            targets(&layout),
            [
                "/usr/bin/gniri",
                "/usr/share/gniri/gniri.kdl",
                "/usr/lib/systemd/user/gniri.service",
                "/usr/share/licenses/gniri/COPYING",
                "/usr/share/doc/gniri/README.md",
                "/usr/share/doc/gniri/CONFIG.md",
                "/usr/share/doc/gniri/FIDELITY.md",
                "/usr/share/doc/gniri/config.example.toml",
                "/etc/pam.d/gniri",
                "/etc/pam.d/gniri-fingerprint",
            ]
        );
        assert_eq!(
            staged(Some(Path::new("/tmp/stage")), Path::new("/etc/pam.d/gniri")),
            Path::new("/tmp/stage/etc/pam.d/gniri")
        );
        assert_eq!(
            staged(None, Path::new("/usr/bin/gniri")),
            Path::new("/usr/bin/gniri")
        );
    }

    #[test]
    fn a_personal_installation_stays_below_the_home_directory() {
        let home = Path::new("/home/someone");
        let layout = layout(&home.join(".local"), Some(home));
        assert!(layout.personal);
        assert!(
            layout
                .items
                .iter()
                .all(|item| item.target.starts_with(home)),
            "{:?}",
            targets(&layout)
        );
        assert_eq!(
            layout.unit,
            Path::new("/home/someone/.local/share/systemd/user/gniri.service")
        );
    }

    #[test]
    fn every_file_to_install_is_in_the_repository() {
        let root = workspace_root();
        let layout = layout(Path::new("/usr"), None);
        for item in &layout.items {
            if let Source::Copy(file) = item.source {
                assert!(root.join(file).is_file(), "{file} is missing");
            }
        }
    }

    #[test]
    fn the_installed_fragment_and_unit_name_the_installed_binary() {
        let root = workspace_root();
        let layout = layout(Path::new("/opt/gniri"), None);
        let fragment = fs::read_to_string(root.join("data/niri/gniri.kdl")).unwrap();
        let binds = fragment.matches("spawn \"gniri\"").count();
        assert!(binds >= 8, "the fragment binds gniri's keys");
        let rendered = render_fragment(&fragment, &layout);
        assert_eq!(rendered.matches("spawn \"gniri\"").count(), 0);
        assert_eq!(
            rendered.matches("spawn \"/opt/gniri/bin/gniri\"").count(),
            binds
        );
        assert!(rendered.contains("include \"/opt/gniri/share/gniri/gniri.kdl\""));

        let unit = fs::read_to_string(root.join("data/systemd/gniri.service")).unwrap();
        let rendered = render_unit(&unit, &layout);
        assert!(rendered.contains("\nExecStart=/opt/gniri/bin/gniri\n"));
    }

    /// The plan's rule for the fragment, kept by a test: whatever is
    /// shipped must be a config niri takes.
    #[test]
    fn niri_takes_the_fragment_as_written_and_as_installed() {
        let has_niri = Command::new("niri")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success());
        if !has_niri {
            eprintln!("skipped: niri is not installed");
            return;
        }
        let root = workspace_root();
        let source = root.join("data/niri/gniri.kdl");
        let rendered = std::env::temp_dir().join(format!("gniri-{}.kdl", std::process::id()));
        let layout = layout(Path::new("/usr"), None);
        fs::write(
            &rendered,
            render_fragment(&fs::read_to_string(&source).unwrap(), &layout),
        )
        .unwrap();
        for config in [&source, &rendered] {
            let output = Command::new("niri")
                .arg("validate")
                .arg("-c")
                .arg(config)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "niri rejects {}: {}",
                config.display(),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let _ = fs::remove_file(rendered);
    }
}
