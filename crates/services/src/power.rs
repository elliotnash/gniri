//! Battery and power profile.
//!
//! M1 has the state and the indicator logic only; fixture scenes fill it in.
//! The UPower and power-profiles backends arrive in M2.

use serde::Deserialize;

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct State {
    /// The machine's battery, if it has one.
    pub battery: Option<Battery>,
    pub profile: Option<Profile>,
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
    fn only_non_default_profiles_show() {
        assert!(Profile::Performance.shows_indicator());
        assert!(Profile::PowerSaver.shows_indicator());
        assert!(!Profile::Balanced.shows_indicator());
    }
}
