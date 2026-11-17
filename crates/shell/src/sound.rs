// SPDX-License-Identifier: GPL-3.0-or-later

//! Plays a notification's sound, as Mutter's `MetaSoundPlayer` does for
//! GNOME Shell: a sound of the desktop's sound theme by name, or a file,
//! unless event sounds are turned off (`org.gnome.desktop.sound
//! event-sounds`).
//!
//! `canberra-gtk-play` plays it where it is installed, knowing sound themes
//! as libcanberra does; else gniri finds the theme's file itself and hands
//! it to `pw-play` or `paplay`.

use std::path::PathBuf;

use gniri_services::desktop_entry;
use gniri_services::notifications::Sound;
use gtk::gio;
use gtk::prelude::*;

use crate::services::Services;

const SOUND_SCHEMA: &str = "org.gnome.desktop.sound";

/// The desktop's sound settings: whether event sounds are on, and the
/// theme's name.
fn settings() -> (bool, String) {
    let installed = gio::SettingsSchemaSource::default()
        .and_then(|source| source.lookup(SOUND_SCHEMA, true))
        .is_some_and(|schema| schema.has_key("event-sounds") && schema.has_key("theme-name"));
    if !installed {
        return (true, "freedesktop".to_owned());
    }
    let settings = gio::Settings::new(SOUND_SCHEMA);
    (
        settings.boolean("event-sounds"),
        settings.string("theme-name").into(),
    )
}

/// The file of the sound `name` in `theme`, or in the freedesktop theme
/// every other falls back to, among `data_dirs`.
fn theme_file(name: &str, theme: &str, data_dirs: &[PathBuf]) -> Option<PathBuf> {
    if name.contains('/') {
        return None;
    }
    let themes = [theme, "freedesktop"];
    themes.iter().find_map(|theme| {
        data_dirs.iter().find_map(|dir| {
            let base = dir.join("sounds").join(theme);
            ["stereo", ""].iter().find_map(|subdir| {
                ["oga", "ogg", "wav"].iter().find_map(|extension| {
                    let path = base.join(subdir).join(format!("{name}.{extension}"));
                    path.is_file().then_some(path)
                })
            })
        })
    })
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

/// The command that plays `sound`, if anything can.
fn command(sound: &Sound, theme: &str) -> Option<Vec<String>> {
    const DESCRIPTION: &str = "--description=Notification sound";
    if on_path("canberra-gtk-play") {
        let what = match sound {
            Sound::Name(name) => format!("--id={name}"),
            Sound::File(path) => format!("--file={}", path.display()),
        };
        return Some(vec![
            "canberra-gtk-play".to_owned(),
            what,
            DESCRIPTION.to_owned(),
        ]);
    }
    let file = match sound {
        Sound::Name(name) => theme_file(name, theme, &desktop_entry::data_dirs())?,
        Sound::File(path) => path.clone(),
    };
    let player = ["pw-play", "paplay"]
        .into_iter()
        .find(|player| on_path(player))?;
    Some(vec![player.to_owned(), file.display().to_string()])
}

/// Plays `sound`. Under a fixture it is only logged.
pub fn play(services: &Services, sound: &Sound) {
    services.commands.push("sound", sound);
    if services.sandboxed {
        return;
    }
    let (on, theme) = settings();
    if !on {
        return;
    }
    let Some(command) = command(sound, &theme) else {
        return;
    };
    match std::process::Command::new(&command[0])
        .args(&command[1..])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        // Waited for on a thread of its own, so it leaves no zombie.
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(error) => eprintln!("gniri: cannot play a sound: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn a_sound_of_the_theme_is_found_by_its_name() {
        let dir = std::env::temp_dir().join(format!("gniri-sounds-{}", std::process::id()));
        let stereo = dir.join("sounds/freedesktop/stereo");
        std::fs::create_dir_all(&stereo).unwrap();
        std::fs::write(stereo.join("message-new-instant.oga"), b"").unwrap();
        std::fs::create_dir_all(dir.join("sounds/mine")).unwrap();
        std::fs::write(dir.join("sounds/mine/bell.wav"), b"").unwrap();
        let dirs = [dir.clone()];
        assert_eq!(
            theme_file("message-new-instant", "mine", &dirs),
            Some(stereo.join("message-new-instant.oga")),
            "the freedesktop theme stands in"
        );
        assert_eq!(
            theme_file("bell", "mine", &dirs),
            Some(Path::new(&dir).join("sounds/mine/bell.wav"))
        );
        assert_eq!(theme_file("nothing", "mine", &dirs), None);
        assert_eq!(theme_file("../bell", "mine", &dirs), None);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
