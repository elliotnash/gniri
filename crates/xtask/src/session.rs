//! A nested niri session running gniri, for development and visual tests.
//!
//! niri runs as a window inside the current Wayland session (its winit
//! backend). The session gets a private D-Bus session bus, so gniri can own
//! names such as `org.freedesktop.Notifications` that the host desktop
//! already holds, and nothing it does leaks into the real session.

use std::fs::{self, File};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use niri_ipc::socket::Socket;
use niri_ipc::{Action, Event, Request};

const READY_TIMEOUT: Duration = Duration::from_secs(15);
const SCREENSHOT_TIMEOUT: Duration = Duration::from_secs(5);

pub struct SessionOptions {
    /// Names the log and ready files of this run.
    pub name: String,
    pub niri_config: Option<PathBuf>,
    pub renderer: String,
    pub gniri_args: Vec<String>,
}

pub struct Session {
    /// `dbus-run-session`, whose child is niri, whose child is gniri.
    process: Child,
    gniri_pid: u32,
    pub niri_socket: PathBuf,
    pub wayland_display: String,
}

/// The workspace root: two levels above this crate.
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("xtask lives in <root>/crates/xtask")
        .to_path_buf()
}

/// Where screenshots, logs and reports go. Created on demand.
pub fn visual_dir() -> Result<PathBuf> {
    let dir = workspace_root().join("target/visual");
    fs::create_dir_all(dir.join("logs"))?;
    Ok(dir)
}

/// Fails if the host screen is locked or blanked.
///
/// Nested niri only draws while the host compositor shows its window. Behind
/// a lock screen it stops completely, IPC included, so a run would hang until
/// the timeout (docs/DECISIONS.md, S8). Hosts without the GNOME screensaver
/// interface are not checked.
fn check_host_is_showing() -> Result<()> {
    let output = Command::new("gdbus")
        .args(["call", "--session", "--dest", "org.gnome.ScreenSaver"])
        .args(["--object-path", "/org/gnome/ScreenSaver"])
        .args(["--method", "org.gnome.ScreenSaver.GetActive"])
        .output();
    match output {
        Ok(output) if output.status.success() && output.stdout.starts_with(b"(true") => bail!(
            "the screen is locked or blanked. Nested niri cannot render behind the lock \
             screen; unlock the session and run this again."
        ),
        _ => Ok(()),
    }
}

/// True if `program` is on PATH.
fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

fn build_gniri(root: &Path) -> Result<PathBuf> {
    let status = Command::new(env!("CARGO"))
        .args(["build", "--quiet", "--package", "gniri"])
        .current_dir(root)
        .status()
        .context("running cargo build")?;
    if !status.success() {
        bail!("building gniri failed");
    }
    Ok(root.join("target/debug/gniri"))
}

impl Session {
    pub fn start(options: &SessionOptions) -> Result<Self> {
        if std::env::var_os("WAYLAND_DISPLAY").is_none() {
            bail!("WAYLAND_DISPLAY is not set: nested niri needs a running Wayland session");
        }
        check_host_is_showing()?;

        let root = workspace_root();
        let gniri = build_gniri(&root)?;
        let visual = visual_dir()?;
        let logs = visual.join("logs");
        let niri_log = logs.join(format!("{}-niri.log", options.name));
        let gniri_log = logs.join(format!("{}-gniri.log", options.name));
        let ready_file = logs.join(format!("{}.ready", options.name));
        if ready_file.exists() {
            fs::remove_file(&ready_file)?;
        }

        let niri_config = options
            .niri_config
            .clone()
            .unwrap_or_else(|| root.join("tests/visual/niri/test.kdl"));

        // niri gives the command after `--` no way to redirect its output,
        // so a small shell wrapper sends gniri's output to its own log.
        //
        // While the session runs, hold off the host's idle blanking: a long
        // test run with nobody at the keyboard would otherwise lock the
        // screen and stall niri halfway through.
        let mut command = if on_path("gnome-session-inhibit") {
            let mut command = Command::new("gnome-session-inhibit");
            command
                .args(["--inhibit", "idle", "--reason", "gniri nested test session"])
                .arg("dbus-run-session");
            command
        } else {
            Command::new("dbus-run-session")
        };
        command
            .arg("--")
            .arg("niri")
            .arg("-c")
            .arg(&niri_config)
            .arg("--")
            .args([
                "sh",
                "-c",
                r#"log="$1"; shift; exec "$@" >"$log" 2>&1"#,
                "sh",
            ])
            .arg(&gniri_log)
            .arg(&gniri)
            .args(&options.gniri_args)
            .current_dir(&root)
            .env("GNIRI_INSTANCE", "test")
            .env("GNIRI_READY_FILE", &ready_file)
            .env("GNIRI_STRICT_CSS", "1")
            .env("GSK_RENDERER", &options.renderer)
            // No accessibility bus on the private session bus.
            .env("GTK_A11Y", "none")
            // Keep the user's real settings out of test runs.
            .env("GSETTINGS_BACKEND", "memory")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(File::create(&niri_log)?)
            // Its own process group, so the wrappers, niri and gniri can all
            // be stopped together and none is left behind.
            .process_group(0);

        let mut process = command.spawn().context("starting dbus-run-session niri")?;

        let started = Instant::now();
        let ready = loop {
            if let Ok(contents) = fs::read_to_string(&ready_file) {
                break contents;
            }
            if let Some(status) = process.try_wait()? {
                bail!(
                    "niri exited early ({status}).\n--- {}\n{}\n--- {}\n{}",
                    niri_log.display(),
                    tail(&niri_log),
                    gniri_log.display(),
                    tail(&gniri_log),
                );
            }
            if started.elapsed() > READY_TIMEOUT {
                kill_group(&mut process);
                bail!(
                    "gniri did not become ready within {READY_TIMEOUT:?}.\n--- {}\n{}\n--- {}\n{}",
                    niri_log.display(),
                    tail(&niri_log),
                    gniri_log.display(),
                    tail(&gniri_log),
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        };

        let field = |key: &str| -> Result<String> {
            ready
                .lines()
                .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
                .map(str::to_owned)
                .ok_or_else(|| anyhow!("ready file has no {key}: {ready:?}"))
        };

        Ok(Self {
            process,
            gniri_pid: field("pid")?.parse().context("pid in ready file")?,
            niri_socket: PathBuf::from(field("niri_socket")?),
            wayland_display: field("wayland_display")?,
        })
    }

    /// PID of the gniri process inside the session.
    #[allow(dead_code)]
    pub fn gniri_pid(&self) -> u32 {
        self.gniri_pid
    }

    fn connect(&self) -> Result<Socket> {
        Socket::connect_to(&self.niri_socket)
            .with_context(|| format!("connecting to {}", self.niri_socket.display()))
    }

    /// Runs one niri action in the nested compositor.
    pub fn action(&self, action: Action) -> Result<()> {
        let reply = self.connect()?.send(Request::Action(action))?;
        reply
            .map(drop)
            .map_err(|message| anyhow!("niri refused the action: {message}"))
    }

    /// Captures the nested output to `path` (PNG, physical pixels).
    pub fn screenshot(&self, path: &Path) -> Result<()> {
        if path.exists() {
            fs::remove_file(path)?;
        }

        // Subscribe before asking, so the completion event cannot be missed.
        let mut events = self.connect()?;
        events
            .send(Request::EventStream)?
            .map_err(|message| anyhow!("niri refused the event stream: {message}"))?;
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut read_event = events.read_events();
            while let Ok(event) = read_event() {
                if let Event::ScreenshotCaptured { path } = event
                    && sender.send(path).is_err()
                {
                    break;
                }
            }
        });

        let target = path
            .to_str()
            .ok_or_else(|| anyhow!("non-UTF-8 screenshot path"))?;
        self.action(Action::ScreenshotScreen {
            write_to_disk: true,
            show_pointer: false,
            path: Some(target.to_owned()),
        })?;

        let captured = receiver.recv_timeout(SCREENSHOT_TIMEOUT).map_err(|_| {
            anyhow!("niri did not report a screenshot within {SCREENSHOT_TIMEOUT:?}")
        })?;
        if captured.as_deref() != Some(target) {
            bail!("niri saved the screenshot to {captured:?}, expected {target}");
        }

        // The event can precede the file hitting the disk; wait for it.
        let started = Instant::now();
        while !path.exists() {
            if started.elapsed() > SCREENSHOT_TIMEOUT {
                bail!("{} was never written", path.display());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }

    /// Blocks until the session ends (for `xtask nest`).
    pub fn wait(mut self) -> Result<()> {
        self.process.wait()?;
        Ok(())
    }

    /// Asks niri to quit and waits for the whole session to end.
    pub fn quit(mut self) -> Result<()> {
        // niri may close the socket before replying; that still means it quit.
        let _ = self.action(Action::Quit {
            skip_confirmation: true,
        });
        let started = Instant::now();
        while self.process.try_wait()?.is_none() {
            if started.elapsed() > Duration::from_secs(5) {
                kill_group(&mut self.process);
                bail!("niri did not exit after quit; killed it");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Never leave a nested compositor behind, whatever went wrong.
        if matches!(self.process.try_wait(), Ok(None)) {
            let _ = self.action(Action::Quit {
                skip_confirmation: true,
            });
            std::thread::sleep(Duration::from_millis(200));
            if matches!(self.process.try_wait(), Ok(None)) {
                kill_group(&mut self.process);
            }
        }
    }
}

/// Kills every process of the session.
///
/// SIGKILL, not SIGTERM: a niri stalled behind a lock screen ignores SIGTERM.
/// `process` leads its own process group (see `Session::start`), so the group
/// ID is its PID.
fn kill_group(process: &mut Child) {
    let group = format!("-{}", process.id());
    let _ = Command::new("kill").args(["-KILL", "--", &group]).status();
    let _ = process.wait();
}

/// The last lines of a log, for error messages.
fn tail(path: &Path) -> String {
    match fs::read_to_string(path) {
        Ok(text) => {
            let lines: Vec<&str> = text.lines().collect();
            lines[lines.len().saturating_sub(15)..].join("\n")
        }
        Err(error) => format!("(cannot read: {error})"),
    }
}
