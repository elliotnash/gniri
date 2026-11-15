// SPDX-License-Identifier: GPL-3.0-or-later

//! Battery and power profile.
//!
//! The battery is UPower's display device; the profile comes from the
//! power-profiles daemon (tuned-ppd on Fedora). [`backend`] talks to both.

pub mod backend;

use serde::Deserialize;

use crate::model::Model;

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct State {
    /// The machine's battery, if it has one.
    pub battery: Option<Battery>,
    /// The profile in use. `None` without a power-profiles daemon, or while
    /// it is set to something other than the three standard profiles.
    pub profile: Option<Profile>,
    /// The profiles the daemon offers, in menu order (fastest first).
    pub profiles: Vec<Profile>,
    /// The profile the quick toggle switches to from balanced: the last
    /// non-default one chosen (`last-selected-power-profile` in GNOME).
    pub last_profile: Profile,
}

impl Default for State {
    fn default() -> Self {
        Self {
            battery: None,
            profile: None,
            profiles: Vec::new(),
            last_profile: Profile::PowerSaver,
        }
    }
}

/// What the backend reports.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Battery(Option<Battery>),
    Profiles {
        active: Option<Profile>,
        available: Vec<Profile>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    SetProfile(Profile),
}

impl State {
    /// What clicking the Power Mode toggle asks for. Port of the `clicked`
    /// handler in `status/powerProfiles.js`.
    pub fn toggled_profile(&self) -> Profile {
        match self.profile {
            Some(Profile::Balanced) | None => self.last_profile,
            Some(_) => Profile::Balanced,
        }
    }

    /// GNOME offers the profile menu only when there is a real choice.
    pub fn has_profile_menu(&self) -> bool {
        self.profiles.len() > 2
    }

    fn remember(&mut self) {
        if let Some(profile) = self.profile
            && profile != Profile::Balanced
        {
            self.last_profile = profile;
        }
    }
}

impl Model for State {
    type Event = Event;
    type Command = Command;

    fn apply(&mut self, event: Event) {
        match event {
            Event::Battery(battery) => self.battery = battery,
            Event::Profiles { active, available } => {
                self.profile = active;
                self.profiles = available;
                self.remember();
            }
        }
    }

    fn predict(&mut self, command: &Command) {
        match command {
            Command::SetProfile(profile) => {
                if self.profile.is_some() {
                    self.profile = Some(*profile);
                    self.remember();
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Battery {
    /// 0 to 100.
    pub percentage: f64,
    #[serde(default)]
    pub charge: Charge,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Charge {
    #[default]
    Discharging,
    Charging,
    /// On mains but held below a charge threshold.
    PluggedIn,
    Full,
}

impl Battery {
    /// The panel icon. Port of `status/system.js` (`_sync` in the power
    /// toggle): levels step down in tens.
    pub fn icon_name(&self) -> String {
        let level = 10 * (self.percentage.clamp(0.0, 100.0) / 10.0).floor() as u32;
        let charged =
            self.charge == Charge::Full || (self.charge == Charge::Charging && level == 100);
        if charged {
            return "battery-level-100-charged-symbolic".to_owned();
        }
        let suffix = match self.charge {
            Charge::Charging => "-charging",
            Charge::PluggedIn => "-plugged-in",
            Charge::Discharging | Charge::Full => "",
        };
        format!("battery-level-{level}{suffix}-symbolic")
    }

    /// The percentage label beside the icon.
    pub fn label(&self) -> String {
        format!("{}%", self.percentage.round() as u32)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Profile {
    PowerSaver,
    Balanced,
    Performance,
}

impl Profile {
    /// The daemon's name for the profile.
    pub fn id(self) -> &'static str {
        match self {
            Self::PowerSaver => "power-saver",
            Self::Balanced => "balanced",
            Self::Performance => "performance",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        [Self::PowerSaver, Self::Balanced, Self::Performance]
            .into_iter()
            .find(|profile| profile.id() == id)
    }

    /// The name shown in the menu and as the toggle's subtitle.
    pub fn label(self) -> &'static str {
        match self {
            Self::PowerSaver => "Power Saver",
            Self::Balanced => "Balanced",
            Self::Performance => "Performance",
        }
    }

    /// Port of `PROFILE_PARAMS` in `status/powerProfiles.js`.
    pub fn icon_name(self) -> &'static str {
        match self {
            Self::PowerSaver => "power-profile-power-saver-symbolic",
            Self::Balanced => "power-profile-balanced-symbolic",
            Self::Performance => "power-profile-performance-symbolic",
        }
    }

    /// GNOME shows the panel icon only for a profile other than the default.
    pub fn shows_indicator(self) -> bool {
        self != Self::Balanced
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn battery(percentage: f64, charge: Charge) -> Battery {
        Battery { percentage, charge }
    }

    #[test]
    fn battery_icons() {
        let icon = |percentage, charge| battery(percentage, charge).icon_name();
        // The reference captures: 91% on battery, 37% charging.
        assert_eq!(icon(91.0, Charge::Discharging), "battery-level-90-symbolic");
        assert_eq!(
            icon(37.0, Charge::Charging),
            "battery-level-30-charging-symbolic"
        );
        assert_eq!(icon(9.9, Charge::Discharging), "battery-level-0-symbolic");
        assert_eq!(
            icon(100.0, Charge::Discharging),
            "battery-level-100-symbolic"
        );
        assert_eq!(
            icon(100.0, Charge::Charging),
            "battery-level-100-charged-symbolic"
        );
        assert_eq!(
            icon(80.0, Charge::Full),
            "battery-level-100-charged-symbolic"
        );
        assert_eq!(
            icon(80.0, Charge::PluggedIn),
            "battery-level-80-plugged-in-symbolic"
        );
    }

    #[test]
    fn battery_label_rounds() {
        assert_eq!(battery(90.6, Charge::Discharging).label(), "91%");
    }

    #[test]
    fn the_toggle_alternates_between_balanced_and_the_last_other_profile() {
        let mut state = State::default();
        state.apply(Event::Profiles {
            active: Some(Profile::Balanced),
            available: vec![Profile::Performance, Profile::Balanced, Profile::PowerSaver],
        });
        assert!(state.has_profile_menu());
        // GNOME's default for the last selected profile.
        assert_eq!(state.toggled_profile(), Profile::PowerSaver);

        state.predict(&Command::SetProfile(Profile::Performance));
        assert_eq!(state.profile, Some(Profile::Performance));
        assert_eq!(state.toggled_profile(), Profile::Balanced);

        state.predict(&Command::SetProfile(Profile::Balanced));
        assert_eq!(state.toggled_profile(), Profile::Performance);
    }

    #[test]
    fn nothing_is_predicted_without_a_daemon() {
        let mut state = State::default();
        state.predict(&Command::SetProfile(Profile::Performance));
        assert_eq!(state, State::default());
        assert!(!state.has_profile_menu());
    }

    #[test]
    fn profile_ids_round_trip() {
        for profile in [Profile::PowerSaver, Profile::Balanced, Profile::Performance] {
            assert_eq!(Profile::from_id(profile.id()), Some(profile));
        }
        assert_eq!(Profile::from_id("custom"), None);
    }

    #[test]
    fn only_non_default_profiles_show() {
        assert!(Profile::Performance.shows_indicator());
        assert!(Profile::PowerSaver.shows_indicator());
        assert!(!Profile::Balanced.shows_indicator());
    }
}
