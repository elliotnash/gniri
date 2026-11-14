//! Sound output.
//!
//! M1 has the state and the indicator logic only; fixture scenes fill it in.
//! The PulseAudio backend arrives in M2.

use serde::Deserialize;

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct State {
    /// The default output, if there is one.
    pub output: Option<Output>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Output {
    /// 1.0 is 100%; more is over-amplified.
    pub volume: f64,
    #[serde(default)]
    pub muted: bool,
}

impl Output {
    /// The panel icon. Port of `StreamSlider.getIcon` in `status/volume.js`.
    pub fn icon_name(&self) -> &'static str {
        const ICONS: [&str; 5] = [
            "audio-volume-muted-symbolic",
            "audio-volume-low-symbolic",
            "audio-volume-medium-symbolic",
            "audio-volume-high-symbolic",
            "audio-volume-overamplified-symbolic",
        ];
        if self.muted || self.volume <= 0.0 {
            return ICONS[0];
        }
        let step = (3.0 * self.volume).ceil() as usize;
        ICONS[step.clamp(1, ICONS.len() - 1)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_icons() {
        let icon = |volume, muted| Output { volume, muted }.icon_name();
        assert_eq!(icon(0.0, false), "audio-volume-muted-symbolic");
        assert_eq!(icon(0.8, true), "audio-volume-muted-symbolic");
        assert_eq!(icon(0.01, false), "audio-volume-low-symbolic");
        assert_eq!(icon(1.0 / 3.0, false), "audio-volume-low-symbolic");
        assert_eq!(icon(0.5, false), "audio-volume-medium-symbolic");
        assert_eq!(icon(1.0, false), "audio-volume-high-symbolic");
        assert_eq!(icon(1.2, false), "audio-volume-overamplified-symbolic");
        assert_eq!(icon(5.0, false), "audio-volume-overamplified-symbolic");
    }
}
