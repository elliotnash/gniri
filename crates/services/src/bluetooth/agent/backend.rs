//! The agent on the system bus: registered with BlueZ as the default
//! agent, and serving `org.bluez.Agent1`.
//!
//! BlueZ asks one question at a time and waits for the call to return,
//! except when it only has a PIN to show: that call returns at once, and
//! the PIN stays up until the device is paired, the link drops, or the
//! user gives up.

use std::collections::HashMap;

use async_channel::{Receiver, Sender};
use futures_lite::{StreamExt, future};
use zbus::fdo::DBusProxy;
use zbus::message::{Header, Type};
use zbus::names::BusName;
use zbus::proxy::CacheProperties;
use zbus::{MatchRule, Message, MessageStream, ObjectPath, OwnedObjectPath, OwnedVariant};

use super::{Answer, Command, Event, Kind, Request, passkey_text};
use crate::bus::{Wakes, merge};

const BLUEZ: &str = "org.bluez";
const DEVICE: &str = "org.bluez.Device1";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
/// Where the agent's object is. BlueZ is told the path, so any would do.
pub const PATH: &str = "/dev/gniri/BluetoothAgent";
/// What the agent can do for a pairing: show a PIN and take one. BlueZ
/// chooses how to pair by it.
pub const CAPABILITY: &str = "KeyboardDisplay";

#[zbus::proxy(
    interface = "org.bluez.AgentManager1",
    default_service = "org.bluez",
    default_path = "/org/bluez"
)]
trait AgentManager {
    /// Asking must not start BlueZ: see the Bluetooth backend.
    #[zbus(no_autostart)]
    fn register_agent(&self, agent: &ObjectPath<'_>, capability: &str) -> zbus::Result<()>;
    #[zbus(no_autostart)]
    fn request_default_agent(&self, agent: &ObjectPath<'_>) -> zbus::Result<()>;
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.bluez.Error")]
enum AgentError {
    #[zbus(error)]
    ZBus(zbus::Error),
    /// The user said no.
    Rejected(String),
    /// The question went away before it was answered.
    Canceled(String),
}

type Reply = Sender<Result<Answer, AgentError>>;

enum Call {
    /// BlueZ asks. Without `reply` the call has returned already: a PIN to
    /// show.
    Ask {
        sender: Option<String>,
        /// The device's object path.
        device: String,
        kind: Kind,
        reply: Option<Reply>,
    },
    /// BlueZ withdrew its question.
    Cancel { sender: Option<String> },
    /// BlueZ has let the agent go.
    Release { sender: Option<String> },
}

/// The object BlueZ talks to. It only passes calls on to [`run`].
struct Agent {
    calls: Sender<Call>,
}

fn sender(header: &Header<'_>) -> Option<String> {
    header.sender().map(|sender| sender.to_string())
}

impl Agent {
    /// Passes a question on and waits for the user's answer.
    async fn ask(
        &self,
        header: &Header<'_>,
        device: &OwnedObjectPath,
        kind: Kind,
    ) -> Result<Answer, AgentError> {
        let gone = || AgentError::Canceled("the agent is shutting down".to_owned());
        let (reply, result) = async_channel::bounded(1);
        let call = Call::Ask {
            sender: sender(header),
            device: device.as_str().to_owned(),
            kind,
            reply: Some(reply),
        };
        self.calls.send(call).await.map_err(|_| gone())?;
        result.recv().await.map_err(|_| gone())?
    }

    /// Passes a PIN to show on, without waiting.
    async fn show(&self, header: &Header<'_>, device: &OwnedObjectPath, pin: String) {
        let call = Call::Ask {
            sender: sender(header),
            device: device.as_str().to_owned(),
            kind: Kind::ShowPin(pin),
            reply: None,
        };
        let _ = self.calls.send(call).await;
    }
}

#[zbus::interface(name = "org.bluez.Agent1")]
impl Agent {
    async fn release(&self, #[zbus(header)] header: Header<'_>) {
        let release = Call::Release {
            sender: sender(&header),
        };
        let _ = self.calls.send(release).await;
    }

    async fn request_pin_code(
        &self,
        #[zbus(header)] header: Header<'_>,
        device: OwnedObjectPath,
    ) -> Result<String, AgentError> {
        match self.ask(&header, &device, Kind::EnterPin).await? {
            Answer::Pin(pin) => Ok(pin),
            _ => Err(AgentError::Rejected("no PIN was given".to_owned())),
        }
    }

    async fn display_pin_code(
        &self,
        #[zbus(header)] header: Header<'_>,
        device: OwnedObjectPath,
        pincode: String,
    ) {
        self.show(&header, &device, pincode).await;
    }

    async fn request_passkey(
        &self,
        #[zbus(header)] header: Header<'_>,
        device: OwnedObjectPath,
    ) -> Result<u32, AgentError> {
        match self.ask(&header, &device, Kind::EnterPasskey).await? {
            Answer::Passkey(passkey) => Ok(passkey),
            _ => Err(AgentError::Rejected("no passkey was given".to_owned())),
        }
    }

    /// Called again for every key typed on the device, with how many it
    /// has now: the count is not shown.
    async fn display_passkey(
        &self,
        #[zbus(header)] header: Header<'_>,
        device: OwnedObjectPath,
        passkey: u32,
        _entered: u16,
    ) {
        self.show(&header, &device, passkey_text(passkey)).await;
    }

    async fn request_confirmation(
        &self,
        #[zbus(header)] header: Header<'_>,
        device: OwnedObjectPath,
        passkey: u32,
    ) -> Result<(), AgentError> {
        let kind = Kind::Confirm(passkey_text(passkey));
        self.ask(&header, &device, kind).await.map(drop)
    }

    async fn request_authorization(
        &self,
        #[zbus(header)] header: Header<'_>,
        device: OwnedObjectPath,
    ) -> Result<(), AgentError> {
        self.ask(&header, &device, Kind::Pair).await.map(drop)
    }

    async fn authorize_service(
        &self,
        #[zbus(header)] header: Header<'_>,
        device: OwnedObjectPath,
        _uuid: String,
    ) -> Result<(), AgentError> {
        self.ask(&header, &device, Kind::Connect).await.map(drop)
    }

    async fn cancel(&self, #[zbus(header)] header: Header<'_>) {
        let cancel = Call::Cancel {
            sender: sender(&header),
        };
        let _ = self.calls.send(cancel).await;
    }
}

enum Wake {
    Call(Call),
    Command(Command),
    /// A property of some device changed.
    Signal(Message),
    /// The `org.bluez` name changed hands; the new owner, if any.
    Owner(Option<String>),
}

/// The question being asked.
struct Active {
    /// The device's object path.
    device: String,
    request: Request,
    reply: Option<Reply>,
}

/// What the agent reads of an `org.bluez.Device1`.
struct DeviceInfo {
    name: String,
    keyboard: bool,
    paired: bool,
}

/// "AA:BB:CC:DD:EE:FF" from ".../dev_AA_BB_CC_DD_EE_FF", for a device
/// BlueZ will not describe.
fn address_from_path(path: &str) -> String {
    path.rsplit('/')
        .next()
        .and_then(|last| last.strip_prefix("dev_"))
        .map_or_else(|| path.to_owned(), |address| address.replace('_', ":"))
}

struct Runner {
    connection: zbus::Connection,
    manager: AgentManagerProxy<'static>,
    events: Sender<Event>,
    /// The connection that is BlueZ: who may ask.
    owner: Option<String>,
    active: Option<Active>,
}

impl Runner {
    async fn tell(&self, event: Event) {
        let _ = self.events.send(event).await;
    }

    /// Tells the BlueZ that now owns the name, if any, about this agent,
    /// and asks to be the one it turns to.
    async fn register(&self) {
        if self.owner.is_none() {
            return;
        }
        let path = ObjectPath::from_static_str_unchecked(PATH);
        let registered = match self.manager.register_agent(&path, CAPABILITY).await {
            Ok(()) => self.manager.request_default_agent(&path).await,
            Err(error) => Err(error),
        };
        if let Err(error) = registered {
            eprintln!("gniri: bluetooth agent: BlueZ will not have it: {error}");
        }
    }

    async fn device(&self, path: &str) -> DeviceInfo {
        let properties: HashMap<String, OwnedVariant> = match self
            .connection
            .call_method(Some(BLUEZ), path, Some(PROPERTIES), "GetAll", &(DEVICE))
            .await
        {
            Ok(reply) => reply.body().deserialize().unwrap_or_default(),
            Err(_) => HashMap::new(),
        };
        let text = |name: &str| {
            properties
                .get(name)
                .and_then(|value| value.try_clone().ok())
                .and_then(|value| String::try_from(value).ok())
                .filter(|text| !text.is_empty())
        };
        let flag = |name: &str| {
            properties
                .get(name)
                .and_then(|value| value.try_clone().ok())
                .and_then(|value| bool::try_from(value).ok())
                .unwrap_or(false)
        };
        DeviceInfo {
            name: text("Alias")
                .or_else(|| text("Name"))
                .or_else(|| text("Address"))
                .unwrap_or_else(|| address_from_path(path)),
            keyboard: text("Icon").as_deref() == Some("input-keyboard"),
            paired: flag("Paired"),
        }
    }

    /// Ends the active question, answering it with `result` if BlueZ still
    /// waits for an answer.
    async fn end(&mut self, result: Result<Answer, AgentError>) {
        if let Some(active) = self.active.take() {
            if let Some(reply) = active.reply {
                let _ = reply.send(result).await;
            }
            self.tell(Event::End).await;
        }
    }

    async fn ask(&mut self, device: String, kind: Kind, reply: Option<Reply>) {
        // The same PIN again: BlueZ repeats it as keys are typed.
        if reply.is_none()
            && self
                .active
                .as_ref()
                .is_some_and(|active| active.device == device && active.request.kind == kind)
        {
            return;
        }
        let info = self.device(&device).await;
        // A device that was paired is let in without asking, as GNOME's
        // agent lets it; trusting it for good is left to BlueZ's user.
        if kind == Kind::Connect && info.paired {
            if let Some(reply) = reply {
                let _ = reply.send(Ok(Answer::Accepted)).await;
            }
            return;
        }
        // BlueZ asks one question at a time: a new one means the last is
        // over.
        self.end(Err(AgentError::Canceled(
            "another question took its place".to_owned(),
        )))
        .await;
        let request = Request {
            device: info.name,
            keyboard: info.keyboard,
            kind,
        };
        self.tell(Event::Begin(request.clone())).await;
        self.active = Some(Active {
            device,
            request,
            reply,
        });
    }

    async fn command(&mut self, command: Command) {
        let Some(active) = &self.active else { return };
        let Some(answer) = active.request.answer(&command) else {
            return;
        };
        let rejected = answer == Answer::Rejected;
        if rejected && active.reply.is_none() {
            // Giving up on a PIN that is being shown: the pairing it is
            // part of is still going on, and is stopped.
            let stopped = self
                .connection
                .call_method(
                    Some(BLUEZ),
                    active.device.as_str(),
                    Some(DEVICE),
                    "CancelPairing",
                    &(),
                )
                .await;
            if let Err(error) = stopped {
                eprintln!("gniri: bluetooth agent: cannot stop the pairing: {error}");
            }
        }
        self.end(if rejected {
            Err(AgentError::Rejected("the user said no".to_owned()))
        } else {
            Ok(answer)
        })
        .await;
    }

    /// A shown PIN goes when its device is paired or the link is gone.
    async fn signal(&mut self, message: &Message) {
        let header = message.header();
        if header.sender().map(|sender| sender.as_str()) != self.owner.as_deref() {
            return;
        }
        let Some(active) = &self.active else { return };
        if active.reply.is_some() || header.path().map(|path| path.as_str()) != Some(&active.device)
        {
            return;
        }
        let body = message.body();
        let Ok((interface, changed, _)) =
            body.deserialize::<(String, HashMap<String, OwnedVariant>, Vec<String>)>()
        else {
            return;
        };
        if interface != DEVICE {
            return;
        }
        let flag = |name: &str| {
            changed
                .get(name)
                .and_then(|value| value.try_clone().ok())
                .and_then(|value| bool::try_from(value).ok())
        };
        if flag("Paired") == Some(true) || flag("Connected") == Some(false) {
            self.end(Ok(Answer::Accepted)).await;
        }
    }

    async fn handle(&mut self, wake: Wake) {
        let from_bluez = |sender: &Option<String>| sender.is_some() && *sender == self.owner;
        match wake {
            Wake::Call(Call::Ask {
                sender,
                device,
                kind,
                reply,
            }) => {
                // What is typed here is a secret between the user and
                // BlueZ: nobody else gets to ask.
                if from_bluez(&sender) {
                    self.ask(device, kind, reply).await;
                } else if let Some(reply) = reply {
                    let denied = AgentError::Rejected("only BlueZ may ask".to_owned());
                    let _ = reply.send(Err(denied)).await;
                }
            }
            Wake::Call(Call::Cancel { sender } | Call::Release { sender }) => {
                if from_bluez(&sender) {
                    self.end(Err(AgentError::Canceled("BlueZ withdrew it".to_owned())))
                        .await;
                }
            }
            Wake::Command(command) => self.command(command).await,
            Wake::Signal(message) => self.signal(&message).await,
            Wake::Owner(owner) => {
                if owner != self.owner {
                    self.end(Err(AgentError::Canceled("BlueZ went away".to_owned())))
                        .await;
                    self.owner = owner;
                    self.register().await;
                }
            }
        }
    }
}

async fn serve(
    connection: zbus::Connection,
    events: Sender<Event>,
    commands: Receiver<Command>,
    signals: Receiver<Message>,
) -> zbus::Result<()> {
    let (calls, incoming) = async_channel::unbounded();
    connection.object_server().at(PATH, Agent { calls }).await?;

    let bus = DBusProxy::new(&connection).await?;
    let owners = bus
        .receive_name_owner_changed_with_args(&[(0, BLUEZ)])
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
        connection,
        manager,
        events,
        // No BlueZ is an ordinary answer: it is not installed, or only
        // started when an adapter appears.
        owner: bus
            .get_name_owner(BusName::try_from(BLUEZ)?)
            .await
            .ok()
            .map(|owner| owner.to_string()),
        active: None,
    };
    runner.register().await;

    let mut wakes = merge(vec![
        Box::pin(incoming.map(Wake::Call)) as Wakes<Wake>,
        Box::pin(commands.map(Wake::Command)),
        Box::pin(signals.map(Wake::Signal)),
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

pub async fn run(
    connection: zbus::Connection,
    events: Sender<Event>,
    commands: Receiver<Command>,
) -> zbus::Result<()> {
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .sender(BLUEZ)
        .interface(PROPERTIES)
        .member("PropertiesChanged")
        .arg(0, DEVICE)
        .build()?;
    let mut stream = MessageStream::for_match_rule(rule, &connection, None).await?;

    // The loop below waits for BlueZ's replies, and zbus stops reading the
    // connection while a stream's queue is full (docs/DECISIONS.md, M2):
    // devices in range report their signal strength all the time. So the
    // stream is drained into a queue without a bound.
    let (queue, signals) = async_channel::unbounded();
    let drain = async move {
        while let Some(message) = stream.next().await {
            if let Ok(message) = message
                && queue.send(message).await.is_err()
            {
                break;
            }
        }
        Ok(())
    };
    future::or(drain, serve(connection, events, commands, signals)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_is_named_by_its_address_at_worst() {
        assert_eq!(
            address_from_path("/org/bluez/hci0/dev_AA_BB_CC_00_00_01"),
            "AA:BB:CC:00:00:01"
        );
        assert_eq!(address_from_path("/somewhere/else"), "/somewhere/else");
    }
}
