// SPDX-License-Identifier: GPL-3.0-or-later

//! The session backend against python-dbusmock's logind, with a stand-in
//! for the screen locker.

mod common;

use std::time::{Duration, Instant};

use async_channel::{Receiver, Sender};
use common::{Follower, MockBus, TIMEOUT};
use gniri_services::session::backend::{self, Options};
use gniri_services::session::locker::{Exit, Locker, Report};
use gniri_services::session::{self, Command};
use zbus::Variant;

const LOGIND: &str = "org.freedesktop.login1";
const MANAGER_PATH: &str = "/org/freedesktop/login1";
const MANAGER: &str = "org.freedesktop.login1.Manager";
const SESSION_PATH: &str = "/org/freedesktop/login1/session/c1";
const SESSION: &str = "org.freedesktop.login1.Session";
const MOCK: &str = "org.freedesktop.DBus.Mock";

/// A locker the test plays: each start hands the test the channel a real
/// locker would report on.
struct StandIn {
    starts: Sender<Sender<Report>>,
}

impl Locker for StandIn {
    fn start(&self, reports: Sender<Report>) -> std::io::Result<()> {
        self.starts
            .send_blocking(reports)
            .map_err(|error| std::io::Error::other(error.to_string()))
    }
}

struct Setup {
    bus: MockBus,
    session: Follower<session::State>,
    starts: Receiver<Sender<Report>>,
}

fn setup(lock_on_suspend: bool) -> Option<Setup> {
    let mut bus = MockBus::start()?;
    bus.template(
        "logind",
        LOGIND,
        r#"{"CanReboot": "challenge", "CanPowerOff": "no"}"#,
    );
    bus.call(LOGIND, MANAGER_PATH, MOCK, "AddSeat", &("seat0"));
    bus.call(
        LOGIND,
        MANAGER_PATH,
        MOCK,
        "AddUser",
        &(1000u32, "ada", true),
    );
    bus.call(
        LOGIND,
        MANAGER_PATH,
        MOCK,
        "AddSession",
        &("c1", "seat0", 1000u32, "ada", true),
    );

    let (starts_sender, starts) = async_channel::unbounded();
    let link = bus.run(move |connection, events, commands| {
        backend::run(
            connection,
            events,
            commands,
            Box::new(StandIn {
                starts: starts_sender,
            }),
            Options {
                session_id: Some("c1".to_owned()),
                lock_on_suspend,
            },
        )
    });
    Some(Setup {
        bus,
        session: Follower::new(link),
        starts,
    })
}

impl Setup {
    /// Waits for the backend to start a locker.
    fn started(&self) -> Sender<Report> {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Ok(reports) = self.starts.try_recv() {
                return reports;
            }
            assert!(Instant::now() < deadline, "no locker was started");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn not_started(&self) {
        std::thread::sleep(Duration::from_millis(300));
        assert!(self.starts.try_recv().is_err(), "a locker was started");
    }

    /// Whether gniri holds logind back from sleeping.
    fn delays_sleep(&self) -> bool {
        let inhibitors: Vec<(String, String, String, String, u32, u32)> = self
            .bus
            .call(LOGIND, MANAGER_PATH, MANAGER, "ListInhibitors", &())
            .body()
            .deserialize()
            .expect("the inhibitors");
        inhibitors
            .iter()
            .any(|(what, who, _, mode, _, _)| what == "sleep" && who == "gniri" && mode == "delay")
    }

    fn wait_for_sleep_delay(&self, held: bool) {
        let deadline = Instant::now() + TIMEOUT;
        while self.delays_sleep() != held {
            assert!(
                Instant::now() < deadline,
                "the sleep inhibitor never became {held}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// logind announcing that the machine is about to sleep, or has woken.
    fn prepare_for_sleep(&self, start: bool) {
        self.bus.call(
            LOGIND,
            MANAGER_PATH,
            MOCK,
            "EmitSignal",
            &(MANAGER, "PrepareForSleep", "b", vec![Variant::Bool(start)]),
        );
    }

    fn locked_hint(&self) -> bool {
        self.bus
            .property(LOGIND, SESSION_PATH, SESSION, "LockedHint")
    }
}

#[test]
fn a_lock_request_starts_the_locker_and_its_reports_become_state() {
    let Some(mut setup) = setup(true) else {
        return;
    };
    setup
        .session
        .wait("what logind allows", |state| state.can_suspend);
    assert!(setup.session.state.can_restart, "challenge counts as yes");
    assert!(!setup.session.state.can_power_off);
    assert!(!setup.session.state.locked);

    // The command only asks logind; the locker starts on logind's signal,
    // as it would for `loginctl lock-session`.
    setup.session.send(Command::Lock);
    let locker = setup.started();
    assert!(!setup.session.state.locked, "not until the locker says so");
    locker.send_blocking(Report::Locked).unwrap();
    setup.session.wait("the lock", |state| state.locked);
    assert!(setup.locked_hint());

    // A second request while one locker runs starts no second one.
    setup.session.send(Command::Lock);
    setup.not_started();

    locker.send_blocking(Report::Gone(Exit::Unlocked)).unwrap();
    setup.session.wait("the unlock", |state| !state.locked);
    assert!(!setup.locked_hint());
}

#[test]
fn sleep_waits_for_the_lock() {
    let Some(mut setup) = setup(true) else {
        return;
    };
    setup.session.wait("logind", |state| state.can_suspend);
    setup.wait_for_sleep_delay(true);

    // Going to sleep unlocked: the locker starts, and logind is held back
    // until the screen is covered.
    setup.prepare_for_sleep(true);
    let locker = setup.started();
    std::thread::sleep(Duration::from_millis(200));
    assert!(setup.delays_sleep(), "sleep must wait for the lock");
    locker.send_blocking(Report::Locked).unwrap();
    setup.session.wait("the lock", |state| state.locked);
    setup.wait_for_sleep_delay(false);

    // Awake again: ready to delay the next sleep.
    setup.prepare_for_sleep(false);
    setup.wait_for_sleep_delay(true);

    // Sleeping while already locked needs no waiting.
    setup.prepare_for_sleep(true);
    setup.wait_for_sleep_delay(false);
    setup.not_started();
}

#[test]
fn without_lock_on_suspend_sleep_is_left_alone() {
    let Some(mut setup) = setup(false) else {
        return;
    };
    setup.session.wait("logind", |state| state.can_suspend);
    assert!(!setup.delays_sleep());
    setup.prepare_for_sleep(true);
    setup.not_started();
}

#[test]
fn a_locker_that_dies_is_started_again_and_one_refused_is_not() {
    let Some(mut setup) = setup(true) else {
        return;
    };
    setup.session.wait("logind", |state| state.can_suspend);

    setup.session.send(Command::Lock);
    let locker = setup.started();
    locker.send_blocking(Report::Locked).unwrap();
    setup.session.wait("the lock", |state| state.locked);

    // It crashes. The compositor keeps the screen locked, so the state
    // does too, and a new locker takes over.
    locker.send_blocking(Report::Gone(Exit::Failed)).unwrap();
    let second = setup.started();
    assert!(setup.session.state.locked);
    second.send_blocking(Report::Locked).unwrap();
    second.send_blocking(Report::Gone(Exit::Unlocked)).unwrap();
    setup.session.wait("the unlock", |state| !state.locked);

    // The compositor turning a locker away is final.
    setup.session.send(Command::Lock);
    let refused = setup.started();
    refused.send_blocking(Report::Gone(Exit::Refused)).unwrap();
    setup.not_started();
    assert!(!setup.session.state.locked);
}
