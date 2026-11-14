//! What the volume and brightness keys show on the on-screen display.
//! Port of the `_showOsd`-style helpers gnome-settings-daemon's media-keys
//! plugin drives GNOME Shell's OSD with.

use gniri_services::{audio, brightness};

use crate::surfaces::osd::Content;

/// The volume display: the output's icon and its level; an empty bar while
/// muted. `None` without an output.
pub fn volume_osd(state: &audio::State) -> Option<Content> {
    let output = state.output.as_ref()?;
    let level = if output.muted { 0.0 } else { output.volume };
    Some(Content {
        icon: output.icon_name().to_owned(),
        label: None,
        // The bar ends at 100% unless the volume is already above it.
        level: Some((level, level.max(1.0))),
    })
}

/// The brightness display. `None` without a backlight.
pub fn brightness_osd(state: &brightness::State) -> Option<Content> {
    Some(Content {
        icon: "display-brightness-symbolic".to_owned(),
        label: None,
        level: Some((state.level?, 1.0)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_muted_output_shows_an_empty_bar() {
        let state = |volume, muted| audio::State {
            output: Some(audio::Output {
                volume,
                muted,
                headphones: false,
            }),
            ..audio::State::default()
        };
        let shown = volume_osd(&state(0.5, false)).unwrap();
        assert_eq!(shown.icon, "audio-volume-medium-symbolic");
        assert_eq!(shown.level, Some((0.5, 1.0)));

        let muted = volume_osd(&state(0.5, true)).unwrap();
        assert_eq!(muted.icon, "audio-volume-muted-symbolic");
        assert_eq!(muted.level, Some((0.0, 1.0)));

        assert_eq!(
            volume_osd(&state(1.2, false)).unwrap().level,
            Some((1.2, 1.2))
        );
        assert_eq!(volume_osd(&audio::State::default()), None);
    }

    #[test]
    fn brightness_needs_a_backlight() {
        assert_eq!(brightness_osd(&brightness::State::default()), None);
        let shown = brightness_osd(&brightness::State { level: Some(0.4) }).unwrap();
        assert_eq!(shown.level, Some((0.4, 1.0)));
    }
}
