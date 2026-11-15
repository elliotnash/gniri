//! `gniri msg`: requests to the running shell over a Unix socket.
//!
//! One request per connection: the client writes a line of JSON, the shell
//! answers with a line of JSON and closes. The socket is
//! `$XDG_RUNTIME_DIR/gniri.<wayland display>.sock`, so each compositor
//! session has its own, and `$GNIRI_SOCKET` overrides it.
//!
//! The lock screen and the polkit agent are processes of their own. In a
//! test scene, and only there, each answers the requests that inspect it
//! on a socket of its own ([`Role`]).

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use gtk::prelude::*;
use gtk::{gio, glib};
use serde::{Deserialize, Serialize};

use crate::dialogs::end_session;
use crate::menus::{Menu, Submenu};

/// Which of gniri's processes a request is for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Role {
    #[default]
    Shell,
    /// `gniri lock`
    Lock,
    /// `gniri polkit`
    Polkit,
}

impl Role {
    /// What the socket's name starts with.
    fn socket_prefix(self) -> &'static str {
        match self {
            Self::Shell => "gniri",
            Self::Lock => "gniri-lock",
            Self::Polkit => "gniri-polkit",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Request {
    /// Describe every mapped widget of every surface (`debug::dump`).
    DumpTree,
    /// Reply once everything asked for so far is on screen: every mapped
    /// surface has drawn a frame after this request arrived.
    Sync,
    /// List every command the shell has sent to a service.
    Commands,
    /// Open a panel menu, on `output` or the first output with a panel.
    OpenMenu {
        menu: Menu,
        output: Option<String>,
    },
    /// Open a menu inside the quick settings menu.
    OpenSubmenu(Submenu),
    /// Open the group of one application's notifications in the date menu.
    OpenGroup(String),
    /// Close whatever menu is open.
    CloseMenu,
    Volume(VolumeKey),
    Brightness(Step),
    /// Lock the screen.
    Lock,
    /// Open the dialog that asks before ending the session.
    EndSession(end_session::Kind),
    /// Close that dialog, as its Cancel button does.
    CloseDialog,
    /// Report an idle timer as passed, or as over, as the compositor would.
    Idle {
        timer: IdleTimer,
        resumed: bool,
    },
    /// Show the on-screen display with exactly this, as GNOME Shell's
    /// `ShowOSD` D-Bus method does.
    Osd {
        icon: String,
        label: Option<String>,
        level: Option<f64>,
    },
    /// Deliver a notification as if an application had sent it. Answers
    /// with its id.
    Notify(Notify),
    /// Show the app grid, opening the overview for it; with the grid
    /// showing, close the overview.
    Apps,
    /// Show the app grid (or with false the windows) in the open overview.
    ShowApps(bool),
    /// Put a text in the overview's search entry. `quiet` shows it as
    /// GNOME Shell shows a search that a script started.
    Search {
        text: String,
        quiet: bool,
    },
    /// Turn to a page of the app grid.
    Page(usize),
}

/// A notification for [`Request::Notify`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, clap::Args)]
pub struct Notify {
    pub summary: String,
    #[arg(default_value = "")]
    pub body: String,
    /// Name of the application it comes from.
    #[arg(long, default_value = "")]
    pub app: String,
    /// The application's icon: a name or a file.
    #[arg(long)]
    pub icon: Option<String>,
    /// A picture shown beside the text: an icon name or a file.
    #[arg(long)]
    pub image: Option<String>,
    #[arg(long, value_enum, default_value = "normal")]
    pub urgency: Urgency,
    /// A button, as KEY=LABEL; "default" as the key makes the notification
    /// itself clickable. May be given several times.
    #[arg(long = "action", value_name = "KEY=LABEL")]
    pub actions: Vec<String>,
    /// Id of a notification to replace.
    #[arg(long)]
    pub replaces: Option<u32>,
    #[arg(long)]
    pub transient: bool,
    #[arg(long)]
    pub resident: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
pub enum Urgency {
    Low,
    Normal,
    Critical,
}

/// A point in the idle time ([`gniri_services::idle::Timer`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
pub enum IdleTimer {
    /// The screen dims.
    Dim,
    /// The screens turn off.
    Blank,
    /// The session locks.
    Lock,
    /// The screens turn off while the lock screen shows.
    BlankLocked,
}

impl IdleTimer {
    pub fn timer(self) -> gniri_services::idle::Timer {
        use gniri_services::idle::Timer;
        match self {
            Self::Dim => Timer::Dim,
            Self::Blank => Timer::Blank,
            Self::Lock => Timer::Lock,
            Self::BlankLocked => Timer::BlankLocked,
        }
    }
}

/// What a volume key does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
pub enum VolumeKey {
    Up,
    Down,
    /// Toggle mute.
    Mute,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
pub enum Step {
    Up,
    Down,
}

/// What the shell answers: the result, or why it could not be produced.
pub type Reply = Result<serde_json::Value, String>;

pub fn socket_path(role: Role) -> Option<PathBuf> {
    if role == Role::Shell
        && let Some(path) = std::env::var_os("GNIRI_SOCKET").filter(|path| !path.is_empty())
    {
        return Some(PathBuf::from(path));
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty())?;
    let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".to_owned());
    // The display may itself be a path; only its last part names it.
    let display = display.rsplit('/').next().unwrap_or("wayland-0").to_owned();
    Some(PathBuf::from(runtime).join(format!("{}.{display}.sock", role.socket_prefix())))
}

/// Sends one request to the running process of `role` and waits for the
/// reply.
pub fn send(role: Role, request: &Request) -> Result<serde_json::Value, String> {
    let path = socket_path(role).ok_or("XDG_RUNTIME_DIR is not set")?;
    let mut stream = UnixStream::connect(&path)
        .map_err(|error| format!("cannot reach gniri at {}: {error}", path.display()))?;
    let mut line = serde_json::to_string(request).map_err(|error| error.to_string())?;
    line.push('\n');
    stream
        .write_all(line.as_bytes())
        .map_err(|error| error.to_string())?;

    let mut answer = String::new();
    BufReader::new(stream)
        .read_line(&mut answer)
        .map_err(|error| error.to_string())?;
    let reply: Reply =
        serde_json::from_str(&answer).map_err(|error| format!("unreadable reply: {error}"))?;
    reply
}

/// A reply that may take a while to be ready.
pub type Pending = std::pin::Pin<Box<dyn std::future::Future<Output = Reply>>>;

/// Starts answering requests. `handle` runs on the main loop.
///
/// Returns the listener, which must be kept alive, or `None` (after saying
/// why on stderr) if the socket cannot be set up: the shell works without.
pub fn serve(
    role: Role,
    handle: impl Fn(Request) -> Pending + 'static,
) -> Option<gio::SocketService> {
    let path = socket_path(role)?;
    // A socket file left by a shell that crashed would block the bind. If
    // nothing answers on it, it is stale.
    if path.exists() {
        if UnixStream::connect(&path).is_ok() {
            eprintln!(
                "gniri: another gniri already answers on {}; `gniri msg` will reach that one",
                path.display()
            );
            return None;
        }
        let _ = std::fs::remove_file(&path);
    }

    let service = gio::SocketService::new();
    let address = gio::UnixSocketAddress::new(&path);
    if let Err(error) = service.add_address(
        &address,
        gio::SocketType::Stream,
        gio::SocketProtocol::Default,
        None::<&glib::Object>,
    ) {
        eprintln!("gniri: cannot listen on {}: {error}", path.display());
        return None;
    }

    let handle = std::rc::Rc::new(handle);
    service.connect_incoming(move |_, connection, _| {
        let connection = connection.clone();
        let handle = std::rc::Rc::clone(&handle);
        glib::spawn_future_local(async move {
            let input = gio::DataInputStream::new(&connection.input_stream());
            let reply: Reply = match input.read_line_utf8_future(glib::Priority::DEFAULT).await {
                Ok(Some(line)) => match serde_json::from_str::<Request>(&line) {
                    Ok(request) => handle(request).await,
                    Err(error) => Err(format!("not a request: {error}")),
                },
                Ok(None) => return,
                Err(error) => Err(error.to_string()),
            };
            let mut line = serde_json::to_string(&reply).unwrap_or_else(|error| {
                format!("{{\"Err\":\"cannot encode the reply: {error}\"}}")
            });
            line.push('\n');
            let _ = connection
                .output_stream()
                .write_all_future(line.into_bytes(), glib::Priority::DEFAULT)
                .await;
            let _ = connection.close_future(glib::Priority::DEFAULT).await;
        });
        true
    });
    Some(service)
}

/// Removes the socket file at exit.
pub fn cleanup(role: Role) {
    if let Some(path) = socket_path(role) {
        let _ = std::fs::remove_file(path);
    }
}
