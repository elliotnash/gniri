//! Sound: the default output and input, and the devices to choose from.
//!
//! [`backend`] talks to the PulseAudio server (pipewire-pulse on Fedora).

pub mod backend;

use serde::Deserialize;

use crate::model::Model;

/// How far one press of a volume key moves the volume
/// (gnome-settings-daemon's `VOLUME_STEP`, 6%).
pub const VOLUME_STEP: f64 = 0.06;

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct State {
    /// The default output, if there is one.
    pub output: Option<Output>,
    /// Every output device, for the menu beside the slider.
    pub outputs: Vec<Device>,
    /// The default input, if there is one.
    pub input: Option<Input>,
    pub inputs: Vec<Device>,
    /// Some application is recording. GNOME shows the input slider and the
    /// microphone indicator only then.
    pub recording: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Output {
    /// 1.0 is 100%; more is over-amplified.
    pub volume: f64,
    #[serde(default)]
    pub muted: bool,
    /// Headphones or a headset are plugged in: GNOME swaps the slider icon.
    #[serde(default)]
    pub headphones: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub volume: f64,
    #[serde(default)]
    pub muted: bool,
}

/// An output or input device in a device menu.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Device {
    /// The server's name for it; what [`Command::SelectOutput`] takes.
    pub name: String,
    /// What the menu shows, such as "Speakers – Built-in Audio".
    pub description: String,
    /// It is the default device.
    #[serde(default)]
    pub active: bool,
}

/// What the backend reports.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Everything, as it is now.
    State(State),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    SetOutputVolume(f64),
    SetOutputMuted(bool),
    /// Make the device with this name the default output.
    SelectOutput(String),
    SetInputVolume(f64),
    SetInputMuted(bool),
    SelectInput(String),
}

fn level_icon(icons: &[&'static str; 5], volume: f64, muted: bool) -> &'static str {
    if muted || volume <= 0.0 {
        return icons[0];
    }
    let step = (3.0 * volume).ceil() as usize;
    icons[step.clamp(1, icons.len() - 1)]
}

impl Output {
    /// The panel and OSD icon. Port of `StreamSlider.getIcon` in
    /// `status/volume.js`.
    pub fn icon_name(&self) -> &'static str {
        const ICONS: [&str; 5] = [
            "audio-volume-muted-symbolic",
            "audio-volume-low-symbolic",
            "audio-volume-medium-symbolic",
            "audio-volume-high-symbolic",
            "audio-volume-overamplified-symbolic",
        ];
        level_icon(&ICONS, self.volume, self.muted)
    }

    /// The icon on the slider's mute button: the level icon, or the
    /// headphones when they are plugged in and not muted.
    pub fn slider_icon_name(&self) -> &'static str {
        if self.headphones && !self.muted && self.volume > 0.0 {
            "audio-headphones-symbolic"
        } else {
            self.icon_name()
        }
    }
}

impl Input {
    /// Port of `InputStreamSlider`'s icons in `status/volume.js`.
    pub fn icon_name(&self) -> &'static str {
        const ICONS: [&str; 5] = [
            "microphone-sensitivity-muted-symbolic",
            "microphone-sensitivity-low-symbolic",
            "microphone-sensitivity-medium-symbolic",
            "microphone-sensitivity-high-symbolic",
            "microphone-sensitivity-high-symbolic",
        ];
        level_icon(&ICONS, self.volume, self.muted)
    }
}

impl State {
    /// The command a volume key sends: one step up or down from the current
    /// output volume, never above 100%. `None` without an output.
    pub fn step_output(&self, steps: i32) -> Option<Command> {
        let output = self.output.as_ref()?;
        let volume = (output.volume + f64::from(steps) * VOLUME_STEP).clamp(0.0, 1.0);
        Some(Command::SetOutputVolume(volume))
    }

    /// The command the mute key sends.
    pub fn toggle_output_mute(&self) -> Option<Command> {
        let output = self.output.as_ref()?;
        Some(Command::SetOutputMuted(!output.muted))
    }
}

fn select(devices: &mut [Device], name: &str) {
    if devices.iter().any(|device| device.name == name) {
        for device in devices {
            device.active = device.name == name;
        }
    }
}

impl Model for State {
    type Event = Event;
    type Command = Command;

    fn apply(&mut self, event: Event) {
        match event {
            Event::State(state) => *self = state,
        }
    }

    fn predict(&mut self, command: &Command) {
        match command {
            Command::SetOutputVolume(volume) => {
                if let Some(output) = &mut self.output {
                    output.volume = volume.max(0.0);
                }
            }
            Command::SetOutputMuted(muted) => {
                if let Some(output) = &mut self.output {
                    output.muted = *muted;
                }
            }
            Command::SelectOutput(name) => select(&mut self.outputs, name),
            Command::SetInputVolume(volume) => {
                if let Some(input) = &mut self.input {
                    input.volume = volume.max(0.0);
                }
            }
            Command::SetInputMuted(muted) => {
                if let Some(input) = &mut self.input {
                    input.muted = *muted;
                }
            }
            Command::SelectInput(name) => select(&mut self.inputs, name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(volume: f64, muted: bool) -> Output {
        Output {
            volume,
            muted,
            headphones: false,
        }
    }

    #[test]
    fn volume_icons() {
        let icon = |volume, muted| output(volume, muted).icon_name();
        assert_eq!(icon(0.0, false), "audio-volume-muted-symbolic");
        assert_eq!(icon(0.8, true), "audio-volume-muted-symbolic");
        assert_eq!(icon(0.01, false), "audio-volume-low-symbolic");
        assert_eq!(icon(1.0 / 3.0, false), "audio-volume-low-symbolic");
        assert_eq!(icon(0.5, false), "audio-volume-medium-symbolic");
        assert_eq!(icon(1.0, false), "audio-volume-high-symbolic");
        assert_eq!(icon(1.2, false), "audio-volume-overamplified-symbolic");
        assert_eq!(icon(5.0, false), "audio-volume-overamplified-symbolic");
    }

    #[test]
    fn headphones_replace_the_slider_icon_unless_muted() {
        let mut output = output(0.5, false);
        output.headphones = true;
        assert_eq!(output.slider_icon_name(), "audio-headphones-symbolic");
        assert_eq!(output.icon_name(), "audio-volume-medium-symbolic");
        output.muted = true;
        assert_eq!(output.slider_icon_name(), "audio-volume-muted-symbolic");
    }

    #[test]
    fn volume_keys_step_and_stop_at_the_ends() {
        let mut state = State {
            output: Some(output(0.97, false)),
            ..State::default()
        };
        assert_eq!(state.step_output(1), Some(Command::SetOutputVolume(1.0)));
        state.predict(&Command::SetOutputVolume(0.03));
        assert_eq!(state.step_output(-1), Some(Command::SetOutputVolume(0.0)));
        assert_eq!(
            state.toggle_output_mute(),
            Some(Command::SetOutputMuted(true))
        );
        assert_eq!(State::default().step_output(1), None);
    }

    #[test]
    fn selecting_a_device_moves_the_active_mark() {
        let device = |name: &str, active| Device {
            name: name.to_owned(),
            description: name.to_uppercase(),
            active,
        };
        let mut state = State {
            outputs: vec![device("speakers", true), device("hdmi", false)],
            ..State::default()
        };
        state.predict(&Command::SelectOutput("hdmi".to_owned()));
        assert_eq!(
            state.outputs,
            [device("speakers", false), device("hdmi", true)]
        );
        // An unknown name changes nothing.
        state.predict(&Command::SelectOutput("gone".to_owned()));
        assert!(state.outputs[1].active);
    }
}
