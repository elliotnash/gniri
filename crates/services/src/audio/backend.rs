//! The PulseAudio backend.
//!
//! One thread owns everything: a standard (poll-based) libpulse main loop,
//! the connection, and the last snapshot of the server (what `mixer` makes
//! the state from). The thread runs the loop itself, one iteration at a
//! time, and does its own work between two iterations.
//!
//! Commands come from the shell's thread on an `async_channel`. The loop
//! polls that channel with a waker that writes a byte into a pipe the loop
//! watches, so a command wakes the loop out of `poll(2)` at once. That needs
//! neither a second thread nor a lock (with libpulse's threaded main loop,
//! every call from outside has to take the loop's lock, and the closures
//! shared with its callbacks are not `Send`).
//!
//! libpulse's callbacks only write down what the server said (into
//! `Shared`). Every request is made from the loop's body, so no callback
//! re-enters libpulse and nothing is borrowed twice.
//!
//! A change on the server only marks the snapshot as out of date. A moment
//! (`REFRESH_DELAY`) after the first such mark, everything is asked for
//! again in one round: a slider dragged in another mixer costs one round and
//! one report every 30 ms however many events it makes, and each report is
//! one consistent picture rather than a new default beside an old list.
//!
//! Without a server (not started yet, or pipewire-pulse restarting) the
//! state is the default one, and the connection is tried again every five
//! seconds (`RECONNECT_DELAY`).

use std::cell::RefCell;
use std::convert::Infallible;
use std::io::{PipeWriter, Read, Write};
use std::os::fd::AsRawFd;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll, Wake, Waker};
use std::time::{Duration, Instant};

use async_channel::{Receiver, Sender};
use futures_lite::Stream as _;
use libpulse_binding::callbacks::ListResult;
use libpulse_binding::context::introspect::{
    CardInfo, CardPortInfo, Introspector, SinkInfo, SourceInfo, SourceOutputInfo,
};
use libpulse_binding::context::subscribe::InterestMaskSet;
use libpulse_binding::context::{Context, FlagSet, State as ContextState};
use libpulse_binding::def::{PortAvailable, SinkState, SourceState};
use libpulse_binding::direction::FlagSet as PortDirection;
use libpulse_binding::error::PAErr;
use libpulse_binding::mainloop::api::Mainloop as _;
use libpulse_binding::mainloop::events::io::FlagSet as IoEvents;
use libpulse_binding::mainloop::standard::Mainloop;
use libpulse_binding::time::MicroSeconds;

use super::mixer::{Action, Card, Direction, Port, Profile, Snapshot, SourceOutput, Stream};
use super::{Command, Event, State};
use crate::model::Link;

/// How long after the server announces a change everything is read again.
/// Long enough to take a burst of events as one, short enough for a slider
/// to follow a volume changed elsewhere.
const REFRESH_DELAY: Duration = Duration::from_millis(30);

/// How long to wait before looking for the server again (gvc's
/// `RECONNECT_DELAY`).
const RECONNECT_DELAY: Duration = Duration::from_secs(5);

/// A server that has not answered a round of queries after this long is
/// taken for gone.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a device chosen from a menu may wait for its card to change
/// profile before the choice is forgotten.
const PROFILE_TIMEOUT: Duration = Duration::from_secs(5);

/// Starts the backend on its own thread, for the user's sound server.
pub fn spawn() -> Link<State> {
    spawn_at(None)
}

/// As [`spawn`], for the server at this address (such as
/// `unix:/run/user/1000/pulse/native`) rather than the one libpulse finds.
pub fn spawn_at(server: Option<String>) -> Link<State> {
    let (link, events, commands) = Link::new();
    let thread = std::thread::Builder::new().name("gniri-audio".to_owned());
    if let Err(error) = thread.spawn(move || run(server, events, commands)) {
        eprintln!("gniri: audio: cannot start a thread: {error}");
    }
    link
}

fn run(server: Option<String>, events: Sender<Event>, commands: Receiver<Command>) {
    let Some(mut mainloop) = Mainloop::new() else {
        eprintln!("gniri: audio: cannot create a main loop");
        return;
    };
    let (reader, writer) = match std::io::pipe() {
        Ok(pipe) => pipe,
        Err(error) => {
            eprintln!("gniri: audio: cannot create a pipe: {error}");
            return;
        }
    };
    let watch = mainloop.new_io_event(
        reader.as_raw_fd(),
        IoEvents::INPUT,
        Box::new(move |_, _, _| {
            // Being woken is the whole message; the loop reads the channel.
            let _ = (&reader).read(&mut [0; 64]);
        }),
    );
    // Kept to the end: dropping it stops the watch.
    let Some(_watch) = watch else {
        eprintln!("gniri: audio: cannot watch for commands");
        return;
    };
    let mut backend = Backend {
        mainloop,
        inbox: Inbox {
            commands: Box::pin(commands),
            waker: Waker::from(Arc::new(PipeWaker(writer))),
        },
        events,
        reported: State::default(),
        server,
        complained: false,
    };
    let Err(stop) = backend.run();
    if let Stop::MainLoop(error) = stop {
        eprintln!("gniri: audio: the main loop failed: {error}");
    }
}

/// Wakes the main loop: a byte into the pipe it polls.
struct PipeWaker(PipeWriter);

impl Wake for PipeWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        // The channel wakes once for each time it was polled empty, and the
        // loop empties the pipe before it polls again, so the pipe cannot
        // fill up and block the shell.
        let _ = (&self.0).write(&[0]);
    }
}

/// The command channel, as the main loop reads it.
struct Inbox {
    commands: Pin<Box<Receiver<Command>>>,
    waker: Waker,
}

impl Inbox {
    /// The next command if one is waiting, `None` in `Ready` once the shell
    /// has dropped its end. `Pending` leaves the waker with the channel.
    fn next(&mut self) -> Poll<Option<Command>> {
        let mut context = TaskContext::from_waker(&self.waker);
        self.commands.as_mut().poll_next(&mut context)
    }
}

/// Why the backend ends for good.
enum Stop {
    /// The shell dropped its end of the link.
    ShellGone,
    MainLoop(PAErr),
}

struct Backend {
    mainloop: Mainloop,
    inbox: Inbox,
    events: Sender<Event>,
    /// What the shell was last told.
    reported: State,
    server: Option<String>,
    /// The missing server has been mentioned; once is enough.
    complained: bool,
}

impl Backend {
    fn run(&mut self) -> Result<Infallible, Stop> {
        loop {
            self.connection()?;
            self.report(State::default(), false)?;
            self.sleep(RECONNECT_DELAY)?;
        }
    }

    /// One connection to the server, from the attempt to its loss.
    fn connection(&mut self) -> Result<(), Stop> {
        let Some(mut context) = Context::new(&self.mainloop, "gniri") else {
            eprintln!("gniri: audio: cannot create a connection");
            return Ok(());
        };
        // Starting the user's sound server is systemd's business.
        if let Err(error) = context.connect(self.server.as_deref(), FlagSet::NOAUTOSPAWN, None) {
            self.complain(error);
            return Ok(());
        }
        let mut connection = Connection::new(context);
        loop {
            let ready = match connection.context.get_state() {
                ContextState::Ready => true,
                ContextState::Failed | ContextState::Terminated => {
                    self.complain(connection.context.errno());
                    return Ok(());
                }
                _ => false,
            };
            while let Poll::Ready(command) = self.inbox.next() {
                let command = command.ok_or(Stop::ShellGone)?;
                // Before the server is there, nothing can be done with it.
                if ready {
                    connection.command(&command);
                }
            }
            if ready {
                self.complained = false;
                match connection.advance() {
                    Ok(Some((state, force))) => self.report(state, force)?,
                    Ok(None) => {}
                    Err(Silent) => {
                        eprintln!("gniri: audio: the sound server does not answer");
                        return Ok(());
                    }
                }
            }
            self.iterate(connection.timeout())?;
        }
    }

    fn complain(&mut self, error: PAErr) {
        if !self.complained {
            self.complained = true;
            eprintln!("gniri: audio: no sound server ({error}); waiting for one");
        }
    }

    /// Tells the shell, unless it knows already. `force` tells it anyway:
    /// what it knows may be its own wrong guess.
    fn report(&mut self, state: State, force: bool) -> Result<(), Stop> {
        if force || state != self.reported {
            self.reported = state.clone();
            self.events
                .try_send(Event::State(state))
                .map_err(|_| Stop::ShellGone)?;
        }
        Ok(())
    }

    /// Waits without a server. Commands are dropped: there is nothing to
    /// apply them to.
    fn sleep(&mut self, duration: Duration) -> Result<(), Stop> {
        let until = Instant::now() + duration;
        loop {
            while let Poll::Ready(command) = self.inbox.next() {
                command.ok_or(Stop::ShellGone)?;
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(());
            }
            self.iterate(Some(left))?;
        }
    }

    /// Runs the main loop until something happened or `timeout` has passed.
    fn iterate(&mut self, timeout: Option<Duration>) -> Result<(), Stop> {
        // Rounded up, so that the deadline has passed when the loop returns
        // for it. Every timeout here is seconds at most.
        let timeout = timeout.map(|timeout| MicroSeconds(timeout.as_micros() as u64 + 1));
        self.mainloop
            .prepare(timeout)
            .and_then(|()| self.mainloop.poll())
            .and_then(|_| self.mainloop.dispatch())
            .map(drop)
            .map_err(Stop::MainLoop)
    }
}

/// What libpulse's callbacks leave for the loop.
#[derive(Default)]
struct Shared {
    /// The server announced a change.
    changed: bool,
    /// Queries of the round in flight that are not answered yet.
    unanswered: u32,
    /// The answers so far.
    answers: Snapshot,
}

impl Shared {
    /// Takes one item of a list, or its end.
    fn list<T>(&mut self, result: ListResult<T>, add: impl FnOnce(&mut Snapshot, T)) {
        match result {
            ListResult::Item(item) => add(&mut self.answers, item),
            ListResult::End => self.answered(),
            ListResult::Error => {
                // The list stays as far as it got.
                eprintln!("gniri: audio: a query failed");
                self.answered();
            }
        }
    }

    fn answered(&mut self) {
        self.unanswered = self.unanswered.saturating_sub(1);
    }
}

/// A device chosen from a menu whose card is changing profile for it.
struct Awaited {
    direction: Direction,
    device: String,
    until: Instant,
}

/// The server stopped answering.
struct Silent;

struct Connection {
    context: Context,
    introspector: Introspector,
    shared: Rc<RefCell<Shared>>,
    subscribed: bool,
    /// The server as of the last complete round of queries, with the
    /// commands sent since assumed to have worked.
    snapshot: Snapshot,
    /// When to ask for everything again.
    refresh_at: Option<Instant>,
    /// When the round in flight was sent.
    asked_at: Option<Instant>,
    /// A command went out while a round was in flight: its answers are
    /// older than the command, and would take the shell back in time.
    overtaken: bool,
    /// A command went out since the last report. The next one is sent even
    /// if nothing came of the command, to put the shell's prediction right.
    unconfirmed: bool,
    awaited: Option<Awaited>,
}

impl Connection {
    fn new(context: Context) -> Self {
        Self {
            introspector: context.introspect(),
            context,
            shared: Rc::default(),
            subscribed: false,
            snapshot: Snapshot::default(),
            refresh_at: None,
            asked_at: None,
            overtaken: false,
            unconfirmed: false,
            awaited: None,
        }
    }

    /// Called between two iterations of the main loop once the connection
    /// is ready. Returns the state to report, if a round of queries has
    /// just completed, and whether to report it even if it is not news.
    fn advance(&mut self) -> Result<Option<(State, bool)>, Silent> {
        let now = Instant::now();
        if !self.subscribed {
            self.subscribe();
            self.refresh_at = Some(now);
        }

        let mut report = None;
        if let Some(asked_at) = self.asked_at {
            if self.shared.borrow().unanswered == 0 {
                self.asked_at = None;
                let answers = std::mem::take(&mut self.shared.borrow_mut().answers);
                if std::mem::take(&mut self.overtaken) {
                    self.refresh_at = Some(now);
                } else {
                    self.snapshot = answers;
                    report = Some((self.snapshot.state(), std::mem::take(&mut self.unconfirmed)));
                    self.resume_selection(now);
                }
            } else if now.duration_since(asked_at) > ANSWER_TIMEOUT {
                // libpulse-binding drops a failed server-info reply without
                // calling back, so a round can stay open for ever.
                return Err(Silent);
            }
        }

        if std::mem::take(&mut self.shared.borrow_mut().changed) && self.refresh_at.is_none() {
            self.refresh_at = Some(now + REFRESH_DELAY);
        }
        if self.asked_at.is_none() && self.refresh_at.is_some_and(|at| at <= now) {
            self.refresh_at = None;
            self.asked_at = Some(now);
            self.ask();
        }
        Ok(report)
    }

    /// How long the main loop may sleep if nothing happens.
    fn timeout(&self) -> Option<Duration> {
        let deadline = match self.asked_at {
            Some(asked_at) => Some(asked_at + ANSWER_TIMEOUT),
            None => self.refresh_at,
        };
        deadline.map(|deadline| deadline.saturating_duration_since(Instant::now()))
    }

    fn subscribe(&mut self) {
        let shared = Rc::clone(&self.shared);
        self.context
            .set_subscribe_callback(Some(Box::new(move |_, _, _| {
                shared.borrow_mut().changed = true;
            })));
        // Cards too: plugging in headphones changes a port of a card.
        let interest = InterestMaskSet::SERVER
            | InterestMaskSet::CARD
            | InterestMaskSet::SINK
            | InterestMaskSet::SOURCE
            | InterestMaskSet::SOURCE_OUTPUT;
        self.context.subscribe(interest, |success| {
            if !success {
                eprintln!("gniri: audio: cannot subscribe to the server's changes");
            }
        });
        self.subscribed = true;
    }

    /// Sends one round of queries: everything the snapshot holds.
    fn ask(&mut self) {
        {
            let mut shared = self.shared.borrow_mut();
            shared.answers = Snapshot::default();
            shared.unanswered = 5;
        }
        let shared = Rc::clone(&self.shared);
        self.introspector.get_server_info(move |info| {
            let mut shared = shared.borrow_mut();
            shared.answers.default_sink = info.default_sink_name.as_deref().map(str::to_owned);
            shared.answers.default_source = info.default_source_name.as_deref().map(str::to_owned);
            shared.answered();
        });
        let shared = Rc::clone(&self.shared);
        self.introspector.get_card_info_list(move |result| {
            let add = |answers: &mut Snapshot, info| answers.cards.extend(card(info));
            shared.borrow_mut().list(result, add);
        });
        let shared = Rc::clone(&self.shared);
        self.introspector.get_sink_info_list(move |result| {
            let add = |answers: &mut Snapshot, info| answers.sinks.extend(sink(info));
            shared.borrow_mut().list(result, add);
        });
        let shared = Rc::clone(&self.shared);
        self.introspector.get_source_info_list(move |result| {
            let add = |answers: &mut Snapshot, info| answers.sources.extend(source(info));
            shared.borrow_mut().list(result, add);
        });
        let shared = Rc::clone(&self.shared);
        self.introspector
            .get_source_output_info_list(move |result| {
                let add = |answers: &mut Snapshot, info| {
                    answers.source_outputs.push(source_output(info));
                };
                shared.borrow_mut().list(result, add);
            });
    }

    fn command(&mut self, command: &Command) {
        let now = Instant::now();
        let actions = self.snapshot.actions(command);
        self.perform(&actions);

        let selected = match command {
            Command::SelectOutput(device) => Some((Direction::Output, device)),
            Command::SelectInput(device) => Some((Direction::Input, device)),
            _ => None,
        };
        if let Some((direction, device)) = selected {
            // A new choice replaces one still waiting.
            let waits = self.snapshot.waits_for_profile(direction, device);
            self.awaited = waits.then(|| Awaited {
                direction,
                device: device.clone(),
                until: now + PROFILE_TIMEOUT,
            });
        }

        self.unconfirmed = true;
        self.overtaken |= !actions.is_empty() && self.asked_at.is_some();
        self.refresh_at.get_or_insert(now + REFRESH_DELAY);
    }

    /// Finishes choosing a device once its card has changed profile and its
    /// sink or source is there. gvc does the same when the new sink appears
    /// (`profile_swapping_device_id`).
    fn resume_selection(&mut self, now: Instant) {
        let Some(awaited) = &self.awaited else {
            return;
        };
        let waits = self
            .snapshot
            .waits_for_profile(awaited.direction, &awaited.device);
        if !waits {
            let actions = self.snapshot.select(awaited.direction, &awaited.device);
            self.awaited = None;
            self.perform(&actions);
        } else if now > awaited.until {
            self.awaited = None;
        }
    }

    fn perform(&mut self, actions: &[Action]) {
        for action in actions {
            self.request(action);
            self.snapshot.apply(action);
        }
    }

    fn request(&mut self, action: &Action) {
        // The requests panic unless the connection is ready; `connection`
        // passes commands on only when it is.
        let done = |what: &'static str| -> Option<Box<dyn FnMut(bool)>> {
            Some(Box::new(move |success| {
                if !success {
                    eprintln!("gniri: audio: the server would not {what}");
                }
            }))
        };
        let introspector = &mut self.introspector;
        match action {
            Action::SetVolume {
                direction: Direction::Output,
                stream,
                volume,
            } => {
                introspector.set_sink_volume_by_name(stream, volume, done("set the volume"));
            }
            Action::SetVolume {
                direction: Direction::Input,
                stream,
                volume,
            } => {
                introspector.set_source_volume_by_name(stream, volume, done("set the volume"));
            }
            Action::SetMuted {
                direction: Direction::Output,
                stream,
                muted,
            } => {
                introspector.set_sink_mute_by_name(stream, *muted, done("mute or unmute"));
            }
            Action::SetMuted {
                direction: Direction::Input,
                stream,
                muted,
            } => {
                introspector.set_source_mute_by_name(stream, *muted, done("mute or unmute"));
            }
            Action::SwitchPort {
                direction: Direction::Output,
                stream,
                port,
            } => {
                introspector.set_sink_port_by_name(stream, port, done("switch the port"));
            }
            Action::SwitchPort {
                direction: Direction::Input,
                stream,
                port,
            } => {
                introspector.set_source_port_by_name(stream, port, done("switch the port"));
            }
            Action::MakeDefault {
                direction: Direction::Output,
                stream,
            } => {
                let mut done = done("change the output");
                self.context.set_default_sink(stream, move |success| {
                    if let Some(done) = &mut done {
                        done(success);
                    }
                });
            }
            Action::MakeDefault {
                direction: Direction::Input,
                stream,
            } => {
                let mut done = done("change the input");
                self.context.set_default_source(stream, move |success| {
                    if let Some(done) = &mut done {
                        done(success);
                    }
                });
            }
            Action::SwitchProfile { card, profile } => {
                introspector.set_card_profile_by_name(card, profile, done("switch the profile"));
            }
        }
    }
}

fn owned(text: &Option<std::borrow::Cow<'_, str>>) -> Option<String> {
    text.as_deref().map(str::to_owned)
}

fn card(info: &CardInfo) -> Option<Card> {
    Some(Card {
        index: info.index,
        name: owned(&info.name)?,
        // What gvc calls the card's name.
        description: info
            .proplist
            .get_str("device.description")
            .unwrap_or_default(),
        active_profile: info
            .active_profile
            .as_ref()
            .and_then(|profile| owned(&profile.name)),
        ports: info.ports.iter().filter_map(port).collect(),
    })
}

fn port(info: &CardPortInfo) -> Option<Port> {
    Some(Port {
        name: owned(&info.name)?,
        description: owned(&info.description).unwrap_or_default(),
        // As gvc sorts them: whatever is not purely an output is an input.
        direction: if info.direction == PortDirection::OUTPUT {
            Direction::Output
        } else {
            Direction::Input
        },
        // "Unknown" is a port without jack detection, such as speakers.
        available: info.available != PortAvailable::No,
        profiles: info
            .profiles
            .iter()
            .filter_map(|profile| {
                Some(Profile {
                    name: owned(&profile.name)?,
                    priority: profile.priority,
                })
            })
            .collect(),
    })
}

fn sink(info: &SinkInfo) -> Option<Stream> {
    Some(Stream {
        name: owned(&info.name)?,
        description: owned(&info.description).unwrap_or_default(),
        card: info.card,
        volume: info.volume,
        muted: info.mute,
        form_factor: info.proplist.get_str("device.form_factor"),
        ports: info
            .ports
            .iter()
            .filter_map(|port| owned(&port.name))
            .collect(),
        active_port: info.active_port.as_ref().and_then(|port| owned(&port.name)),
        running: info.state == SinkState::Running,
    })
}

/// `None` for the monitor of a sink, which gvc "completely ignores":
/// monitors are "not real sources".
fn source(info: &SourceInfo) -> Option<Stream> {
    if info.monitor_of_sink.is_some() {
        return None;
    }
    Some(Stream {
        name: owned(&info.name)?,
        description: owned(&info.description).unwrap_or_default(),
        card: info.card,
        volume: info.volume,
        muted: info.mute,
        form_factor: info.proplist.get_str("device.form_factor"),
        ports: info
            .ports
            .iter()
            .filter_map(|port| owned(&port.name))
            .collect(),
        active_port: info.active_port.as_ref().and_then(|port| owned(&port.name)),
        running: info.state == SourceState::Running,
    })
}

fn source_output(info: &SourceOutputInfo) -> SourceOutput {
    SourceOutput {
        application_id: info.proplist.get_str("application.id"),
        // PulseAudio resamples a level meter's stream with "peaks";
        // pipewire-pulse marks the stream's node instead.
        peak_detect: info.resample_method.as_deref() == Some("peaks")
            || info.proplist.get_str("resample.peaks").as_deref() == Some("true"),
    }
}
