// SPDX-License-Identifier: GPL-3.0-or-later

//! The mixer model: from what the server has to what the shell shows.
//!
//! GNOME Shell does not look at the PulseAudio server itself.
//! `status/volume.js` sits on libgnome-volume-control ("gvc"), which turns
//! the server's cards, ports, sinks and sources into the devices of the two
//! menus and decides what choosing one of them does. gvc's source was not at
//! hand when this was written: whatever reproduces it here does so **from
//! memory** of `gvc-mixer-control.c` and `gvc-mixer-ui-device.c` and is not
//! a line by line port. What follows `volume.js` is a port, and names the
//! function.
//!
//! Everything is a pure function of a [`Snapshot`], the server's objects
//! reduced to the fields that matter, so it is tested without a server:
//! [`Snapshot::state`] is what the shell is told, and [`Snapshot::actions`]
//! is what a command asks of the server.

use libpulse_binding::volume::{ChannelVolumes, Volume};

use super::{Command, Device, Input, Output, State};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Output,
    Input,
}

/// The server's objects, as of one round of queries.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub default_sink: Option<String>,
    pub default_source: Option<String>,
    pub cards: Vec<Card>,
    pub sinks: Vec<Stream>,
    /// Without the monitors of the sinks: gvc ignores them, so they are
    /// never a device and never the input.
    pub sources: Vec<Stream>,
    pub source_outputs: Vec<SourceOutput>,
}

/// A sound card: the thing with sockets on it.
#[derive(Clone, Debug)]
pub struct Card {
    pub index: u32,
    pub name: String,
    /// Its `device.description`, such as "Built-in Audio".
    pub description: String,
    pub active_profile: Option<String>,
    pub ports: Vec<Port>,
}

/// One socket (or built-in speaker or microphone) of a card.
#[derive(Clone, Debug)]
pub struct Port {
    pub name: String,
    /// Such as "Speakers" or "Headphones".
    pub description: String,
    pub direction: Direction,
    /// False when the server knows that nothing is plugged in.
    pub available: bool,
    /// The card profiles under which this port can be used.
    pub profiles: Vec<Profile>,
}

#[derive(Clone, Debug)]
pub struct Profile {
    pub name: String,
    pub priority: u32,
}

/// A sink or a source; gvc calls both a stream.
#[derive(Clone, Debug)]
pub struct Stream {
    pub name: String,
    pub description: String,
    /// The index of its card. Virtual and network devices have none.
    pub card: Option<u32>,
    pub volume: ChannelVolumes,
    pub muted: bool,
    /// Its `device.form_factor`.
    pub form_factor: Option<String>,
    /// The names of the ports it can play to or record from.
    pub ports: Vec<String>,
    pub active_port: Option<String>,
    /// Something is playing to it or recording from it right now.
    pub running: bool,
}

/// An application's recording stream.
#[derive(Clone, Debug, Default)]
pub struct SourceOutput {
    /// Its `application.id`.
    pub application_id: Option<String>,
    /// It only measures the level, as the meters of a mixer do.
    pub peak_detect: bool,
}

/// One request to the server.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    SetVolume {
        direction: Direction,
        stream: String,
        volume: ChannelVolumes,
    },
    SetMuted {
        direction: Direction,
        stream: String,
        muted: bool,
    },
    SwitchPort {
        direction: Direction,
        stream: String,
        port: String,
    },
    MakeDefault {
        direction: Direction,
        stream: String,
    },
    SwitchProfile {
        card: String,
        profile: String,
    },
}

/// The server's volume as the shell counts it: 1.0 is `PA_VOLUME_NORM`.
pub fn level(volume: Volume) -> f64 {
    f64::from(volume.0) / f64::from(Volume::NORMAL.0)
}

/// The server's volume for a level from the shell, to the nearest step and
/// within what a volume control may set (`PA_VOLUME_UI_MAX`).
pub fn volume(level: f64) -> Volume {
    // The cast saturates and takes NaN to zero.
    let volume = (level * f64::from(Volume::NORMAL.0)).round() as u32;
    Volume(volume.min(Volume::ui_max().0))
}

/// The line in a device menu. Port of `StreamSlider._addDevice`; gvc gives a
/// port's description and, as the origin, its card's.
pub fn label(description: &str, origin: &str) -> String {
    if origin.is_empty() {
        description.to_owned()
    } else {
        format!("{description} – {origin}")
    }
}

/// Port of `OutputStreamSlider._findHeadphones`.
pub fn has_headphones(sink: &Stream) -> bool {
    // "This only works for external headphones (e.g. bluetooth)".
    if matches!(sink.form_factor.as_deref(), Some("headset" | "headphone")) {
        return true;
    }
    // "A bit hackish, but ALSA/PulseAudio have a number of different
    // identifiers for headphones".
    sink.active_port
        .as_deref()
        .is_some_and(|port| port.to_lowercase().contains("headphone"))
}

/// Whether a recording stream is an application recording. `volume.js`
/// (`_maybeShowInput`) skips GNOME Settings and pavucontrol, "which appear as
/// recording because they show the input level". Level meters of other
/// mixers are skipped here as well, which upstream does not do.
pub fn counts_as_recording(output: &SourceOutput) -> bool {
    const SKIPPED_APPS: [&str; 2] = ["org.gnome.VolumeControl", "org.PulseAudio.pavucontrol"];
    let skipped = output
        .application_id
        .as_deref()
        .is_some_and(|id| SKIPPED_APPS.contains(&id));
    !skipped && !output.peak_detect
}

/// A profile's name without the parts for one direction:
/// `output:analog-stereo+input:analog-stereo` without `input:` is
/// `output:analog-stereo`. gvc's `get_profile_canonical_name`.
fn canonical_profile(name: &str, skip_prefix: &str) -> String {
    let parts: Vec<&str> = name
        .split('+')
        .filter(|part| !part.starts_with(skip_prefix))
        .collect();
    if parts.is_empty() {
        "off".to_owned()
    } else {
        parts.join("+")
    }
}

/// The profile to switch `card` to so that `port` can be used; `None` when
/// the port is in no profile at all. gvc's
/// `gvc_mixer_ui_device_get_best_profile`.
fn best_profile<'a>(card: &'a Card, port: &'a Port) -> Option<&'a str> {
    let current = card.active_profile.as_deref().unwrap_or_default();
    // Maybe nothing has to change.
    if let Some(profile) = port.profiles.iter().find(|profile| profile.name == current) {
        return Some(&profile.name);
    }
    // Then try to keep the other direction as it is: someone choosing
    // another output does not expect to lose the microphone.
    let own_prefix = match port.direction {
        Direction::Output => "output:",
        Direction::Input => "input:",
    };
    let other_side = canonical_profile(current, own_prefix);
    let keeps_other_side =
        |profile: &&Profile| canonical_profile(&profile.name, own_prefix) == other_side;
    // Failing that, the profile with the highest priority.
    highest(port.profiles.iter().filter(keeps_other_side))
        .or_else(|| highest(port.profiles.iter()))
        .map(|profile| profile.name.as_str())
}

/// The first of those with the highest priority.
fn highest<'a>(profiles: impl Iterator<Item = &'a Profile>) -> Option<&'a Profile> {
    profiles.fold(None, |best, profile| match best {
        Some(best) if best.priority >= profile.priority => Some(best),
        _ => Some(profile),
    })
}

/// What a line of a device menu stands for.
enum Target<'a> {
    /// A port of a card. Its sink or source exists only while the card's
    /// profile includes the port.
    Port { card: &'a Card, port: &'a Port },
    /// A sink or source that is a device by itself.
    Stream(&'a Stream),
}

struct Entry<'a> {
    device: Device,
    target: Target<'a>,
}

impl Snapshot {
    fn streams(&self, direction: Direction) -> &[Stream] {
        match direction {
            Direction::Output => &self.sinks,
            Direction::Input => &self.sources,
        }
    }

    fn stream_mut(&mut self, direction: Direction, name: &str) -> Option<&mut Stream> {
        let streams = match direction {
            Direction::Output => &mut self.sinks,
            Direction::Input => &mut self.sources,
        };
        streams.iter_mut().find(|stream| stream.name == name)
    }

    fn default_name(&self, direction: Direction) -> Option<&str> {
        match direction {
            Direction::Output => self.default_sink.as_deref(),
            Direction::Input => self.default_source.as_deref(),
        }
    }

    /// The default sink or source. A default the server names but does not
    /// list (a monitor as the default source, say) is none.
    fn default_stream(&self, direction: Direction) -> Option<&Stream> {
        let name = self.default_name(direction)?;
        self.streams(direction)
            .iter()
            .find(|stream| stream.name == name)
    }

    /// The sink or source that plays to or records from a port, if the
    /// card's profile has one. gvc's `match_stream_with_devices`.
    fn stream_of(&self, direction: Direction, card: &Card, port: &Port) -> Option<&Stream> {
        self.streams(direction)
            .iter()
            .find(|stream| stream.card == Some(card.index) && stream.ports.contains(&port.name))
    }

    /// Whether a sink or source is in the menu as ports of its card.
    fn is_on_card(&self, direction: Direction, stream: &Stream) -> bool {
        self.cards
            .iter()
            .filter(|card| stream.card == Some(card.index))
            .flat_map(|card| &card.ports)
            .any(|port| port.direction == direction && stream.ports.contains(&port.name))
    }

    /// The device menu, as gvc builds it (`create_ui_device_from_port`,
    /// `sync_devices`). There is one device for each port of each card
    /// unless the port is known to have nothing plugged in, whether or not
    /// the card's profile includes the port right now: "Headphones –
    /// Built-in Audio" beside "Speakers – Built-in Audio" are two ports of
    /// one sink, and "HDMI – Built-in Audio" may need the profile changed.
    /// A sink or source without a card (virtual or on the network) is a
    /// device under its own description.
    ///
    /// Departure from gvc: so is one whose card has no such ports (the
    /// sinks of PipeWire's Pro Audio profile), which gvc hangs on some
    /// other port of the card.
    fn entries(&self, direction: Direction) -> Vec<Entry<'_>> {
        let default = self.default_stream(direction);
        let mut entries = Vec::new();
        for card in &self.cards {
            let ports = card.ports.iter();
            for port in ports.filter(|port| port.direction == direction && port.available) {
                let active = default.is_some_and(|stream| {
                    stream.card == Some(card.index)
                        && stream.active_port.as_deref() == Some(port.name.as_str())
                });
                entries.push(Entry {
                    device: Device {
                        name: format!("{}:{}", card.name, port.name),
                        description: label(&port.description, &card.description),
                        active,
                    },
                    target: Target::Port { card, port },
                });
            }
        }
        let streams = self.streams(direction).iter();
        for stream in streams.filter(|stream| !self.is_on_card(direction, stream)) {
            entries.push(Entry {
                device: Device {
                    name: stream.name.clone(),
                    description: label(&stream.description, ""),
                    active: default.is_some_and(|default| default.name == stream.name),
                },
                target: Target::Stream(stream),
            });
        }
        entries
    }

    fn devices(&self, direction: Direction) -> Vec<Device> {
        self.entries(direction)
            .into_iter()
            .map(|entry| entry.device)
            .collect()
    }

    /// Port of `InputStreamSlider._maybeShowInput`: "only show input widgets
    /// if any application is recording audio".
    fn is_recording(&self) -> bool {
        self.sources.iter().any(|source| source.running)
            && self.source_outputs.iter().any(counts_as_recording)
    }

    /// What the shell is told.
    pub fn state(&self) -> State {
        let output = self.default_stream(Direction::Output).map(|sink| Output {
            // The loudest channel, as `gvc_mixer_stream_get_volume` gives.
            volume: level(sink.volume.max()),
            muted: sink.muted,
            headphones: has_headphones(sink),
        });
        let input = self.default_stream(Direction::Input).map(|source| Input {
            volume: level(source.volume.max()),
            muted: source.muted,
        });
        State {
            recording: input.is_some() && self.is_recording(),
            output,
            outputs: self.devices(Direction::Output),
            input,
            inputs: self.devices(Direction::Input),
        }
    }

    /// gvc's `gvc_mixer_stream_set_volume` and `push_volume`: every channel
    /// is scaled so that the loudest is at `level`, which keeps the balance.
    fn set_volume(&self, direction: Direction, level: f64) -> Vec<Action> {
        let Some(stream) = self.default_stream(direction) else {
            return Vec::new();
        };
        let wanted = volume(level);
        let mut scaled = stream.volume;
        // No channels to scale: the server gave no usable volume.
        if stream.volume.max() == wanted || scaled.scale(wanted).is_none() {
            return Vec::new();
        }
        vec![Action::SetVolume {
            direction,
            stream: stream.name.clone(),
            volume: scaled,
        }]
    }

    fn set_muted(&self, direction: Direction, muted: bool) -> Vec<Action> {
        self.default_stream(direction)
            .map(|stream| Action::SetMuted {
                direction,
                stream: stream.name.clone(),
                muted,
            })
            .into_iter()
            .collect()
    }

    /// Choosing a line of a device menu. gvc's
    /// `gvc_mixer_control_change_output` and `change_input`: switch the sink
    /// to the port, then make the sink the default. A port outside the
    /// card's profile has no sink yet; then the card changes profile, and
    /// the device is to be chosen again once its sink is there (see
    /// [`Snapshot::waits_for_profile`]).
    pub fn select(&self, direction: Direction, device: &str) -> Vec<Action> {
        let entries = self.entries(direction);
        let Some(entry) = entries.iter().find(|entry| entry.device.name == device) else {
            return Vec::new();
        };
        let (stream, port) = match entry.target {
            Target::Stream(stream) => (stream, None),
            Target::Port { card, port } => match self.stream_of(direction, card, port) {
                Some(stream) => (stream, Some(port)),
                None => {
                    let profile = best_profile(card, port)
                        .filter(|profile| card.active_profile.as_deref() != Some(*profile));
                    return profile
                        .map(|profile| Action::SwitchProfile {
                            card: card.name.clone(),
                            profile: profile.to_owned(),
                        })
                        .into_iter()
                        .collect();
                }
            },
        };
        let mut actions = Vec::new();
        if let Some(port) = port
            && stream.active_port.as_deref() != Some(port.name.as_str())
        {
            actions.push(Action::SwitchPort {
                direction,
                stream: stream.name.clone(),
                port: port.name.clone(),
            });
        }
        if self.default_name(direction) != Some(stream.name.as_str()) {
            actions.push(Action::MakeDefault {
                direction,
                stream: stream.name.clone(),
            });
        }
        actions
    }

    /// The device is a port that has no sink or source until its card has
    /// changed profile.
    pub fn waits_for_profile(&self, direction: Direction, device: &str) -> bool {
        self.entries(direction).iter().any(|entry| {
            entry.device.name == device
                && match entry.target {
                    Target::Port { card, port } => self.stream_of(direction, card, port).is_none(),
                    Target::Stream(_) => false,
                }
        })
    }

    /// What a command asks of the server.
    pub fn actions(&self, command: &Command) -> Vec<Action> {
        match command {
            Command::SetOutputVolume(level) => self.set_volume(Direction::Output, *level),
            Command::SetOutputMuted(muted) => self.set_muted(Direction::Output, *muted),
            Command::SelectOutput(device) => self.select(Direction::Output, device),
            Command::SetInputVolume(level) => self.set_volume(Direction::Input, *level),
            Command::SetInputMuted(muted) => self.set_muted(Direction::Input, *muted),
            Command::SelectInput(device) => self.select(Direction::Input, device),
        }
    }

    /// Assumes an action worked, so that the next command builds on it
    /// before the server's own account arrives: a slider being dragged sends
    /// volumes faster than they come back. A profile change is not assumed:
    /// only the server knows which sinks come of it.
    pub fn apply(&mut self, action: &Action) {
        match action {
            Action::SetVolume {
                direction,
                stream,
                volume,
            } => {
                if let Some(stream) = self.stream_mut(*direction, stream) {
                    stream.volume = *volume;
                }
            }
            Action::SetMuted {
                direction,
                stream,
                muted,
            } => {
                if let Some(stream) = self.stream_mut(*direction, stream) {
                    stream.muted = *muted;
                }
            }
            Action::SwitchPort {
                direction,
                stream,
                port,
            } => {
                if let Some(stream) = self.stream_mut(*direction, stream) {
                    stream.active_port = Some(port.clone());
                }
            }
            Action::MakeDefault { direction, stream } => {
                let default = match direction {
                    Direction::Output => &mut self.default_sink,
                    Direction::Input => &mut self.default_source,
                };
                *default = Some(stream.clone());
            }
            Action::SwitchProfile { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::MAX_AMPLIFIED_VOLUME;

    fn volumes(channels: &[u32]) -> ChannelVolumes {
        let mut volumes = ChannelVolumes::default();
        volumes.set_len(channels.len() as u8);
        for (volume, channel) in volumes.get_mut().iter_mut().zip(channels) {
            *volume = Volume(*channel);
        }
        volumes
    }

    fn stream(name: &str, description: &str) -> Stream {
        Stream {
            name: name.to_owned(),
            description: description.to_owned(),
            card: None,
            volume: volumes(&[65536, 65536]),
            muted: false,
            form_factor: None,
            ports: Vec::new(),
            active_port: None,
            running: false,
        }
    }

    fn on_card(mut stream: Stream, ports: &[&str], active: &str) -> Stream {
        stream.card = Some(7);
        stream.ports = ports.iter().map(|port| (*port).to_owned()).collect();
        stream.active_port = Some(active.to_owned());
        stream
    }

    fn port(name: &str, description: &str, direction: Direction, profiles: &[&str]) -> Port {
        const PRIORITIES: [(&str, u32); 4] = [
            ("output:analog-stereo+input:analog-stereo", 6565),
            ("output:analog-stereo", 6500),
            ("output:hdmi-stereo", 5900),
            ("output:hdmi-stereo+input:analog-stereo", 5800),
        ];
        Port {
            name: name.to_owned(),
            description: description.to_owned(),
            direction,
            available: true,
            profiles: PRIORITIES
                .iter()
                .filter(|(profile, _)| profiles.contains(profile))
                .map(|(profile, priority)| Profile {
                    name: (*profile).to_owned(),
                    priority: *priority,
                })
                .collect(),
        }
    }

    const DUPLEX: &str = "output:analog-stereo+input:analog-stereo";
    const HDMI_DUPLEX: &str = "output:hdmi-stereo+input:analog-stereo";
    const ANALOG: [&str; 2] = [DUPLEX, "output:analog-stereo"];
    const HDMI: [&str; 2] = [HDMI_DUPLEX, "output:hdmi-stereo"];

    /// A laptop: one card with speakers, a headphone socket with nothing in
    /// it, an HDMI socket that needs another profile and a microphone; and a
    /// network speaker without a card.
    fn laptop() -> Snapshot {
        let mut headphones = port(
            "analog-output-headphones",
            "Headphones",
            Direction::Output,
            &ANALOG,
        );
        headphones.available = false;
        let card = Card {
            index: 7,
            name: "alsa_card.laptop".to_owned(),
            description: "Built-in Audio".to_owned(),
            active_profile: Some(DUPLEX.to_owned()),
            ports: vec![
                port(
                    "analog-output-speaker",
                    "Speakers",
                    Direction::Output,
                    &ANALOG,
                ),
                headphones,
                port(
                    "hdmi-output-0",
                    "HDMI / DisplayPort",
                    Direction::Output,
                    &HDMI,
                ),
                port(
                    "analog-input-internal-mic",
                    "Internal Microphone",
                    Direction::Input,
                    &[DUPLEX, HDMI_DUPLEX],
                ),
            ],
        };
        let speakers = on_card(
            stream(
                "alsa_output.laptop.analog-stereo",
                "Built-in Audio Analog Stereo",
            ),
            &["analog-output-speaker", "analog-output-headphones"],
            "analog-output-speaker",
        );
        let microphone = on_card(
            stream(
                "alsa_input.laptop.analog-stereo",
                "Built-in Audio Analog Stereo",
            ),
            &["analog-input-internal-mic"],
            "analog-input-internal-mic",
        );
        Snapshot {
            default_sink: Some(speakers.name.clone()),
            default_source: Some(microphone.name.clone()),
            cards: vec![card],
            sinks: vec![speakers, stream("raop_sink.kitchen", "Kitchen")],
            sources: vec![microphone],
            source_outputs: Vec::new(),
        }
    }

    fn device(name: &str, description: &str, active: bool) -> Device {
        Device {
            name: name.to_owned(),
            description: description.to_owned(),
            active,
        }
    }

    #[test]
    fn levels_convert_to_and_from_the_servers_scale() {
        assert_eq!(level(Volume::NORMAL), 1.0);
        assert_eq!(level(Volume::MUTED), 0.0);
        assert_eq!(level(Volume(16384)), 0.25);
        assert_eq!(volume(1.0), Volume::NORMAL);
        assert_eq!(volume(0.0), Volume::MUTED);
        assert_eq!(volume(0.3), Volume(19661));
        // Every step survives the round trip.
        for steps in [1, 2, 19661, 65535, 65537, 99957] {
            assert_eq!(volume(level(Volume(steps))), Volume(steps));
        }
        // Never beyond what a volume control may set.
        assert_eq!(volume(MAX_AMPLIFIED_VOLUME), Volume::ui_max());
        assert_eq!(level(Volume::ui_max()), MAX_AMPLIFIED_VOLUME);
        assert_eq!(volume(40.0), Volume::ui_max());
        assert_eq!(volume(f64::INFINITY), Volume::ui_max());
        assert_eq!(volume(-0.5), Volume::MUTED);
        assert_eq!(volume(f64::NAN), Volume::MUTED);
    }

    #[test]
    fn labels_name_the_port_and_then_its_card() {
        assert_eq!(
            label("Speakers", "Built-in Audio"),
            "Speakers – Built-in Audio"
        );
        assert_eq!(label("Kitchen", ""), "Kitchen");
    }

    #[test]
    fn headphones_are_a_form_factor_or_a_port_name() {
        let mut sink = stream("sink", "Sink");
        assert!(!has_headphones(&sink));

        for (form_factor, expected) in [
            ("headset", true),
            ("headphone", true),
            ("speaker", false),
            ("internal", false),
        ] {
            sink.form_factor = Some(form_factor.to_owned());
            assert_eq!(has_headphones(&sink), expected, "{form_factor}");
        }

        for (port, expected) in [
            ("analog-output-headphones", true),
            ("[Out] Headphones", true),
            ("analog-output-speaker", false),
            ("hdmi-output-0", false),
        ] {
            sink.active_port = Some(port.to_owned());
            assert_eq!(has_headphones(&sink), expected, "{port}");
        }
    }

    #[test]
    fn mixers_and_level_meters_do_not_count_as_recording() {
        let from = |application_id: Option<&str>, peak_detect| SourceOutput {
            application_id: application_id.map(str::to_owned),
            peak_detect,
        };
        assert!(counts_as_recording(&from(None, false)));
        assert!(counts_as_recording(&from(
            Some("org.example.Recorder"),
            false
        )));
        assert!(!counts_as_recording(&from(
            Some("org.gnome.VolumeControl"),
            false
        )));
        assert!(!counts_as_recording(&from(
            Some("org.PulseAudio.pavucontrol"),
            false
        )));
        assert!(!counts_as_recording(&from(Some("org.example.Mixer"), true)));
    }

    #[test]
    fn recording_needs_a_running_source_and_a_real_recorder() {
        let mut laptop = laptop();
        let recorder = SourceOutput::default();
        let meter = SourceOutput {
            application_id: Some("org.gnome.VolumeControl".to_owned()),
            peak_detect: true,
        };
        assert!(!laptop.state().recording);

        // A recorder on a monitor: no microphone is running.
        laptop.source_outputs = vec![recorder.clone()];
        assert!(!laptop.state().recording);

        laptop.sources[0].running = true;
        assert!(laptop.state().recording);

        // Only the sound settings showing the input level.
        laptop.source_outputs = vec![meter.clone()];
        assert!(!laptop.state().recording);
        laptop.source_outputs = vec![meter, recorder];
        assert!(laptop.state().recording);

        // Without an input there is nothing to show.
        laptop.default_source = None;
        assert!(!laptop.state().recording);
    }

    #[test]
    fn the_state_is_the_default_devices_and_both_menus() {
        let mut laptop = laptop();
        laptop.sinks[0].volume = volumes(&[32768, 16384]);
        laptop.sources[0].muted = true;
        // The monitor of a sink is not listed, so the menu cannot offer it.
        assert_eq!(
            laptop.state(),
            State {
                output: Some(Output {
                    // The loudest channel.
                    volume: 0.5,
                    muted: false,
                    headphones: false,
                }),
                outputs: vec![
                    device(
                        "alsa_card.laptop:analog-output-speaker",
                        "Speakers – Built-in Audio",
                        true
                    ),
                    // Not the headphones: nothing is plugged in.
                    device(
                        "alsa_card.laptop:hdmi-output-0",
                        "HDMI / DisplayPort – Built-in Audio",
                        false
                    ),
                    device("raop_sink.kitchen", "Kitchen", false),
                ],
                input: Some(Input {
                    volume: 1.0,
                    muted: true,
                }),
                inputs: vec![device(
                    "alsa_card.laptop:analog-input-internal-mic",
                    "Internal Microphone – Built-in Audio",
                    true
                )],
                recording: false,
            }
        );
    }

    #[test]
    fn plugging_in_headphones_adds_a_device_and_swaps_the_icon() {
        let mut laptop = laptop();
        laptop.cards[0].ports[1].available = true;
        laptop.sinks[0].active_port = Some("analog-output-headphones".to_owned());
        let state = laptop.state();
        assert!(state.output.unwrap().headphones);
        assert_eq!(
            state.outputs[..2],
            [
                device(
                    "alsa_card.laptop:analog-output-speaker",
                    "Speakers – Built-in Audio",
                    false
                ),
                device(
                    "alsa_card.laptop:analog-output-headphones",
                    "Headphones – Built-in Audio",
                    true
                ),
            ]
        );
    }

    #[test]
    fn no_default_or_an_unlisted_one_is_no_device() {
        let mut laptop = laptop();
        laptop.default_sink = None;
        // What the server says without a microphone.
        laptop.default_source = Some("alsa_output.laptop.analog-stereo.monitor".to_owned());
        let state = laptop.state();
        assert_eq!(state.output, None);
        assert_eq!(state.input, None);
        assert!(state.outputs.iter().all(|device| !device.active));
        assert_eq!(Snapshot::default().state(), State::default());
    }

    #[test]
    fn a_sink_whose_card_has_no_such_port_is_its_own_device() {
        let mut laptop = laptop();
        let mut pro = stream("alsa_output.laptop.pro-output-0", "Built-in Audio Pro");
        pro.card = Some(7);
        laptop.default_sink = Some(pro.name.clone());
        laptop.sinks = vec![pro];
        assert_eq!(
            laptop.state().outputs,
            [
                device(
                    "alsa_card.laptop:analog-output-speaker",
                    "Speakers – Built-in Audio",
                    false
                ),
                device(
                    "alsa_card.laptop:hdmi-output-0",
                    "HDMI / DisplayPort – Built-in Audio",
                    false
                ),
                device(
                    "alsa_output.laptop.pro-output-0",
                    "Built-in Audio Pro",
                    true
                ),
            ]
        );
    }

    #[test]
    fn setting_the_volume_keeps_the_balance() {
        let mut laptop = laptop();
        laptop.sinks[0].volume = volumes(&[40000, 20000]);
        let actions = laptop.actions(&Command::SetOutputVolume(0.5));
        assert_eq!(
            actions,
            [Action::SetVolume {
                direction: Direction::Output,
                stream: "alsa_output.laptop.analog-stereo".to_owned(),
                volume: volumes(&[32768, 16384]),
            }]
        );
        laptop.apply(&actions[0]);
        assert_eq!(laptop.state().output.unwrap().volume, 0.5);
        // Nothing to push when it is there already.
        assert_eq!(laptop.actions(&Command::SetOutputVolume(0.5)), []);

        // Silence and back: there is no balance left to keep.
        let silence = laptop.actions(&Command::SetOutputVolume(0.0));
        laptop.apply(&silence[0]);
        assert_eq!(laptop.sinks[0].volume, volumes(&[0, 0]));
        assert_eq!(
            laptop.actions(&Command::SetOutputVolume(1.0)),
            [Action::SetVolume {
                direction: Direction::Output,
                stream: "alsa_output.laptop.analog-stereo".to_owned(),
                volume: volumes(&[65536, 65536]),
            }]
        );

        // Never louder than the amplified maximum.
        assert_eq!(
            laptop.actions(&Command::SetInputVolume(7.0)),
            [Action::SetVolume {
                direction: Direction::Input,
                stream: "alsa_input.laptop.analog-stereo".to_owned(),
                volume: volumes(&[99957, 99957]),
            }]
        );
    }

    #[test]
    fn muting_goes_to_the_default_device() {
        let mut laptop = laptop();
        let actions = laptop.actions(&Command::SetInputMuted(true));
        assert_eq!(
            actions,
            [Action::SetMuted {
                direction: Direction::Input,
                stream: "alsa_input.laptop.analog-stereo".to_owned(),
                muted: true,
            }]
        );
        laptop.apply(&actions[0]);
        assert!(laptop.state().input.unwrap().muted);
        assert!(!laptop.state().output.unwrap().muted);

        laptop.default_sink = None;
        assert_eq!(laptop.actions(&Command::SetOutputMuted(true)), []);
        assert_eq!(laptop.actions(&Command::SetOutputVolume(0.2)), []);
    }

    #[test]
    fn selecting_a_device_without_a_card_makes_it_the_default() {
        let mut laptop = laptop();
        let select = Command::SelectOutput("raop_sink.kitchen".to_owned());
        let actions = laptop.actions(&select);
        assert_eq!(
            actions,
            [Action::MakeDefault {
                direction: Direction::Output,
                stream: "raop_sink.kitchen".to_owned(),
            }]
        );
        laptop.apply(&actions[0]);
        assert!(laptop.state().outputs[2].active);
        assert_eq!(laptop.actions(&select), []);
        assert_eq!(
            laptop.actions(&Command::SelectOutput("gone".to_owned())),
            []
        );
        // An input's name is not an output.
        assert_eq!(
            laptop.actions(&Command::SelectOutput(
                "alsa_card.laptop:analog-input-internal-mic".to_owned()
            )),
            []
        );
    }

    #[test]
    fn selecting_another_port_of_a_sink_switches_the_port() {
        let mut laptop = laptop();
        laptop.cards[0].ports[1].available = true;
        let headphones = "alsa_card.laptop:analog-output-headphones";
        let set_port = Action::SwitchPort {
            direction: Direction::Output,
            stream: "alsa_output.laptop.analog-stereo".to_owned(),
            port: "analog-output-headphones".to_owned(),
        };
        assert!(!laptop.waits_for_profile(Direction::Output, headphones));
        assert_eq!(
            laptop.select(Direction::Output, headphones),
            std::slice::from_ref(&set_port)
        );

        // From another default it is the port and then the sink.
        laptop.default_sink = Some("raop_sink.kitchen".to_owned());
        let actions = laptop.select(Direction::Output, headphones);
        assert_eq!(
            actions,
            [
                set_port,
                Action::MakeDefault {
                    direction: Direction::Output,
                    stream: "alsa_output.laptop.analog-stereo".to_owned(),
                }
            ]
        );
        for action in &actions {
            laptop.apply(action);
        }
        assert!(laptop.state().outputs[1].active);
        assert!(laptop.state().output.unwrap().headphones);
    }

    #[test]
    fn selecting_a_port_outside_the_profile_switches_the_profile_first() {
        let mut laptop = laptop();
        let hdmi = "alsa_card.laptop:hdmi-output-0";
        assert!(laptop.waits_for_profile(Direction::Output, hdmi));
        // The profile that keeps the microphone, not the one of the highest
        // priority.
        let actions = laptop.select(Direction::Output, hdmi);
        assert_eq!(
            actions,
            [Action::SwitchProfile {
                card: "alsa_card.laptop".to_owned(),
                profile: HDMI_DUPLEX.to_owned(),
            }]
        );
        // Nothing is assumed about what the server makes of it.
        laptop.apply(&actions[0]);
        assert!(laptop.waits_for_profile(Direction::Output, hdmi));

        // The card has switched, and its new sink is not the default yet.
        laptop.cards[0].active_profile = Some(HDMI_DUPLEX.to_owned());
        laptop.sinks[0] = on_card(
            stream("alsa_output.laptop.hdmi-stereo", "Built-in Audio HDMI"),
            &["hdmi-output-0"],
            "hdmi-output-0",
        );
        laptop.default_sink = Some("raop_sink.kitchen".to_owned());
        assert!(!laptop.waits_for_profile(Direction::Output, hdmi));
        assert_eq!(
            laptop.select(Direction::Output, hdmi),
            [Action::MakeDefault {
                direction: Direction::Output,
                stream: "alsa_output.laptop.hdmi-stereo".to_owned(),
            }]
        );
        assert!(!laptop.waits_for_profile(Direction::Output, "raop_sink.kitchen"));
        assert!(!laptop.waits_for_profile(Direction::Output, "gone"));
    }

    #[test]
    fn the_best_profile_changes_as_little_as_it_can() {
        let laptop = laptop();
        let mut card = laptop.cards[0].clone();
        let speakers = card.ports[0].clone();
        let hdmi = card.ports[2].clone();
        let microphone = card.ports[3].clone();

        // The current profile will do.
        assert_eq!(best_profile(&card, &speakers), Some(DUPLEX));
        // The other direction stays.
        assert_eq!(best_profile(&card, &hdmi), Some(HDMI_DUPLEX));
        card.active_profile = Some("output:analog-stereo".to_owned());
        assert_eq!(best_profile(&card, &hdmi), Some("output:hdmi-stereo"));
        card.active_profile = Some("output:hdmi-stereo".to_owned());
        assert_eq!(best_profile(&card, &microphone), Some(HDMI_DUPLEX));
        card.active_profile = Some("off".to_owned());
        assert_eq!(best_profile(&card, &hdmi), Some("output:hdmi-stereo"));
        // No profile has a microphone and no output: the highest priority.
        assert_eq!(best_profile(&card, &microphone), Some(DUPLEX));
        card.active_profile = None;
        assert_eq!(best_profile(&card, &speakers), Some(DUPLEX));

        let mut orphan = hdmi;
        orphan.profiles.clear();
        assert_eq!(best_profile(&card, &orphan), None);
    }

    #[test]
    fn canonical_profile_names_drop_one_direction() {
        assert_eq!(canonical_profile(DUPLEX, "input:"), "output:analog-stereo");
        assert_eq!(canonical_profile(DUPLEX, "output:"), "input:analog-stereo");
        assert_eq!(canonical_profile("output:hdmi-stereo", "output:"), "off");
        assert_eq!(
            canonical_profile("HiFi (Speaker)", "output:"),
            "HiFi (Speaker)"
        );
    }
}
