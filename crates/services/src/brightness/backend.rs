// SPDX-License-Identifier: GPL-3.0-or-later

//! The backlight backend: `/sys/class/backlight` for reading, logind's
//! `SetBrightness` for writing.

use std::path::{Path, PathBuf};

use async_channel::{Receiver, Sender};

use super::{Command, Event};

#[zbus::proxy(
    interface = "org.freedesktop.login1.Session",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1/session/auto"
)]
trait Session {
    fn set_brightness(&self, subsystem: &str, name: &str, brightness: u32) -> zbus::Result<()>;
}

/// One device under `/sys/class/backlight`.
#[derive(Clone, Debug, PartialEq)]
pub struct Backlight {
    directory: PathBuf,
    name: String,
    max: u32,
}

fn read_number(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

impl Backlight {
    /// The backlight to control: the kernel's preferred kind first
    /// ("firmware", then "platform", then "raw"), as systemd-backlight and
    /// gnome-settings-daemon choose.
    pub fn find(class: &Path) -> Option<Self> {
        let rank = |kind: &str| match kind {
            "firmware" => 0,
            "platform" => 1,
            _ => 2,
        };
        let mut found: Vec<(u8, Self)> = std::fs::read_dir(class)
            .ok()?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let directory = entry.path();
                let max = read_number(&directory.join("max_brightness")).filter(|max| *max > 0)?;
                let kind = std::fs::read_to_string(directory.join("type")).unwrap_or_default();
                Some((
                    rank(kind.trim()),
                    Self {
                        name: entry.file_name().to_string_lossy().into_owned(),
                        directory,
                        max,
                    },
                ))
            })
            .collect();
        found.sort_by(|a, b| (a.0, &a.1.name).cmp(&(b.0, &b.1.name)));
        found.into_iter().next().map(|(_, backlight)| backlight)
    }

    /// The level now, 0.0 to 1.0.
    pub fn level(&self) -> Option<f64> {
        let raw = read_number(&self.directory.join("brightness"))?;
        Some(f64::from(raw.min(self.max)) / f64::from(self.max))
    }

    /// The raw value for `level`. Never 0 for a level above 0, and never
    /// below 1 at all: on many panels 0 switches the backlight off, which
    /// GNOME's slider cannot do either.
    pub fn raw(&self, level: f64) -> u32 {
        let raw = (level.clamp(0.0, 1.0) * f64::from(self.max)).round() as u32;
        raw.clamp(1, self.max)
    }
}

pub async fn run(
    connection: zbus::Connection,
    events: Sender<Event>,
    commands: Receiver<Command>,
) -> zbus::Result<()> {
    let backlight = Backlight::find(Path::new("/sys/class/backlight"));
    let level = || backlight.as_ref().and_then(Backlight::level);
    if events.send(Event::Level(level())).await.is_err() {
        return Ok(());
    }
    // Without a backlight nothing is ever set, but the backend runs on all
    // the same: to the shell a backend that has returned has stopped, and
    // opening the quick settings still asks this one for a Refresh.
    let session = if backlight.is_some() {
        Some(SessionProxy::new(&connection).await?)
    } else {
        None
    };

    while let Ok(mut command) = commands.recv().await {
        // A slider being dragged sends faster than the backlight follows:
        // only the newest level matters.
        while let Ok(newer) = commands.try_recv() {
            if matches!(newer, Command::Set(_)) || !matches!(command, Command::Set(_)) {
                command = newer;
            }
        }
        if let (Command::Set(level), Some(backlight), Some(session)) =
            (command, &backlight, &session)
        {
            let raw = backlight.raw(level);
            if let Err(error) = session
                .set_brightness("backlight", &backlight.name, raw)
                .await
            {
                eprintln!("gniri: brightness: cannot set the backlight: {error}");
            }
        }
        // After a set, and on Refresh: report what the kernel now says.
        if events.send(Event::Level(level())).await.is_err() {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(class: &Path, name: &str, kind: &str, max: u32, now: u32) {
        let directory = class.join(name);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("type"), format!("{kind}\n")).unwrap();
        std::fs::write(directory.join("max_brightness"), format!("{max}\n")).unwrap();
        std::fs::write(directory.join("brightness"), format!("{now}\n")).unwrap();
    }

    fn scratch(test: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("gniri-backlight-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn prefers_firmware_then_platform_then_raw() {
        let class = scratch("prefers");
        device(&class, "intel_backlight", "raw", 1000, 500);
        device(&class, "acpi_video0", "firmware", 10, 3);
        device(&class, "broken", "firmware", 0, 0);
        let backlight = Backlight::find(&class).unwrap();
        assert_eq!(backlight.name, "acpi_video0");
        assert_eq!(backlight.level(), Some(0.3));
        std::fs::remove_dir_all(class).unwrap();
    }

    #[test]
    fn levels_convert_and_never_reach_zero() {
        let class = scratch("levels");
        device(&class, "backlight", "raw", 200, 150);
        let backlight = Backlight::find(&class).unwrap();
        assert_eq!(backlight.level(), Some(0.75));
        assert_eq!(backlight.raw(0.5), 100);
        assert_eq!(backlight.raw(1.5), 200);
        assert_eq!(backlight.raw(0.0), 1);
        std::fs::remove_dir_all(class).unwrap();
    }

    #[test]
    fn no_backlight_is_none() {
        let class = scratch("none");
        assert_eq!(Backlight::find(&class), None);
        assert_eq!(Backlight::find(&class.join("missing")), None);
        std::fs::remove_dir_all(class).unwrap();
    }
}
