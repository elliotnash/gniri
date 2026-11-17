// SPDX-License-Identifier: GPL-3.0-or-later

//! What the shell itself changes and has to find again when it next
//! starts: how the applications are arranged, and the switches GNOME keeps
//! in settings of its own.
//!
//! One TOML file, `$XDG_STATE_HOME/gniri/state.toml`. It is the shell's to
//! write, where the config file is the user's: nothing in it is meant to
//! be edited by hand, though nothing breaks if it is, or if it is deleted.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::apps::Layout;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Saved {
    /// Do Not Disturb is on (GNOME: `show-banners`, the other way round).
    pub do_not_disturb: bool,
    /// Night Light is on (GNOME: `night-light-enabled`).
    pub night_light: bool,
    /// The arrangement of the app grid and the dash, once anything in it
    /// has been dragged. Absent, it is GNOME's own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apps: Option<Layout>,
}

/// `$XDG_STATE_HOME/gniri/state.toml`, falling back to `~/.local/state`.
pub fn default_path() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_STATE_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(std::env::var_os("HOME")?).join(".local/state"),
    };
    Some(base.join("gniri/state.toml"))
}

/// Reads the file. One that is not there gives the defaults; so does one
/// that cannot be read or understood, which is first set aside under
/// another name, so that the next [`save`] does not write over the only
/// copy of an arrangement someone made.
pub fn load(path: &Path) -> Saved {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Saved::default(),
        Err(error) => {
            eprintln!("gniri: cannot read {}: {error}", path.display());
            return Saved::default();
        }
    };
    match toml::from_str(&text) {
        Ok(saved) => saved,
        Err(error) => {
            let aside = path.with_extension("toml.unreadable");
            eprintln!(
                "gniri: {} makes no sense ({}); starting afresh and keeping it as {}",
                path.display(),
                error.message(),
                aside.display()
            );
            let _ = std::fs::rename(path, aside);
            Saved::default()
        }
    }
}

/// Writes the file, whole or not at all: beside it first, then moved into
/// its place.
pub fn save(path: &Path, saved: &Saved) -> std::io::Result<()> {
    let text = toml::to_string(saved).map_err(std::io::Error::other)?;
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory)?;
    }
    let partial = path.with_extension("toml.partial");
    std::fs::write(&partial, text)?;
    std::fs::rename(partial, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::{Entry, Folder};

    fn directory(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("gniri-state-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        directory
    }

    #[test]
    fn what_is_saved_is_read_back() {
        let directory = directory("round-trip");
        let path = directory.join("gniri/state.toml");
        assert_eq!(load(&path), Saved::default(), "no file yet");

        let saved = Saved {
            do_not_disturb: true,
            night_light: false,
            apps: Some(Layout {
                favorites: None,
                grid: vec![
                    Entry::App("org.gnome.Maps".to_owned()),
                    Entry::Folder {
                        folder: "folder-1".to_owned(),
                    },
                ],
                folders: vec![Folder {
                    id: "folder-1".to_owned(),
                    name: "Games".to_owned(),
                    apps: vec!["org.gnome.Chess".to_owned()],
                    ..Folder::default()
                }],
            }),
        };
        save(&path, &saved).unwrap();
        assert_eq!(load(&path), saved);
        // Nothing arranged: nothing written about it, and GNOME's own
        // arrangement stays the default as it changes between releases.
        save(&path, &Saved::default()).unwrap();
        assert!(!std::fs::read_to_string(&path).unwrap().contains("apps"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_file_that_makes_no_sense_is_set_aside() {
        let directory = directory("unreadable");
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("state.toml");
        std::fs::write(&path, "do_not_disturb = \"perhaps\"").unwrap();
        assert_eq!(load(&path), Saved::default());
        assert!(!path.exists());
        assert!(directory.join("state.toml.unreadable").exists());
        std::fs::remove_dir_all(directory).unwrap();
    }
}
