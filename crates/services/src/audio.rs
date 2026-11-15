//! Sound: the default output and input, and the devices to choose from.
//!
//! [`backend`] talks to the PulseAudio server (pipewire-pulse on Fedora) and
//! `mixer` turns what the server says into the [`State`] here. The logic in
//! this file is what GNOME Shell's `status/volume.js` does with that state.
//!
//! The commands are the server's primitives: setting the volume does not
//! touch the mute switch. What a slider or its icon does on top of that
//! (the bottom of the slider mutes) is in [`State::slide_output`] and
//! friends, which return the commands to send.

pub mod backend;
mod mixer;

use serde::Deserialize;

use crate::model::Model;

/// How far one press of a volume key moves the volume
/// (gnome-settings-daemon's `VOLUME_STEP`, 6%).
pub const VOLUME_STEP: f64 = 0.06;

/// The loudest an amplified volume goes: PulseAudio's `PA_VOLUME_UI_MAX`
/// (+11 dB, about 153%), which is gvc's `get_vol_max_amplified`. The backend
/// never sets more.
pub const MAX_AMPLIFIED_VOLUME: f64 = 99957.0 / 65536.0;

/// What unmuting a silent device sets it to (`UNMUTE_DEFAULT_VOLUME` in
/// `status/volume.js`).
pub const UNMUTE_DEFAULT_VOLUME: f64 = 0.25;

/// The server counts volume in 65536ths (`PA_VOLUME_NORM`); upstream treats
/// less than one of them as silence.
const VOLUME_UNIT: f64 = 1.0 / 65536.0;

/// Where a volume slider ends. Port of `StreamSlider.getMaxLevel`:
/// `allow_amplified` is `allow-volume-above-100-percent` in
/// `org.gnome.desktop.sound`, which also puts a mark on the slider at 1.0.
pub fn max_volume(allow_amplified: bool) -> f64 {
    if allow_amplified {
        MAX_AMPLIFIED_VOLUME
    } else {
        1.0
    }
}

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
    /// Some application is recording from an input. GNOME shows the input
    /// slider and the microphone indicator only then.
    pub recording: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Output {
    /// 1.0 is 100%; more is over-amplified.
    pub volume: f64,
    #[serde(default)]
    pub muted: bool,
    /// The sound comes out of headphones or a headset: GNOME swaps the
    /// slider icon.
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
    /// What [`Command::SelectOutput`] takes. The backend makes it up (one
    /// sink is a device for each of its ports); the server does not know it.
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
    /// Sets the loudest channel to this and keeps the balance; at most
    /// [`MAX_AMPLIFIED_VOLUME`]. The mute switch stays as it is.
    SetOutputVolume(f64),
    SetOutputMuted(bool),
    /// Make the device with this name the default output.
    SelectOutput(String),
    SetInputVolume(f64),
    SetInputMuted(bool),
    SelectInput(String),
}

/// Port of `StreamSlider.getIcon`: the first icon is for silence, the next
/// three share out the volume in thirds, and a fifth, where there is one, is
/// for volumes above 100%.
fn level_icon(icons: &[&'static str], volume: f64, muted: bool) -> &'static str {
    if muted || volume <= 0.0 {
        return icons[0];
    }
    let step = (3.0 * volume).ceil() as usize;
    icons[step.clamp(1, icons.len() - 1)]
}

/// Port of `StreamSlider._updateVolume`: a muted stream shows an empty
/// slider, whatever its volume.
fn slider_value(volume: f64, muted: bool) -> f64 {
    if muted { 0.0 } else { volume }
}

/// Port of `StreamSlider._sliderChanged`: the bottom of the slider mutes and
/// anywhere else unmutes.
fn slide(
    value: f64,
    muted: bool,
    set_volume: fn(f64) -> Command,
    set_muted: fn(bool) -> Command,
) -> Vec<Command> {
    let silent = value < VOLUME_UNIT;
    let mut commands = vec![set_volume(if silent { 0.0 } else { value })];
    if muted != silent {
        commands.push(set_muted(silent));
    }
    commands
}

/// Port of the `icon-clicked` handler in `StreamSlider`: the mute switch,
/// except that unmuting a silent stream also makes it audible.
fn click_icon(
    volume: f64,
    muted: bool,
    set_volume: fn(f64) -> Command,
    set_muted: fn(bool) -> Command,
) -> Vec<Command> {
    let mut commands = Vec::new();
    if muted && volume < VOLUME_UNIT {
        commands.push(set_volume(UNMUTE_DEFAULT_VOLUME));
    }
    commands.push(set_muted(!muted));
    commands
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

    /// The icon on the slider's mute button. Port of
    /// `OutputStreamSlider._updateIcon`: with headphones it is the
    /// headphones at any volume, muted or not.
    pub fn slider_icon_name(&self) -> &'static str {
        if self.headphones {
            "audio-headphones-symbolic"
        } else {
            self.icon_name()
        }
    }

    /// Where the slider's handle is.
    pub fn slider_value(&self) -> f64 {
        slider_value(self.volume, self.muted)
    }
}

impl Input {
    /// Port of `InputStreamSlider`'s icons in `status/volume.js`.
    pub fn icon_name(&self) -> &'static str {
        const ICONS: [&str; 4] = [
            "microphone-sensitivity-muted-symbolic",
            "microphone-sensitivity-low-symbolic",
            "microphone-sensitivity-medium-symbolic",
            "microphone-sensitivity-high-symbolic",
        ];
        level_icon(&ICONS, self.volume, self.muted)
    }

    /// Where the slider's handle is.
    pub fn slider_value(&self) -> f64 {
        slider_value(self.volume, self.muted)
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

    /// The commands for the output slider moving to `value`: its bottom
    /// mutes, anywhere else unmutes. Nothing without an output.
    pub fn slide_output(&self, value: f64) -> Vec<Command> {
        self.output.as_ref().map_or_else(Vec::new, |output| {
            slide(
                value,
                output.muted,
                Command::SetOutputVolume,
                Command::SetOutputMuted,
            )
        })
    }

    /// As [`State::slide_output`], for the input slider.
    pub fn slide_input(&self, value: f64) -> Vec<Command> {
        self.input.as_ref().map_or_else(Vec::new, |input| {
            slide(
                value,
                input.muted,
                Command::SetInputVolume,
                Command::SetInputMuted,
            )
        })
    }

    /// The commands for a click on the output slider's icon: the mute
    /// switch, and a quarter of the volume when unmuting silence.
    pub fn click_output_icon(&self) -> Vec<Command> {
        self.output.as_ref().map_or_else(Vec::new, |output| {
            click_icon(
                output.volume,
                output.muted,
                Command::SetOutputVolume,
                Command::SetOutputMuted,
            )
        })
    }

    /// As [`State::click_output_icon`], for the input slider.
    pub fn click_input_icon(&self) -> Vec<Command> {
        self.input.as_ref().map_or_else(Vec::new, |input| {
            click_icon(
                input.volume,
                input.muted,
                Command::SetInputVolume,
                Command::SetInputMuted,
            )
        })
    }

    /// GNOME shows the input slider and the microphone indicator only while
    /// something records (`InputStreamSlider._shouldBeVisible`).
    pub fn shows_input(&self) -> bool {
        self.input.is_some() && self.recording
    }

    /// A device menu is offered only when there is a choice
    /// (`StreamSlider._sync`).
    pub fn has_output_menu(&self) -> bool {
        self.outputs.len() > 1
    }

    pub fn has_input_menu(&self) -> bool {
        self.inputs.len() > 1
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
                    output.volume = volume.clamp(0.0, MAX_AMPLIFIED_VOLUME);
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
                    input.volume = volume.clamp(0.0, MAX_AMPLIFIED_VOLUME);
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
    fn microphone_icons_have_no_amplified_level() {
        let icon = |volume, muted| Input { volume, muted }.icon_name();
        assert_eq!(icon(0.0, false), "microphone-sensitivity-muted-symbolic");
        assert_eq!(icon(0.9, true), "microphone-sensitivity-muted-symbolic");
        assert_eq!(icon(0.2, false), "microphone-sensitivity-low-symbolic");
        assert_eq!(icon(0.6, false), "microphone-sensitivity-medium-symbolic");
        assert_eq!(icon(0.7, false), "microphone-sensitivity-high-symbolic");
        assert_eq!(icon(1.4, false), "microphone-sensitivity-high-symbolic");
    }

    #[test]
    fn headphones_replace_the_slider_icon_but_not_the_panel_icon() {
        let mut output = output(0.5, false);
        output.headphones = true;
        assert_eq!(output.slider_icon_name(), "audio-headphones-symbolic");
        assert_eq!(output.icon_name(), "audio-volume-medium-symbolic");
        // Upstream keeps the headphones on the slider even when muted.
        output.muted = true;
        assert_eq!(output.slider_icon_name(), "audio-headphones-symbolic");
        assert_eq!(output.icon_name(), "audio-volume-muted-symbolic");
        output.headphones = false;
        assert_eq!(output.slider_icon_name(), "audio-volume-muted-symbolic");
    }

    #[test]
    fn a_muted_slider_is_empty() {
        assert_eq!(output(0.4, false).slider_value(), 0.4);
        assert_eq!(output(0.4, true).slider_value(), 0.0);
        let input = Input {
            volume: 0.7,
            muted: true,
        };
        assert_eq!(input.slider_value(), 0.0);
    }

    #[test]
    fn the_slider_ends_at_100_percent_unless_amplified() {
        assert_eq!(max_volume(false), 1.0);
        assert_eq!(max_volume(true), MAX_AMPLIFIED_VOLUME);
        assert!((MAX_AMPLIFIED_VOLUME - 1.5252).abs() < 0.0001);
    }

    #[test]
    fn the_bottom_of_the_slider_mutes_and_the_rest_unmutes() {
        let mut state = State {
            output: Some(output(0.4, false)),
            input: Some(Input {
                volume: 0.5,
                muted: true,
            }),
            ..State::default()
        };
        assert_eq!(state.slide_output(0.6), [Command::SetOutputVolume(0.6)]);
        assert_eq!(
            state.slide_output(0.0),
            [Command::SetOutputVolume(0.0), Command::SetOutputMuted(true)]
        );
        // Less than one of the server's steps is silence too.
        assert_eq!(
            state.slide_output(0.000_01),
            [Command::SetOutputVolume(0.0), Command::SetOutputMuted(true)]
        );
        for command in state.slide_output(0.0) {
            state.predict(&command);
        }
        assert_eq!(state.output, Some(output(0.0, true)));
        assert_eq!(state.slide_output(0.0), [Command::SetOutputVolume(0.0)]);
        assert_eq!(
            state.slide_output(0.2),
            [
                Command::SetOutputVolume(0.2),
                Command::SetOutputMuted(false)
            ]
        );

        assert_eq!(
            state.slide_input(0.8),
            [Command::SetInputVolume(0.8), Command::SetInputMuted(false)]
        );
        assert_eq!(State::default().slide_output(0.5), []);
        assert_eq!(State::default().slide_input(0.5), []);
    }

    #[test]
    fn the_slider_icon_toggles_mute_and_never_unmutes_into_silence() {
        let mut state = State {
            output: Some(output(0.4, false)),
            input: Some(Input {
                volume: 0.0,
                muted: true,
            }),
            ..State::default()
        };
        assert_eq!(state.click_output_icon(), [Command::SetOutputMuted(true)]);
        state.predict(&Command::SetOutputMuted(true));
        assert_eq!(state.click_output_icon(), [Command::SetOutputMuted(false)]);
        state.predict(&Command::SetOutputVolume(0.0));
        assert_eq!(
            state.click_output_icon(),
            [
                Command::SetOutputVolume(UNMUTE_DEFAULT_VOLUME),
                Command::SetOutputMuted(false)
            ]
        );
        assert_eq!(
            state.click_input_icon(),
            [
                Command::SetInputVolume(UNMUTE_DEFAULT_VOLUME),
                Command::SetInputMuted(false)
            ]
        );
        assert_eq!(State::default().click_output_icon(), []);
        assert_eq!(State::default().click_input_icon(), []);
    }

    #[test]
    fn predicted_volumes_stay_within_what_the_backend_sets() {
        let mut state = State {
            output: Some(output(0.4, false)),
            ..State::default()
        };
        state.predict(&Command::SetOutputVolume(9.0));
        assert_eq!(state.output, Some(output(MAX_AMPLIFIED_VOLUME, false)));
        state.predict(&Command::SetOutputVolume(-1.0));
        assert_eq!(state.output, Some(output(0.0, false)));
    }

    #[test]
    fn the_input_shows_only_while_something_records() {
        let device = |name: &str| Device {
            name: name.to_owned(),
            description: name.to_owned(),
            active: false,
        };
        let mut state = State {
            input: Some(Input {
                volume: 0.5,
                muted: false,
            }),
            inputs: vec![device("mic")],
            outputs: vec![device("speakers"), device("hdmi")],
            ..State::default()
        };
        assert!(!state.shows_input());
        state.recording = true;
        assert!(state.shows_input());
        state.input = None;
        assert!(!state.shows_input());
        assert!(state.has_output_menu());
        assert!(!state.has_input_menu());
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
