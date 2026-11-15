// SPDX-License-Identifier: GPL-3.0-or-later

//! The gniri stylesheet.
//!
//! The SCSS under `scss/` is compiled at build time. `scss/upstream/` holds
//! unmodified gnome-shell-sass files; everything else is written for GTK.

/// The compiled dark stylesheet, ready for `gtk::CssProvider`.
pub const DARK_CSS: &str = include_str!(concat!(env!("OUT_DIR"), "/gniri-dark.css"));

/// Design tokens as `(name, value)`, without the `--gniri-` prefix.
pub const TOKENS: &[(&str, &str)] = include!(concat!(env!("OUT_DIR"), "/tokens.rs"));

/// Looks up one design token.
pub fn token(name: &str) -> Option<&'static str> {
    TOKENS
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
            assert!(
                !DARK_CSS.contains(function),
                "{function} left in the compiled CSS"
            );
        }
    }
}
