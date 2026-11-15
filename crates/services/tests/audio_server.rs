// SPDX-License-Identifier: GPL-3.0-or-later

//! The audio backend against a private sound server.
//!
//! Each test starts its own `pipewire`, `wireplumber` and `pipewire-pulse`
//! in a fresh runtime directory, with two null sinks and two null sources
//! and no hardware, and points the backend at that server's socket. Nothing
//! here can reach the sound server of whoever runs the tests: the children
//! get an empty environment, a home of their own and no D-Bus, and every
//! client is given the private socket by name.
//!
//! Without those three programs the tests pass with a note. The parts that
//! look at the server from outside (`pactl`) or record from it (`parec`)
//! are skipped the same way when those are missing.
//!
//! The server has no sound cards, so the devices that are ports of a card,
//! and switching ports and profiles, are covered by the unit tests of
//! `audio/mixer.rs` only.

mod common;

use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use common::{Follower, TIMEOUT};
use gniri_services::audio::{self, Command as Audio, Device, MAX_AMPLIFIED_VOLUME, State};
use libpulse_binding::context::{Context, FlagSet as ContextFlags, State as ContextState};
use libpulse_binding::mainloop::standard::Mainloop;
use libpulse_binding::sample::{Format, Spec};
use libpulse_binding::stream::{FlagSet as StreamFlags, State as StreamState, Stream};
use libpulse_binding::time::MicroSeconds;

const SPEAKERS: &str = "gniri-test-speakers";
const TELEVISION: &str = "gniri-test-television";
const MICROPHONE: &str = "gniri-test-microphone";
const HEADSET: &str = "gniri-test-headset";

/// The devices: nodes of PipeWire's null sink, which needs no hardware.
/// The higher session priority is the default.
fn devices() -> String {
    let node = |name: &str, description: &str, class: &str, priority: u32| {
        format!(
            r#"    {{ factory = adapter
        args = {{
            factory.name     = support.null-audio-sink
            node.name        = "{name}"
            node.description = "{description}"
            media.class      = {class}
            audio.position   = [ FL FR ]
            priority.session = {priority}
            object.linger    = true
        }}
    }}
"#
        )
    };
    format!(
        "context.objects = [\n{}{}{}{}]\n",
        node(SPEAKERS, "Test Speakers", "Audio/Sink", 200),
        node(TELEVISION, "Test Television", "Audio/Sink", 100),
        node(MICROPHONE, "Test Microphone", "Audio/Source/Virtual", 200),
        node(HEADSET, "Test Headset", "Audio/Source/Virtual", 100),
    )
}

/// The session manager without its hardware monitors (it would open the
/// machine's sound cards, cameras and Bluetooth), without the session's
/// services and without saved state.
const SESSION_MANAGER: &str = "wireplumber.profiles = {
  gniri-test = {
    inherits = [ policy, mixin.systemwide-session, mixin.stateless ]
    hardware.audio = disabled
    hardware.bluetooth = disabled
    hardware.video-capture = disabled
  }
}
";

/// `sockaddr_un` holds 108 bytes, and this is the longest socket name.
const LONGEST_SOCKET: &str = "/runtime/pipewire-0-manager";

struct SoundServer {
    directory: PathBuf,
    /// In the order they were started.
    children: Vec<(&'static str, Child)>,
}

impl SoundServer {
    /// Starts PipeWire and its session manager, not yet the PulseAudio
    /// server. `None`, with a note, when the programs are not installed.
    fn start() -> Option<Self> {
        for program in ["pipewire", "wireplumber", "pipewire-pulse"] {
            if !is_installed(program) {
                eprintln!("skipped: {program} is not installed");
                return None;
            }
        }
        static COUNT: AtomicU32 = AtomicU32::new(0);
        let name = format!(
            "gniri-audio-{}-{}",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::Relaxed)
        );
        let Some(directory) = [std::env::temp_dir(), PathBuf::from("/tmp")]
            .into_iter()
            .map(|parent| parent.join(&name))
            .find(|directory| directory.as_os_str().len() + LONGEST_SOCKET.len() < 108)
        else {
            eprintln!("skipped: the temporary directory's path is too long for a socket");
            return None;
        };

        let mut private = std::fs::DirBuilder::new();
        private.recursive(true).mode(0o700);
        let write = |file: &str, text: &str| {
            let path = directory.join(file);
            private
                .create(path.parent().expect("a file in a directory"))
                .and_then(|()| std::fs::write(&path, text))
                .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
        };
        private
            .create(directory.join("runtime"))
            .expect("creating the runtime directory");
        write("config/pipewire/pipewire.conf.d/devices.conf", &devices());
        write(
            "config/wireplumber/wireplumber.conf.d/profile.conf",
            SESSION_MANAGER,
        );

        let mut server = Self {
            directory,
            children: Vec::new(),
        };
        server.spawn("pipewire", &[]);
        server.wait_for_socket("pipewire-0");
        server.spawn("wireplumber", &["--profile", "gniri-test"]);
        Some(server)
    }

    /// Starts the PulseAudio server and waits for its socket.
    fn start_pulse(&mut self) {
        self.spawn("pipewire-pulse", &[]);
        self.wait_for_socket("pulse/native");
    }

    /// Kills the PulseAudio server, as a crash would.
    fn stop_pulse(&mut self) {
        let position = self
            .children
            .iter()
            .position(|(program, _)| *program == "pipewire-pulse")
            .expect("the PulseAudio server was started");
        let (_, mut child) = self.children.remove(position);
        let _ = child.kill();
        let _ = child.wait();
        // The next one is waited for by its socket.
        let _ = std::fs::remove_file(self.directory.join("runtime/pulse/native"));
    }

    /// What the backend, `pactl` and `parec` are told to connect to.
    fn address(&self) -> String {
        format!("unix:{}/runtime/pulse/native", self.directory.display())
    }

    /// A command that sees nothing of the user's session: no environment
    /// but the private directories, and no D-Bus.
    fn command(&self, program: &str) -> Command {
        let directory = |name: &str| self.directory.join(name);
        let mut command = Command::new(program);
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &self.directory)
            .env("XDG_RUNTIME_DIR", directory("runtime"))
            .env("XDG_CONFIG_HOME", directory("config"))
            .env("XDG_STATE_HOME", directory("state"))
            .env("XDG_DATA_HOME", directory("data"))
            .env("XDG_CACHE_HOME", directory("cache"))
            .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent/bus")
            .env("DBUS_SYSTEM_BUS_ADDRESS", "unix:path=/nonexistent/bus")
            .env("PULSE_SERVER", self.address())
            .stdin(Stdio::null());
        command
    }

    fn spawn(&mut self, program: &'static str, arguments: &[&str]) {
        let log = std::fs::File::create(self.directory.join(format!("{program}.log")))
            .expect("creating a log file");
        let child = self
            .command(program)
            .args(arguments)
            .stdout(log.try_clone().expect("a second handle on the log"))
            .stderr(log)
            .spawn()
            .unwrap_or_else(|error| panic!("cannot run {program}: {error}"));
        self.children.push((program, child));
    }

    fn wait_for_socket(&mut self, name: &str) {
        let socket = self.directory.join("runtime").join(name);
        let deadline = Instant::now() + TIMEOUT;
        while !socket.exists() {
            for (program, child) in &mut self.children {
                if let Ok(Some(status)) = child.try_wait() {
                    let log = self.directory.join(format!("{program}.log"));
                    panic!(
                        "{program} ended ({status}):\n{}",
                        std::fs::read_to_string(log).unwrap_or_default()
                    );
                }
            }
            assert!(Instant::now() < deadline, "{name} never appeared");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Runs `pactl` on the private server and returns what it printed.
    /// `None`, with a note, when it is not installed.
    fn pactl(&self, arguments: &[&str]) -> Option<String> {
        if !is_installed("pactl") {
            eprintln!("partly skipped: pactl is not installed");
            return None;
        }
        let output = self
            .command("pactl")
            .arg("--server")
            .arg(self.address())
            .args(arguments)
            .output()
            .expect("pactl was found a moment ago");
        assert!(
            output.status.success(),
            "pactl {arguments:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Some(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// The channel volumes of a sink, in the server's steps.
    fn sink_volumes(&self, sink: &str) -> Option<Vec<u32>> {
        // "Volume: front-left: 26214 /  40% / -23.88 dB,   front-right: …"
        let output = self.pactl(&["get-sink-volume", sink])?;
        let channels = output.lines().next().unwrap_or_default().split(',');
        let volumes = channels.filter_map(|channel| {
            let (_, levels) = channel.rsplit_once(": ")?;
            levels.split_whitespace().next()?.parse().ok()
        });
        Some(volumes.collect())
    }

    /// Waits until something records from a source. Without `pactl` to ask,
    /// it waits a while.
    fn wait_until_running(&self, source: &str) {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let Some(sources) = self.pactl(&["list", "short", "sources"]) else {
                std::thread::sleep(Duration::from_millis(500));
                return;
            };
            let mut lines = sources.lines();
            if lines.any(|line| line.contains(source) && line.ends_with("RUNNING")) {
                return;
            }
            assert!(Instant::now() < deadline, "{source} never ran:\n{sources}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// A backend on the private server.
    fn backend(&self) -> Follower<State> {
        Follower::new(audio::backend::spawn_at(Some(self.address())))
    }
}

impl Drop for SoundServer {
    fn drop(&mut self) {
        // The clients of PipeWire first, PipeWire last.
        for (_, child) in self.children.iter_mut().rev() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn is_installed(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

/// A recorder that is killed when the test is done with it.
struct Recorder(Child);

impl Drop for Recorder {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A level meter, as a mixer shows beside a microphone: a recording stream
/// that asks for peaks only. It runs on a thread until it is dropped.
struct Meter {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Meter {
    /// Returns once the server has the stream.
    fn start(address: String, source: &'static str) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let ready = Arc::new(AtomicBool::new(false));
        let thread = std::thread::spawn({
            let (stop, ready) = (Arc::clone(&stop), Arc::clone(&ready));
            move || {
                let mut mainloop = Mainloop::new().expect("a main loop");
                let mut context = Context::new(&mainloop, "gniri-test-meter").expect("a context");
                context
                    .connect(Some(&address), ContextFlags::NOAUTOSPAWN, None)
                    .expect("connecting to the private server");
                // What pavucontrol asks for.
                let spec = Spec {
                    format: Format::F32le,
                    channels: 1,
                    rate: 25,
                };
                let mut stream = None;
                while !stop.load(Ordering::Relaxed) {
                    mainloop
                        .prepare(Some(MicroSeconds(20_000)))
                        .and_then(|()| mainloop.poll())
                        .and_then(|_| mainloop.dispatch())
                        .expect("the meter's main loop");
                    if stream.is_none() && context.get_state() == ContextState::Ready {
                        let mut new = Stream::new(&mut context, "Peak detect", &spec, None)
                            .expect("a stream");
                        new.connect_record(Some(source), None, StreamFlags::PEAK_DETECT)
                            .expect("recording from the private server");
                        stream = Some(new);
                    }
                    if let Some(stream) = &stream
                        && stream.get_state() == StreamState::Ready
                    {
                        ready.store(true, Ordering::Relaxed);
                    }
                }
            }
        });
        let meter = Self {
            stop,
            thread: Some(thread),
        };
        let deadline = Instant::now() + TIMEOUT;
        while !ready.load(Ordering::Relaxed) {
            assert!(Instant::now() < deadline, "the meter never connected");
            std::thread::sleep(Duration::from_millis(10));
        }
        meter
    }
}

impl Drop for Meter {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// PipeWire keeps volumes as linear factors, so a volume can come back a
/// step away from what was set.
fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.001
}

fn device(name: &str, description: &str, active: bool) -> Device {
    Device {
        name: name.to_owned(),
        description: description.to_owned(),
        active,
    }
}

fn output_volume(state: &State) -> f64 {
    state
        .output
        .as_ref()
        .map_or(f64::NAN, |output| output.volume)
}

fn is_complete(state: &State) -> bool {
    state.output.is_some()
        && state.input.is_some()
        && state.outputs.len() == 2
        && state.inputs.len() == 2
}

#[test]
fn the_devices_are_reported_and_commands_reach_the_server() {
    let Some(mut server) = SoundServer::start() else {
        return;
    };
    server.start_pulse();
    let mut shell = server.backend();
    // A second connection, to see that a change is on the server and not
    // only in the backend that made it.
    let mut witness = server.backend();

    shell.wait("the devices", is_complete);
    let speakers = device(SPEAKERS, "Test Speakers", true);
    let television = device(TELEVISION, "Test Television", false);
    assert_eq!(shell.state.outputs, [speakers, television]);
    assert_eq!(
        shell.state.inputs,
        [
            device(MICROPHONE, "Test Microphone", true),
            device(HEADSET, "Test Headset", false),
        ]
    );
    let output = shell.state.output.clone().unwrap();
    assert!(close(output.volume, 1.0), "{output:?}");
    assert!(!output.muted && !output.headphones, "{output:?}");
    assert!(!shell.state.recording);

    // Volume and mute, out and in.
    shell.send(Audio::SetOutputVolume(0.5));
    shell.send(Audio::SetOutputMuted(true));
    shell.send(Audio::SetInputVolume(0.3));
    shell.send(Audio::SetInputMuted(true));
    let changed = |state: &State| {
        let (Some(output), Some(input)) = (&state.output, &state.input) else {
            return false;
        };
        close(output.volume, 0.5) && output.muted && close(input.volume, 0.3) && input.muted
    };
    shell.wait("the new volumes", changed);
    witness.wait("the new volumes on a second connection", changed);

    shell.send(Audio::SetOutputMuted(false));
    shell.send(Audio::SetInputMuted(false));
    let unmuted = |state: &State| {
        state.output.as_ref().is_some_and(|output| !output.muted)
            && state.input.as_ref().is_some_and(|input| !input.muted)
    };
    shell.wait("unmuting", unmuted);
    witness.wait("unmuting on a second connection", unmuted);

    // Never louder than the amplified maximum.
    shell.send(Audio::SetOutputVolume(4.0));
    let loudest = |state: &State| close(output_volume(state), MAX_AMPLIFIED_VOLUME);
    shell.wait("the amplified maximum", loudest);
    witness.wait("the amplified maximum on a second connection", loudest);

    // The balance survives: the right channel at half the left one.
    if server
        .pactl(&["set-sink-volume", SPEAKERS, "40000", "20000"])
        .is_some()
    {
        shell.wait("the volume set from outside", |state| {
            close(output_volume(state), 40000.0 / 65536.0)
        });
        shell.send(Audio::SetOutputVolume(0.8));
        shell.wait("the scaled volume", |state| {
            close(output_volume(state), 0.8)
        });
        let volumes = server.sink_volumes(SPEAKERS).unwrap();
        assert!(
            volumes.len() == 2
                && volumes[0].abs_diff(52429) <= 2
                && volumes[1].abs_diff(26214) <= 2,
            "{volumes:?}"
        );
    }

    // Choosing the other devices.
    shell.send(Audio::SelectOutput(TELEVISION.to_owned()));
    shell.send(Audio::SelectInput(HEADSET.to_owned()));
    let selected = |state: &State| {
        is_complete(state)
            && !state.outputs[0].active
            && state.outputs[1].active
            && !state.inputs[0].active
            && state.inputs[1].active
    };
    shell.wait("the other devices", selected);
    witness.wait("the other devices on a second connection", selected);
    if let Some(info) = server.pactl(&["info"]) {
        assert!(
            info.contains(&format!("Default Sink: {TELEVISION}\n")),
            "{info}"
        );
        assert!(
            info.contains(&format!("Default Source: {HEADSET}\n")),
            "{info}"
        );
    }

    // The volume now goes to the television, and the speakers keep theirs.
    shell.send(Audio::SetOutputVolume(0.25));
    shell.wait("the television's volume", |state| {
        close(output_volume(state), 0.25)
    });
    if let Some(volumes) = server.sink_volumes(SPEAKERS) {
        assert!(volumes[0].abs_diff(52429) <= 2, "{volumes:?}");
    }

    // A command that changes nothing is answered all the same: the shell
    // may have predicted otherwise.
    while shell.link.events.try_recv().is_ok() {}
    shell.send(Audio::SelectOutput("no-such-device".to_owned()));
    let deadline = Instant::now() + TIMEOUT;
    while shell.link.events.try_recv().is_err() {
        assert!(Instant::now() < deadline, "no answer to a useless command");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn recording_shows_while_an_application_records() {
    let Some(mut server) = SoundServer::start() else {
        return;
    };
    if !is_installed("parec") {
        eprintln!("skipped: parec is not installed");
        return;
    }
    server.start_pulse();
    let mut shell = server.backend();
    shell.wait("the devices", is_complete);
    assert!(!shell.state.recording);

    // A level meter is not a recording, though the microphone runs for it.
    let _meter = Meter::start(server.address(), MICROPHONE);
    server.wait_until_running(MICROPHONE);
    // A change after it, to know the backend has looked since.
    shell.send(Audio::SetInputVolume(0.6));
    shell.wait("the volume set after the meter started", |state| {
        state
            .input
            .as_ref()
            .is_some_and(|input| close(input.volume, 0.6))
    });
    assert!(!shell.state.recording);

    let recorder = server
        .command("parec")
        .arg("--server")
        .arg(server.address())
        .args(["--device", MICROPHONE, "--raw", "/dev/null"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(Recorder)
        .expect("parec was found a moment ago");
    shell.wait("the recording", |state| state.recording);

    drop(recorder);
    shell.wait("the end of the recording", |state| !state.recording);
}

#[test]
fn the_backend_waits_for_the_server_and_finds_it_again() {
    let Some(mut server) = SoundServer::start() else {
        return;
    };
    // No server yet: the first attempt fails.
    let mut shell = server.backend();
    std::thread::sleep(Duration::from_millis(300));
    assert!(shell.link.events.try_recv().is_err());

    server.start_pulse();
    shell.wait("the server that came late", is_complete);

    server.stop_pulse();
    shell.wait("the loss of the server", |state| *state == State::default());

    server.start_pulse();
    shell.wait("the server that came back", is_complete);
    // And it is a working connection.
    shell.send(Audio::SetOutputVolume(0.4));
    shell.wait("a volume on the new connection", |state| {
        close(output_volume(state), 0.4)
    });
}
