//! Every service's state, in one place, for widgets to subscribe to.

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gniri_config::Config;
use gniri_core::{Clock, LocalTime, Scheduler, Store};
use gniri_services::keyboard::LayoutNames;
use gniri_services::{
    Fixture, Link, Model, SystemBus, audio, bluetooth, brightness, network, niri, power, rfkill,
    session, toggles,
};
use gtk::glib;

use crate::menus::Menus;
use crate::settings;

/// Runs store notifications from the GLib main loop.
///
/// Default priority, which is ahead of GTK's layout and drawing: a state
/// change reaches the widgets before the frame that would show it.
struct GlibScheduler;

impl Scheduler for GlibScheduler {
    fn schedule(&self, task: Box<dyn FnOnce()>) {
        glib::idle_add_local_full(glib::Priority::DEFAULT, {
            let mut task = Some(task);
            move || {
                if let Some(task) = task.take() {
                    task();
                }
                glib::ControlFlow::Break
            }
        });
    }
}

/// The system's local time.
struct SystemClock;

impl SystemClock {
    fn read() -> (LocalTime, u32) {
        let now = glib::DateTime::now_local().expect("the local time is representable");
        let time = LocalTime {
            year: now.year(),
            month: now.month() as u8,
            day: now.day_of_month() as u8,
            hour: now.hour() as u8,
            minute: now.minute() as u8,
            second: now.second() as u8,
        };
        (time, now.microsecond() as u32)
    }
}

impl Clock for SystemClock {
    fn now(&self) -> LocalTime {
        Self::read().0
    }
}

/// Every command the shell sent, in order, as text. Under a fixture
/// nothing else happens to a command, so a test can assert on what a click
/// asked for (`gniri msg debug commands`).
#[derive(Clone, Default)]
pub struct CommandLog(Rc<RefCell<Vec<String>>>);

impl CommandLog {
    fn push(&self, service: &str, command: &dyn std::fmt::Debug) {
        self.0.borrow_mut().push(format!("{service}: {command:?}"));
    }

    pub fn entries(&self) -> Vec<String> {
        self.0.borrow().clone()
    }
}

/// One service as widgets see it: state to subscribe to and a way to ask
/// for changes.
pub struct Service<M: Model> {
    pub state: Store<M>,
    send: Box<dyn Fn(M::Command)>,
}

impl<M: Model> Service<M> {
    /// A service with no system behind it: a command only changes the state
    /// the way [`Model::predict`] says, and is logged.
    fn fake(name: &'static str, state: M, scheduler: &Rc<dyn Scheduler>, log: &CommandLog) -> Self {
        let state = Store::new(state, Rc::clone(scheduler));
        let send = {
            let (state, log) = (state.clone(), log.clone());
            move |command: M::Command| {
                log.push(name, &command);
                state.update(|current| current.predict(&command));
            }
        };
        Self {
            state,
            send: Box::new(send),
        }
    }

    /// A service fed by a running backend. Commands are predicted at once,
    /// so widgets respond without waiting for the system, and whatever the
    /// backend reports afterwards is the truth.
    fn live(
        name: &'static str,
        link: Link<M>,
        scheduler: &Rc<dyn Scheduler>,
        log: &CommandLog,
    ) -> Self
    where
        M: Default,
    {
        let state = Store::new(M::default(), Rc::clone(scheduler));
        let Link { events, commands } = link;
        glib::spawn_future_local({
            let state = state.clone();
            async move {
                while let Ok(event) = events.recv().await {
                    state.update(|current| current.apply(event));
                }
            }
        });
        let send = {
            let (state, log) = (state.clone(), log.clone());
            move |command: M::Command| {
                log.push(name, &command);
                state.update(|current| current.predict(&command));
                // The channel is unbounded; this only fails once the
                // backend has stopped, and then there is no one to tell.
                let _ = commands.try_send(command);
            }
        };
        Self {
            state,
            send: Box::new(send),
        }
    }

    pub fn send(&self, command: M::Command) {
        (self.send)(command);
    }
}

pub struct Services {
    pub config: Config,
    /// The current time, updated when what the clock shows would change.
    pub time: Store<LocalTime>,
    pub niri: Store<niri::State>,
    /// Absent when gniri is not running under niri.
    pub niri_client: Option<niri::Client>,
    pub power: Service<power::State>,
    pub audio: Service<audio::State>,
    pub network: Service<network::State>,
    pub bluetooth: Service<bluetooth::State>,
    pub brightness: Service<brightness::State>,
    pub rfkill: Service<rfkill::State>,
    pub session: Service<session::State>,
    pub dark_style: Service<toggles::State>,
    pub night_light: Service<toggles::State>,
    pub do_not_disturb: Service<toggles::State>,
    /// Which menu is open, and where.
    pub menus: Store<Menus>,
    pub commands: CommandLog,
    /// True under a fixture: nothing the shell does may reach the system.
    pub sandboxed: bool,
    /// The settings application, if it is configured and installed.
    settings_program: Option<String>,
    layout_names: OnceCell<LayoutNames>,
}

impl Services {
    /// Starts every service. With a fixture the clock is frozen at its time,
    /// every service takes its state from it and no backend runs.
    pub fn start(config: Config, fixture: Option<Fixture>) -> Rc<Self> {
        let scheduler: Rc<dyn Scheduler> = Rc::new(GlibScheduler);
        let log = CommandLog::default();

        let fixed = fixture.as_ref().and_then(|fixture| {
            fixture.clock().unwrap_or_else(|error| {
                eprintln!("gniri: fixture {error}");
                None
            })
        });
        let time = match fixed {
            Some(clock) => Store::new(clock.now(), Rc::clone(&scheduler)),
            None => {
                let time = Store::new(SystemClock.now(), Rc::clone(&scheduler));
                tick(time.clone(), config.clock.show_seconds);
                time
            }
        };

        let niri = Store::new(niri::State::default(), Rc::clone(&scheduler));
        let socket = niri::socket_path();
        match &socket {
            Some(socket) => follow_niri(niri.clone(), niri::watch(socket)),
            None => eprintln!("gniri: NIRI_SOCKET is not set; workspaces will not be shown"),
        }

        let sandboxed = fixture.is_some();
        // Under a fixture the program is never started, so it need not be
        // installed for its buttons to show.
        let settings_program = Some(config.commands.settings.clone())
            .filter(|program| !program.is_empty() && (sandboxed || on_path(program)));
        let menus = Store::new(Menus::default(), Rc::clone(&scheduler));
        let (scheduler, log_ref) = (&scheduler, &log);
        let services = match fixture {
            Some(fixture) => Self {
                power: Service::fake("power", fixture.power, scheduler, log_ref),
                audio: Service::fake("audio", fixture.audio, scheduler, log_ref),
                network: Service::fake("network", fixture.network, scheduler, log_ref),
                bluetooth: Service::fake("bluetooth", fixture.bluetooth, scheduler, log_ref),
                brightness: Service::fake("brightness", fixture.brightness, scheduler, log_ref),
                rfkill: Service::fake("rfkill", fixture.rfkill, scheduler, log_ref),
                session: Service::fake("session", fixture.session, scheduler, log_ref),
                dark_style: Service::fake("dark-style", fixture.dark_style, scheduler, log_ref),
                night_light: Service::fake("night-light", fixture.night_light, scheduler, log_ref),
                do_not_disturb: Service::fake(
                    "do-not-disturb",
                    fixture.do_not_disturb,
                    scheduler,
                    log_ref,
                ),
                config,
                time,
                niri,
                niri_client: socket.map(niri::Client::new),
                menus,
                commands: log.clone(),
                sandboxed,
                settings_program,
                layout_names: OnceCell::new(),
            },
            None => {
                let mut bus = SystemBus::new();
                let power = bus.add("power", power::backend::run);
                let network = bus.add("network", network::backend::run);
                let bluetooth = bus.add("bluetooth", bluetooth::backend::run);
                let brightness = bus.add("brightness", brightness::backend::run);
                let session = bus.add("session", session::backend::run);
                bus.start();
                Self {
                    power: Service::live("power", power, scheduler, log_ref),
                    audio: Service::live("audio", audio::backend::spawn(), scheduler, log_ref),
                    network: Service::live("network", network, scheduler, log_ref),
                    bluetooth: Service::live("bluetooth", bluetooth, scheduler, log_ref),
                    brightness: Service::live("brightness", brightness, scheduler, log_ref),
                    rfkill: Service::live("rfkill", rfkill::backend::spawn(), scheduler, log_ref),
                    session: Service::live("session", session, scheduler, log_ref),
                    dark_style: Service::live(
                        "dark-style",
                        settings::dark_style(),
                        scheduler,
                        log_ref,
                    ),
                    // Until the notification server exists (M3) the toggle
                    // only remembers its own state.
                    night_light: Service::fake(
                        "night-light",
                        toggles::State::default(),
                        scheduler,
                        log_ref,
                    ),
                    do_not_disturb: Service::fake(
                        "do-not-disturb",
                        toggles::State {
                            available: true,
                            on: false,
                        },
                        scheduler,
                        log_ref,
                    ),
                    config,
                    time,
                    niri,
                    niri_client: socket.map(niri::Client::new),
                    menus,
                    commands: log.clone(),
                    sandboxed,
                    settings_program,
                    layout_names: OnceCell::new(),
                }
            }
        };
        Rc::new(services)
    }

    /// The XKB registry's short layout names, read on first use: most people
    /// have one layout and never need it.
    pub fn layout_names(&self) -> &LayoutNames {
        self.layout_names.get_or_init(LayoutNames::load_system)
    }

    /// Whether there is a settings application to open.
    pub fn has_settings(&self) -> bool {
        self.settings_program.is_some()
    }

    /// Closes any open menu and opens `panel` of the settings application
    /// (its main window for an empty `panel`).
    pub fn open_settings(&self, panel: &str) {
        self.menus.update(|menus| menus.close());
        let Some(program) = &self.settings_program else {
            return;
        };
        let mut command = vec![program.as_str()];
        if !panel.is_empty() {
            command.push(panel);
        }
        self.launch(&command);
    }

    /// Starts a program, detached. Under a fixture it is only logged.
    pub fn launch(&self, command: &[&str]) {
        self.commands.push("launch", &command);
        let Some((program, arguments)) = command.split_first() else {
            return;
        };
        if self.sandboxed {
            return;
        }
        match std::process::Command::new(program).args(arguments).spawn() {
            // Wait for it on a thread of its own, so it does not stay
            // behind as a zombie when it exits.
            Ok(mut child) => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Err(error) => eprintln!("gniri: cannot start {program}: {error}"),
        }
    }

    /// Sends an action to niri, reporting a failure on stderr.
    pub fn niri_action(&self, action: niri_ipc::Action) {
        if let Some(client) = &self.niri_client
            && let Err(error) = client.action(action)
        {
            eprintln!("gniri: niri refused an action: {error}");
        }
    }
}

/// True if `program` is an executable on `PATH` (or a path to one).
fn on_path(program: &str) -> bool {
    if program.contains('/') {
        return std::path::Path::new(program).is_file();
    }
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

/// Updates `time` at each minute (or second) boundary.
///
/// The timer does not run while the machine is suspended, so after a resume
/// the clock can be stale until the next boundary.
fn tick(time: Store<LocalTime>, every_second: bool) {
    let (now, microsecond) = SystemClock::read();
    let whole_seconds = if every_second {
        0
    } else {
        59 - u64::from(now.second.min(59))
    };
    let until_boundary = Duration::from_secs(whole_seconds)
        + Duration::from_micros(1_000_000 - u64::from(microsecond.min(999_999)))
        // Land just after the boundary, never just before it.
        + Duration::from_millis(5);
    glib::timeout_add_local_once(until_boundary, move || {
        time.update(|current| *current = SystemClock.now());
        tick(time, every_second);
    });
}

fn follow_niri(store: Store<niri::State>, messages: async_channel::Receiver<niri::Message>) {
    glib::spawn_future_local(async move {
        let mut tracker = niri::Tracker::default();
        while let Ok(message) = messages.recv().await {
            match message {
                niri::Message::Event(event) => {
                    let state = tracker.apply(event);
                    store.update(|current| *current = state);
                }
                niri::Message::Disconnected(reason) => {
                    eprintln!("gniri: lost the niri event stream: {reason}");
                    store.update(|current| current.connected = false);
                    break;
                }
            }
        }
    });
}
