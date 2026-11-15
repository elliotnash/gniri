//! BlueZ, over the system bus.
//!
//! GNOME goes through the gnome-bluetooth library; this talks to `org.bluez`
//! directly and ports the library's rules that `status/bluetooth.js` relies
//! on (`bluetooth-client.c`, `bluetooth-device.c`, `bluetooth-utils.c`).
//!
//! BlueZ lists its adapters and devices through the object manager at `/`.
//! [`Objects`] is a copy of the part that matters, kept up to date from
//! BlueZ's signals; the state is a pure function of that copy and of the
//! requests that are under way ([`Busy`]).
//!
//! Nothing here waits for a reply in line. A connect takes seconds, and the
//! bus connection stops reading once a queue of unread signals is full, so a
//! reply awaited while signals pile up would never arrive. Every call runs
//! beside the loop instead ([`Calls`]).
//!
//! Switching Bluetooth on and off also sets its kill switch in
//! `/dev/rfkill`, as GNOME does: see [`set_power`].

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};

use async_channel::{Receiver, Sender};
use futures_lite::{Stream, StreamExt};
use zbus::names::BusName;
use zbus::{OwnedObjectPath, OwnedVariant, Variant};

use super::{Adapter, Command, Device, Event, State, Status};
use crate::rfkill;
use crate::system_bus::{Wakes, merge};

const BLUEZ: &str = "org.bluez";
const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";
const BATTERY: &str = "org.bluez.Battery1";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
const OBJECT_MANAGER: &str = "org.freedesktop.DBus.ObjectManager";

/// The properties of one interface of one object.
pub type Properties = HashMap<String, OwnedVariant>;

type Interfaces = HashMap<String, Properties>;

#[zbus::proxy(
    interface = "org.freedesktop.DBus.ObjectManager",
    default_service = "org.bluez",
    default_path = "/"
)]
trait ObjectManager {
    /// Asking must not start BlueZ: gnome-bluetooth does not either
    /// (`G_DBUS_OBJECT_MANAGER_CLIENT_FLAGS_DO_NOT_AUTO_START`).
    #[zbus(no_autostart)]
    fn get_managed_objects(&self) -> zbus::Result<HashMap<OwnedObjectPath, Interfaces>>;
}

fn read<T: TryFrom<OwnedVariant>>(properties: &Properties, name: &str) -> Option<T> {
    T::try_from(properties.get(name)?.try_clone().ok()?).ok()
}

/// Overwrites `field` if `properties` has `name`: a change signal carries
/// only what changed.
fn update<T: TryFrom<OwnedVariant>>(field: &mut T, properties: &Properties, name: &str) {
    if let Some(value) = read(properties, name) {
        *field = value;
    }
}

/// What is kept of an `org.bluez.Adapter1`.
#[derive(Clone, Debug, Default, PartialEq)]
struct AdapterRecord {
    powered: bool,
    /// Empty before BlueZ 5.65, which has no `PowerState`.
    power_state: String,
}

impl AdapterRecord {
    fn update(&mut self, properties: &Properties) {
        update(&mut self.powered, properties, "Powered");
        update(&mut self.power_state, properties, "PowerState");
    }

    /// Port of `adapter_get_state` in `bluetooth-client.c`. `target` is the
    /// power state a request of ours is still bringing about, which stands
    /// in for the answer until it is there, as `BtClient._predictedState`
    /// does: "on many systems, there's a significant delay until the rfkill
    /// state results in an adapter state change".
    fn adapter(&self, target: Option<bool>) -> Adapter {
        if let Some(powered) = target {
            return Adapter {
                powered,
                changing: true,
            };
        }
        let (powered, changing) = match self.power_state.as_str() {
            "on" => (true, false),
            "off" | "off-blocked" => (false, false),
            "off-enabling" => (true, true),
            "on-disabling" => (false, true),
            _ => (self.powered, false),
        };
        Adapter { powered, changing }
    }
}

/// What is kept of an `org.bluez.Device1`.
#[derive(Clone, Debug, Default, PartialEq)]
struct DeviceRecord {
    /// The object path of the adapter the device belongs to.
    adapter: String,
    address: String,
    /// The name the device gives itself; `alias` is what the user sees.
    name: String,
    alias: String,
    icon: String,
    class: u32,
    appearance: u16,
    uuids: Vec<String>,
    paired: bool,
    trusted: bool,
    connected: bool,
}

impl DeviceRecord {
    fn update(&mut self, properties: &Properties) {
        if let Some(adapter) = read::<OwnedObjectPath>(properties, "Adapter") {
            self.adapter = adapter.as_str().to_owned();
        }
        update(&mut self.address, properties, "Address");
        update(&mut self.name, properties, "Name");
        update(&mut self.alias, properties, "Alias");
        update(&mut self.icon, properties, "Icon");
        update(&mut self.class, properties, "Class");
        update(&mut self.appearance, properties, "Appearance");
        update(&mut self.uuids, properties, "UUIDs");
        update(&mut self.paired, properties, "Paired");
        update(&mut self.trusted, properties, "Trusted");
        update(&mut self.connected, properties, "Connected");
    }
}

/// The adapters and devices BlueZ has, by object path.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Objects {
    adapters: BTreeMap<String, AdapterRecord>,
    devices: BTreeMap<String, DeviceRecord>,
    /// `org.bluez.Battery1` sits on the device's path and can be listed
    /// before the device itself.
    batteries: BTreeMap<String, u8>,
}

impl Objects {
    /// An interface appeared, with all its properties.
    pub fn add(&mut self, path: &str, interface: &str, properties: &Properties) {
        match interface {
            ADAPTER => {
                let mut record = AdapterRecord::default();
                record.update(properties);
                self.adapters.insert(path.to_owned(), record);
            }
            DEVICE => {
                let mut record = DeviceRecord::default();
                record.update(properties);
                self.devices.insert(path.to_owned(), record);
            }
            BATTERY => self.change(path, interface, properties),
            _ => {}
        }
    }

    /// Some properties of an interface changed.
    pub fn change(&mut self, path: &str, interface: &str, properties: &Properties) {
        match interface {
            ADAPTER => {
                if let Some(record) = self.adapters.get_mut(path) {
                    record.update(properties);
                }
            }
            DEVICE => {
                if let Some(record) = self.devices.get_mut(path) {
                    record.update(properties);
                }
            }
            BATTERY => {
                if let Some(percentage) = read(properties, "Percentage") {
                    self.batteries.insert(path.to_owned(), percentage);
                }
            }
            _ => {}
        }
    }

    /// An interface went away.
    pub fn remove(&mut self, path: &str, interface: &str) {
        match interface {
            ADAPTER => {
                self.adapters.remove(path);
                // BlueZ removes them itself, one signal each; a BlueZ that
                // crashed may not get to it.
                self.devices.retain(|_, device| device.adapter != path);
            }
            DEVICE => {
                self.devices.remove(path);
                self.batteries.remove(path);
            }
            BATTERY => drop(self.batteries.remove(path)),
            _ => {}
        }
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// The adapter everything is about: the first by object path.
    ///
    /// gnome-bluetooth takes the *last* instead (`should_be_default_adapter`
    /// in `bluetooth-client.c`), so that a dongle plugged in later wins
    /// over the built-in adapter. With one adapter the two agree.
    fn default_adapter(&self) -> Option<(&str, &AdapterRecord)> {
        self.adapters
            .first_key_value()
            .map(|(path, record)| (path.as_str(), record))
    }

    /// The devices of the default adapter that `BtClient.getDevices` yields:
    /// "paired or trusted".
    fn listed(&self) -> impl Iterator<Item = (&str, &DeviceRecord)> {
        let adapter = self.default_adapter().map(|(path, _)| path);
        self.devices
            .iter()
            .filter(move |(_, device)| Some(device.adapter.as_str()) == adapter)
            .filter(|(_, device)| device.paired || device.trusted)
            .map(|(path, device)| (path.as_str(), device))
    }

    /// The object path of the listed device with this address.
    fn device_path(&self, address: &str) -> Option<&str> {
        self.listed()
            .find(|(_, device)| device.address.eq_ignore_ascii_case(address))
            .map(|(path, _)| path)
    }

    /// What the shell shows for these objects while `busy` is under way.
    pub fn state(&self, busy: &Busy) -> State {
        let Some((_, record)) = self.default_adapter() else {
            return State::default();
        };
        let adapter = record.adapter(busy.power);
        // `BtClient.getDevices`: "Ignore any lingering device references
        // when turned off".
        let mut devices: Vec<Device> = if adapter.powered {
            self.listed()
                .map(|(path, device)| Device {
                    address: device.address.clone(),
                    name: device.alias.clone(),
                    icon: icon(device),
                    // A request under way shows until BlueZ has answered
                    // it, whatever `Connected` says in between: the link
                    // comes up before the profiles do.
                    status: match busy.devices.get(path) {
                        Some(request) => request.status,
                        None if device.connected => Status::Connected,
                        None => Status::Disconnected,
                    },
                    battery: self.batteries.get(path).copied(),
                    connectable: connectable(&device.uuids),
                })
                .collect()
        } else {
            Vec::new()
        };
        devices.sort_by(Device::menu_order);
        State {
            adapter: Some(adapter),
            devices,
        }
    }
}

/// The requests that are under way, which the state shows until BlueZ has
/// answered them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Busy {
    /// The power state the adapter is being brought to.
    power: Option<bool>,
    /// By object path of the device.
    devices: HashMap<String, Request>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Request {
    /// Tells a request from the one that replaced it on the same device.
    id: u64,
    /// [`Status::Connecting`] or [`Status::Disconnecting`].
    status: Status,
}

/// Port of `update_connectable` in `bluetooth-device.c`: the device has a
/// profile that connecting to makes use of.
pub fn connectable(uuids: &[String]) -> bool {
    uuids.iter().any(|uuid| is_connectable(uuid))
}

/// Port of `bluetooth_uuid_to_string` in `bluetooth-utils.c`, for the
/// profiles in `connectable_uuids` only.
fn is_connectable(uuid: &str) -> bool {
    const MIDI: &str = "03B80E5A-EDE8-4B33-A751-6CE34EC4C700";
    // The library reads these as vendor numbers, none of which connects.
    const CUSTOM_SUFFIX: &str = "-0000-1000-8000-0002ee000002";

    if uuid.eq_ignore_ascii_case(MIDI) {
        return true;
    }
    if uuid.ends_with(CUSTOM_SUFFIX) {
        return false;
    }
    // As the library does, only the first group is looked at.
    let short = uuid.split('-').next().unwrap_or_default();
    u32::from_str_radix(short, 16).is_ok_and(|profile| CONNECTABLE.contains(&profile))
}

/// `connectable_uuids` in `bluetooth-device.c`, by number.
const CONNECTABLE: [u32; 10] = [
    0x1108, // HSP
    0x110A, // AudioSource
    0x110B, // AudioSink
    0x110C, // A/V_RemoteControlTarget
    0x110E, // A/V_RemoteControl
    0x1112, // Headset_-_AG
    0x111E, // Handsfree
    0x111F, // HandsfreeAudioGateway
    0x1124, // HumanInterfaceDeviceService
    0x1812, // Human Interface Device (Bluetooth LE)
];

/// `BluetoothType`, as far as the icon depends on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Headset,
    Headphones,
    OtherAudio,
    Display,
    Scanner,
    Mouse,
    /// Any type without an icon of its own.
    Other,
}

/// Port of `bluetooth_appearance_to_type` in `bluetooth-utils.c`.
fn appearance_kind(appearance: u16) -> Option<Kind> {
    match (appearance & 0xffc0) >> 6 {
        // Phone, computer.
        0x01 | 0x02 => Some(Kind::Other),
        0x05 => Some(Kind::Display),
        0x0a => Some(Kind::OtherAudio),
        0x0b => Some(Kind::Scanner),
        // HID.
        0x0f => match appearance & 0x3f {
            0x02 => Some(Kind::Mouse),
            0x08 => Some(Kind::Scanner),
            // Keyboard, joypads, tablet.
            0x01 | 0x03 | 0x04 | 0x05 => Some(Kind::Other),
            _ => None,
        },
        // Speakers.
        0x21 => Some(Kind::Other),
        // Audio.
        0x25 => match appearance & 0x3f {
            0x01 | 0x02 | 0x04 => Some(Kind::Headset),
            0x03 => Some(Kind::Headphones),
            _ => Some(Kind::OtherAudio),
        },
        _ => None,
    }
}

/// Port of `bluetooth_class_to_type` in `bluetooth-utils.c`.
fn class_kind(class: u32) -> Option<Kind> {
    match (class & 0x1f00) >> 8 {
        // Computer, network.
        0x01 | 0x03 => Some(Kind::Other),
        // Phones and modems.
        0x02 => match (class & 0xfc) >> 2 {
            0x01..=0x05 => Some(Kind::Other),
            _ => None,
        },
        0x04 => match (class & 0xfc) >> 2 {
            0x01 | 0x02 => Some(Kind::Headset),
            0x06 => Some(Kind::Headphones),
            // Speakers; VCR, video camera, camcorder.
            0x05 | 0x0b | 0x0c | 0x0d => Some(Kind::Other),
            _ => Some(Kind::OtherAudio),
        },
        0x05 => match (class & 0xc0) >> 6 {
            // Joypads and remote controls.
            0x00 => match (class & 0x1e) >> 2 {
                0x01..=0x03 => Some(Kind::Other),
                _ => None,
            },
            // Keyboard.
            0x01 => Some(Kind::Other),
            0x02 => match (class & 0x1e) >> 2 {
                // Tablet.
                0x05 => Some(Kind::Other),
                _ => Some(Kind::Mouse),
            },
            _ => None,
        },
        0x06 => {
            if class & 0x80 != 0 {
                Some(Kind::Other) // Printer.
            } else if class & 0x40 != 0 {
                Some(Kind::Scanner)
            } else if class & 0x20 != 0 {
                Some(Kind::Other) // Camera.
            } else if class & 0x10 != 0 {
                Some(Kind::Display)
            } else {
                None
            }
        }
        // Wearable, toy.
        0x07 | 0x08 => Some(Kind::Other),
        _ => None,
    }
}

/// Port of `device_resolve_type_and_icon` and `icon_override` in
/// `bluetooth-client.c`: BlueZ's icon, except where the library knows
/// better ("audio-card, you're ugly"). The result is what
/// [`Device::icon`] holds, without "-symbolic".
///
/// Left out: the library names phones by maker, from the address
/// ("phone-apple-iphone"); no icon theme GNOME ships has such icons, so the
/// shell falls back to BlueZ's "phone" all the same.
fn icon(device: &DeviceRecord) -> String {
    if matches!(
        device.name.as_str(),
        "ION iCade Game Controller" | "8Bitdo Zero GamePad"
    ) {
        return "input-gaming".to_owned();
    }
    let kind = appearance_kind(device.appearance).or_else(|| class_kind(device.class));
    let overridden = match kind {
        Some(Kind::Headset) => Some("audio-headset"),
        Some(Kind::Headphones) => Some("audio-headphones"),
        Some(Kind::OtherAudio) => Some("audio-speakers"),
        Some(Kind::Display) => Some("video-display"),
        Some(Kind::Scanner) => Some("scanner"),
        Some(Kind::Mouse) if device.name.to_lowercase().contains("tablet") => Some("input-tablet"),
        _ => None,
    };
    match overridden {
        Some(icon) => icon.to_owned(),
        None if device.icon.is_empty() => "bluetooth".to_owned(),
        None => device.icon.clone(),
    }
}

/// What a call that ran beside the loop came back with.
enum Finished {
    /// BlueZ's objects, read from scratch. `owner` is the connection that
    /// holds the `org.bluez` name, if BlueZ runs.
    Loaded {
        owner: Option<String>,
        objects: HashMap<OwnedObjectPath, Interfaces>,
    },
    Power {
        result: zbus::Result<()>,
    },
    Device {
        path: String,
        id: u64,
        result: zbus::Result<()>,
    },
}

/// The calls that are under way. As a stream it yields each one's result
/// and never ends: more calls can start.
#[derive(Default)]
struct Calls {
    running: Vec<Pin<Box<dyn Future<Output = Finished>>>>,
}

impl Calls {
    fn start(&mut self, call: impl Future<Output = Finished> + 'static) {
        self.running.push(Box::pin(call));
    }

    /// Gives up on every call. BlueZ still carries out what reached it.
    fn clear(&mut self) {
        self.running.clear();
    }
}

impl Stream for Calls {
    type Item = Finished;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Finished>> {
        for index in 0..self.running.len() {
            if let Poll::Ready(finished) = self.running[index].as_mut().poll(context) {
                drop(self.running.swap_remove(index));
                return Poll::Ready(Some(finished));
            }
        }
        // Nothing to wake for when there are no calls: whoever starts one
        // polls again.
        Poll::Pending
    }
}

/// Runs `work` on a thread of its own and waits for it without blocking the
/// bus thread.
async fn off_thread<T: Send + 'static>(
    work: impl FnOnce() -> std::io::Result<T> + Send + 'static,
) -> std::io::Result<T> {
    let (sender, receiver) = async_channel::bounded(1);
    std::thread::Builder::new()
        .name("bluetooth-rfkill".to_owned())
        .spawn(move || {
            let _ = sender.send_blocking(work());
        })?;
    receiver
        .recv()
        .await
        .unwrap_or_else(|_| Err(std::io::Error::other("the thread stopped")))
}

/// Switches Bluetooth on or off. Port of `BtClient.toggleActive`:
///
/// ```js
/// this._proxy.BluetoothAirplaneMode = active;
/// if (!this._client.default_adapter_powered)
///     this._client.default_adapter_powered = true;
/// ```
///
/// Off, to GNOME, is the Bluetooth kill switch: it survives a reboot and
/// powers the radio down, and an adapter behind a blocked switch refuses to
/// be powered. So the switch is set first, as gnome-settings-daemon does for
/// `BluetoothAirplaneMode`, and that needs no adapter: on some laptops the
/// adapter is gone while its switch is off.
///
/// Then `Powered` is set, off as well as on, which GNOME leaves to the
/// switch: without a kill switch, or without the right to write to
/// `/dev/rfkill`, it is all there is, and BlueZ makes nothing of being told
/// what it already did.
async fn set_power(
    connection: zbus::Connection,
    adapter: Option<String>,
    rfkill: PathBuf,
    on: bool,
) -> zbus::Result<()> {
    let switched = off_thread(move || {
        rfkill::backend::set_blocked(&rfkill, rfkill::backend::TYPE_BLUETOOTH, !on)
    })
    .await;
    if let Err(error) = switched {
        eprintln!("gniri: bluetooth: cannot set the kill switch: {error}");
    }
    let Some(adapter) = adapter else {
        return Ok(());
    };
    connection
        .call_method(
            Some(BLUEZ),
            adapter.as_str(),
            Some(PROPERTIES),
            "Set",
            &(ADAPTER, "Powered", Variant::from(on)),
        )
        .await
        .map(drop)
}

enum Wake {
    /// The `org.bluez` name changed hands; the new owner, if any.
    Owner(Option<String>),
    Signal(zbus::Message),
    Command(Command),
}

struct Backend {
    connection: zbus::Connection,
    bus: zbus::fdo::DBusProxy<'static>,
    manager: ObjectManagerProxy<'static>,
    rfkill: PathBuf,
    events: Sender<Event>,

    objects: Objects,
    busy: Busy,
    calls: Calls,
    /// The connection that is BlueZ. Signals of anyone else are from a
    /// BlueZ that is gone.
    owner: Option<String>,
    /// While the objects are being read: the signals that arrived
    /// meanwhile, to be applied on top of what comes back. Before the
    /// reply they would be overwritten by it, though some are newer.
    held: Option<Vec<zbus::Message>>,
    /// The power state asked for while another request was under way.
    wanted: Option<bool>,
    next_id: u64,
    reported: Option<State>,
}

impl Backend {
    /// Reads everything again, beside the loop.
    fn load(&mut self) {
        self.held = Some(Vec::new());
        let bus = self.bus.clone();
        let manager = self.manager.clone();
        self.calls.start(async move {
            let name = BusName::from_static_str(BLUEZ).expect("a valid bus name");
            // No BlueZ is an ordinary answer: it is not installed, or is
            // only started when an adapter appears.
            let owner = bus.get_name_owner(name).await.ok();
            let objects = match &owner {
                Some(_) => manager.get_managed_objects().await.unwrap_or_else(|error| {
                    eprintln!("gniri: bluetooth: cannot list BlueZ's objects: {error}");
                    HashMap::new()
                }),
                None => HashMap::new(),
            };
            Finished::Loaded {
                owner: owner.map(|owner| owner.to_string()),
                objects,
            }
        });
    }

    /// Forgets everything about a BlueZ that is gone.
    fn forget(&mut self) {
        self.objects.clear();
        self.busy = Busy::default();
        self.calls.clear();
        self.held = None;
        self.wanted = None;
    }

    fn signal(&mut self, message: &zbus::Message) {
        let header = message.header();
        if header.sender().map(|sender| sender.as_str()) != self.owner.as_deref() {
            return;
        }
        let (Some(interface), Some(member), Some(path)) =
            (header.interface(), header.member(), header.path())
        else {
            return;
        };
        let body = message.body();
        match (interface.as_str(), member.as_str()) {
            (PROPERTIES, "PropertiesChanged") => {
                if let Ok((interface, changed, _invalidated)) =
                    body.deserialize::<(String, Properties, Vec<String>)>()
                {
                    self.objects.change(path.as_str(), &interface, &changed);
                }
            }
            (OBJECT_MANAGER, "InterfacesAdded") => {
                if let Ok((path, interfaces)) = body.deserialize::<(OwnedObjectPath, Interfaces)>()
                {
                    for (interface, properties) in &interfaces {
                        self.objects.add(path.as_str(), interface, properties);
                    }
                }
            }
            (OBJECT_MANAGER, "InterfacesRemoved") => {
                if let Ok((path, interfaces)) = body.deserialize::<(OwnedObjectPath, Vec<String>)>()
                {
                    for interface in &interfaces {
                        self.objects.remove(path.as_str(), interface);
                    }
                }
            }
            _ => {}
        }
    }

    fn command(&mut self, command: Command) {
        match command {
            Command::SetPowered(on) => {
                // One at a time: two would race each other to the switch.
                if self.busy.power.is_some() {
                    self.wanted = Some(on);
                } else {
                    self.power(on);
                }
            }
            Command::Connect(address) => self.request(&address, "Connect", Status::Connecting),
            Command::Disconnect(address) => {
                self.request(&address, "Disconnect", Status::Disconnecting);
            }
        }
    }

    fn power(&mut self, on: bool) {
        self.busy.power = Some(on);
        let adapter = self
            .objects
            .default_adapter()
            .map(|(path, _)| path.to_owned());
        let call = set_power(self.connection.clone(), adapter, self.rfkill.clone(), on);
        self.calls
            .start(async move { Finished::Power { result: call.await } });
    }

    /// Port of `bluetooth_client_connect_service`: `Connect` and
    /// `Disconnect` on the device, which BlueZ answers once it is done or
    /// has given up.
    fn request(&mut self, address: &str, method: &'static str, status: Status) {
        let Some(path) = self.objects.device_path(address).map(str::to_owned) else {
            return;
        };
        let id = self.next_id;
        self.next_id += 1;
        self.busy
            .devices
            .insert(path.clone(), Request { id, status });
        let connection = self.connection.clone();
        self.calls.start(async move {
            let result = connection
                .call_method(Some(BLUEZ), path.as_str(), Some(DEVICE), method, &())
                .await
                .map(drop);
            Finished::Device { path, id, result }
        });
    }

    fn finished(&mut self, finished: Finished) {
        match finished {
            Finished::Loaded { owner, objects } => {
                self.owner = owner;
                self.objects.clear();
                for (path, interfaces) in &objects {
                    for (interface, properties) in interfaces {
                        self.objects.add(path.as_str(), interface, properties);
                    }
                }
                for message in self.held.take().unwrap_or_default() {
                    self.signal(&message);
                }
            }
            Finished::Power { result } => {
                let done = self.busy.power.take();
                if let Err(error) = result {
                    eprintln!("gniri: bluetooth: cannot switch the adapter: {error}");
                }
                if let Some(on) = self.wanted.take()
                    && Some(on) != done
                {
                    self.power(on);
                }
            }
            Finished::Device { path, id, result } => {
                // Unless a newer request took the device over.
                if self.busy.devices.get(&path).map(|request| request.id) == Some(id) {
                    self.busy.devices.remove(&path);
                }
                if let Err(error) = result {
                    // By name, as GNOME logs it.
                    let name = match self.objects.devices.get(&path) {
                        Some(device) => device.alias.as_str(),
                        None => "a device that is gone",
                    };
                    eprintln!("gniri: bluetooth: request to {name} failed: {error}");
                }
            }
        }
    }

    /// Sends the state if it changed, or in any case when `always`: after
    /// a command, to undo a prediction that did not come true.
    async fn report(&mut self, always: bool) -> bool {
        let state = self.objects.state(&self.busy);
        if !always && self.reported.as_ref() == Some(&state) {
            return true;
        }
        self.reported = Some(state.clone());
        self.events.send(Event::State(state)).await.is_ok()
    }
}

enum Next {
    Wake(Option<Wake>),
    Finished(Finished),
}

pub async fn run(
    connection: zbus::Connection,
    events: Sender<Event>,
    commands: Receiver<Command>,
) -> zbus::Result<()> {
    run_with(
        connection,
        events,
        commands,
        Path::new(rfkill::backend::DEVICE),
    )
    .await
}

/// [`run`] with the kill switches somewhere else than `/dev/rfkill`.
///
/// Tests must come here and never to [`run`]: switching Bluetooth off
/// writes to that device, and the real one would take the machine's
/// Bluetooth with it.
pub async fn run_with(
    connection: zbus::Connection,
    events: Sender<Event>,
    commands: Receiver<Command>,
    rfkill: &Path,
) -> zbus::Result<()> {
    let bus = zbus::fdo::DBusProxy::new(&connection).await?;
    let manager = ObjectManagerProxy::new(&connection).await?;
    let owners = bus
        .receive_name_owner_changed_with_args(&[(0, BLUEZ)])
        .await?
        .filter_map(|signal| {
            let arguments = signal.args().ok()?;
            let owner = arguments.new_owner().as_ref().map(ToString::to_string);
            Some(Wake::Owner(owner))
        });
    // One stream for all of BlueZ's signals keeps them in the order BlueZ
    // sent them: a device removed and added again must not arrive as added
    // and then removed.
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender(BLUEZ)
        .build()?;
    let signals = zbus::MessageStream::for_match_rule(rule, &connection, None)
        .await?
        .filter_map(|message| message.ok().map(Wake::Signal));

    let streams: Vec<Wakes<Wake>> = vec![
        Box::pin(owners),
        Box::pin(signals),
        Box::pin(commands.map(Wake::Command)),
    ];
    let mut wakes = merge(streams);

    let mut backend = Backend {
        connection,
        bus,
        manager,
        rfkill: rfkill.to_owned(),
        events,
        objects: Objects::default(),
        busy: Busy::default(),
        calls: Calls::default(),
        owner: None,
        held: None,
        wanted: None,
        next_id: 0,
        reported: None,
    };
    backend.load();

    loop {
        let next = futures_lite::future::or(async { Next::Wake(wakes.next().await) }, async {
            match backend.calls.next().await {
                Some(finished) => Next::Finished(finished),
                None => std::future::pending().await,
            }
        })
        .await;
        let always = match next {
            // The bus went away.
            Next::Wake(None) => break,
            Next::Wake(Some(Wake::Owner(owner))) => {
                // The same owner again is a change the last read already
                // knew of; forgetting BlueZ for it would make it flicker.
                if owner != backend.owner {
                    backend.forget();
                    backend.owner = owner;
                    if backend.owner.is_some() {
                        backend.load();
                    }
                }
                false
            }
            Next::Wake(Some(Wake::Signal(message))) => {
                match &mut backend.held {
                    Some(held) => held.push(message),
                    None => backend.signal(&message),
                }
                false
            }
            Next::Wake(Some(Wake::Command(command))) => {
                backend.command(command);
                true
            }
            Next::Finished(finished) => {
                // A request that failed leaves the state as it was, which
                // is not what the shell predicted.
                let always = !matches!(finished, Finished::Loaded { .. });
                backend.finished(finished);
                always
            }
        };
        // Nothing is known until the first read is back.
        if backend.held.is_some() && backend.reported.is_none() {
            continue;
        }
        if !backend.report(always).await {
            break; // The shell went away.
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HCI0: &str = "/org/bluez/hci0";
    const HCI1: &str = "/org/bluez/hci1";
    const BUDS: &str = "/org/bluez/hci0/dev_AA_BB_CC_00_00_01";
    const MOUSE: &str = "/org/bluez/hci0/dev_AA_BB_CC_00_00_02";

    // Service class UUIDs.
    const AUDIO_SINK: &str = "0000110b-0000-1000-8000-00805f9b34fb";
    const HID: &str = "00001124-0000-1000-8000-00805f9b34fb";
    const HOGP: &str = "00001812-0000-1000-8000-00805f9b34fb";
    const OBEX_PUSH: &str = "00001105-0000-1000-8000-00805f9b34fb";
    const PNP: &str = "00001200-0000-1000-8000-00805f9b34fb";
    const BATTERY_SERVICE: &str = "0000180f-0000-1000-8000-00805f9b34fb";

    fn properties<const N: usize>(entries: [(&str, Variant<'_>); N]) -> Properties {
        entries
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value.try_to_owned().unwrap()))
            .collect()
    }

    fn uuids(uuids: &[&str]) -> Variant<'static> {
        Variant::from(
            uuids
                .iter()
                .map(|uuid| (*uuid).to_owned())
                .collect::<Vec<_>>(),
        )
    }

    fn adapter(powered: bool) -> Properties {
        properties([("Powered", powered.into())])
    }

    /// A paired pair of headphones on `adapter`.
    fn device(adapter: &str, address: &str, alias: &str) -> Properties {
        properties([
            (
                "Adapter",
                zbus::ObjectPath::try_from(adapter.to_owned())
                    .unwrap()
                    .into(),
            ),
            ("Address", address.to_owned().into()),
            ("Name", alias.to_owned().into()),
            ("Alias", alias.to_owned().into()),
            ("Icon", "audio-card".into()),
            ("Class", 0x0024_0418u32.into()),
            ("UUIDs", uuids(&[AUDIO_SINK])),
            ("Paired", true.into()),
            ("Trusted", false.into()),
            ("Connected", false.into()),
        ])
    }

    /// An adapter that is on, with headphones and a mouse.
    fn objects() -> Objects {
        let mut objects = Objects::default();
        objects.add(HCI0, ADAPTER, &adapter(true));
        objects.add(BUDS, DEVICE, &device(HCI0, "AA:BB:CC:00:00:01", "Buds"));
        objects.add(MOUSE, DEVICE, &device(HCI0, "AA:BB:CC:00:00:02", "Mouse"));
        objects.change(
            MOUSE,
            DEVICE,
            &properties([
                ("Icon", "input-mouse".into()),
                ("Class", 0x0000_2580u32.into()),
                ("UUIDs", uuids(&[HID, PNP])),
            ]),
        );
        objects
    }

    fn state(objects: &Objects) -> State {
        objects.state(&Busy::default())
    }

    fn names(state: &State) -> Vec<&str> {
        state
            .devices
            .iter()
            .map(|device| device.name.as_str())
            .collect()
    }

    fn request(status: Status) -> Request {
        Request { id: 0, status }
    }

    #[test]
    fn no_adapter_is_the_default_state() {
        let mut objects = Objects::default();
        assert_eq!(state(&objects), State::default());
        // A device BlueZ lists before its adapter.
        objects.add(BUDS, DEVICE, &device(HCI0, "AA:BB:CC:00:00:01", "Buds"));
        assert_eq!(state(&objects), State::default());
    }

    #[test]
    fn devices_are_read_from_their_properties() {
        let state = state(&objects());
        assert_eq!(
            state.adapter,
            Some(Adapter {
                powered: true,
                changing: false
            })
        );
        assert_eq!(
            state.devices,
            [
                Device {
                    address: "AA:BB:CC:00:00:01".to_owned(),
                    name: "Buds".to_owned(),
                    icon: "audio-headphones".to_owned(),
                    status: Status::Disconnected,
                    battery: None,
                    connectable: true,
                },
                Device {
                    address: "AA:BB:CC:00:00:02".to_owned(),
                    name: "Mouse".to_owned(),
                    icon: "input-mouse".to_owned(),
                    status: Status::Disconnected,
                    battery: None,
                    connectable: true,
                },
            ]
        );
    }

    #[test]
    fn only_paired_or_trusted_devices_are_listed() {
        let mut objects = objects();
        let unknown = "/org/bluez/hci0/dev_AA_BB_CC_00_00_03";
        objects.add(
            unknown,
            DEVICE,
            &device(HCI0, "AA:BB:CC:00:00:03", "Nearby"),
        );
        objects.change(unknown, DEVICE, &properties([("Paired", false.into())]));
        assert_eq!(names(&state(&objects)), ["Buds", "Mouse"]);

        // Trusted is enough.
        objects.change(unknown, DEVICE, &properties([("Trusted", true.into())]));
        assert_eq!(names(&state(&objects)), ["Buds", "Mouse", "Nearby"]);

        // Forgetting a device.
        objects.change(BUDS, DEVICE, &properties([("Paired", false.into())]));
        assert_eq!(names(&state(&objects)), ["Mouse", "Nearby"]);
    }

    #[test]
    fn connected_devices_come_first() {
        let mut objects = objects();
        objects.change(MOUSE, DEVICE, &properties([("Connected", true.into())]));
        let state = state(&objects);
        assert_eq!(names(&state), ["Mouse", "Buds"]);
        assert_eq!(state.devices[0].status, Status::Connected);
        assert_eq!(state.subtitle().as_deref(), Some("Mouse"));
        assert!(state.shows_indicator());
    }

    #[test]
    fn a_renamed_device_shows_its_alias() {
        let mut objects = objects();
        objects.change(BUDS, DEVICE, &properties([("Alias", "Zed".into())]));
        assert_eq!(names(&state(&objects)), ["Mouse", "Zed"]);
    }

    #[test]
    fn devices_are_not_listed_while_the_adapter_is_off() {
        let mut objects = objects();
        objects.change(BUDS, DEVICE, &properties([("Connected", true.into())]));
        objects.change(HCI0, ADAPTER, &adapter(false));
        let off = state(&objects);
        assert_eq!(
            off.adapter,
            Some(Adapter {
                powered: false,
                changing: false
            })
        );
        assert!(off.devices.is_empty());
        assert_eq!(off.icon_name(), "bluetooth-disabled-symbolic");
        assert_eq!(
            off.placeholder(),
            Some("Turn on Bluetooth to connect to devices")
        );

        objects.change(HCI0, ADAPTER, &adapter(true));
        assert_eq!(names(&state(&objects)), ["Buds", "Mouse"]);
    }

    #[test]
    fn the_power_state_tells_a_change_under_way() {
        let read = |power_state: &str, powered: bool| {
            let mut objects = Objects::default();
            objects.add(
                HCI0,
                ADAPTER,
                &properties([
                    ("Powered", powered.into()),
                    ("PowerState", power_state.to_owned().into()),
                ]),
            );
            let adapter = state(&objects).adapter.unwrap();
            (adapter.powered, adapter.changing)
        };
        assert_eq!(read("on", true), (true, false));
        assert_eq!(read("off", false), (false, false));
        assert_eq!(read("off-blocked", false), (false, false));
        // `Powered` lags behind in both of these.
        assert_eq!(read("off-enabling", false), (true, true));
        assert_eq!(read("on-disabling", true), (false, true));
        // A value from the future: `Powered` decides.
        assert_eq!(read("on-fire", true), (true, false));
    }

    #[test]
    fn a_power_request_shows_until_it_is_answered() {
        let objects = objects();
        let busy = Busy {
            power: Some(false),
            ..Busy::default()
        };
        let turning_off = objects.state(&busy);
        assert_eq!(
            turning_off.adapter,
            Some(Adapter {
                powered: false,
                changing: true
            })
        );
        assert_eq!(turning_off.icon_name(), "bluetooth-acquiring-symbolic");
        assert!(turning_off.devices.is_empty());

        // Without an adapter there is nothing to show it on.
        assert_eq!(Objects::default().state(&busy), State::default());
    }

    #[test]
    fn a_request_shows_on_its_device_whatever_the_link_says() {
        let mut objects = objects();
        let mut busy = Busy::default();
        busy.devices
            .insert(BUDS.to_owned(), request(Status::Connecting));
        assert_eq!(objects.state(&busy).devices[0].status, Status::Connecting);
        // The link is up; the profiles are not yet.
        objects.change(BUDS, DEVICE, &properties([("Connected", true.into())]));
        let connecting = objects.state(&busy);
        assert_eq!(connecting.devices[0].name, "Buds");
        assert_eq!(connecting.devices[0].status, Status::Connecting);
        assert!(!connecting.shows_indicator());

        busy.devices.clear();
        assert_eq!(objects.state(&busy).devices[0].status, Status::Connected);
        busy.devices
            .insert(BUDS.to_owned(), request(Status::Disconnecting));
        let disconnecting = objects.state(&busy);
        assert_eq!(disconnecting.devices[0].status, Status::Disconnecting);
        assert!(disconnecting.shows_indicator());
    }

    #[test]
    fn the_default_adapter_is_the_first_and_only_its_devices_count() {
        let mut objects = objects();
        objects.add(HCI1, ADAPTER, &adapter(false));
        let other = "/org/bluez/hci1/dev_AA_BB_CC_00_00_09";
        objects.add(other, DEVICE, &device(HCI1, "AA:BB:CC:00:00:09", "Other"));
        assert!(state(&objects).powered());
        assert_eq!(names(&state(&objects)), ["Buds", "Mouse"]);
        assert_eq!(objects.device_path("AA:BB:CC:00:00:09"), None);

        // The first adapter is unplugged: the second takes over.
        objects.remove(HCI0, ADAPTER);
        assert!(!state(&objects).powered());
        objects.change(HCI1, ADAPTER, &adapter(true));
        assert_eq!(names(&state(&objects)), ["Other"]);
        assert_eq!(objects.device_path("aa:bb:cc:00:00:09"), Some(other));
    }

    #[test]
    fn an_adapter_takes_its_devices_with_it() {
        let mut objects = objects();
        objects.remove(HCI0, ADAPTER);
        assert_eq!(state(&objects), State::default());
        // Back again, as after a suspend: BlueZ lists the devices anew.
        objects.add(HCI0, ADAPTER, &adapter(true));
        assert!(state(&objects).devices.is_empty());
    }

    #[test]
    fn devices_come_and_go() {
        let mut objects = objects();
        objects.remove(BUDS, DEVICE);
        assert_eq!(names(&state(&objects)), ["Mouse"]);
        // Changes to what is not there are dropped.
        objects.change(BUDS, DEVICE, &properties([("Connected", true.into())]));
        objects.change(HCI1, ADAPTER, &adapter(true));
        objects.add(BUDS, "org.bluez.MediaControl1", &Properties::new());
        assert_eq!(names(&state(&objects)), ["Mouse"]);

        objects.clear();
        assert_eq!(objects, Objects::default());
    }

    #[test]
    fn the_battery_is_an_interface_of_its_own() {
        let mut objects = Objects::default();
        // Listed before the device and the adapter.
        objects.add(BUDS, BATTERY, &properties([("Percentage", 80u8.into())]));
        objects.add(BUDS, DEVICE, &device(HCI0, "AA:BB:CC:00:00:01", "Buds"));
        objects.add(HCI0, ADAPTER, &adapter(true));
        assert_eq!(state(&objects).devices[0].battery, Some(80));

        objects.change(BUDS, BATTERY, &properties([("Percentage", 75u8.into())]));
        assert_eq!(state(&objects).devices[0].battery, Some(75));

        // The device stops reporting it (it disconnected).
        objects.remove(BUDS, BATTERY);
        assert_eq!(state(&objects).devices[0].battery, None);

        // The battery goes with the device.
        objects.add(BUDS, BATTERY, &properties([("Percentage", 60u8.into())]));
        objects.remove(BUDS, DEVICE);
        objects.add(BUDS, DEVICE, &device(HCI0, "AA:BB:CC:00:00:01", "Buds"));
        assert_eq!(state(&objects).devices[0].battery, None);
    }

    #[test]
    fn commands_find_listed_devices_by_address() {
        let mut objects = objects();
        assert_eq!(objects.device_path("AA:BB:CC:00:00:02"), Some(MOUSE));
        assert_eq!(objects.device_path("AA:BB:CC:00:00:07"), None);
        objects.change(MOUSE, DEVICE, &properties([("Paired", false.into())]));
        assert_eq!(objects.device_path("AA:BB:CC:00:00:02"), None);
    }

    #[test]
    fn connectable_devices_have_an_audio_input_or_midi_profile() {
        let list = |uuids: &[&str]| {
            uuids
                .iter()
                .map(|uuid| (*uuid).to_owned())
                .collect::<Vec<_>>()
        };
        assert!(connectable(&list(&[PNP, AUDIO_SINK])));
        assert!(connectable(&list(&[HID])));
        assert!(connectable(&list(&[HOGP, BATTERY_SERVICE])));
        assert!(connectable(&list(&[
            "0000111E-0000-1000-8000-00805F9B34FB"
        ])));
        assert!(connectable(&list(&[
            "03b80e5a-ede8-4b33-a751-6ce34ec4c700"
        ])));

        assert!(!connectable(&[]));
        assert!(!connectable(&list(&[OBEX_PUSH, PNP, BATTERY_SERVICE])));
        // A vendor's own number that happens to read as a profile.
        assert!(!connectable(&list(&[
            "0000110b-0000-1000-8000-0002ee000002"
        ])));
        assert!(!connectable(&list(&["not-a-uuid", ""])));

        // What the menu makes of it.
        let mut objects = objects();
        objects.change(BUDS, DEVICE, &properties([("UUIDs", uuids(&[OBEX_PUSH]))]));
        let state = state(&objects);
        assert!(!state.devices[0].connectable);
        let menu: Vec<&str> = state
            .menu_devices()
            .map(|device| device.name.as_str())
            .collect();
        assert_eq!(menu, ["Mouse"]);
    }

    #[test]
    fn icons_follow_the_type_then_bluez() {
        let icon_of = |entries: Properties| {
            let mut record = DeviceRecord::default();
            record.update(&entries);
            icon(&record)
        };
        // Nothing known.
        assert_eq!(icon_of(Properties::new()), "bluetooth");
        assert_eq!(icon_of(properties([("Icon", "".into())])), "bluetooth");
        // BlueZ's icon where the library has no better one: a phone, a
        // keyboard, loudspeakers.
        assert_eq!(
            icon_of(properties([
                ("Icon", "phone".into()),
                ("Class", 0x005a_020cu32.into())
            ])),
            "phone"
        );
        assert_eq!(
            icon_of(properties([
                ("Icon", "input-keyboard".into()),
                ("Class", 0x0000_2540u32.into())
            ])),
            "input-keyboard"
        );
        assert_eq!(
            icon_of(properties([
                ("Icon", "audio-card".into()),
                ("Class", 0x0024_0414u32.into())
            ])),
            "audio-card"
        );
        // "audio-card, you're ugly".
        assert_eq!(
            icon_of(properties([
                ("Icon", "audio-card".into()),
                ("Class", 0x0024_0404u32.into())
            ])),
            "audio-headset"
        );
        assert_eq!(
            icon_of(properties([
                ("Icon", "audio-card".into()),
                ("Class", 0x0024_0418u32.into())
            ])),
            "audio-headphones"
        );
        assert_eq!(
            icon_of(properties([
                ("Icon", "audio-card".into()),
                ("Class", 0x0024_0428u32.into())
            ])),
            "audio-speakers"
        );
        // The appearance (Bluetooth LE) wins over the class: a mouse.
        assert_eq!(
            icon_of(properties([
                ("Icon", "input-mouse".into()),
                ("Appearance", 0x03c2u16.into()),
                ("Class", 0x0024_0418u32.into())
            ])),
            "input-mouse"
        );
        // LE earbuds.
        assert_eq!(
            icon_of(properties([("Appearance", 0x0941u16.into())])),
            "audio-headset"
        );
        // A drawing tablet that says it is a mouse.
        assert_eq!(
            icon_of(properties([
                ("Name", "Sketch Tablet 2".into()),
                ("Icon", "input-mouse".into()),
                ("Class", 0x0000_2580u32.into())
            ])),
            "input-tablet"
        );
        assert_eq!(
            icon_of(properties([
                ("Name", "8Bitdo Zero GamePad".into()),
                ("Icon", "input-keyboard".into())
            ])),
            "input-gaming"
        );
        // Imaging: a display, a scanner.
        assert_eq!(
            icon_of(properties([("Class", 0x0000_0610u32.into())])),
            "video-display"
        );
        assert_eq!(
            icon_of(properties([("Class", 0x0000_0640u32.into())])),
            "scanner"
        );
    }

    #[test]
    fn calls_finish_in_any_order_and_never_end() {
        use futures_lite::future::{block_on, poll_once};

        let (first_sender, first) = async_channel::bounded::<()>(1);
        let (second_sender, second) = async_channel::bounded::<()>(1);
        let mut calls = Calls::default();
        block_on(async {
            // No calls: nothing yet, rather than the end.
            assert!(poll_once(calls.next()).await.is_none());

            let call = |receiver: Receiver<()>, id| async move {
                let _ = receiver.recv().await;
                Finished::Device {
                    path: String::new(),
                    id,
                    result: Ok(()),
                }
            };
            calls.start(call(first, 1));
            calls.start(call(second, 2));
            assert!(poll_once(calls.next()).await.is_none());

            // The call started last finishes first.
            second_sender.send(()).await.unwrap();
            assert!(matches!(
                calls.next().await,
                Some(Finished::Device { id: 2, .. })
            ));
            assert!(poll_once(calls.next()).await.is_none());
            first_sender.send(()).await.unwrap();
            assert!(matches!(
                calls.next().await,
                Some(Finished::Device { id: 1, .. })
            ));
            assert!(poll_once(calls.next()).await.is_none());
        });
    }
}
