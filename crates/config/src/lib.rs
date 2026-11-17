// SPDX-License-Identifier: GPL-3.0-or-later

//! The gniri configuration file.
//!
//! One TOML file, `$XDG_CONFIG_HOME/gniri/config.toml`. Every key is
//! optional; a missing file means all defaults. Unknown keys are errors, so a
//! typo is reported instead of silently ignored.
//!
//! `data/config.example.toml` documents every key.

use std::fmt;
use std::path::{Path, PathBuf};

use gniri_core::ClockFormat;
use serde::Deserialize;

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub appearance: Appearance,
    pub wallpaper: Wallpaper,
    pub clock: Clock,
    pub panel: Panel,
    pub date_menu: DateMenu,
    pub night_light: NightLight,
    pub idle: Idle,
    pub lock: Lock,
    pub dash: Dash,
    pub search: Search,
    pub commands: Commands,
}

/// How the shell itself looks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Appearance {
    pub style: Style,
}

/// Which of its two styles the shell has: its bar, its menus and the rest.
/// Not the applications' style, which the Dark Style switch in the quick
/// settings sets. The names of the middle two are GNOME Shell's own for
/// its two rules.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Style {
    /// Always dark.
    Dark,
    /// Always light.
    Light,
    /// GNOME's rule: dark, unless the desktop's colour scheme says
    /// "prefer light", which no switch sets. In effect dark.
    #[default]
    PreferDark,
    /// GNOME Classic's rule: light, unless the desktop's colour scheme
    /// says "prefer dark". The shell then follows the Dark Style switch.
    PreferLight,
}

/// What gniri draws behind the windows. With neither key set it draws no
/// wallpaper, for people who use another wallpaper program.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Wallpaper {
    /// An image file, scaled to cover each output. `~/` is expanded.
    pub image: Option<PathBuf>,
    /// A flat colour, `#rgb` or `#rrggbb`. Used when there is no image.
    pub color: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub enum HourFormat {
    #[default]
    #[serde(rename = "24h")]
    H24,
    #[serde(rename = "12h")]
    H12,
}

/// The panel clock. Defaults are GNOME's.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Clock {
    pub format: HourFormat,
    pub show_date: bool,
    pub show_weekday: bool,
    pub show_seconds: bool,
}

impl Default for Clock {
    fn default() -> Self {
        Self {
            format: HourFormat::H24,
            show_date: true,
            show_weekday: false,
            show_seconds: false,
        }
    }
}

impl Clock {
    pub fn format(&self) -> ClockFormat {
        ClockFormat {
            hour12: self.format == HourFormat::H12,
            show_date: self.show_date,
            show_weekday: self.show_weekday,
            show_seconds: self.show_seconds,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Panel {
    /// Show the charge level as a number beside the battery icon.
    pub show_battery_percentage: bool,
    /// Outputs that get a panel, by connector name (`niri msg outputs`).
    /// Unset means every output.
    pub outputs: Option<Vec<String>>,
}

impl Panel {
    pub fn shows_on(&self, output: &str) -> bool {
        match &self.outputs {
            Some(outputs) => outputs.iter().any(|name| name == output),
            None => true,
        }
    }
}

/// The menu under the clock.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DateMenu {
    /// Show the "Add World Clocks…" card. gniri does not show the clocks
    /// themselves; the card only starts GNOME Clocks.
    pub world_clocks: bool,
    /// Show the "Select Weather Location…" card. gniri shows no forecast;
    /// the card only starts GNOME Weather.
    pub weather: bool,
    /// The day a week starts on in the calendar: "sunday", "monday" or
    /// "saturday". Unset, it is the day the user's locale starts a week on.
    pub week_start: Option<WeekStart>,
}

impl Default for DateMenu {
    fn default() -> Self {
        Self {
            world_clocks: true,
            weather: true,
            week_start: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WeekStart {
    #[default]
    Sunday,
    Monday,
    Saturday,
}

impl WeekStart {
    pub fn weekday(self) -> gniri_core::Weekday {
        match self {
            Self::Sunday => gniri_core::Weekday::Sunday,
            Self::Monday => gniri_core::Weekday::Monday,
            Self::Saturday => gniri_core::Weekday::Saturday,
        }
    }
}

/// The Night Light toggle.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NightLight {
    /// Colour temperature of the screen while night light is on, in kelvin.
    /// Lower is warmer. 1700 to 4700, the range GNOME Settings offers.
    pub temperature: u32,
}

impl Default for NightLight {
    fn default() -> Self {
        Self { temperature: 2700 }
    }
}

/// What happens when nobody touches the machine. GNOME Settings calls the
/// delay "Blank Screen Delay".
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Idle {
    /// Seconds without input after which the screens turn off. 0 for
    /// never.
    pub blank_after: u32,
    /// Dim the screen for the last fifth of that time, as a warning.
    pub dim: bool,
}

impl Default for Idle {
    fn default() -> Self {
        Self {
            blank_after: 300,
            dim: true,
        }
    }
}

/// The lock screen. GNOME Settings: "Automatic Screen Lock" and its delay.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Lock {
    /// Lock when the screens turn off for idleness.
    pub automatic: bool,
    /// Seconds between the screens turning off and the lock.
    pub delay: u32,
    /// Lock before the machine sleeps.
    pub on_suspend: bool,
}

impl Default for Lock {
    fn default() -> Self {
        Self {
            automatic: true,
            delay: 0,
            on_suspend: true,
        }
    }
}

/// The dash at the bottom of the overview.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Dash {
    /// The applications that are always in the dash, in order, by desktop
    /// entry: "org.gnome.Nautilus", with or without ".desktop". One that
    /// is not installed is left out. The default is GNOME's.
    pub favorites: Vec<String>,
    /// The size of the icons, of 16, 22, 24, 32, 48 and 64. Unset, it is
    /// the largest that fits the screen, as in GNOME.
    pub icon_size: Option<u32>,
}

/// dash.js `baseIconSizes`.
const DASH_ICON_SIZES: [u32; 6] = [16, 22, 24, 32, 48, 64];

impl Default for Dash {
    fn default() -> Self {
        // org.gnome.shell's `favorite-apps`.
        let favorites = [
            "org.mozilla.firefox",
            "org.gnome.Calendar",
            "org.gnome.Nautilus",
            "org.gnome.Software",
            "org.gnome.TextEditor",
            "org.gnome.Calculator",
        ];
        Self {
            favorites: favorites.map(str::to_owned).to_vec(),
            icon_size: None,
        }
    }
}

/// The overview's search.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Search {
    /// Ask the applications that offer results (Files, Calculator,
    /// Software and so on), beside finding applications by name. GNOME
    /// Settings calls these "App Results".
    pub providers: bool,
    /// Applications that are not asked, by desktop entry.
    pub disabled: Vec<String>,
}

impl Default for Search {
    fn default() -> Self {
        Self {
            providers: true,
            disabled: Vec::new(),
        }
    }
}

/// Programs gniri starts.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Commands {
    /// The settings application. It is started as `<settings> <panel>` for
    /// entries such as "Power Settings", with GNOME Settings' panel names
    /// (`power`, `wifi`, `bluetooth`, `sound`). Empty for none: the buttons
    /// and menu entries that open settings are then hidden.
    pub settings: String,
}

impl Default for Commands {
    fn default() -> Self {
        Self {
            settings: "gnome-control-center".to_owned(),
        }
    }
}

#[derive(Debug)]
pub enum Error {
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    /// Not valid TOML, or a key or value the schema does not allow. The
    /// message carries the line and column.
    Parse(String),
    /// Well-formed, but a value makes no sense.
    Invalid(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::Parse(message) | Self::Invalid(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

fn is_hex_colour(text: &str) -> bool {
    match text.strip_prefix('#') {
        Some(digits) => {
            matches!(digits.len(), 3 | 6) && digits.bytes().all(|byte| byte.is_ascii_hexdigit())
        }
        None => false,
    }
}

impl Config {
    /// Parses and validates a config file's text. `home` replaces a leading
    /// `~/` in paths.
    pub fn from_toml(text: &str, home: Option<&Path>) -> Result<Self, Error> {
        let mut config: Self =
            toml::from_str(text).map_err(|error| Error::Parse(error.to_string()))?;

        if let Some(colour) = &config.wallpaper.color
            && !is_hex_colour(colour)
        {
            return Err(Error::Invalid(format!(
                "wallpaper.color: {colour:?} is not a colour like \"#241f31\""
            )));
        }
        if let Some(image) = &config.wallpaper.image
            && let Ok(rest) = image.strip_prefix("~")
        {
            let home = home.ok_or_else(|| {
                Error::Invalid("wallpaper.image starts with ~ but HOME is not set".to_owned())
            })?;
            config.wallpaper.image = Some(home.join(rest));
        }
        if !(1700..=4700).contains(&config.night_light.temperature) {
            return Err(Error::Invalid(format!(
                "night_light.temperature: {} is not between 1700 and 4700",
                config.night_light.temperature
            )));
        }
        if let Some(outputs) = &config.panel.outputs
            && outputs.iter().any(String::is_empty)
        {
            return Err(Error::Invalid(
                "panel.outputs: an output name cannot be empty".to_owned(),
            ));
        }
        if let Some(size) = config.dash.icon_size
            && !DASH_ICON_SIZES.contains(&size)
        {
            return Err(Error::Invalid(format!(
                "dash.icon_size: {size} is not one of 16, 22, 24, 32, 48 and 64"
            )));
        }
        for (key, list) in [
            ("dash.favorites", &config.dash.favorites),
            ("search.disabled", &config.search.disabled),
        ] {
            if list.iter().any(|id| id.is_empty() || id.contains('/')) {
                return Err(Error::Invalid(format!(
                    "{key}: an application is named by its desktop entry, \
                     such as \"org.gnome.Nautilus\""
                )));
            }
        }
        Ok(config)
    }

    /// Loads `path`. A file that does not exist gives the defaults.
    pub fn load(path: &Path) -> Result<Self, Error> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let home = std::env::var_os("HOME").map(PathBuf::from);
                Self::from_toml(&text, home.as_deref()).map_err(|error| match error {
                    Error::Parse(message) => Error::Parse(format!("{}: {message}", path.display())),
                    Error::Invalid(message) => {
                        Error::Invalid(format!("{}: {message}", path.display()))
                    }
                    other => other,
                })
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(Error::Read {
                path: path.to_owned(),
                source,
            }),
        }
    }
}

/// `$XDG_CONFIG_HOME/gniri/config.toml`, falling back to `~/.config`.
pub fn default_path() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(std::env::var_os("HOME")?).join(".config"),
    };
    Some(base.join("gniri/config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Config, Error> {
        Config::from_toml(text, Some(Path::new("/home/someone")))
    }

    #[test]
    fn empty_file_is_all_defaults() {
        let config = parse("").unwrap();
        assert_eq!(config, Config::default());
        assert!(!config.clock.format().hour12);
        assert!(config.clock.format().show_date);
        assert!(config.panel.shows_on("eDP-1"));
        assert_eq!(config.wallpaper.image, None);
        // The locale's.
        assert_eq!(config.date_menu.week_start, None);
        assert_eq!(config.appearance.style, Style::PreferDark);
        assert_eq!(config.commands.settings, "gnome-control-center");
        // GNOME's defaults: blank and lock after five minutes.
        assert_eq!(config.idle.blank_after, 300);
        assert!(config.idle.dim && config.lock.automatic && config.lock.on_suspend);
        assert_eq!(config.lock.delay, 0);
        // GNOME's favourites, icons as large as fit, every provider asked.
        assert_eq!(config.dash.favorites.len(), 6);
        assert_eq!(config.dash.favorites[2], "org.gnome.Nautilus");
        assert_eq!(config.dash.icon_size, None);
        assert!(config.search.providers && config.search.disabled.is_empty());
    }

    #[test]
    fn reads_every_key() {
        let config = parse(
            r##"
            [appearance]
            style = "prefer-light"

            [wallpaper]
            image = "~/Pictures/wall.jxl"
            color = "#241F31"

            [clock]
            format = "12h"
            show_date = false
            show_weekday = true
            show_seconds = true

            [panel]
            show_battery_percentage = true
            outputs = ["eDP-1"]

            [date_menu]
            week_start = "monday"
            world_clocks = false
            weather = false

            [night_light]
            temperature = 3500

            [idle]
            blank_after = 600
            dim = false

            [lock]
            automatic = false
            delay = 30
            on_suspend = false

            [dash]
            favorites = ["org.gnome.Nautilus.desktop", "firefox"]
            icon_size = 48

            [search]
            providers = false
            disabled = ["org.gnome.Software"]

            [commands]
            settings = "my-settings"
            "##,
        )
        .unwrap();
        assert_eq!(config.appearance.style, Style::PreferLight);
        assert_eq!(
            config.dash,
            Dash {
                favorites: vec![
                    "org.gnome.Nautilus.desktop".to_owned(),
                    "firefox".to_owned()
                ],
                icon_size: Some(48),
            }
        );
        assert_eq!(
            config.search,
            Search {
                providers: false,
                disabled: vec!["org.gnome.Software".to_owned()],
            }
        );
        assert_eq!(
            config.wallpaper.image.as_deref(),
            Some(Path::new("/home/someone/Pictures/wall.jxl"))
        );
        assert_eq!(config.wallpaper.color.as_deref(), Some("#241F31"));
        assert_eq!(
            config.clock.format(),
            ClockFormat {
                hour12: true,
                show_date: false,
                show_weekday: true,
                show_seconds: true
            }
        );
        assert!(config.panel.show_battery_percentage);
        assert!(config.panel.shows_on("eDP-1"));
        assert!(!config.panel.shows_on("HDMI-A-1"));
        assert_eq!(config.commands.settings, "my-settings");
        assert_eq!(config.night_light.temperature, 3500);
        assert_eq!(
            config.idle,
            Idle {
                blank_after: 600,
                dim: false
            }
        );
        assert_eq!(
            config.lock,
            Lock {
                automatic: false,
                delay: 30,
                on_suspend: false
            }
        );
        assert_eq!(
            config.date_menu,
            DateMenu {
                world_clocks: false,
                weather: false,
                week_start: Some(WeekStart::Monday),
            }
        );
        assert_eq!(
            config.date_menu.week_start.map(WeekStart::weekday),
            Some(gniri_core::Weekday::Monday)
        );
    }

    #[test]
    fn the_example_file_is_valid() {
        let text = include_str!("../../../data/config.example.toml");
        parse(text).unwrap();
        // Uncommenting every setting must still parse, so the documented
        // keys cannot drift from the schema.
        let uncommented: String = text
            .lines()
            .map(|line| match line.strip_prefix("# ") {
                Some(setting) if setting.contains(" = ") => setting,
                _ => line,
            })
            .collect::<Vec<_>>()
            .join("\n");
        let config = parse(&uncommented).unwrap();
        assert_ne!(config, Config::default());
    }

    #[test]
    fn rejects_mistakes() {
        let unknown = parse("[clock]\nformat24 = true").unwrap_err();
        assert!(matches!(unknown, Error::Parse(_)), "{unknown:?}");
        assert!(unknown.to_string().contains("format24"));

        assert!(matches!(
            parse("[clock]\nformat = \"13h\""),
            Err(Error::Parse(_))
        ));
        assert!(matches!(
            parse("[appearance]\nstyle = \"sepia\""),
            Err(Error::Parse(_))
        ));
        assert!(matches!(
            parse("[wallpaper]\ncolor = \"blue\""),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            parse("[panel]\noutputs = [\"\"]"),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            parse("[night_light]\ntemperature = 9000"),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            Config::from_toml("[wallpaper]\nimage = \"~/a.png\"", None),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            parse("[dash]\nicon_size = 40"),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            parse("[dash]\nfavorites = [\"/usr/share/applications/a.desktop\"]"),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            parse("[search]\ndisabled = [\"\"]"),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn missing_file_is_defaults() {
        let config = Config::load(Path::new("/nonexistent/gniri/config.toml")).unwrap();
        assert_eq!(config, Config::default());
    }
}
