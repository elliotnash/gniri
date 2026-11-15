//! The agent on the system bus: registered with NetworkManager, and serving
//! `org.freedesktop.NetworkManager.SecretAgent`.
//!
//! NetworkManager calls `GetSecrets` and waits for the call to return: with
//! the secrets, or with an error that says the user gave up or that this
//! agent has none (it then asks the next agent, if there is one). Requests
//! are answered one at a time, in the order they came.

use std::collections::VecDeque;

use async_channel::{Receiver, Sender};
use futures_lite::StreamExt;
use zbus::fdo::DBusProxy;
use zbus::message::Header;
use zbus::names::BusName;
use zbus::{OwnedObjectPath, proxy::CacheProperties};

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

    /// NetworkManager asks this for the secrets it is told to leave to the
    /// agent. GNOME's agent puts them in the keyring; this one keeps
    /// nothing, and asks again the next time.
    async fn save_secrets(&self, _connection: Settings, _connection_path: OwnedObjectPath) {}

    async fn delete_secrets(&self, _connection: Settings, _connection_path: OwnedObjectPath) {}
}

enum Wake {
    Call(Call),
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
        }
    }

    /// Port of `shell_network_agent_get_secrets`, without its keyring.
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
        // Nothing is stored here, so there is an answer only if the user
        // may be asked for one.
        if get.flags & (ALLOW_INTERACTION | REQUEST_NEW) == 0 {
            let none = AgentError::NoSecrets("gniri stores no secrets".to_owned());
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
    let (calls, incoming) = async_channel::unbounded();
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
    };
    runner.register().await;

    let mut wakes = merge(vec![
        Box::pin(incoming.map(Wake::Call)) as Wakes<Wake>,
        Box::pin(commands.map(Wake::Command)),
        Box::pin(owners),
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
