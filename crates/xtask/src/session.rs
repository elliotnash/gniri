// SPDX-License-Identifier: GPL-3.0-or-later

//! A nested niri session running gniri, for development and visual tests.
//!
//! niri runs as a window inside the current Wayland session (its winit
//! backend). The session gets a private D-Bus session bus, so gniri can own
//! names such as `org.freedesktop.Notifications` that the host desktop
//! already holds, and nothing it does leaks into the real session.

use std::cell::RefCell;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use niri_ipc::socket::Socket;
use niri_ipc::{Action, Event, Request, Response};

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
    /// The gniri binary the session runs.
    gniri: PathBuf,
    gniri_socket: PathBuf,
    /// What the shell was started with: its helpers get the same.
    gniri_args: Vec<String>,
    /// Names the log files of this run.
    name: String,
    /// The helper processes started so far (`lock`, `polkit`), which
    /// have widgets of their own to dump.
    helpers: RefCell<Vec<String>>,
    pub niri_socket: PathBuf,
    pub wayland_display: String,
    /// The language the session's processes run in.
    language: Language,
}

/// The language of a session: its locale, and the list gettext goes by
/// before it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Language {
    pub locale: String,
    pub languages: Option<String>,
}

impl Language {
    /// What every comparison with the reference runs in, whatever the
    /// language of the session around it: the reference was captured in
    /// American English.
    pub fn english() -> Self {
        Self {
            locale: "en_US.UTF-8".to_owned(),
            languages: None,
        }
    }

    fn apply(&self, command: &mut Command) {
        command.env("LC_ALL", &self.locale);
        match &self.languages {
            Some(languages) => command.env("LANGUAGE", languages),
            None => command.env_remove("LANGUAGE"),
        };
    }
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
/// interface are not checked, nor asked: a call to a name nobody has waits
/// for the bus to give up on starting its owner.
fn check_host_is_showing() -> Result<()> {
    if !session_bus_has("org.gnome.ScreenSaver") {
        return Ok(());
    }
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

/// Whether the host session is GNOME's, whose session manager
/// `gnome-session-inhibit` asks. Where it is installed but the session is
/// another (niri with gniri, say), it fails without running anything.
fn gnome_session_manager_runs() -> bool {
    on_path("gnome-session-inhibit") && session_bus_has("org.gnome.SessionManager")
}

/// Whether some program has `name` on the session bus.
fn session_bus_has(name: &str) -> bool {
    Command::new("gdbus")
        .args(["call", "--session", "--dest", "org.freedesktop.DBus"])
        .args(["--object-path", "/org/freedesktop/DBus"])
        .args(["--method", "org.freedesktop.DBus.NameHasOwner"])
        .arg(name)
        .output()
        .is_ok_and(|output| output.stdout.starts_with(b"(true"))
}

/// The size every comparison with the reference assumes of the nested
/// output, in the host's logical pixels: the size nested niri asks for its
/// window, which GNOME gives it.
const NESTED_SIZE: (i32, i32) = (1280, 800);

/// The socket of the niri this process runs in, if it runs in one.
fn host_niri_socket() -> Option<PathBuf> {
    std::env::var_os("NIRI_SOCKET")
        .filter(|socket| !socket.is_empty())
        .map(PathBuf::from)
}

/// The process whose child `pid` is.
fn parent_of(pid: i32) -> Option<u32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // After the name, which is in parentheses and may hold anything: the
    // state, then the parent.
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

/// Under a host that is niri itself, a new window is laid out as a column:
/// as wide as a column is there and as high as the screen. The nested
/// session's window is taken out of the columns and given the size it
/// asked for. Returns the window's id on the host once it is there and
/// has been sized.
fn size_nested_window(session: u32) -> Option<u64> {
    let Some(Response::Windows(windows)) = ask_host(Request::Windows) else {
        return None;
    };
    // `dbus-run-session` is the session's process, niri its child.
    let window = windows
        .iter()
        .find(|window| window.pid.and_then(parent_of) == Some(session))?;
    let id = Some(window.id);
    for action in [
        Action::MoveWindowToFloating { id },
        Action::SetWindowWidth {
            id,
            change: niri_ipc::SizeChange::SetFixed(NESTED_SIZE.0),
        },
        Action::SetWindowHeight {
            id,
            change: niri_ipc::SizeChange::SetFixed(NESTED_SIZE.1),
        },
    ] {
        ask_host(Request::Action(action))?;
    }
    id
}

/// Makes the nested session's window on the host larger by `by`.
fn grow_host_window(window: u64, by: (i32, i32)) {
    let id = Some(window);
    let _ = ask_host(Request::Action(Action::SetWindowWidth {
        id,
        change: niri_ipc::SizeChange::AdjustFixed(by.0),
    }));
    let _ = ask_host(Request::Action(Action::SetWindowHeight {
        id,
        change: niri_ipc::SizeChange::AdjustFixed(by.1),
    }));
}

/// Asks the niri this process runs in.
fn ask_host(request: Request) -> Option<Response> {
    Socket::connect_to(host_niri_socket()?)
        .ok()?
        .send(request)
        .ok()?
        .ok()
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
        Self::start_in(options, Language::english())
    }

    /// Starts a session whose processes run in `language`.
    pub fn start_in(options: &SessionOptions, language: Language) -> Result<Self> {
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
        let mut command = if gnome_session_manager_runs() {
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
        language.apply(&mut command);

        let mut process = command.spawn().context("starting dbus-run-session niri")?;

        let started = Instant::now();
        let mut host_window = None;
        let ready = loop {
            if host_window.is_none() {
                host_window = size_nested_window(process.id());
            }
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

        let session = Self {
            process,
            gniri_pid: field("pid")?.parse().context("pid in ready file")?,
            gniri,
            gniri_socket: PathBuf::from(field("gniri_socket")?),
            gniri_args: options.gniri_args.clone(),
            name: options.name.clone(),
            helpers: RefCell::new(Vec::new()),
            niri_socket: PathBuf::from(field("niri_socket")?),
            wayland_display: field("wayland_display")?,
            language,
        };
        if let Some(window) = host_window {
            // The window was resized after it was mapped; nothing is to be
            // looked at before the nested output has its size. The host
            // may also have taken its border, or a title bar, out of what
            // was asked for: then the window is made larger by as much.
            let mut tries = 0;
            let settled = wait_until_within(
                "the nested output to have its size",
                Duration::from_secs(5),
                || {
                    let Response::Outputs(outputs) = session.request(Request::Outputs)? else {
                        return Ok(false);
                    };
                    let Some(logical) = outputs.values().find_map(|output| output.logical) else {
                        return Ok(false);
                    };
                    let short = (
                        NESTED_SIZE.0 - logical.width as i32,
                        NESTED_SIZE.1 - logical.height as i32,
                    );
                    if short == (0, 0) {
                        return Ok(true);
                    }
                    tries += 1;
                    // Every few looks: the output follows the window a
                    // moment late.
                    if tries % 8 == 0 {
                        grow_host_window(window, short);
                    }
                    Ok(false)
                },
            );
            if settled.is_err() {
                eprintln!(
                    "xtask: the nested output is not {}x{}; comparisons that assume it will fail",
                    NESTED_SIZE.0, NESTED_SIZE.1
                );
            }
        }
        Ok(session)
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

    /// Sends niri a request that has a reply.
    pub fn request(&self, request: Request) -> Result<Response> {
        self.connect()?
            .send(request)?
            .map_err(|message| anyhow!("niri refused the request: {message}"))
    }

    /// Whether niri's overview is open.
    pub fn overview_is_open(&self) -> Result<bool> {
        match self.request(Request::OverviewState)? {
            Response::OverviewState(overview) => Ok(overview.is_open),
            other => bail!("unexpected reply to OverviewState: {other:?}"),
        }
    }

    /// The application id of the window with the keyboard focus, if a
    /// window has it.
    pub fn focused_app(&self) -> Result<Option<String>> {
        match self.request(Request::FocusedWindow)? {
            Response::FocusedWindow(window) => Ok(window.and_then(|window| window.app_id)),
            other => bail!("unexpected reply to FocusedWindow: {other:?}"),
        }
    }

    fn window_count(&self) -> Result<usize> {
        match self.request(Request::Windows)? {
            Response::Windows(windows) => Ok(windows.len()),
            other => bail!("unexpected reply to Windows: {other:?}"),
        }
    }

    /// Opens a flat-coloured application window (`gniri debug-window`) in the
    /// session and waits until niri has mapped it.
    pub fn spawn_window(&self, colour: &str) -> Result<()> {
        self.spawn_app_window(colour, None)
    }

    /// The same, as a window of the application called `app_id`.
    pub fn spawn_app_window(&self, colour: &str, app_id: Option<&str>) -> Result<()> {
        let before = self.window_count()?;
        let gniri = self
            .gniri
            .to_str()
            .ok_or_else(|| anyhow!("non-UTF-8 path to gniri"))?;
        let mut command = vec![
            gniri.to_owned(),
            "debug-window".to_owned(),
            "--color".to_owned(),
            colour.to_owned(),
        ];
        if let Some(app_id) = app_id {
            command.extend(["--app-id".to_owned(), app_id.to_owned()]);
        }
        self.action(Action::Spawn { command })?;
        wait_until("the debug window to open", || {
            Ok(self.window_count()? > before)
        })
    }

    /// The socket one of gniri's helper processes answers on in this
    /// session (crates/shell/src/ipc.rs).
    fn helper_socket(&self, helper: &str) -> PathBuf {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR").unwrap_or_default();
        PathBuf::from(runtime).join(format!("gniri-{helper}.{}.sock", self.wayland_display))
    }

    /// Sends one request to a gniri process and returns what it answered.
    fn ask(socket: &Path, request: &str) -> Result<serde_json::Value> {
        let mut stream = UnixStream::connect(socket)
            .with_context(|| format!("connecting to {}", socket.display()))?;
        stream.write_all(format!("\"{request}\"\n").as_bytes())?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line)?;
        let mut reply: serde_json::Value =
            serde_json::from_str(&line).context("gniri's reply is not JSON")?;
        if let Some(error) = reply.get("Err") {
            bail!("gniri answered {request} with an error: {error}");
        }
        Ok(reply["Ok"].take())
    }

    /// Starts `gniri <subcommand>` in the session, with the options the
    /// shell has (the same config, fixture and wallpaper), and waits until
    /// it answers. For the lock screen and the polkit agent, which are
    /// processes of their own.
    pub fn spawn_helper(&self, subcommand: &str) -> Result<()> {
        let gniri = self
            .gniri
            .to_str()
            .ok_or_else(|| anyhow!("non-UTF-8 path to gniri"))?;
        let log = visual_dir()?
            .join("logs")
            .join(format!("{}-{subcommand}.log", self.name));
        let socket = self.helper_socket(subcommand);
        // One left behind by a run that was killed would look like an
        // answer.
        let _ = fs::remove_file(&socket);
        let mut command = vec![
            "sh".to_owned(),
            "-c".to_owned(),
            r#"log="$1"; shift; exec "$@" >"$log" 2>&1"#.to_owned(),
            "sh".to_owned(),
            log.display().to_string(),
            gniri.to_owned(),
        ];
        command.extend(self.gniri_args.iter().cloned());
        command.push(subcommand.to_owned());
        self.action(Action::Spawn { command })?;
        wait_until(&format!("gniri {subcommand} to start"), || {
            Ok(Self::ask(&socket, "Sync").is_ok())
        })
        .with_context(|| format!("see {}", log.display()))?;
        self.helpers.borrow_mut().push(subcommand.to_owned());
        Ok(())
    }

    /// Starts `gniri <subcommand>` as a child of this process, a client of
    /// the nested compositor with the options the shell has, and waits
    /// until it answers. Unlike [`Session::spawn_helper`] this also works
    /// while the session is locked, when niri starts nothing, and the
    /// caller can kill what it started.
    pub fn start_helper(&self, subcommand: &str) -> Result<Child> {
        let log = visual_dir()?
            .join("logs")
            .join(format!("{}-{subcommand}.log", self.name));
        let socket = self.helper_socket(subcommand);
        let _ = fs::remove_file(&socket);
        let output = File::create(&log)?;
        let mut command = Command::new(&self.gniri);
        command
            .args(&self.gniri_args)
            .arg(subcommand)
            .env("WAYLAND_DISPLAY", &self.wayland_display)
            .env("NIRI_SOCKET", &self.niri_socket)
            .env("GNIRI_INSTANCE", "test")
            .env("GSK_RENDERER", "vulkan")
            .env("GTK_A11Y", "none")
            .env("GSETTINGS_BACKEND", "memory")
            .stdin(Stdio::null())
            .stdout(output.try_clone()?)
            .stderr(output);
        self.language.apply(&mut command);
        let child = command
            .spawn()
            .with_context(|| format!("starting gniri {subcommand}"))?;
        wait_until(&format!("gniri {subcommand} to start"), || {
            Ok(Self::ask(&socket, "Sync").is_ok())
        })
        .with_context(|| format!("see {}", log.display()))?;
        let mut helpers = self.helpers.borrow_mut();
        if !helpers.iter().any(|helper| helper == subcommand) {
            helpers.push(subcommand.to_owned());
        }
        Ok(child)
    }

    /// Whether a helper process started with [`Session::spawn_helper`]
    /// still answers.
    pub fn helper_runs(&self, subcommand: &str) -> bool {
        Self::ask(&self.helper_socket(subcommand), "Sync").is_ok()
    }

    /// Every mapped widget of the shell and of its helper processes
    /// (`gniri msg debug dump-tree`).
    pub fn dump_tree(&self) -> Result<serde_json::Value> {
        let mut dump = Self::ask(&self.gniri_socket, "DumpTree")?;
        for helper in self.helpers.borrow().iter() {
            // A helper that has done its work (the screen was unlocked)
            // is gone, and so are its widgets.
            let Ok(mut other) = Self::ask(&self.helper_socket(helper), "DumpTree") else {
                continue;
            };
            if let (Some(surfaces), Some(more)) = (
                dump["surfaces"].as_array_mut(),
                other["surfaces"].as_array_mut(),
            ) {
                surfaces.append(more);
            }
        }
        Ok(dump)
    }

    /// Runs a command as a client of the nested compositor (for `wlrctl`).
    /// Runs `gniri msg <arguments>` against this session's shell and
    /// returns what it printed.
    pub fn gniri_msg(&self, arguments: &[&str]) -> Result<String> {
        let output = Command::new(&self.gniri)
            .arg("msg")
            .args(arguments)
            .env("GNIRI_SOCKET", &self.gniri_socket)
            .output()
            .context("running gniri msg")?;
        if !output.status.success() {
            bail!(
                "gniri msg {} failed: {}",
                arguments.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Waits until the session shows the result of everything done so far.
    ///
    /// Steps are done when niri has acted, but gniri hears about that a
    /// moment later; then it must draw, and niri must composite what it
    /// drew. Fixtures turn animations off, so nothing still moves after.
    pub fn settle(&self) -> Result<()> {
        std::thread::sleep(Duration::from_millis(100));
        self.gniri_msg(&["debug", "sync"])?;
        for helper in self.helpers.borrow().iter() {
            let _ = Self::ask(&self.helper_socket(helper), "Sync");
        }
        std::thread::sleep(Duration::from_millis(200));
        Ok(())
    }

    pub fn client_command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command.env("WAYLAND_DISPLAY", &self.wayland_display);
        command
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

    /// Blocks until the session ends or this process is told to stop (for
    /// `xtask nest`).
    ///
    /// The session runs in its own process group, so a Ctrl-C in the terminal
    /// does not reach it: catch the signal here and let `Drop` end the session.
    pub fn wait(mut self) -> Result<()> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        ctrlc::set_handler(move || flag.store(true, Ordering::SeqCst))
            .context("installing the Ctrl-C handler")?;
        while self.process.try_wait()?.is_none() && !stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(50));
        }
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

/// Polls `done` until it returns true, for at most three seconds.
pub fn wait_until(what: &str, done: impl FnMut() -> Result<bool>) -> Result<()> {
    wait_until_within(what, Duration::from_secs(3), done)
}

/// Polls `done` until it holds, for at most `limit`.
pub fn wait_until_within(
    what: &str,
    limit: Duration,
    mut done: impl FnMut() -> Result<bool>,
) -> Result<()> {
    let started = Instant::now();
    loop {
        if done()? {
            return Ok(());
        }
        if started.elapsed() > limit {
            bail!("timed out waiting for {what}");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
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
