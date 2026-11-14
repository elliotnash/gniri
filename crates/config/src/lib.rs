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
    pub wallpaper: Wallpaper,
    pub clock: Clock,
    pub panel: Panel,
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
        if let Some(outputs) = &config.panel.outputs
            && outputs.iter().any(String::is_empty)
        {
            return Err(Error::Invalid(
                "panel.outputs: an output name cannot be empty".to_owned(),
            ));
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
    }

    #[test]
    fn reads_every_key() {
        let config = parse(
            r##"
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
            "##,
        )
        .unwrap();
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
            parse("[wallpaper]\ncolor = \"blue\""),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            parse("[panel]\noutputs = [\"\"]"),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            Config::from_toml("[wallpaper]\nimage = \"~/a.png\"", None),
            Err(Error::Invalid(_))
        ));
    }

    #[test]
    fn missing_file_is_defaults() {
        let config = Config::load(Path::new("/nonexistent/gniri/config.toml")).unwrap();
        assert_eq!(config, Config::default());
    }
}
