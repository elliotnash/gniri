// SPDX-License-Identifier: GPL-3.0-or-later

//! The pairing agent against a stand-in for BlueZ: the test itself owns
//! BlueZ's name on a private bus, with an agent manager and one device,
//! and asks the agent what BlueZ would.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use async_channel::{Receiver, Sender};
use common::{Follower, MockBus, TIMEOUT};
use futures_lite::future::block_on;
use gniri_services::bluetooth::agent::backend::{self, CAPABILITY, PATH};
use gniri_services::bluetooth::agent::{self, Command, Kind};
use zbus::{ObjectPath, OwnedObjectPath};

const BLUEZ: &str = "org.bluez";
const AGENT: &str = "org.bluez.Agent1";
const KEYS: &str = "/org/bluez/hci0/dev_AA_BB_CC_00_00_01";
const ERROR: &str = "org.bluez.Error";

/// BlueZ's agent manager, as far as the agent uses it.
struct Manager {
    /// What was asked: "register <path> <capability>" and "default <path>".
    asked: Sender<String>,
}

#[zbus::interface(name = "org.bluez.AgentManager1")]
impl Manager {
    async fn register_agent(&self, agent: OwnedObjectPath, capability: String) {
        let _ = self
            .asked
            .send(format!("register {} {capability}", agent.as_str()))
            .await;
    }

    async fn request_default_agent(&self, agent: OwnedObjectPath) {
        let _ = self.asked.send(format!("default {}", agent.as_str())).await;
    }
}

/// A keyboard that is being paired.
struct Device {
    paired: Arc<AtomicBool>,
    /// How often the pairing was stopped.
    cancelled: Arc<AtomicU32>,
}

#[zbus::interface(name = "org.bluez.Device1")]
impl Device {
    #[zbus(property)]
    fn alias(&self) -> &str {
        "Keys"
    }

    #[zbus(property)]
    fn icon(&self) -> &str {
        "input-keyboard"
    }

    #[zbus(property)]
    fn paired(&self) -> bool {
        self.paired.load(Ordering::SeqCst)
    }

    #[zbus(property)]
    fn connected(&self) -> bool {
        true
    }

    fn cancel_pairing(&self) {
        self.cancelled.fetch_add(1, Ordering::SeqCst);
    }
}

struct Setup {
    bus: MockBus,
    /// The connection that is BlueZ.
    bluez: zbus::Connection,
    paired: Arc<AtomicBool>,
    cancelled: Arc<AtomicU32>,
    agent: Follower<agent::State>,
    /// The bus name of the agent's connection.
    agent_name: String,
}

fn next(asked: &Receiver<String>) -> String {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Ok(asked) = asked.try_recv() {
            return asked;
        }
        assert!(Instant::now() < deadline, "the agent never registered");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn setup() -> Option<Setup> {
    let bus = MockBus::start_empty()?;
    let (sender, asked) = async_channel::unbounded();
    let paired = Arc::new(AtomicBool::new(false));
    let cancelled = Arc::new(AtomicU32::new(0));
    let bluez = bus.connection();
    block_on(async {
        let server = bluez.object_server();
        server
            .at("/org/bluez", Manager { asked: sender })
            .await
            .unwrap();
        let device = Device {
            paired: Arc::clone(&paired),
            cancelled: Arc::clone(&cancelled),
        };
        server.at(KEYS, device).await.unwrap();
        bluez.request_name(BLUEZ).await.unwrap();
    });

    // The agent's own connection, so the test knows where to call it.
    let connection = bus.connection();
    let agent_name = connection.unique_name().expect("a unique name").to_string();
    let (link, events, commands) = gniri_services::Link::new();
    std::thread::spawn(move || {
        if let Err(error) = block_on(backend::run(connection, events, commands)) {
            eprintln!("the agent stopped: {error}");
        }
    });
    assert_eq!(next(&asked), format!("register {PATH} {CAPABILITY}"));
    assert_eq!(next(&asked), format!("default {PATH}"));
    Some(Setup {
        bus,
        bluez,
        paired,
        cancelled,
        agent: Follower::new(link),
        agent_name,
    })
}

type Call = std::thread::JoinHandle<zbus::Result<zbus::Message>>;

/// Calls a method of the agent from `caller`; the reply comes when the
/// agent gives it.
fn call<B>(caller: &zbus::Connection, agent_name: &str, method: &'static str, body: B) -> Call
where
    B: serde::Serialize + zbus::zvariant::DynamicType + Send + 'static,
{
    let (caller, agent_name) = (caller.clone(), agent_name.to_owned());
    std::thread::spawn(move || {
        block_on(caller.call_method(Some(agent_name.as_str()), PATH, Some(AGENT), method, &body))
    })
}

fn keys() -> ObjectPath<'static> {
    ObjectPath::from_static_str_unchecked(KEYS)
}

impl Setup {
    fn ask<B>(&self, method: &'static str, body: B) -> Call
    where
        B: serde::Serialize + zbus::zvariant::DynamicType + Send + 'static,
    {
        call(&self.bluez, &self.agent_name, method, body)
    }

    fn wait_for(&mut self, kind: Kind) {
        self.agent.wait(&format!("{kind:?}"), move |state| {
            state
                .request
                .as_ref()
                .is_some_and(|request| request.kind == kind)
        });
    }

    fn wait_for_no_request(&mut self) {
        self.agent
            .wait("the request to end", |state| state.request.is_none());
    }
}

/// The name of the error a call came back with.
fn error_of(call: Call) -> String {
    match call.join().expect("the caller does not panic") {
        Err(zbus::Error::MethodError(name, _, _)) => name.to_string(),
        other => panic!("not an error from the agent: {other:?}"),
    }
}

fn answer_of<T>(call: Call) -> T
where
    T: for<'de> serde::Deserialize<'de> + zbus::zvariant::Type,
{
    let reply = call.join().unwrap().expect("the agent answers");
    reply.body().deserialize().expect("the expected answer")
}

#[test]
fn a_pin_and_a_passkey_are_typed_and_handed_to_bluez() {
    let Some(mut setup) = setup() else { return };
    let pin = setup.ask("RequestPinCode", keys());
    setup.wait_for(Kind::EnterPin);
    let request = setup.agent.state.request.clone().unwrap();
    assert_eq!(request.device, "Keys");
    assert!(request.keyboard);
    // Not a PIN, and not an answer to this question.
    setup.agent.send(Command::Respond(String::new()));
    setup.agent.send(Command::Accept);
    std::thread::sleep(Duration::from_millis(200));
    assert!(!pin.is_finished());
    setup.agent.send(Command::Respond("0000".to_owned()));
    assert_eq!(answer_of::<String>(pin), "0000");
    setup.wait_for_no_request();

    let passkey = setup.ask("RequestPasskey", keys());
    setup.wait_for(Kind::EnterPasskey);
    setup.agent.send(Command::Respond("004821".to_owned()));
    assert_eq!(answer_of::<u32>(passkey), 4821);
    setup.wait_for_no_request();
}

#[test]
fn a_number_is_confirmed_or_refused() {
    let Some(mut setup) = setup() else { return };
    let confirm = setup.ask("RequestConfirmation", (keys(), 4821u32));
    setup.wait_for(Kind::Confirm("004821".to_owned()));
    setup.agent.send(Command::Accept);
    answer_of::<()>(confirm);
    setup.wait_for_no_request();

    let confirm = setup.ask("RequestConfirmation", (keys(), 999_999u32));
    setup.wait_for(Kind::Confirm("999999".to_owned()));
    setup.agent.send(Command::Reject);
    assert_eq!(error_of(confirm), format!("{ERROR}.Rejected"));
    setup.wait_for_no_request();

    let pair = setup.ask("RequestAuthorization", keys());
    setup.wait_for(Kind::Pair);
    setup.agent.send(Command::Accept);
    answer_of::<()>(pair);
}

#[test]
fn a_shown_pin_stays_until_the_device_is_paired() {
    let Some(mut setup) = setup() else { return };
    // The call itself returns at once.
    let shown = setup.ask("DisplayPinCode", (keys(), "482193"));
    answer_of::<()>(shown);
    setup.wait_for(Kind::ShowPin("482193".to_owned()));

    // A passkey is told again with every key typed on the device: still
    // the one request.
    let shown = setup.ask("DisplayPasskey", (keys(), 482_193u32, 3u16));
    answer_of::<()>(shown);
    std::thread::sleep(Duration::from_millis(200));
    assert!(setup.agent.link.events.is_empty());

    // Some other property changes: nothing to do with the pairing.
    block_on(async {
        let device = setup
            .bluez
            .object_server()
            .interface::<_, Device>(KEYS)
            .await
            .unwrap();
        let emitter = device.signal_emitter();
        device.get().await.alias_changed(emitter).await.unwrap();
    });
    std::thread::sleep(Duration::from_millis(200));
    assert!(setup.agent.state.request.is_some());
    assert!(setup.agent.link.events.is_empty());

    setup.paired.store(true, Ordering::SeqCst);
    block_on(async {
        let device = setup
            .bluez
            .object_server()
            .interface::<_, Device>(KEYS)
            .await
            .unwrap();
        let emitter = device.signal_emitter();
        device.get().await.paired_changed(emitter).await.unwrap();
    });
    setup.wait_for_no_request();
    assert_eq!(setup.cancelled.load(Ordering::SeqCst), 0);
}

#[test]
fn giving_up_on_a_shown_pin_stops_the_pairing() {
    let Some(mut setup) = setup() else { return };
    answer_of::<()>(setup.ask("DisplayPinCode", (keys(), "482193")));
    setup.wait_for(Kind::ShowPin("482193".to_owned()));
    setup.agent.send(Command::Reject);
    setup.wait_for_no_request();
    let deadline = Instant::now() + TIMEOUT;
    while setup.cancelled.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "the pairing was never stopped");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn bluez_withdraws_a_question_or_asks_another() {
    let Some(mut setup) = setup() else { return };
    let confirm = setup.ask("RequestConfirmation", (keys(), 1u32));
    setup.wait_for(Kind::Confirm("000001".to_owned()));
    answer_of::<()>(setup.ask("Cancel", ()));
    assert_eq!(error_of(confirm), format!("{ERROR}.Canceled"));
    setup.wait_for_no_request();

    // A second question while the first waits: the first is over.
    let first = setup.ask("RequestConfirmation", (keys(), 2u32));
    setup.wait_for(Kind::Confirm("000002".to_owned()));
    let second = setup.ask("RequestPinCode", keys());
    assert_eq!(error_of(first), format!("{ERROR}.Canceled"));
    setup.wait_for(Kind::EnterPin);
    setup.agent.send(Command::Reject);
    assert_eq!(error_of(second), format!("{ERROR}.Rejected"));
}

#[test]
fn a_paired_device_connects_without_a_question_and_an_unknown_one_is_asked_about() {
    let Some(mut setup) = setup() else { return };
    let service = "0000110b-0000-1000-8000-00805f9b34fb";
    let connect = setup.ask("AuthorizeService", (keys(), service));
    setup.wait_for(Kind::Connect);
    setup.agent.send(Command::Reject);
    assert_eq!(error_of(connect), format!("{ERROR}.Rejected"));
    setup.wait_for_no_request();

    setup.paired.store(true, Ordering::SeqCst);
    let connect = setup.ask("AuthorizeService", (keys(), service));
    answer_of::<()>(connect);
    assert_eq!(setup.agent.state.request, None);
    assert!(setup.agent.link.events.is_empty());
}

#[test]
fn only_bluez_is_answered() {
    let Some(mut setup) = setup() else { return };
    let stranger = setup.bus.connection();
    let pin = call(&stranger, &setup.agent_name, "RequestPinCode", keys());
    assert_eq!(error_of(pin), format!("{ERROR}.Rejected"));
    assert!(setup.agent.link.events.is_empty());

    // Nor can a stranger put a PIN on the screen, or take BlueZ's
    // question away.
    let shown = call(
        &stranger,
        &setup.agent_name,
        "DisplayPinCode",
        (keys(), "1234"),
    );
    shown.join().unwrap().unwrap();
    let confirm = setup.ask("RequestConfirmation", (keys(), 7u32));
    setup.wait_for(Kind::Confirm("000007".to_owned()));
    let cancel = call(&stranger, &setup.agent_name, "Cancel", ());
    cancel.join().unwrap().unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert!(!confirm.is_finished());
    setup.agent.send(Command::Accept);
    answer_of::<()>(confirm);
}
