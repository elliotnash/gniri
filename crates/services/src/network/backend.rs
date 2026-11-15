//! NetworkManager, over the system bus.
//!
//! NetworkManager exports an object per device, access point and connection,
//! and they come and go. Like libnm, the backend reads them all once through
//! the object manager and then follows the service's signals, in the one
//! order they were sent in, into [`System`]. What the shell sees is computed
//! from that copy; only a saved connection's settings take a further call.

use std::collections::HashMap;
use std::time::Duration;

use async_channel::{Receiver, Sender};
use futures_lite::{StreamExt, future};
use zbus::fdo::{DBusProxy, ObjectManagerProxy};
use zbus::message::Type;
use zbus::names::{BusName, OwnedUniqueName};
use zbus::proxy::CacheProperties;
use zbus::{MatchRule, Message, MessageStream, ObjectPath, OwnedObjectPath, OwnedVariant, Variant};

use super::system::{PROFILE, Profile, Request, System};
use super::{Command, Event, State};
use crate::bus::{merge, wake};

const SERVICE: &str = "org.freedesktop.NetworkManager";

/// How long changes collect before the shell hears of them. Access points
/// arrive in bursts after a scan and their strengths change all the time;
/// this makes one event of each burst.
const SETTLE: Duration = Duration::from_millis(50);

#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager",
    default_service = "org.freedesktop.NetworkManager",
    default_path = "/org/freedesktop/NetworkManager"
)]
trait Manager {
    fn activate_connection(
        &self,
        connection: &ObjectPath<'_>,
        device: &ObjectPath<'_>,
        specific_object: &ObjectPath<'_>,
    ) -> zbus::Result<OwnedObjectPath>;
    fn add_and_activate_connection(
        &self,
        connection: HashMap<&str, HashMap<&str, Variant<'_>>>,
        device: &ObjectPath<'_>,
        specific_object: &ObjectPath<'_>,
    ) -> zbus::Result<(OwnedObjectPath, OwnedObjectPath)>;
    fn deactivate_connection(&self, active_connection: &ObjectPath<'_>) -> zbus::Result<()>;
    fn get_permissions(&self) -> zbus::Result<HashMap<String, String>>;
    #[zbus(property)]
    fn set_wireless_enabled(&self, enabled: bool) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager.Device",
    default_service = "org.freedesktop.NetworkManager"
)]
trait Device {
    fn disconnect(&self) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager.Device.Wireless",
    default_service = "org.freedesktop.NetworkManager"
)]
trait Wireless {
    fn request_scan(&self, options: HashMap<&str, Variant<'_>>) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager.Settings.Connection",
    default_service = "org.freedesktop.NetworkManager"
)]
trait SavedConnection {
    fn get_settings(&self) -> zbus::Result<HashMap<String, HashMap<String, OwnedVariant>>>;
}

/// Wakes the loop [`SETTLE`] after it asks. There is no runtime here to keep
/// a timer, so a thread sleeps; it ends when the backend does.
fn settle_timer() -> std::io::Result<(Sender<()>, Receiver<()>)> {
    // One request can wait while another is slept on: a change that comes
    // during the sleep may come after the report it leads to was made.
    let (ask, asked) = async_channel::bounded::<()>(1);
    let (tell, told) = async_channel::bounded::<()>(1);
    std::thread::Builder::new()
        .name("network-settle".to_owned())
        .spawn(move || {
            while asked.recv_blocking().is_ok() {
                std::thread::sleep(SETTLE);
                if tell.send_blocking(()).is_err() {
                    break;
                }
            }
        })?;
    Ok((ask, told))
}

/// The name GNOME gives a profile's permission entry (`g_get_user_name`).
fn user_name() -> Option<String> {
    ["USER", "LOGNAME"]
        .into_iter()
        .filter_map(|variable| std::env::var(variable).ok())
        .find(|name| !name.is_empty())
}

/// For the log, after "cannot".
fn describe(command: &Command) -> &'static str {
    match command {
        Command::SetWifiEnabled(true) => "turn Wi-Fi on",
        Command::SetWifiEnabled(false) => "turn Wi-Fi off",
        Command::ConnectWifi(_) => "join the network",
        Command::DisconnectWifi => "leave the network",
        Command::ScanWifi => "scan for networks",
        Command::SetWiredConnected(true) => "bring the wired connection up",
        Command::SetWiredConnected(false) => "take the wired connection down",
    }
}

#[derive(Clone)]
enum Wake {
    /// NetworkManager came, went or was replaced; its new unique name.
    Owner(Option<OwnedUniqueName>),
    Signal(Message),
    Settled,
    Command(Command),
}

struct Backend {
    connection: zbus::Connection,
    manager: ManagerProxy<'static>,
    objects: ObjectManagerProxy<'static>,
    events: Sender<Event>,
    settle: Sender<()>,
    system: System,
    /// NetworkManager's unique name, which is what its signals carry.
    owner: Option<OwnedUniqueName>,
    /// What the shell was last told.
    sent: Option<State>,
}

impl Backend {
    /// Starts over with the NetworkManager that now owns the name, if any.
    async fn restart(&mut self, owner: Option<OwnedUniqueName>) {
        self.system = System::default();
        self.owner = owner;
        if self.owner.is_none() {
            return;
        }
        // Signals sent since are waiting in the queue and are applied on
        // top of this, in order, so the copy ends up current.
        match self.objects.get_managed_objects().await {
            Ok(objects) => {
                self.system.running = true;
                for (path, interfaces) in &objects {
                    for (interface, properties) in interfaces {
                        for (name, value) in properties {
                            self.system.set(path, interface, name, value);
                        }
                    }
                }
            }
            Err(error) => {
                eprintln!("gniri: network: cannot read NetworkManager's objects: {error}")
            }
        }
    }

    /// Folds a signal into the copy; says whether the shell could see a
    /// difference.
    fn signal(&mut self, message: &Message) -> bool {
        let header = message.header();
        // The stream's rule names the service, which zbus cannot check
        // against a signal's sender; other backends' signals arrive on this
        // connection too.
        let sender = header.sender().map(|name| name.as_str());
        if sender.is_none() || sender != self.owner.as_ref().map(|name| name.as_str()) {
            return false;
        }
        let (Some(path), Some(interface), Some(member)) =
            (header.path(), header.interface(), header.member())
        else {
            return false;
        };
        let body = message.body();
        type Properties<'a> = HashMap<&'a str, Variant<'a>>;
        match (interface.as_str(), member.as_str()) {
            ("org.freedesktop.DBus.Properties", "PropertiesChanged") => {
                let Ok((interface, changed, _)) =
                    body.deserialize::<(&str, Properties<'_>, Vec<&str>)>()
                else {
                    return false;
                };
                let mut dirty = false;
                for (name, value) in &changed {
                    dirty |= self.system.set(path, interface, name, value);
                }
                dirty
            }
            ("org.freedesktop.DBus.ObjectManager", "InterfacesAdded") => {
                let Ok((path, interfaces)) =
                    body.deserialize::<(ObjectPath<'_>, HashMap<&str, Properties<'_>>)>()
                else {
                    return false;
                };
                let mut dirty = false;
                for (interface, properties) in &interfaces {
                    for (name, value) in properties {
                        dirty |= self.system.set(&path, interface, name, value);
                    }
                }
                dirty
            }
            ("org.freedesktop.DBus.ObjectManager", "InterfacesRemoved") => {
                let Ok((path, interfaces)) = body.deserialize::<(ObjectPath<'_>, Vec<&str>)>()
                else {
                    return false;
                };
                let mut dirty = false;
                for interface in interfaces {
                    dirty |= self.system.remove(&path, interface);
                }
                dirty
            }
            // The settings are read again for the next report.
            (PROFILE, "Updated") => self.system.remove(path, PROFILE),
            _ => false,
        }
    }

    /// Asks to be woken once the changes have settled.
    fn settle(&self) {
        // Full means it has been asked already.
        let _ = self.settle.try_send(());
    }

    /// Reads the settings of the saved connections that matter and are not
    /// known yet.
    async fn load_profiles(&mut self) {
        // Which ones matter can change with what was just read: a hotspot
        // decides which adapter the toggle stands for. Hence the second
        // round.
        for _ in 0..2 {
            let wanted = self.system.wanted_profiles();
            self.system.profiles.retain(|path, _| wanted.contains(path));
            let missing: Vec<String> = wanted
                .into_iter()
                .filter(|path| !self.system.profiles.contains_key(path))
                .collect();
            if missing.is_empty() {
                break;
            }
            for path in missing {
                // A failure means it was deleted a moment ago. The empty
                // profile keeps it from being asked for again until its
                // removal arrives.
                let profile = match self.settings(&path).await {
                    Ok(settings) => Profile::from_settings(&settings),
                    Err(_) => Profile::default(),
                };
                self.system.profiles.insert(path, profile);
            }
        }
    }

    async fn settings(
        &self,
        path: &str,
    ) -> zbus::Result<HashMap<String, HashMap<String, OwnedVariant>>> {
        SavedConnectionProxy::builder(&self.connection)
            .path(path)
            .cache_properties(CacheProperties::No)
            .build()
            .await?
            .get_settings()
            .await
    }

    /// Tells the shell the state, if it is news or `always`.
    async fn report(&mut self, always: bool) -> Result<(), async_channel::SendError<Event>> {
        if self.system.running {
            self.load_profiles().await;
        }
        let state = self.system.state();
        if !always && self.sent.as_ref() == Some(&state) {
            return Ok(());
        }
        self.sent = Some(state.clone());
        self.events.send(Event::State(state)).await
    }

    /// Carries a command out; says whether NetworkManager took it. If not,
    /// nothing will come of it and the shell's prediction is to be undone.
    async fn command(&mut self, command: &Command) -> bool {
        let what = describe(command);
        if !self.system.running {
            eprintln!("gniri: network: cannot {what}: NetworkManager is not running");
            return false;
        }
        let request = match self.system.plan(command) {
            Ok(Some(request)) => request,
            Ok(None) => return false,
            Err(reason) => {
                eprintln!("gniri: network: cannot {what}: {reason}");
                return false;
            }
        };
        if let Err(error) = self.perform(&request).await {
            eprintln!("gniri: network: cannot {what}: {error}");
            return false;
        }
        if let Request::Scan { device } = request {
            let last_scan = self
                .system
                .devices
                .get(&device)
                .map(|device| device.last_scan);
            self.system.scan = last_scan.map(|last_scan| (device, last_scan));
        }
        true
    }

    async fn perform(&self, request: &Request) -> zbus::Result<()> {
        let none = ObjectPath::from_static_str_unchecked("/");
        let path = |path: &str| ObjectPath::try_from(path.to_owned());
        match request {
            Request::SetWirelessEnabled(enabled) => {
                self.manager.set_wireless_enabled(*enabled).await?;
            }
            Request::Activate { profile, device } => {
                self.manager
                    .activate_connection(&path(profile)?, &path(device)?, &none)
                    .await?;
            }
            // Port of `WirelessNetwork.activate`: NetworkManager fills the
            // profile in from the access point, and asks the secret agent
            // for a password it turns out to need. Someone who may not
            // change settings for everybody gets a profile of their own.
            Request::Join {
                device,
                access_point,
            } => {
                let mut settings = HashMap::new();
                if !self.may_share().await
                    && let Some(user) = user_name()
                {
                    let permissions = Variant::from(vec![format!("user:{user}:")]);
                    settings.insert("connection", HashMap::from([("permissions", permissions)]));
                }
                self.manager
                    .add_and_activate_connection(settings, &path(device)?, &path(access_point)?)
                    .await?;
            }
            // Port of `NMDeviceItem._autoConnect`.
            Request::AutoConnect { device } => {
                self.manager
                    .add_and_activate_connection(HashMap::new(), &path(device)?, &none)
                    .await?;
            }
            Request::Deactivate { active } => {
                self.manager.deactivate_connection(&path(active)?).await?;
            }
            Request::Disconnect { device } => {
                DeviceProxy::builder(&self.connection)
                    .path(device.as_str())
                    .cache_properties(CacheProperties::No)
                    .build()
                    .await?
                    .disconnect()
                    .await?;
            }
            Request::Scan { device } => {
                WirelessProxy::builder(&self.connection)
                    .path(device.as_str())
                    .cache_properties(CacheProperties::No)
                    .build()
                    .await?
                    .request_scan(HashMap::new())
                    .await?;
            }
        }
        Ok(())
    }

    /// Whether the user may make connections for everybody without being
    /// asked to authenticate: what GNOME asks polkit, from NetworkManager's
    /// own answer.
    async fn may_share(&self) -> bool {
        const MODIFY_SYSTEM: &str = "org.freedesktop.NetworkManager.settings.modify.system";
        self.manager
            .get_permissions()
            .await
            .is_ok_and(|permissions| {
                permissions.get(MODIFY_SYSTEM).map(String::as_str) == Some("yes")
            })
    }
}

async fn serve(
    connection: zbus::Connection,
    events: Sender<Event>,
    commands: Receiver<Command>,
    signals: Receiver<Message>,
) -> zbus::Result<()> {
    let bus = DBusProxy::new(&connection).await?;
    let owners = bus
        .receive_name_owner_changed_with_args(&[(0, SERVICE)])
        .await?;
    let manager = ManagerProxy::builder(&connection)
        .cache_properties(CacheProperties::No)
        .build()
        .await?;
    let objects = ObjectManagerProxy::builder(&connection)
        .destination(SERVICE)
        .path("/org/freedesktop")
        .build()
        .await?;
    let (settle, settled) = settle_timer()?;
    let mut backend = Backend {
        connection,
        manager,
        objects,
        events,
        settle,
        system: System::default(),
        owner: None,
        sent: None,
    };

    // NetworkManager is not started by asking for it, so this fails while
    // it is not running.
    let owner = bus.get_name_owner(BusName::try_from(SERVICE)?).await.ok();
    backend.restart(owner).await;
    if backend.report(true).await.is_err() {
        return Ok(());
    }

    let mut wakes = merge(vec![
        Box::pin(owners.map(|signal| {
            let owner = signal
                .args()
                .ok()
                .and_then(|args| args.new_owner().as_ref().map(|name| name.to_owned().into()));
            Wake::Owner(owner)
        })),
        Box::pin(signals.map(Wake::Signal)),
        wake(settled, Wake::Settled),
        Box::pin(commands.map(Wake::Command)),
    ]);
    while let Some(wake) = wakes.next().await {
        let sent = match wake {
            Wake::Owner(owner) => {
                backend.restart(owner).await;
                backend.report(false).await
            }
            Wake::Signal(message) => {
                if backend.signal(&message) {
                    backend.settle();
                }
                Ok(())
            }
            Wake::Settled => backend.report(false).await,
            Wake::Command(command) => {
                if backend.command(&command).await {
                    // Its effect arrives as signals; a scan is only noted.
                    backend.settle();
                    Ok(())
                } else {
                    backend.report(true).await
                }
            }
        };
        if sent.is_err() {
            break; // The shell went away.
        }
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
        .sender(SERVICE)
        .build()?;
    let mut stream = MessageStream::for_match_rule(rule, &connection, None).await?;

    // zbus stops reading the connection, for every backend on it, while a
    // stream's queue is full. A scan fills this one faster than the loop
    // empties it when the loop is waiting for a reply, which would then
    // never be read. So the stream is drained into a queue without a bound
    // whenever anything wakes the thread.
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
