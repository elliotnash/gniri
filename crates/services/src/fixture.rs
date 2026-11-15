//! Fixture scenes: service state from a TOML file instead of the system.
//!
//! `gniri --fixture scene.toml` freezes the clock and fills in the services
//! that have no deterministic source, so a screenshot shows the same thing
//! every time. The compositor is never faked: a test drives the nested niri
//! to the state it wants.

use std::path::Path;

use gniri_core::{FixedClock, LocalTime};
use serde::Deserialize;

use crate::{
    audio, bluetooth, brightness, mpris, network, notifications, power, rfkill, session, toggles,
};

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Fixture {
    /// The moment the clock is frozen at, `YYYY-MM-DDTHH:MM:SS`.
    pub time: Option<String>,
    pub power: power::State,
    pub audio: audio::State,
    pub network: network::State,
    pub bluetooth: bluetooth::State,
    pub brightness: brightness::State,
    pub rfkill: rfkill::State,
    pub session: session::State,
    pub dark_style: toggles::State,
    pub night_light: toggles::State,
    pub do_not_disturb: toggles::State,
    pub notifications: notifications::State,
    pub mpris: mpris::State,
}

impl Fixture {
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let fixture: Self = toml::from_str(text).map_err(|error| error.to_string())?;
        fixture.clock()?;
        Ok(fixture)
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        Self::from_toml(&text).map_err(|error| format!("{}: {error}", path.display()))
    }

    /// The frozen clock, if the scene sets a time.
    pub fn clock(&self) -> Result<Option<FixedClock>, String> {
        match &self.time {
            Some(time) => LocalTime::parse(time)
                .map(|time| Some(FixedClock(time)))
                .map_err(|error| format!("time: {error}")),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_scene() {
        let fixture = Fixture::from_toml(
            r#"
            time = "2026-11-14T08:28:00"

            [power]
            profile = "performance"
            battery = { percentage = 91 }

            [audio]
            output = { volume = 0.3 }

            [network]
            primary = { kind = "wifi", strength = 70 }

            [bluetooth]
            adapter = { powered = true }

            [brightness]
            level = 1.0

            [dark_style]
            available = true
            on = true
            "#,
        )
        .unwrap();
        assert_eq!(fixture.clock().unwrap().unwrap().0.hour, 8);
        assert_eq!(
            fixture.power.battery.as_ref().unwrap().icon_name(),
            "battery-level-90-symbolic"
        );
        assert_eq!(fixture.power.profile, Some(power::Profile::Performance));
        assert_eq!(
            fixture.audio.output.as_ref().unwrap().icon_name(),
            "audio-volume-low-symbolic"
        );
        assert_eq!(
            fixture.network.panel_icon_name(),
            Some("network-wireless-signal-good-symbolic")
        );
        assert_eq!(fixture.brightness.level, Some(1.0));
        assert!(fixture.dark_style.on);
        assert_eq!(fixture.bluetooth.subtitle(), None);
    }

    #[test]
    fn empty_scene_and_mistakes() {
        assert_eq!(Fixture::from_toml("").unwrap(), Fixture::default());
        assert!(Fixture::from_toml("time = \"noon\"").is_err());
        assert!(Fixture::from_toml("[power]\nbatery = {}").is_err());
        assert!(Fixture::from_toml("[network]\nprimary = { kind = \"wired\" }").is_ok());
    }
}
