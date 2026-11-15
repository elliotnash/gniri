//! The secret agent against a stand-in for NetworkManager: the test itself
//! owns NetworkManager's name on a private bus, takes the agent's
//! registration and asks it for secrets as NetworkManager would.

mod common;

use std::collections::HashMap;
use std::time::Duration;

use async_channel::{Receiver, Sender};
use common::{Follower, MockBus, TIMEOUT};
use futures_lite::future::block_on;
use gniri_services::network::agent::backend::{self, IDENTIFIER, PATH};
use gniri_services::network::agent::{self, ALLOW_INTERACTION, Command, USER_REQUESTED};
use zbus::{ObjectPath, OwnedVariant, Variant};

const NM: &str = "org.freedesktop.NetworkManager";
const AGENT_MANAGER: &str = "/org/freedesktop/NetworkManager/AgentManager";
const AGENT: &str = "org.freedesktop.NetworkManager.SecretAgent";
const WIFI_SECURITY: &str = "802-11-wireless-security";
const ERROR: &str = "org.freedesktop.NetworkManager.SecretAgent";

/// NetworkManager's agent manager, as far as the agent uses it.
struct Manager {
    registered: Sender<String>,
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.AgentManager")]
impl Manager {
    async fn register(&self, identifier: String) {
        let _ = self.registered.send(identifier).await;
    }
}

type Secrets = HashMap<String, HashMap<String, OwnedVariant>>;
type Call = std::thread::JoinHandle<zbus::Result<Secrets>>;

struct Setup {
    bus: MockBus,
    /// The connection that is NetworkManager.
    manager: zbus::Connection,
    registered: Receiver<String>,
    agent: Follower<agent::State>,
    /// The bus name of the agent's connection.
    agent_name: String,
}

/// A connection that owns NetworkManager's name and takes registrations.
fn network_manager(bus: &MockBus) -> (zbus::Connection, Receiver<String>) {
    let (sender, registered) = async_channel::unbounded();
    let connection = bus.connection();
    block_on(async {
        connection
            .object_server()
            .at(AGENT_MANAGER, Manager { registered: sender })
            .await
            .unwrap();
        connection.request_name(NM).await.unwrap();
    });
    (connection, registered)
}

fn next_registration(registered: &Receiver<String>) -> String {
    block_on(futures_lite::future::or(
        async { registered.recv().await.ok() },
        async {
            let (sender, receiver) = async_channel::bounded::<()>(1);
            std::thread::spawn(move || {
                std::thread::sleep(TIMEOUT);
                let _ = sender.send_blocking(());
            });
            let _ = receiver.recv().await;
            None
        },
    ))
    .expect("the agent registers")
}

fn setup() -> Option<Setup> {
    let bus = MockBus::start_empty()?;
    let (manager, registered) = network_manager(&bus);

    // The agent's own connection, so the test knows where to call it.
    let connection = bus.connection();
    let agent_name = connection.unique_name().expect("a unique name").to_string();
    let (link, events, commands) = gniri_services::Link::new();
    std::thread::spawn(move || {
        if let Err(error) = block_on(backend::run(connection, events, commands)) {
            eprintln!("the agent stopped: {error}");
        }
    });
    assert_eq!(next_registration(&registered), IDENTIFIER);
    Some(Setup {
        bus,
        manager,
        registered,
        agent: Follower::new(link),
        agent_name,
    })
}

/// The settings of a saved Wi-Fi connection that wants a password.
fn wifi(ssid: &str) -> HashMap<&'static str, HashMap<&'static str, Variant<'static>>> {
    HashMap::from([
        (
            "connection",
            HashMap::from([
                ("id", Variant::from(ssid.to_owned())),
                ("type", Variant::from("802-11-wireless")),
            ]),
        ),
        (
            "802-11-wireless",
            HashMap::from([("ssid", Variant::from(ssid.as_bytes().to_vec()))]),
        ),
        (
            WIFI_SECURITY,
            HashMap::from([("key-mgmt", Variant::from("wpa-psk"))]),
        ),
    ])
}

/// Calls `GetSecrets` from `caller`, as NetworkManager does: the reply
/// comes when the user has answered.
fn get_secrets(
    caller: &zbus::Connection,
    agent_name: &str,
    ssid: &str,
    profile: &str,
    flags: u32,
) -> Call {
    let (caller, agent_name) = (caller.clone(), agent_name.to_owned());
    let (ssid, profile) = (ssid.to_owned(), profile.to_owned());
    std::thread::spawn(move || {
        let path = ObjectPath::try_from(profile).expect("a valid object path");
        let hints: Vec<&str> = Vec::new();
        let reply = block_on(caller.call_method(
            Some(agent_name.as_str()),
            PATH,
            Some(AGENT),
            "GetSecrets",
            &(wifi(&ssid), path, WIFI_SECURITY, hints, flags),
        ))?;
        reply.body().deserialize::<Secrets>()
    })
}

impl Setup {
    fn ask(&self, ssid: &str, profile: &str) -> Call {
        get_secrets(
            &self.manager,
            &self.agent_name,
            ssid,
            profile,
            ALLOW_INTERACTION | USER_REQUESTED,
        )
    }

    fn wait_for_request(&mut self, ssid: &str) {
        let wanted = format!("“{ssid}”");
        self.agent
            .wait(&format!("the request for {ssid}"), move |state| {
                state.request.as_ref().is_some_and(|request| {
                    request
                        .message
                        .as_deref()
                        .is_some_and(|message| message.ends_with(&wanted))
                })
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

#[test]
fn a_password_is_asked_for_and_handed_to_network_manager() {
    let Some(mut setup) = setup() else { return };
    let call = setup.ask("home", "/profile/1");
    setup.wait_for_request("home");
    let request = setup.agent.state.request.clone().unwrap();
    assert_eq!(request.title, "Authentication required");
    assert_eq!(request.fields.len(), 1);
    assert_eq!(request.fields[0].label, "Password");

    // Too short to be a Wi-Fi password: not something the dialog sends,
    // and not passed on.
    setup.agent.send(Command::Respond(vec!["short".to_owned()]));
    std::thread::sleep(Duration::from_millis(200));
    assert!(!call.is_finished());

    setup
        .agent
        .send(Command::Respond(vec!["open sesame".to_owned()]));
    let secrets = call.join().unwrap().expect("the agent answers");
    assert_eq!(secrets.len(), 1);
    assert_eq!(*secrets[WIFI_SECURITY]["psk"], Variant::from("open sesame"));
    setup.wait_for_no_request();
}

#[test]
fn dismissing_the_dialog_tells_network_manager_the_user_gave_up() {
    let Some(mut setup) = setup() else { return };
    let call = setup.ask("home", "/profile/1");
    setup.wait_for_request("home");
    setup.agent.send(Command::Cancel);
    assert_eq!(error_of(call), format!("{ERROR}.UserCanceled"));
    setup.wait_for_no_request();
}

#[test]
fn nothing_is_asked_when_the_user_may_not_be_or_cannot_be() {
    let Some(setup) = setup() else { return };
    // An automatic connection: only stored secrets would do, and there are
    // none.
    let quiet = get_secrets(&setup.manager, &setup.agent_name, "home", "/profile/1", 0);
    assert_eq!(error_of(quiet), format!("{ERROR}.NoSecrets"));
    assert_eq!(setup.agent.state.request, None);

    // A VPN's secrets are for its own plugin to ask for.
    let vpn: HashMap<&str, HashMap<&str, Variant<'_>>> = HashMap::from([(
        "connection",
        HashMap::from([
            ("id", Variant::from("Work")),
            ("type", Variant::from("vpn")),
        ]),
    )]);
    let hints: Vec<&str> = Vec::new();
    let reply = block_on(setup.manager.call_method(
        Some(setup.agent_name.as_str()),
        PATH,
        Some(AGENT),
        "GetSecrets",
        &(
            vpn,
            ObjectPath::try_from("/profile/2").unwrap(),
            "vpn",
            hints,
            ALLOW_INTERACTION,
        ),
    ));
    match reply {
        Err(zbus::Error::MethodError(name, _, _)) => {
            assert_eq!(name.as_str(), format!("{ERROR}.NoSecrets"));
        }
        other => panic!("not an error from the agent: {other:?}"),
    }
}

#[test]
fn only_network_manager_gets_an_answer() {
    let Some(mut setup) = setup() else { return };
    let stranger = setup.bus.connection();
    let call = get_secrets(
        &stranger,
        &setup.agent_name,
        "home",
        "/profile/1",
        ALLOW_INTERACTION | USER_REQUESTED,
    );
    assert_eq!(error_of(call), format!("{ERROR}.PermissionDenied"));

    // And cannot withdraw NetworkManager's request either.
    let call = setup.ask("home", "/profile/1");
    setup.wait_for_request("home");
    block_on(stranger.call_method(
        Some(setup.agent_name.as_str()),
        PATH,
        Some(AGENT),
        "CancelGetSecrets",
        &(ObjectPath::try_from("/profile/1").unwrap(), WIFI_SECURITY),
    ))
    .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert!(!call.is_finished());
    setup.agent.send(Command::Cancel);
    assert_eq!(error_of(call), format!("{ERROR}.UserCanceled"));
}

#[test]
fn requests_wait_their_turn_and_can_be_withdrawn_or_replaced() {
    let Some(mut setup) = setup() else { return };
    let home = setup.ask("home", "/profile/1");
    setup.wait_for_request("home");
    let cafe = setup.ask("cafe", "/profile/2");
    let office = setup.ask("office", "/profile/3");
    // The same connection asked about again: the first asking is over.
    std::thread::sleep(Duration::from_millis(200));
    let home_again = setup.ask("home", "/profile/1");
    assert_eq!(error_of(home), format!("{ERROR}.AgentCanceled"));

    // The next in line shows.
    setup.wait_for_request("cafe");
    // NetworkManager withdraws one that is still waiting.
    block_on(setup.manager.call_method(
        Some(setup.agent_name.as_str()),
        PATH,
        Some(AGENT),
        "CancelGetSecrets",
        &(ObjectPath::try_from("/profile/3").unwrap(), WIFI_SECURITY),
    ))
    .unwrap();
    assert_eq!(error_of(office), format!("{ERROR}.AgentCanceled"));

    setup
        .agent
        .send(Command::Respond(vec!["coffee coffee".to_owned()]));
    let secrets = cafe.join().unwrap().expect("the agent answers");
    assert_eq!(
        *secrets[WIFI_SECURITY]["psk"],
        Variant::from("coffee coffee")
    );
    setup.wait_for_request("home");
    setup.agent.send(Command::Cancel);
    assert_eq!(error_of(home_again), format!("{ERROR}.UserCanceled"));
    setup.wait_for_no_request();
}

#[test]
fn a_new_network_manager_is_registered_with_and_the_old_one_s_requests_end() {
    let Some(mut setup) = setup() else { return };
    let call = setup.ask("home", "/profile/1");
    setup.wait_for_request("home");

    // NetworkManager restarts: another connection has its name.
    block_on(setup.manager.release_name(NM)).unwrap();
    setup.wait_for_no_request();
    assert_eq!(error_of(call), format!("{ERROR}.AgentCanceled"));

    let (manager, registered) = network_manager(&setup.bus);
    assert_eq!(next_registration(&registered), IDENTIFIER);
    // The old one is no longer listened to, the new one is.
    let stale = get_secrets(
        &setup.manager,
        &setup.agent_name,
        "home",
        "/profile/1",
        ALLOW_INTERACTION,
    );
    assert_eq!(error_of(stale), format!("{ERROR}.PermissionDenied"));
    let call = get_secrets(
        &manager,
        &setup.agent_name,
        "home",
        "/profile/1",
        ALLOW_INTERACTION,
    );
    setup.wait_for_request("home");
    setup.agent.send(Command::Cancel);
    assert_eq!(error_of(call), format!("{ERROR}.UserCanceled"));
    assert!(setup.registered.is_empty());
}
