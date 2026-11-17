// SPDX-License-Identifier: GPL-3.0-or-later

//! The agent on the system bus: registered with NetworkManager, and serving
//! `org.freedesktop.NetworkManager.SecretAgent`.
//!
//! NetworkManager calls `GetSecrets` and waits for the call to return: with
//! the secrets, or with an error that says the user gave up or that this
//! agent has none (it then asks the next agent, if there is one). Requests
//! are answered one at a time, in the order they came.
//!
//! The secrets NetworkManager leaves to the agent are kept in the keyring
//! ([`keyring`]): looked up before the user is asked, and kept once they
//! have answered or NetworkManager hands them over.

use std::collections::{HashMap, VecDeque};

use async_channel::{Receiver, Sender};
use futures_lite::StreamExt;
use zbus::fdo::DBusProxy;
use zbus::message::Header;
use zbus::names::BusName;
use zbus::{OwnedObjectPath, OwnedVariant, Variant, proxy::CacheProperties};

use super::keyring::{self, Keyring};
use super::{ALLOW_INTERACTION, Command, Event, REQUEST_NEW, Request, Settings, request, secrets};
use crate::bus::{Wakes, merge};

const SERVICE: &str = "org.freedesktop.NetworkManager";
/// Where NetworkManager looks for an agent: the path is not negotiable.
pub const PATH: &str = "/org/freedesktop/NetworkManager/SecretAgent";
/// What tells this agent from others of the same user. GNOME Shell's is
/// `org.gnome.Shell.NetworkAgent`.
pub const IDENTIFIER: &str = "dev.gniri.NetworkAgent";

#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager.AgentManager",
    default_service = "org.freedesktop.NetworkManager",
    default_path = "/org/freedesktop/NetworkManager/AgentManager"
)]
trait AgentManager {
    fn register(&self, identifier: &str) -> zbus::Result<()>;
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.NetworkManager.SecretAgent")]
enum AgentError {
    #[zbus(error)]
    ZBus(zbus::Error),
    /// The user dismissed the dialog: NetworkManager gives the connection
    /// up.
    UserCanceled(String),
    /// The request was replaced or the agent is going away.
    AgentCanceled(String),
    /// Nothing to answer with: NetworkManager may try another agent.
    NoSecrets(String),
    /// Someone other than NetworkManager asked.
    PermissionDenied(String),
    /// The keyring would not do what was asked.
    Failed(String),
}

/// One `GetSecrets` call.
struct Get {
    /// The unique name of the caller's connection.
    sender: Option<String>,
    settings: Settings,
    /// The saved connection's object path.
    path: String,
    setting_name: String,
    hints: Vec<String>,
    flags: u32,
    /// Answered when the request is over.
    reply: Sender<Result<Settings, AgentError>>,
}

enum Call {
    Get(Box<Get>),
    /// Keep the secrets of these settings that are left to the agent.
    Save {
        sender: Option<String>,
        settings: Settings,
        reply: Sender<Result<(), AgentError>>,
    },
    /// Forget what is kept for the connection with these settings.
    Delete {
        sender: Option<String>,
        settings: Settings,
        reply: Sender<Result<(), AgentError>>,
    },
    /// NetworkManager withdrew the request for this connection and setting.
    Cancel {
        sender: Option<String>,
        path: String,
        setting_name: String,
    },
}

/// The object NetworkManager talks to. It only passes calls on to [`run`].
struct Agent {
    calls: Sender<Call>,
}

fn sender(header: &Header<'_>) -> Option<String> {
    header.sender().map(|sender| sender.to_string())
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.SecretAgent")]
impl Agent {
    async fn get_secrets(
        &self,
        #[zbus(header)] header: Header<'_>,
        connection: Settings,
        connection_path: OwnedObjectPath,
        setting_name: String,
        hints: Vec<String>,
        flags: u32,
    ) -> Result<Settings, AgentError> {
        let gone = || AgentError::AgentCanceled("the agent is shutting down".to_owned());
        let (reply, result) = async_channel::bounded(1);
        let get = Get {
            sender: sender(&header),
            settings: connection,
            path: connection_path.as_str().to_owned(),
            setting_name,
            hints,
            flags,
            reply,
        };
        self.calls
            .send(Call::Get(Box::new(get)))
            .await
            .map_err(|_| gone())?;
        result.recv().await.map_err(|_| gone())?
    }

    async fn cancel_get_secrets(
        &self,
        #[zbus(header)] header: Header<'_>,
        connection_path: OwnedObjectPath,
        setting_name: String,
    ) {
        let cancel = Call::Cancel {
            sender: sender(&header),
            path: connection_path.as_str().to_owned(),
            setting_name,
        };
        let _ = self.calls.send(cancel).await;
    }

    /// NetworkManager hands over the secrets it is told to leave to the
    /// agent, when a connection is saved with them.
    async fn save_secrets(
        &self,
        #[zbus(header)] header: Header<'_>,
        connection: Settings,
        _connection_path: OwnedObjectPath,
    ) -> Result<(), AgentError> {
        let (reply, result) = async_channel::bounded(1);
        let save = Call::Save {
            sender: sender(&header),
            settings: connection,
            reply,
        };
        self.pass(save, result).await
    }

    /// The connection was deleted: so is what was kept for it.
    async fn delete_secrets(
        &self,
        #[zbus(header)] header: Header<'_>,
        connection: Settings,
        _connection_path: OwnedObjectPath,
    ) -> Result<(), AgentError> {
        let (reply, result) = async_channel::bounded(1);
        let delete = Call::Delete {
            sender: sender(&header),
            settings: connection,
            reply,
        };
        self.pass(delete, result).await
    }
}

impl Agent {
    /// Passes `call` on and waits for its answer.
    async fn pass(
        &self,
        call: Call,
        result: Receiver<Result<(), AgentError>>,
    ) -> Result<(), AgentError> {
        let gone = || AgentError::AgentCanceled("the agent is shutting down".to_owned());
        self.calls.send(call).await.map_err(|_| gone())?;
        result.recv().await.map_err(|_| gone())?
    }
}

enum Wake {
    Call(Call),
    /// What the keyring had for the lookup with this number.
    Found(u64, HashMap<String, String>),
    Command(Command),
    /// NetworkManager came, went or was replaced; its new unique name.
    Owner(Option<String>),
}

/// A request that can be asked: the call and what the dialog shows for it.
struct Asking {
    get: Get,
    request: Request,
}

impl Asking {
    fn is_for(&self, path: &str, setting_name: &str) -> bool {
        self.get.path == path && self.get.setting_name == setting_name
    }
}

struct Runner {
    manager: AgentManagerProxy<'static>,
    events: Sender<Event>,
    /// NetworkManager's unique name: who may ask.
    owner: Option<String>,
    active: Option<Asking>,
    waiting: VecDeque<Asking>,
    /// None without a session bus.
    keyring: Option<Keyring>,
    /// Requests being looked up in the keyring, by number.
    looking: Vec<(u64, Get)>,
    looked_up: u64,
    found: Sender<Wake>,
}

impl Runner {
    async fn tell(&self, event: Event) {
        let _ = self.events.send(event).await;
    }

    /// Tells the NetworkManager that now owns the name, if any, about this
    /// agent.
    async fn register(&self) {
        if self.owner.is_none() {
            return;
        }
        if let Err(error) = self.manager.register(IDENTIFIER).await {
            eprintln!("gniri: network agent: NetworkManager will not have it: {error}");
        }
    }

    async fn begin(&mut self, asking: Asking) {
        self.tell(Event::Begin(asking.request.clone())).await;
        self.active = Some(asking);
    }

    /// Ends the active request with `result` and takes up the next.
    async fn end(&mut self, result: Result<Settings, AgentError>) {
        if let Some(active) = self.active.take() {
            let _ = active.get.reply.send(result).await;
            self.tell(Event::End).await;
        }
        if let Some(next) = self.waiting.pop_front() {
            self.begin(next).await;
        }
    }

    /// Withdraws the request for this connection and setting, wherever it
    /// is, answering it with `why`.
    async fn withdraw(&mut self, path: &str, setting_name: &str, why: &str) {
        let error = || AgentError::AgentCanceled(why.to_owned());
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.is_for(path, setting_name))
        {
            self.end(Err(error())).await;
        } else if let Some(index) = self
            .waiting
            .iter()
            .position(|waiting| waiting.is_for(path, setting_name))
            && let Some(waiting) = self.waiting.remove(index)
        {
            let _ = waiting.get.reply.send(Err(error())).await;
        } else if let Some(index) = self
            .looking
            .iter()
            .position(|(_, get)| get.path == path && get.setting_name == setting_name)
        {
            let (_, get) = self.looking.remove(index);
            let _ = get.reply.send(Err(error())).await;
        }
    }

    /// Port of `shell_network_agent_get_secrets`.
    async fn get(&mut self, get: Get) {
        // Whoever gets an answer gets a password: only NetworkManager may
        // ask, as in libnm's `verify_sender`.
        if get.sender.is_none() || get.sender != self.owner {
            let denied = AgentError::PermissionDenied("only NetworkManager may ask".to_owned());
            let _ = get.reply.send(Err(denied)).await;
            return;
        }
        // "We already have a request pending for this (connection, setting).
        // Cancel it before starting the new one."
        self.withdraw(&get.path, &get.setting_name, "replaced by a newer request")
            .await;
        // What is kept did not work, or nothing is ever kept: the user is
        // asked at once. Otherwise the keyring first.
        let ask_now = get.flags & REQUEST_NEW != 0
            || (get.flags & ALLOW_INTERACTION != 0 && keyring::always_ask(&get.settings));
        if !ask_now
            && get.setting_name != "vpn"
            && let Some(keyring) = self.keyring.clone()
        {
            let finder = keyring.clone();
            self.looked_up += 1;
            let number = self.looked_up;
            let uuid = keyring::text(&get.settings, "connection", "uuid").unwrap_or_default();
            let setting_name = get.setting_name.clone();
            let found = self.found.clone();
            keyring.spawn(async move {
                let secrets = finder
                    .find(&uuid, &setting_name)
                    .await
                    .unwrap_or_else(|error| {
                        eprintln!("gniri: network agent: the keyring cannot be read: {error}");
                        HashMap::new()
                    });
                let _ = found.send(Wake::Found(number, secrets)).await;
            });
            self.looking.push((number, get));
            return;
        }
        self.ask(get).await;
    }

    /// `get_secrets_keyring_cb`: what the keyring had is the answer;
    /// with nothing there, the user is asked if they may be.
    async fn found(&mut self, number: u64, secrets: HashMap<String, String>) {
        let Some(index) = self.looking.iter().position(|(other, _)| *other == number) else {
            // Withdrawn meanwhile.
            return;
        };
        let (_, get) = self.looking.remove(index);
        if secrets.is_empty() {
            self.ask(get).await;
            return;
        }
        let setting = secrets
            .into_iter()
            .filter_map(|(key, value)| {
                Some((key, OwnedVariant::try_from(Variant::from(value)).ok()?))
            })
            .collect();
        let answer = HashMap::from([(get.setting_name.clone(), setting)]);
        let _ = get.reply.send(Ok(answer)).await;
    }

    /// Puts the request before the user, once the others before it are
    /// answered.
    async fn ask(&mut self, get: Get) {
        if get.flags & (ALLOW_INTERACTION | REQUEST_NEW) == 0 {
            let none = AgentError::NoSecrets("gniri has none, and may not ask".to_owned());
            let _ = get.reply.send(Err(none)).await;
            return;
        }
        let request = match request(&get.settings, &get.setting_name, &get.hints, get.flags) {
            Ok(request) => request,
            Err(reason) => {
                eprintln!("gniri: network agent: cannot ask for secrets: {reason}");
                let _ = get.reply.send(Err(AgentError::NoSecrets(reason))).await;
                return;
            }
        };
        let asking = Asking { get, request };
        if self.active.is_some() {
            self.waiting.push_back(asking);
        } else {
            self.begin(asking).await;
        }
    }

    async fn handle(&mut self, wake: Wake) {
        match wake {
            Wake::Call(Call::Get(get)) => self.get(*get).await,
            Wake::Found(number, secrets) => self.found(number, secrets).await,
            Wake::Call(Call::Save {
                sender,
                settings,
                reply,
            }) => {
                if sender.is_none() || sender != self.owner {
                    let denied =
                        AgentError::PermissionDenied("only NetworkManager may ask".to_owned());
                    let _ = reply.send(Err(denied)).await;
                    return;
                }
                let Some(keyring) = self.keyring.clone() else {
                    let _ = reply.send(Ok(())).await;
                    return;
                };
                keyring.clone().spawn(async move {
                    let saved = keyring.save(&settings).await.map_err(|error| {
                        AgentError::Failed(format!("the keyring would not keep them: {error}"))
                    });
                    let _ = reply.send(saved).await;
                });
            }
            Wake::Call(Call::Delete {
                sender,
                settings,
                reply,
            }) => {
                if sender.is_none() || sender != self.owner {
                    let denied =
                        AgentError::PermissionDenied("only NetworkManager may ask".to_owned());
                    let _ = reply.send(Err(denied)).await;
                    return;
                }
                let Some(keyring) = self.keyring.clone() else {
                    let _ = reply.send(Ok(())).await;
                    return;
                };
                let uuid = keyring::text(&settings, "connection", "uuid").unwrap_or_default();
                keyring.clone().spawn(async move {
                    // "The request could not be completed. Keyring result: …"
                    let deleted = keyring.delete(&uuid).await.map_err(|error| {
                        AgentError::Failed(format!(
                            "The request could not be completed.  Keyring result: {error}"
                        ))
                    });
                    let _ = reply.send(deleted).await;
                });
            }
            Wake::Call(Call::Cancel {
                sender,
                path,
                setting_name,
            }) => {
                if sender.is_some() && sender == self.owner {
                    self.withdraw(&path, &setting_name, "NetworkManager withdrew the request")
                        .await;
                }
            }
            Wake::Command(Command::Cancel) => {
                self.end(Err(AgentError::UserCanceled(
                    "the user dismissed the dialog".to_owned(),
                )))
                .await;
            }
            Wake::Command(Command::Respond(values)) => {
                let Some(active) = &self.active else { return };
                // The dialog only sends what the request accepts; anything
                // else did not come from it.
                if !active.request.accepts(&values) {
                    return;
                }
                let answer = secrets(&active.get.setting_name, &active.request.fields, &values);
                // "Save any updated secrets": those left to the agent go
                // to the keyring.
                if let Some(keyring) = self.keyring.clone() {
                    let settings = keyring::with_secrets(&active.get.settings, &answer);
                    keyring.clone().spawn(async move {
                        if let Err(error) = keyring.save(&settings).await {
                            eprintln!(
                                "gniri: network agent: the keyring would not keep it: {error}"
                            );
                        }
                    });
                }
                self.end(Ok(answer)).await;
            }
            Wake::Owner(owner) => {
                if owner == self.owner {
                    return;
                }
                // Its requests went with it.
                let gone = || AgentError::AgentCanceled("NetworkManager went away".to_owned());
                for waiting in std::mem::take(&mut self.waiting) {
                    let _ = waiting.get.reply.send(Err(gone())).await;
                }
                for (_, get) in std::mem::take(&mut self.looking) {
                    let _ = get.reply.send(Err(gone())).await;
                }
                self.end(Err(gone())).await;
                self.owner = owner;
                self.register().await;
            }
        }
    }
}

pub async fn run(
    connection: zbus::Connection,
    events: Sender<Event>,
    commands: Receiver<Command>,
) -> zbus::Result<()> {
    let keyring = match zbus::Connection::session().await {
        Ok(session) => Some(Keyring::new(session)),
        Err(error) => {
            eprintln!("gniri: network agent: no session bus, so no keyring: {error}");
            None
        }
    };
    run_with(connection, keyring, events, commands).await
}

/// [`run`] with the keyring given: a test's, or none.
pub async fn run_with(
    connection: zbus::Connection,
    keyring: Option<Keyring>,
    events: Sender<Event>,
    commands: Receiver<Command>,
) -> zbus::Result<()> {
    let (calls, incoming) = async_channel::unbounded();
    let (found, founds) = async_channel::unbounded();
    connection.object_server().at(PATH, Agent { calls }).await?;

    let bus = DBusProxy::new(&connection).await?;
    let owners = bus
        .receive_name_owner_changed_with_args(&[(0, SERVICE)])
        .await?
        .filter_map(|signal| {
            let arguments = signal.args().ok()?;
            let owner = arguments.new_owner().as_ref().map(ToString::to_string);
            Some(Wake::Owner(owner))
        });
    let manager = AgentManagerProxy::builder(&connection)
        .cache_properties(CacheProperties::No)
        .build()
        .await?;

    let mut runner = Runner {
        manager,
        events,
        // Asking does not start NetworkManager, so this fails while it is
        // not running.
        owner: bus
            .get_name_owner(BusName::try_from(SERVICE)?)
            .await
            .ok()
            .map(|owner| owner.to_string()),
        active: None,
        waiting: VecDeque::new(),
        keyring,
        looking: Vec::new(),
        looked_up: 0,
        found,
    };
    runner.register().await;

    let mut wakes = merge(vec![
        Box::pin(incoming.map(Wake::Call)) as Wakes<Wake>,
        Box::pin(commands.map(Wake::Command)),
        Box::pin(owners),
        Box::pin(founds),
    ]);
    while let Some(wake) = wakes.next().await {
        // The shell closing its end means it is shutting down.
        if runner.events.is_closed() {
            break;
        }
        runner.handle(wake).await;
    }
    Ok(())
}
