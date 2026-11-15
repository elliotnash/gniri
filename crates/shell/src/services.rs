//! Every service's state, in one place, for widgets to subscribe to.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gniri_config::Config;
use gniri_core::{Clock, LocalTime, Scheduler, Store};
use gniri_services::desktop_entry::DesktopEntry;
use gniri_services::keyboard::LayoutNames;
use gniri_services::user::User;
use gniri_services::{
    Bus, Fixture, Link, Model, apps, audio, bluetooth, brightness, idle, logind, mpris, network,
    night_light, niri, notifications, power, rfkill, search, session, toggles,
};
use gtk::glib;

use crate::dialogs::end_session;
use crate::menus::Menus;
use crate::overview::Overview;
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

/// The scheduler every store of this process shares.
pub fn scheduler() -> Rc<dyn Scheduler> {
    Rc::new(GlibScheduler)
}

/// The system's local time.
pub struct SystemClock;

impl SystemClock {
    pub fn read() -> (LocalTime, u32) {
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
    pub fn push(&self, service: &str, command: &dyn std::fmt::Debug) {
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
    pub fn fake(
        name: &'static str,
        state: M,
        scheduler: &Rc<dyn Scheduler>,
        log: &CommandLog,
    ) -> Self {
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
    pub fn live(
        name: &'static str,
        link: Link<M>,
        scheduler: &Rc<dyn Scheduler>,
        log: &CommandLog,
    ) -> Self
    where
        M: Default,
    {
        Self::live_then(name, link, scheduler, log, || {})
    }

    /// The same, running `ended` once the backend has stopped: its end of
    /// the events is closed.
    pub fn live_then(
        name: &'static str,
        link: Link<M>,
        scheduler: &Rc<dyn Scheduler>,
        log: &CommandLog,
        ended: impl FnOnce() + 'static,
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
                ended();
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

    /// Folds in an event as if the backend had reported it. For the debug
    /// interface, which has no system behind it to make things happen.
    pub fn inject(&self, event: M::Event) {
        self.state.update(|current| current.apply(event));
    }
}

pub struct Services {
    /// The config file's settings. They change while the shell runs
    /// ([`Services::reload`]): read them where they are used, and
    /// subscribe where a widget shows one.
    pub config: Store<Config>,
    /// The current time, updated when what the clock shows would change.
    pub time: Store<LocalTime>,
    pub niri: Store<niri::State>,
    /// Absent when gniri is not running under niri.
    pub niri_client: Option<niri::Client>,
    pub power: Service<power::State>,
    pub audio: Service<audio::State>,
    pub network: Service<network::State>,
    /// The password NetworkManager is asking for, while it asks.
    pub network_agent: Service<network::agent::State>,
    pub bluetooth: Service<bluetooth::State>,
    /// What BlueZ is asking about a pairing, while it asks.
    pub bluetooth_agent: Service<bluetooth::agent::State>,
    pub brightness: Service<brightness::State>,
    pub rfkill: Service<rfkill::State>,
    pub session: Service<session::State>,
    pub dark_style: Service<toggles::State>,
    pub night_light: Service<toggles::State>,
    pub do_not_disturb: Service<toggles::State>,
    pub notifications: Service<notifications::State>,
    pub mpris: Service<mpris::State>,
    pub idle: Service<idle::State>,
    /// The installed applications.
    pub apps: Service<apps::State>,
    /// The applications that answer the overview's search, and what they
    /// found.
    pub search: Service<search::State>,
    /// Which menu is open, and where.
    pub menus: Store<Menus>,
    /// What shows on top of niri's overview while it is open.
    pub overview: Store<Overview>,
    /// The dialog that asks before logging out, restarting or powering
    /// off, while it is open.
    pub end_session: Store<Option<end_session::Kind>>,
    /// Who is logged in.
    pub user: User,
    pub commands: CommandLog,
    /// True under a fixture: nothing the shell does may reach the system.
    pub sandboxed: bool,
    /// The settings application, if it is configured and installed.
    settings_program: RefCell<Option<String>>,
    /// Changes Night Light's colour while it runs.
    night_light_tuner: night_light::backend::Tuner,
    /// Keeps `time` current; absent while a fixture has it stand still.
    ticker: Option<Ticker>,
    layout_names: OnceCell<LayoutNames>,
    /// Keeps the notification state in step with the Do Not Disturb toggle.
    _do_not_disturb_link: gniri_core::Subscription,
}

impl Services {
    /// Starts every service. With a fixture the clock is frozen at its time,
    /// every service takes its state from it and no backend runs.
    ///
    /// `locker_arguments` are what the screen locker is started with
    /// before its subcommand: the options that make it read the same
    /// configuration as this process.
    pub fn start(
        config: Config,
        fixture: Option<Fixture>,
        locker_arguments: Vec<String>,
    ) -> Rc<Self> {
        let scheduler = scheduler();
        let log = CommandLog::default();

        let fixed = fixture.as_ref().and_then(|fixture| {
            fixture.clock().unwrap_or_else(|error| {
                eprintln!("gniri: fixture {error}");
                None
            })
        });
        let time = match fixed {
            Some(clock) => Store::new(clock.now(), Rc::clone(&scheduler)),
            None => Store::new(SystemClock.now(), Rc::clone(&scheduler)),
        };
        // A scene's clock stands still.
        let ticker = fixed
            .is_none()
            .then(|| Ticker::start(time.clone(), config.clock.show_seconds));

        let niri = Store::new(niri::State::default(), Rc::clone(&scheduler));
        let socket = niri::socket_path();
        match &socket {
            Some(socket) => follow_niri(niri.clone(), niri::watch(socket)),
            None => eprintln!("gniri: NIRI_SOCKET is not set; workspaces will not be shown"),
        }

        let sandboxed = fixture.is_some();
        let settings_program = RefCell::new(settings_program(&config, sandboxed));
        let menus = Store::new(Menus::default(), Rc::clone(&scheduler));
        let overview = Store::new(Overview::default(), Rc::clone(&scheduler));
        let end_session = Store::new(None, Rc::clone(&scheduler));
        let (scheduler, log_ref) = (&scheduler, &log);
        let services = match fixture {
            Some(fixture) => {
                let do_not_disturb =
                    Service::fake("do-not-disturb", fixture.do_not_disturb, scheduler, log_ref);
                let notifications =
                    Service::fake("notifications", fixture.notifications, scheduler, log_ref);
                let link = link_do_not_disturb(&do_not_disturb, &notifications);
                let apps = Service::fake("apps", apps::State::default(), scheduler, log_ref);
                // Through the event, which puts a scene's list in order.
                apps.inject(apps::Event::Installed(fixture.apps.installed));
                Self {
                    apps,
                    search: Service::live(
                        "search",
                        canned_search(fixture.search),
                        scheduler,
                        log_ref,
                    ),
                    power: Service::fake("power", fixture.power, scheduler, log_ref),
                    audio: Service::fake("audio", fixture.audio, scheduler, log_ref),
                    network: Service::fake("network", fixture.network, scheduler, log_ref),
                    network_agent: Service::fake(
                        "network-agent",
                        fixture.network_agent,
                        scheduler,
                        log_ref,
                    ),
                    bluetooth: Service::fake("bluetooth", fixture.bluetooth, scheduler, log_ref),
                    bluetooth_agent: Service::fake(
                        "bluetooth-agent",
                        fixture.bluetooth_agent,
                        scheduler,
                        log_ref,
                    ),
                    brightness: Service::fake("brightness", fixture.brightness, scheduler, log_ref),
                    rfkill: Service::fake("rfkill", fixture.rfkill, scheduler, log_ref),
                    session: Service::fake("session", fixture.session, scheduler, log_ref),
                    dark_style: Service::fake("dark-style", fixture.dark_style, scheduler, log_ref),
                    night_light: Service::fake(
                        "night-light",
                        fixture.night_light,
                        scheduler,
                        log_ref,
                    ),
                    do_not_disturb,
                    notifications,
                    mpris: Service::fake("mpris", fixture.mpris, scheduler, log_ref),
                    idle: Service::fake("idle", fixture.idle, scheduler, log_ref),
                    user: fixture.user,
                    _do_not_disturb_link: link,
                    config: Store::new(config, Rc::clone(scheduler)),
                    night_light_tuner: night_light::backend::Tuner::default(),
                    ticker,
                    time,
                    niri,
                    niri_client: socket.map(niri::Client::new),
                    menus,
                    overview,
                    end_session,
                    commands: log.clone(),
                    sandboxed,
                    settings_program,
                    layout_names: OnceCell::new(),
                }
            }
            None => {
                let mut bus = Bus::system();
                let power = bus.add("power", power::backend::run);
                let network = bus.add("network", network::backend::run);
                let network_agent = bus.add("network agent", network::agent::backend::run);
                let bluetooth = bus.add("bluetooth", bluetooth::backend::run);
                let bluetooth_agent = bus.add("bluetooth agent", bluetooth::agent::backend::run);
                let brightness = bus.add("brightness", brightness::backend::run);
                let session = bus.add("session", {
                    let options = session::backend::Options {
                        session_id: logind::session_id_from_env(),
                        lock_on_suspend: config.lock.on_suspend,
                    };
                    let locker = session::locker::Process::this_program(locker_arguments);
                    move |connection, events, commands| {
                        session::backend::run(
                            connection,
                            events,
                            commands,
                            Box::new(locker),
                            options,
                        )
                    }
                });
                bus.start();

                let mut session_bus = Bus::session();
                let notifications =
                    session_bus.add("notifications", |connection, events, commands| {
                        notifications::backend::run(connection, events, commands, || {
                            SystemClock.now()
                        })
                    });
                let mpris = session_bus.add("mpris", mpris::backend::run);
                let search = {
                    let options = search_options(&config);
                    session_bus.add("search", move |connection, events, commands| {
                        search::backend::run(connection, events, commands, options)
                    })
                };
                let search = Service::live("search", search, scheduler, log_ref);
                let (night_light, night_light_tuner) =
                    night_light::backend::spawn(config.night_light.temperature);
                session_bus.start();
                let notifications =
                    Service::live("notifications", notifications, scheduler, log_ref);
                // GNOME keeps this in a setting; here it lasts until gniri
                // restarts, like Night Light.
                let do_not_disturb = Service::fake(
                    "do-not-disturb",
                    toggles::State {
                        available: true,
                        on: false,
                    },
                    scheduler,
                    log_ref,
                );
                let link = link_do_not_disturb(&do_not_disturb, &notifications);
                Self {
                    apps: Service::live("apps", apps::backend::spawn(), scheduler, log_ref),
                    search,
                    power: Service::live("power", power, scheduler, log_ref),
                    audio: Service::live("audio", audio::backend::spawn(), scheduler, log_ref),
                    network: Service::live("network", network, scheduler, log_ref),
                    network_agent: Service::live(
                        "network-agent",
                        network_agent,
                        scheduler,
                        log_ref,
                    ),
                    bluetooth: Service::live("bluetooth", bluetooth, scheduler, log_ref),
                    bluetooth_agent: Service::live(
                        "bluetooth-agent",
                        bluetooth_agent,
                        scheduler,
                        log_ref,
                    ),
                    brightness: Service::live("brightness", brightness, scheduler, log_ref),
                    rfkill: Service::live("rfkill", rfkill::backend::spawn(), scheduler, log_ref),
                    session: Service::live("session", session, scheduler, log_ref),
                    dark_style: Service::live(
                        "dark-style",
                        settings::dark_style(),
                        scheduler,
                        log_ref,
                    ),
                    night_light: Service::live("night-light", night_light, scheduler, log_ref),
                    night_light_tuner,
                    do_not_disturb,
                    notifications,
                    mpris: Service::live("mpris", mpris, scheduler, log_ref),
                    idle: Service::live(
                        "idle",
                        idle::backend::spawn(idle::timers(&idle_settings(&config))),
                        scheduler,
                        log_ref,
                    ),
                    user: User::current(),
                    _do_not_disturb_link: link,
                    config: Store::new(config, Rc::clone(scheduler)),
                    ticker,
                    time,
                    niri,
                    niri_client: socket.map(niri::Client::new),
                    menus,
                    overview,
                    end_session,
                    commands: log.clone(),
                    sandboxed,
                    settings_program,
                    layout_names: OnceCell::new(),
                }
            }
        };
        Rc::new(services)
    }

    /// Takes a changed config file's settings into use.
    ///
    /// What widgets show follows through [`Services::config`]; the
    /// backends that were started with a setting are told its new value.
    /// Under a fixture those are stand-ins, and the commands are only
    /// logged.
    pub fn reload(&self, config: Config) {
        let old = self.config.get();
        if config == old {
            return;
        }
        if (&config.idle, config.lock.automatic, config.lock.delay)
            != (&old.idle, old.lock.automatic, old.lock.delay)
        {
            let timers = idle::timers(&idle_settings(&config));
            self.idle.send(idle::Command::SetTimers(timers));
        }
        if config.lock.on_suspend != old.lock.on_suspend {
            self.session
                .send(session::Command::SetLockOnSuspend(config.lock.on_suspend));
        }
        if config.night_light != old.night_light {
            self.commands.push(
                "night-light",
                &format_args!("SetTemperature({})", config.night_light.temperature),
            );
            self.night_light_tuner
                .set_temperature(config.night_light.temperature);
        }
        if config.search != old.search {
            self.search
                .send(search::Command::Configure(search_options(&config)));
        }
        if config.clock.show_seconds != old.clock.show_seconds
            && let Some(ticker) = &self.ticker
        {
            ticker.restart(config.clock.show_seconds);
        }
        if config.commands != old.commands {
            self.settings_program
                .replace(settings_program(&config, self.sandboxed));
        }
        self.config.update(|current| *current = config);
    }

    /// The XKB registry's short layout names, read on first use: most people
    /// have one layout and never need it.
    pub fn layout_names(&self) -> &LayoutNames {
        self.layout_names.get_or_init(LayoutNames::load_system)
    }

    /// Whether there is a settings application to open.
    pub fn has_settings(&self) -> bool {
        self.settings_program.borrow().is_some()
    }

    /// Closes any open menu and opens `panel` of the settings application
    /// (its main window for an empty `panel`).
    pub fn open_settings(&self, panel: &str) {
        self.menus.update(|menus| menus.close());
        let Some(program) = self.settings_program.borrow().clone() else {
            return;
        };
        let mut command = vec![program.as_str()];
        if !panel.is_empty() {
            command.push(panel);
        }
        self.launch(&command);
    }

    /// Starts a program, detached. Under a fixture it is only logged.
    ///
    /// niri starts it where there is one, as it starts the programs its
    /// key bindings name: the program is then no child of gniri's, and
    /// under systemd it gets a scope of its own instead of living in
    /// gniri's unit.
    pub fn launch(&self, command: &[&str]) {
        self.commands.push("launch", &command);
        let Some((program, arguments)) = command.split_first() else {
            return;
        };
        if self.sandboxed {
            return;
        }
        if let Some(client) = &self.niri_client {
            let command = command.iter().map(|&word| word.to_owned()).collect();
            match client.spawn(command) {
                Ok(()) => return,
                Err(error) => eprintln!("gniri: niri cannot start {program}: {error}"),
            }
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

    /// Starts an installed application by its desktop entry. Returns false
    /// if there is no such entry or it has no command.
    pub fn launch_app(&self, id: &str) -> bool {
        // Under a fixture nothing is started, so nothing need be installed.
        if self.sandboxed {
            self.commands.push("launch-app", &id);
            return true;
        }
        // The list has what the overview shows; an entry that is not for
        // menus (`NoDisplay`) is read from its file.
        let listed = self
            .apps
            .state
            .with(|apps| apps.app(id).map(|app| (app.command(), app.terminal)));
        let (command, terminal) = match listed {
            Some(found) => found,
            None => match DesktopEntry::find(id) {
                Some(entry) => (entry.command(), entry.terminal),
                None => return false,
            },
        };
        let Some(mut command) = command else {
            return false;
        };
        if terminal {
            let Some(mut terminal) = apps::terminal_prefix(on_path) else {
                eprintln!("gniri: {id} wants a terminal and none is installed");
                return false;
            };
            terminal.append(&mut command);
            command = terminal;
        }
        let command: Vec<&str> = command.iter().map(String::as_str).collect();
        self.launch(&command);
        true
    }

    /// Whether an application with this desktop entry is installed. Always
    /// true under a fixture, where it would not be started anyway.
    pub fn has_app(&self, id: &str) -> bool {
        self.sandboxed || DesktopEntry::find(id).is_some()
    }

    /// Gives a window the keyboard focus, on whatever workspace it is.
    pub fn focus_window(&self, id: u64) {
        self.commands
            .push("niri", &format_args!("FocusWindow({id})"));
        if let Some(client) = &self.niri_client
            && let Err(error) = client.focus_window(id)
        {
            eprintln!("gniri: cannot focus a window: {error}");
        }
    }

    /// Closes niri's overview, and with it everything gniri shows on it.
    pub fn close_overview(&self) {
        self.niri_action(niri_ipc::Action::CloseOverview {});
    }

    /// Sends an action to niri, reporting a failure on stderr.
    pub fn niri_action(&self, action: niri_ipc::Action) {
        if let Some(client) = &self.niri_client
            && let Err(error) = client.action(action)
        {
            eprintln!("gniri: niri refused an action: {error}");
        }
    }

    /// An action that would end or blank a test's own session: under a
    /// fixture it is only logged.
    fn drastic_niri_action(&self, action: niri_ipc::Action) {
        self.commands.push("niri", &action);
        if !self.sandboxed {
            self.niri_action(action);
        }
    }

    /// Ends the session: niri quits, and with it everything it started.
    pub fn log_out(&self) {
        self.drastic_niri_action(niri_ipc::Action::Quit {
            skip_confirmation: true,
        });
    }

    /// Turns every screen off until the next input.
    pub fn power_off_monitors(&self) {
        self.drastic_niri_action(niri_ipc::Action::PowerOffMonitors {});
    }
}

/// The settings application to start: the configured one, if it is
/// installed. Under a fixture the program is never started, so it need not
/// be installed for its buttons to show.
fn settings_program(config: &Config, sandboxed: bool) -> Option<String> {
    Some(config.commands.settings.clone())
        .filter(|program| !program.is_empty() && (sandboxed || on_path(program)))
}

/// The `[search]` settings as the search service takes them.
fn search_options(config: &Config) -> search::Options {
    search::Options {
        enabled: config.search.providers,
        disabled: config.search.disabled.clone(),
    }
}

/// The `[idle]` and `[lock]` settings as the idle service takes them.
fn idle_settings(config: &Config) -> idle::Settings {
    idle::Settings {
        blank_after: (config.idle.blank_after > 0)
            .then(|| Duration::from_secs(u64::from(config.idle.blank_after))),
        dim: config.idle.dim,
        lock: config.lock.automatic,
        lock_delay: Duration::from_secs(u64::from(config.lock.delay)),
    }
}

/// Makes the notification state follow the Do Not Disturb toggle, from the
/// toggle's present value on.
fn link_do_not_disturb(
    toggle: &Service<toggles::State>,
    notifications: &Service<notifications::State>,
) -> gniri_core::Subscription {
    let notifications = notifications.state.clone();
    toggle.state.select(
        |state| state.on,
        move |on| notifications.update(|state| state.do_not_disturb = *on),
    )
}

/// A stand-in for the search providers under a fixture: whatever is
/// searched for, each provider of the scene answers with the results the
/// scene gives it.
fn canned_search(canned: search::State) -> Link<search::State> {
    let (link, events, commands) = Link::new();
    glib::spawn_future_local(async move {
        let infos = canned
            .providers
            .iter()
            .map(|provider| search::ProviderInfo {
                id: provider.id.clone(),
                name: provider.name.clone(),
                icon: provider.icon.clone(),
            })
            .collect();
        let _ = events.send(search::Event::Providers(infos)).await;
        while let Ok(command) = commands.recv().await {
            let search::Command::Search(terms) = command else {
                continue;
            };
            if terms.is_empty() {
                continue;
            }
            for provider in &canned.providers {
                let _ = events
                    .send(search::Event::Results {
                        terms: terms.clone(),
                        provider: provider.id.clone(),
                        results: provider.results.clone(),
                        total: provider.total,
                    })
                    .await;
            }
        }
    });
    link
}

/// True if `program` is an executable on `PATH` (or a path to one).
fn on_path(program: &str) -> bool {
    if program.contains('/') {
        return std::path::Path::new(program).is_file();
    }
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

/// Keeps a clock's time current.
pub struct Ticker {
    time: Store<LocalTime>,
    /// Counts restarts: a timer of an earlier one finds a newer number and
    /// stops.
    generation: Rc<Cell<u64>>,
}

impl Ticker {
    /// Updates `time` at each minute boundary, or at each second's with
    /// `every_second`: when what the clock shows would change.
    ///
    /// The timer does not run while the machine is suspended, so after a
    /// resume the clock can be stale until the next boundary.
    pub fn start(time: Store<LocalTime>, every_second: bool) -> Self {
        let ticker = Self {
            time,
            generation: Rc::default(),
        };
        ticker.restart(every_second);
        ticker
    }

    /// Starts over, at the other pace.
    pub fn restart(&self, every_second: bool) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        self.time.update(|current| *current = SystemClock.now());
        tick(
            self.time.clone(),
            every_second,
            Rc::clone(&self.generation),
            generation,
        );
    }
}

fn tick(time: Store<LocalTime>, every_second: bool, current: Rc<Cell<u64>>, generation: u64) {
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
        if current.get() != generation {
            return;
        }
        time.update(|current| *current = SystemClock.now());
        tick(time, every_second, current, generation);
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
