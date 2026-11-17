// SPDX-License-Identifier: GPL-3.0-or-later

//! The gniri stylesheet.
//!
//! The SCSS under `scss/` is compiled at build time. `scss/upstream/` holds
//! unmodified gnome-shell-sass files; everything else is written for GTK.

/// The compiled dark stylesheet, ready for `gtk::CssProvider`.
pub const DARK_CSS: &str = include_str!(concat!(env!("OUT_DIR"), "/gniri-dark.css"));

/// The compiled light stylesheet.
pub const LIGHT_CSS: &str = include_str!(concat!(env!("OUT_DIR"), "/gniri-light.css"));

/// Design tokens as `(name, value)`, without the `--gniri-` prefix.
pub const TOKENS: &[(&str, &str)] = include!(concat!(env!("OUT_DIR"), "/tokens.rs"));

/// The same of the light stylesheet.
pub const LIGHT_TOKENS: &[(&str, &str)] = include!(concat!(env!("OUT_DIR"), "/tokens-light.rs"));

/// Which of the two stylesheets: GNOME Shell's "style variant".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Variant {
    #[default]
    Dark,
    Light,
}

impl Variant {
    pub fn css(self) -> &'static str {
        match self {
            Self::Dark => DARK_CSS,
            Self::Light => LIGHT_CSS,
        }
    }
}

/// Looks up one design token of the dark stylesheet.
pub fn token(name: &str) -> Option<&'static str> {
    lookup(TOKENS, name)
}

/// Looks up one design token of the light stylesheet.
pub fn light_token(name: &str) -> Option<&'static str> {
    lookup(LIGHT_TOKENS, name)
}

fn lookup(tokens: &'static [(&str, &str)], name: &str) -> Option<&'static str> {
    tokens
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, value)| *value)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tier 0 of the visual checks: base tokens against GNOME Shell 51.1.
    ///
    /// Each expected value was read from the running shell or its compiled
    /// `gnome-shell-dark.css`; the comment names where it appears. A mismatch
    /// means our Sass compiler disagrees with the one GNOME used.
    #[test]
    fn base_tokens_match_gnome_shell_51() {
        let expected = [
            ("panel-bg-color", "#000000"),          // #panel background-color
            ("bg-color", "#36363a"),                // .popup-menu-content background
            ("fg-color", "#ffffff"),                // stage color
            ("outer-borders-color", "#424247"),     // .popup-menu-content border
            ("card-bg-color", "#47474c"),           // .message, calendar cards
            ("osd-bg-color", "#2e2e33"),            // .osd-window background
            ("system-base-color", "#222226"),       // #overviewGroup background
            ("system-fg-color", "#fafafb"),         // overview text, running dot
            ("system-overlay-bg-color", "#38383b"), // .dash-background
            ("warning-color", "#cd9309"),           // .end-session-dialog battery warning
        ];
        for (name, value) in expected {
            assert_eq!(token(name), Some(value), "token {name}");
        }
    }

    /// The same for the light stylesheet, against GNOME Shell 51.1's
    /// compiled `gnome-shell-light.css`. GNOME was never captured in its
    /// light style, so there are no computed values to go by.
    #[test]
    fn light_base_tokens_match_gnome_shell_51() {
        let expected = [
            ("bg-color", "#fafafb"),            // .popup-menu-content background
            ("fg-color", "#222226"),            // .message color
            ("outer-borders-color", "#e6e6eb"), // .popup-menu-content border
            ("card-bg-color", "#ffffff"),       // .message background
            // What is dark whatever the style: the display and the overview.
            ("osd-bg-color", "#2e2e33"),      // .osd-window background
            ("system-base-color", "#222226"), // #overviewGroup background
            ("system-fg-color", "#fafafb"),
        ];
        for (name, value) in expected {
            assert_eq!(light_token(name), Some(value), "token {name}");
        }
        // Upstream's light accent is the accent itself.
        assert_eq!(light_token("accent-color"), Some("var(--accent-bg-color)"));
    }

    /// blur-my-shell keeps its blur as it is on the light style and
    /// changes the tint: rgba(255,255,255,0.58) for the bar, folded like
    /// the dark one.
    #[test]
    fn the_light_tint_is_folded_too() {
        assert_eq!(
            light_token("bms-panel-tint"),
            Some("rgba(255, 255, 255, 0.58)")
        );
        let folded = light_token("bms-surface-color").expect("bms-surface-color token");
        let numbers: Vec<f64> = folded
            .trim_start_matches("rgba(")
            .trim_end_matches(')')
            .split(',')
            .map(|part| part.trim().parse().expect("a number"))
            .collect();
        let [r, g, b, a] = numbers[..] else {
            panic!("unexpected colour {folded}")
        };
        assert!((a - (1.0 - 0.42 * 0.6)).abs() < 1e-9, "{folded}");
        for channel in [r, g, b] {
            assert!((a * channel - 0.58 * 255.0).abs() < 0.5, "{folded}");
        }
        // Black on it where the dark style has white.
        assert_eq!(
            light_token("bms-popup-fg-color"),
            Some("rgba(0, 0, 0, 0.85)")
        );
        assert_eq!(
            token("bms-popup-fg-color"),
            Some("rgba(255, 255, 255, 0.9)")
        );
    }

    #[test]
    fn accent_tokens_resolve_at_runtime() {
        // The accent is not known at build time, so it must stay a CSS
        // expression over libadwaita's accent variable.
        let accent = token("accent-color").expect("accent-color token");
        assert!(accent.contains("var(--accent-bg-color)"), "got {accent}");
        assert!(accent.starts_with("color-mix("), "got {accent}");
    }

    /// blur-my-shell tints rgba(30,30,33,0.5) over a backdrop dimmed to 0.6.
    /// Folded for an undimmed backdrop that is alpha 0.7 and colour 30*0.5/0.7.
    #[test]
    fn blur_my_shell_tint_is_folded_for_an_undimmed_backdrop() {
        let folded = token("bms-surface-color").expect("bms-surface-color token");
        let numbers: Vec<f64> = folded
            .trim_start_matches("rgba(")
            .trim_end_matches(')')
            .split(',')
            .map(|part| part.trim().parse().expect("a number"))
            .collect();
        let [r, g, b, a] = numbers[..] else {
            panic!("unexpected colour {folded}")
        };
        for (channel, extension) in [(r, 30.0), (g, 30.0), (b, 33.0)] {
            // Over black the two formulas must agree: a' * T' == a * T.
            assert!((a * channel - 0.5 * extension).abs() < 0.5, "{folded}");
        }
        assert!((a - 0.7).abs() < 1e-9, "{folded}");
    }

    #[test]
    fn no_st_functions_survive_compilation() {
        for function in [
            "st-mix(",
            "st-lighten(",
            "st-darken(",
            "st-transparentize(",
            "-st-accent",
        ] {
            for css in [DARK_CSS, LIGHT_CSS] {
                assert!(
                    !css.contains(function),
                    "{function} left in the compiled CSS"
                );
            }
        }
    }
}
