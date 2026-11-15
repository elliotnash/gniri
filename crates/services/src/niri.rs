//! The niri compositor: workspaces, overview, keyboard layouts, screencasts.
//!
//! niri's IPC is a Unix socket (`$NIRI_SOCKET`) speaking JSON. One connection
//! carries the event stream, read on its own thread; requests each open a
//! short-lived second connection.

use std::path::{Path, PathBuf};

use niri_ipc::socket::Socket;
use niri_ipc::state::{EventStreamState, EventStreamStatePart};
use niri_ipc::{Action, Event, Request, Response, WorkspaceReferenceArg};
use serde::Deserialize;

/// What the shell needs to know about the compositor.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct State {
    /// False until the first events arrive, and again if the stream ends.
    pub connected: bool,
    /// Every workspace on every output, ordered by output then position.
    pub workspaces: Vec<Workspace>,
    pub overview_open: bool,
    /// Connector name of the output with the focused workspace.
    pub focused_output: Option<String>,
    /// Names of the configured keyboard layouts, such as "English (US)".
    pub keyboard_layouts: Vec<String>,
    /// Index into `keyboard_layouts` of the layout in use.
    pub keyboard_layout: usize,
    /// PipeWire sessions that are sharing the screen or a window right now.
    pub screencast_sessions: Vec<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Workspace {
    pub id: u64,
    /// Position on its output, starting at 1.
    pub index: u8,
    pub name: Option<String>,
    /// Connector name of the output it is on, such as "eDP-1".
    pub output: Option<String>,
    /// Shown on its output (each output has exactly one).
    pub active: bool,
    pub urgent: bool,
}

impl State {
    /// The workspaces on `output`, top to bottom.
    pub fn workspaces_on<'a>(&'a self, output: &'a str) -> impl Iterator<Item = &'a Workspace> {
        self.workspaces
            .iter()
            .filter(move |workspace| workspace.output.as_deref() == Some(output))
    }
}

/// Folds niri's event stream into a [`State`].
#[derive(Default)]
pub struct Tracker {
    raw: EventStreamState,
}

impl Tracker {
    /// Applies one event and returns the state after it.
    pub fn apply(&mut self, event: Event) -> State {
        // `apply` hands back events it does not know; nothing to do with them.
        let _ = self.raw.apply(event);
        self.state()
    }

    fn state(&self) -> State {
        let mut workspaces: Vec<Workspace> = self
            .raw
            .workspaces
            .workspaces
            .values()
            .map(|workspace| Workspace {
                id: workspace.id,
                index: workspace.idx,
                name: workspace.name.clone(),
                output: workspace.output.clone(),
                active: workspace.is_active,
                urgent: workspace.is_urgent,
            })
            .collect();
        workspaces.sort_by(|a, b| (&a.output, a.index).cmp(&(&b.output, b.index)));
        let focused_output = self
            .raw
            .workspaces
            .workspaces
            .values()
            .find(|workspace| workspace.is_focused)
            .and_then(|workspace| workspace.output.clone());

        let (keyboard_layouts, keyboard_layout) = match &self.raw.keyboard_layouts.keyboard_layouts
        {
            Some(layouts) => (layouts.names.clone(), usize::from(layouts.current_idx)),
            None => (Vec::new(), 0),
        };

        let mut screencast_sessions: Vec<u64> = self
            .raw
            .casts
            .casts
            .values()
            .filter(|cast| cast.is_active)
            .map(|cast| cast.session_id)
            .collect();
        screencast_sessions.sort_unstable();
        screencast_sessions.dedup();

        State {
            connected: true,
            workspaces,
            overview_open: self.raw.overview.is_open,
            focused_output,
            keyboard_layouts,
            keyboard_layout,
            screencast_sessions,
        }
    }
}

/// What the event thread sends to the main loop.
#[derive(Debug)]
pub enum Message {
    Event(Event),
    /// The stream ended; no more messages follow.
    Disconnected(String),
}

/// Where niri listens, from `$NIRI_SOCKET`.
pub fn socket_path() -> Option<PathBuf> {
    std::env::var_os("NIRI_SOCKET")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

/// Starts reading niri's event stream on a new thread.
///
/// niri sends the complete current state as the first events, so the
/// receiver needs no separate initial query.
pub fn watch(socket: &Path) -> async_channel::Receiver<Message> {
    let (sender, receiver) = async_channel::unbounded();
    let socket = socket.to_owned();
    let spawned = std::thread::Builder::new()
        .name("niri-events".to_owned())
        .spawn({
            let sender = sender.clone();
            move || {
                let reason = match read_events(&socket, &sender) {
                    Ok(()) => return, // The receiver went away.
                    Err(reason) => reason,
                };
                let _ = sender.send_blocking(Message::Disconnected(reason));
            }
        });
    if let Err(error) = spawned {
        let _ = sender.send_blocking(Message::Disconnected(format!(
            "cannot start the event thread: {error}"
        )));
    }
    receiver
}

fn read_events(socket: &Path, sender: &async_channel::Sender<Message>) -> Result<(), String> {
    let mut connection =
        Socket::connect_to(socket).map_err(|error| format!("cannot connect: {error}"))?;
    match connection.send(Request::EventStream) {
        Ok(Ok(Response::Handled)) => {}
        Ok(Ok(other)) => return Err(format!("unexpected reply to EventStream: {other:?}")),
        Ok(Err(message)) => return Err(message),
        Err(error) => return Err(error.to_string()),
    }
    let mut next = connection.read_events();
    loop {
        let event = next().map_err(|error| error.to_string())?;
        if sender.send_blocking(Message::Event(event)).is_err() {
            return Ok(());
        }
    }
}

/// Sends requests to niri.
#[derive(Clone, Debug)]
pub struct Client {
    socket: PathBuf,
}

impl Client {
    pub fn new(socket: PathBuf) -> Self {
        Self { socket }
    }

    /// Performs `action`. Blocks for one local socket round trip.
    pub fn action(&self, action: Action) -> Result<(), String> {
        let mut connection = Socket::connect_to(&self.socket).map_err(|error| error.to_string())?;
        match connection.send(Request::Action(action)) {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(message)) => Err(message),
            Err(error) => Err(error.to_string()),
        }
    }

    /// Connector name of the output with the keyboard focus. Blocks for
    /// one local socket round trip.
    pub fn focused_output(&self) -> Option<String> {
        let mut connection = Socket::connect_to(&self.socket).ok()?;
        match connection.send(Request::FocusedOutput) {
            Ok(Ok(niri_ipc::Response::FocusedOutput(output))) => output.map(|output| output.name),
            _ => None,
        }
    }

    pub fn toggle_overview(&self) -> Result<(), String> {
        self.action(Action::ToggleOverview {})
    }

    pub fn focus_workspace(&self, id: u64) -> Result<(), String> {
        self.action(Action::FocusWorkspace {
            reference: WorkspaceReferenceArg::Id(id),
        })
    }

    pub fn stop_screencast(&self, session_id: u64) -> Result<(), String> {
        self.action(Action::StopCast { session_id })
    }
}

#[cfg(test)]
mod tests {
    use niri_ipc::{Cast, CastKind, CastTarget, KeyboardLayouts};

    use super::*;

    fn workspace(id: u64, idx: u8, output: &str, is_active: bool) -> niri_ipc::Workspace {
        niri_ipc::Workspace {
            id,
            idx,
            name: None,
            output: Some(output.to_owned()),
            is_urgent: false,
            is_active,
            is_focused: is_active && output == "eDP-1",
            active_window_id: None,
        }
    }

    fn ids(state: &State, output: &str) -> Vec<(u64, bool)> {
        state
            .workspaces_on(output)
            .map(|workspace| (workspace.id, workspace.active))
            .collect()
    }

    #[test]
    fn starts_disconnected() {
        assert!(!State::default().connected);
    }

    #[test]
    fn workspaces_are_grouped_by_output_in_order() {
        let mut tracker = Tracker::default();
        let state = tracker.apply(Event::WorkspacesChanged {
            workspaces: vec![
                workspace(7, 2, "eDP-1", false),
                workspace(9, 1, "HDMI-A-1", true),
                workspace(3, 1, "eDP-1", true),
            ],
        });
        assert!(state.connected);
        assert_eq!(ids(&state, "eDP-1"), [(3, true), (7, false)]);
        assert_eq!(ids(&state, "HDMI-A-1"), [(9, true)]);
        assert!(ids(&state, "DP-2").is_empty());
    }

    #[test]
    fn activating_a_workspace_moves_the_active_flag_on_its_output_only() {
        let mut tracker = Tracker::default();
        tracker.apply(Event::WorkspacesChanged {
            workspaces: vec![
                workspace(3, 1, "eDP-1", true),
                workspace(7, 2, "eDP-1", false),
                workspace(9, 1, "HDMI-A-1", true),
            ],
        });
        let state = tracker.apply(Event::WorkspaceActivated {
            id: 7,
            focused: true,
        });
        assert_eq!(ids(&state, "eDP-1"), [(3, false), (7, true)]);
        assert_eq!(ids(&state, "HDMI-A-1"), [(9, true)]);
    }

    #[test]
    fn overview_and_keyboard_layouts() {
        let mut tracker = Tracker::default();
        assert!(
            tracker
                .apply(Event::OverviewOpenedOrClosed { is_open: true })
                .overview_open
        );
        tracker.apply(Event::KeyboardLayoutsChanged {
            keyboard_layouts: KeyboardLayouts {
                names: vec!["English (US)".to_owned(), "German".to_owned()],
                current_idx: 0,
            },
        });
        let state = tracker.apply(Event::KeyboardLayoutSwitched { idx: 1 });
        assert_eq!(state.keyboard_layouts, ["English (US)", "German"]);
        assert_eq!(state.keyboard_layout, 1);
        assert!(state.overview_open);
    }

    #[test]
    fn only_active_casts_count_and_sessions_are_listed_once() {
        let cast = |stream_id, session_id, is_active| Cast {
            stream_id,
            session_id,
            kind: CastKind::PipeWire,
            target: CastTarget::Nothing {},
            is_dynamic_target: false,
            is_active,
            pid: None,
            pw_node_id: None,
        };
        let mut tracker = Tracker::default();
        let state = tracker.apply(Event::CastsChanged {
            casts: vec![cast(1, 10, true), cast(2, 10, true), cast(3, 11, false)],
        });
        assert_eq!(state.screencast_sessions, [10]);
        let state = tracker.apply(Event::CastStopped { stream_id: 1 });
        assert_eq!(state.screencast_sessions, [10]);
        let state = tracker.apply(Event::CastStopped { stream_id: 2 });
        assert!(state.screencast_sessions.is_empty());
    }
}
