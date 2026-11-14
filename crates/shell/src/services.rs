//! Every service's state, in one place, for widgets to subscribe to.

use std::cell::OnceCell;
use std::rc::Rc;
use std::time::Duration;

use gniri_config::Config;
use gniri_core::{Clock, LocalTime, Scheduler, Store};
use gniri_services::keyboard::LayoutNames;
use gniri_services::{Fixture, audio, network, niri, power};
use gtk::glib;

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

pub struct Services {
    pub config: Config,
    /// The current time, updated when what the clock shows would change.
    pub time: Store<LocalTime>,
    pub niri: Store<niri::State>,
    /// Absent when gniri is not running under niri.
    pub niri_client: Option<niri::Client>,
    pub power: Store<power::State>,
    pub audio: Store<audio::State>,
    pub network: Store<network::State>,
    layout_names: OnceCell<LayoutNames>,
}

impl Services {
    /// Starts every service. With a fixture the clock is frozen at its time
    /// and the services without a backend yet take their state from it.
    pub fn start(config: Config, fixture: Option<Fixture>) -> Rc<Self> {
        let scheduler: Rc<dyn Scheduler> = Rc::new(GlibScheduler);
        let fixture = fixture.unwrap_or_default();

        let fixed = fixture.clock().unwrap_or_else(|error| {
            eprintln!("gniri: fixture {error}");
            None
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

        Rc::new(Self {
            config,
            time,
            niri,
            niri_client: socket.map(niri::Client::new),
            power: Store::new(fixture.power, Rc::clone(&scheduler)),
            audio: Store::new(fixture.audio, Rc::clone(&scheduler)),
            network: Store::new(fixture.network, scheduler),
            layout_names: OnceCell::new(),
        })
    }

    /// The XKB registry's short layout names, read on first use: most people
    /// have one layout and never need it.
    pub fn layout_names(&self) -> &LayoutNames {
        self.layout_names.get_or_init(LayoutNames::load_system)
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
