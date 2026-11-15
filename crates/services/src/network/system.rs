//! NetworkManager's objects as the backend last saw them, and what the shell
//! makes of them: the state it shows and the request a command turns into.
//!
//! Nothing here talks to the bus, so all of it is tested without one. Object
//! paths are plain strings; `/` is how NetworkManager says "none".

use std::collections::{BTreeSet, HashMap};

use zbus::{OwnedVariant, Variant};

use super::security::{Flags, MODE_ADHOC, Security};
use super::{Command, Connection, Network, State, Status, Wifi, Wired, arrange, collate};

pub const MANAGER: &str = "org.freedesktop.NetworkManager";
pub const DEVICE: &str = "org.freedesktop.NetworkManager.Device";
pub const WIRELESS: &str = "org.freedesktop.NetworkManager.Device.Wireless";
pub const ACCESS_POINT: &str = "org.freedesktop.NetworkManager.AccessPoint";
pub const ACTIVE: &str = "org.freedesktop.NetworkManager.Connection.Active";
pub const PROFILE: &str = "org.freedesktop.NetworkManager.Settings.Connection";

// `NMDeviceType`.
const ETHERNET: u32 = 1;
const WIFI: u32 = 2;

// `NMDeviceState`.
const UNAVAILABLE: u32 = 20;
const DISCONNECTED: u32 = 30;
const FAILED: u32 = 120;

// `NMActiveConnectionState`.
const ACTIVATING: u32 = 1;
const ACTIVATED: u32 = 2;
const DEACTIVATING: u32 = 3;

/// `NM_CONNECTIVITY_FULL`.
const FULL: u32 = 4;

/// `NM_STATE_CONNECTED_GLOBAL`.
const CONNECTED_GLOBAL: u32 = 70;

// `NM80211Mode`.
const MODE_UNKNOWN: u32 = 0;
const MODE_INFRA: u32 = 2;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct System {
    /// NetworkManager is on the bus.
    pub running: bool,
    pub manager: Manager,
    pub devices: HashMap<String, Device>,
    pub access_points: HashMap<String, AccessPoint>,
    pub active: HashMap<String, Active>,
    /// The saved connections [`System::wanted_profiles`] asks for.
    pub profiles: HashMap<String, Profile>,
    /// The adapter the shell asked to scan and its `LastScan` at the time:
    /// the scan is over once that moves.
    pub scan: Option<(String, i64)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Manager {
    pub networking_enabled: bool,
    pub wireless_enabled: bool,
    pub wireless_hardware_enabled: bool,
    pub devices: Vec<String>,
    pub primary_connection: String,
    pub activating_connection: String,
    pub connectivity: u32,
    pub state: u32,
}

/// A device, with what its `Device.Wireless` interface adds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Device {
    pub kind: u32,
    pub state: u32,
    pub active_connection: String,
    pub available_connections: Vec<String>,
    pub access_points: Vec<String>,
    pub active_access_point: String,
    pub capabilities: u32,
    pub last_scan: i64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AccessPoint {
    pub ssid: Vec<u8>,
    pub hw_address: String,
    pub strength: u8,
    pub flags: u32,
    pub wpa_flags: u32,
    pub rsn_flags: u32,
    pub mode: u32,
}

/// An active connection.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Active {
    pub state: u32,
    /// The saved connection it was made from.
    pub profile: String,
    pub devices: Vec<String>,
}

/// The parts of a saved connection's settings that matter here.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Profile {
    pub id: String,
    /// `connection.type`.
    pub kind: String,
    /// When it was last used, in seconds; 0 for never.
    pub timestamp: u64,
    pub ssid: Vec<u8>,
    /// `802-11-wireless.mode`; empty when the profile leaves it out.
    pub mode: String,
    /// The one access point the profile is tied to, if it is.
    pub bssid: Vec<u8>,
    /// `ipv4.method` is `shared`, which is what makes a Wi-Fi connection a
    /// hotspot.
    pub shared: bool,
}

/// What a command asks of NetworkManager.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    SetWirelessEnabled(bool),
    Activate {
        profile: String,
        device: String,
    },
    /// Join a network that has no profile yet. NetworkManager makes one,
    /// and asks its secret agent for the password if the network has one.
    Join {
        device: String,
        access_point: String,
    },
    /// Bring a wired adapter up that has no profile yet.
    AutoConnect {
        device: String,
    },
    Deactivate {
        active: String,
    },
    Disconnect {
        device: String,
    },
    Scan {
        device: String,
    },
}

/// `None` for the path that means "none".
fn object(path: &str) -> Option<&str> {
    (!path.is_empty() && path != "/").then_some(path)
}

fn boolean(value: &Variant<'_>) -> Option<bool> {
    match value {
        Variant::Bool(value) => Some(*value),
        _ => None,
    }
}

/// Any integer: mocks are not always exact about widths.
fn integer(value: &Variant<'_>) -> Option<i64> {
    match *value {
        Variant::U8(value) => Some(value.into()),
        Variant::I16(value) => Some(value.into()),
        Variant::U16(value) => Some(value.into()),
        Variant::I32(value) => Some(value.into()),
        Variant::U32(value) => Some(value.into()),
        Variant::I64(value) => Some(value),
        Variant::U64(value) => value.try_into().ok(),
        _ => None,
    }
}

fn unsigned(value: &Variant<'_>) -> Option<u32> {
    integer(value)?.try_into().ok()
}

/// A string or an object path.
fn text(value: &Variant<'_>) -> Option<String> {
    match value {
        Variant::Str(value) => Some(value.as_str().to_owned()),
        Variant::ObjectPath(value) => Some(value.as_str().to_owned()),
        _ => None,
    }
}

fn texts(value: &Variant<'_>) -> Option<Vec<String>> {
    match value {
        Variant::Array(values) => values.inner().iter().map(text).collect(),
        _ => None,
    }
}

fn bytes(value: &Variant<'_>) -> Option<Vec<u8>> {
    match value {
        Variant::Array(values) => values
            .inner()
            .iter()
            .map(|value| match value {
                Variant::U8(byte) => Some(*byte),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

/// Stores a decoded value; says whether that changed anything.
fn put<T: PartialEq>(slot: &mut T, value: Option<T>) -> bool {
    match value {
        Some(value) if *slot != value => {
            *slot = value;
            true
        }
        _ => false,
    }
}

impl Manager {
    fn set(&mut self, name: &str, value: &Variant<'_>) -> bool {
        match name {
            "NetworkingEnabled" => put(&mut self.networking_enabled, boolean(value)),
            "WirelessEnabled" => put(&mut self.wireless_enabled, boolean(value)),
            "WirelessHardwareEnabled" => put(&mut self.wireless_hardware_enabled, boolean(value)),
            "Devices" => put(&mut self.devices, texts(value)),
            "PrimaryConnection" => put(&mut self.primary_connection, text(value)),
            "ActivatingConnection" => put(&mut self.activating_connection, text(value)),
            "Connectivity" => put(&mut self.connectivity, unsigned(value)),
            "State" => put(&mut self.state, unsigned(value)),
            _ => false,
        }
    }
}

impl Device {
    fn set(&mut self, name: &str, value: &Variant<'_>) -> bool {
        match name {
            "DeviceType" => put(&mut self.kind, unsigned(value)),
            "State" => put(&mut self.state, unsigned(value)),
            "ActiveConnection" => put(&mut self.active_connection, text(value)),
            "AvailableConnections" => put(&mut self.available_connections, texts(value)),
            _ => false,
        }
    }

    fn set_wireless(&mut self, name: &str, value: &Variant<'_>) -> bool {
        match name {
            "AccessPoints" => put(&mut self.access_points, texts(value)),
            "ActiveAccessPoint" => put(&mut self.active_access_point, text(value)),
            "WirelessCapabilities" => put(&mut self.capabilities, unsigned(value)),
            "LastScan" => put(&mut self.last_scan, integer(value)),
            _ => false,
        }
    }

    /// GNOME gives the device a menu item, and its type a toggle. Port of
    /// `NMDeviceToggle._shouldShowDevice`; a Wi-Fi adapter also stays while
    /// its radio is off (`NMWirelessToggle._shouldShowDevice`), where a
    /// wired one without a cable goes.
    fn is_shown(&self) -> bool {
        match self.state {
            DISCONNECTED..=FAILED => true,
            UNAVAILABLE => self.kind == WIFI,
            _ => false,
        }
    }
}

impl AccessPoint {
    fn set(&mut self, name: &str, value: &Variant<'_>) -> bool {
        match name {
            "Ssid" => put(&mut self.ssid, bytes(value)),
            "HwAddress" => put(&mut self.hw_address, text(value)),
            "Strength" => put(
                &mut self.strength,
                integer(value).and_then(|strength| strength.try_into().ok()),
            ),
            "Flags" => put(&mut self.flags, unsigned(value)),
            "WpaFlags" => put(&mut self.wpa_flags, unsigned(value)),
            "RsnFlags" => put(&mut self.rsn_flags, unsigned(value)),
            "Mode" => put(&mut self.mode, unsigned(value)),
            _ => false,
        }
    }

    /// The name, unless the access point hides it: NetworkManager then
    /// reports no SSID, or one of zero bytes.
    fn name(&self) -> Option<String> {
        if self.ssid.iter().all(|byte| *byte == 0) {
            return None;
        }
        Some(String::from_utf8_lossy(&self.ssid).into_owned())
    }

    fn security_flags(&self) -> Flags {
        Flags {
            flags: self.flags,
            wpa: self.wpa_flags,
            rsn: self.rsn_flags,
            mode: self.mode,
        }
    }

    /// `AA:BB:CC:DD:EE:FF` as bytes.
    fn bssid(&self) -> Option<Vec<u8>> {
        self.hw_address
            .split(':')
            .map(|byte| u8::from_str_radix(byte, 16).ok())
            .collect()
    }
}

impl Active {
    fn set(&mut self, name: &str, value: &Variant<'_>) -> bool {
        match name {
            "State" => put(&mut self.state, unsigned(value)),
            "Connection" => put(&mut self.profile, text(value)),
            "Devices" => put(&mut self.devices, texts(value)),
            _ => false,
        }
    }
}

impl Profile {
    /// Reads the reply of `Settings.Connection.GetSettings`.
    pub fn from_settings(settings: &HashMap<String, HashMap<String, OwnedVariant>>) -> Self {
        let get = |group: &str, key: &str| settings.get(group)?.get(key).map(|value| &**value);
        Self {
            id: get("connection", "id").and_then(text).unwrap_or_default(),
            kind: get("connection", "type").and_then(text).unwrap_or_default(),
            timestamp: get("connection", "timestamp")
                .and_then(integer)
                .and_then(|timestamp| timestamp.try_into().ok())
                .unwrap_or_default(),
            ssid: get("802-11-wireless", "ssid")
                .and_then(bytes)
                .unwrap_or_default(),
            mode: get("802-11-wireless", "mode")
                .and_then(text)
                .unwrap_or_default(),
            bssid: get("802-11-wireless", "bssid")
                .and_then(bytes)
                .unwrap_or_default(),
            shared: get("ipv4", "method").and_then(text).as_deref() == Some("shared"),
        }
    }

    /// The profile is for this access point. Port of libnm's
    /// `nm_access_point_connection_valid` up to the mode. Its band, channel
    /// and security checks are left to NetworkManager, which has already
    /// applied them to the profiles it calls available for the adapter.
    fn fits(&self, access_point: &AccessPoint) -> bool {
        if self.kind != "802-11-wireless" || self.ssid.is_empty() {
            return false;
        }
        if self.ssid != access_point.ssid {
            return false;
        }
        let Some(bssid) = access_point.bssid() else {
            return false;
        };
        if !self.bssid.is_empty() && self.bssid != bssid {
            return false;
        }
        match (self.mode.as_str(), access_point.mode) {
            (_, MODE_UNKNOWN) => false,
            ("infrastructure", mode) => mode == MODE_INFRA,
            ("adhoc", mode) => mode == MODE_ADHOC,
            // A hotspot profile is the adapter's own; it never matches.
            ("ap", _) => false,
            _ => true,
        }
    }
}

/// What a device's connection looks like: the facts the icons in
/// `NMWirelessDeviceItem` and `NMWiredDeviceItem` are chosen by.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Link {
    status: Status,
    hotspot: bool,
    limited: bool,
    /// Of the access point in use.
    strength: Option<u8>,
}

/// One row of the Wi-Fi menu: the access points GNOME's `WirelessNetwork`
/// groups together, which share name, mode and security.
struct Group<'a> {
    ssid: &'a [u8],
    name: String,
    mode: u32,
    security: Security,
    /// Paths, in the adapter's order.
    access_points: Vec<&'a str>,
    strength: u8,
    /// The strongest access point.
    best: &'a str,
    /// The available profiles that are for it (`checkConnections`).
    profiles: Vec<&'a str>,
    active: bool,
}

impl System {
    /// Stores a property; says whether the shell could see a difference.
    pub fn set(&mut self, path: &str, interface: &str, name: &str, value: &Variant<'_>) -> bool {
        let path = path.to_owned();
        match interface {
            MANAGER => self.manager.set(name, value),
            DEVICE => self.devices.entry(path).or_default().set(name, value),
            WIRELESS => self
                .devices
                .entry(path)
                .or_default()
                .set_wireless(name, value),
            ACCESS_POINT => self.access_points.entry(path).or_default().set(name, value),
            ACTIVE => self.active.entry(path).or_default().set(name, value),
            _ => false,
        }
    }

    /// Forgets an object that lost `interface`; says whether it was known.
    pub fn remove(&mut self, path: &str, interface: &str) -> bool {
        match interface {
            DEVICE => self.devices.remove(path).is_some(),
            ACCESS_POINT => self.access_points.remove(path).is_some(),
            ACTIVE => self.active.remove(path).is_some(),
            PROFILE => self.profiles.remove(path).is_some(),
            _ => false,
        }
    }

    /// The saved connections whose settings [`System::state`] and
    /// [`System::plan`] read: those the two adapters could use, and those
    /// they are using.
    pub fn wanted_profiles(&self) -> BTreeSet<String> {
        let mut wanted = BTreeSet::new();
        for (_, device) in [self.toggle_device(WIFI), self.toggle_device(ETHERNET)]
            .into_iter()
            .flatten()
        {
            wanted.extend(device.available_connections.iter().cloned());
            wanted.extend(self.active_of(device).map(|active| active.profile.clone()));
        }
        wanted.retain(|path| object(path).is_some());
        wanted
    }

    pub fn state(&self) -> State {
        if !self.running {
            return State::default();
        }
        let wifi = self.toggle_device(WIFI);
        let wired = self.toggle_device(ETHERNET);
        State {
            primary: self.primary(wifi, wired),
            wifi: wifi.map(|(path, device)| self.wifi(path, device)),
            wired: wired.map(|(_, device)| {
                let link = self.link(device);
                Wired {
                    status: link.status,
                    limited: link.limited,
                }
            }),
        }
    }

    /// What `command` asks of NetworkManager: `Ok(None)` when there is
    /// nothing to do, `Err` with the reason when it cannot be done.
    pub fn plan(&self, command: &Command) -> Result<Option<Request>, String> {
        let wifi = || {
            self.toggle_device(WIFI)
                .ok_or_else(|| "there is no Wi-Fi adapter".to_owned())
        };
        let wired = || {
            self.toggle_device(ETHERNET)
                .ok_or_else(|| "there is no wired adapter".to_owned())
        };
        match command {
            Command::SetWifiEnabled(enabled) => Ok(Some(Request::SetWirelessEnabled(*enabled))),
            // Port of `WirelessNetwork.activate`.
            Command::ConnectWifi(ssid) => {
                let (path, device) = wifi()?;
                let rows = self.rows(device);
                let Some((_, group)) = rows.iter().find(|(network, _)| network.ssid == *ssid)
                else {
                    return Err(format!("{ssid:?} is not in range"));
                };
                let device = path.to_owned();
                if let Some(profile) = group.profiles.first() {
                    Ok(Some(Request::Activate {
                        profile: (*profile).to_owned(),
                        device,
                    }))
                } else if !group.security.can_autoconnect() {
                    // GNOME opens Settings for these.
                    Err(format!(
                        "{ssid:?} needs enterprise credentials, which gniri cannot set up"
                    ))
                } else {
                    Ok(Some(Request::Join {
                        device,
                        access_point: group.best.to_owned(),
                    }))
                }
            }
            // The menu has no such item in GNOME; a hotspot is stopped the
            // way `NMWirelessDeviceItem.activate` does, so that the adapter
            // goes back to the networks it knows. Anything else is dropped
            // like a wired connection, which also keeps NetworkManager from
            // joining again on its own.
            Command::DisconnectWifi => {
                let (path, device) = wifi()?;
                Ok(object(&device.active_connection).map(|active| {
                    if self.link(device).hotspot {
                        Request::Deactivate {
                            active: active.to_owned(),
                        }
                    } else {
                        Request::Disconnect {
                            device: path.to_owned(),
                        }
                    }
                }))
            }
            // Port of `NMWirelessToggle._scanDevices`.
            Command::ScanWifi => {
                let (path, _) = wifi()?;
                Ok(self.manager.wireless_enabled.then(|| Request::Scan {
                    device: path.to_owned(),
                }))
            }
            // Port of `NMDeviceItem.activate`.
            Command::SetWiredConnected(connected) => {
                let (path, device) = wired()?;
                let device_path = path.to_owned();
                let is_active = object(&device.active_connection).is_some();
                Ok(match (connected, is_active) {
                    (true, true) | (false, false) => None,
                    (false, true) => Some(Request::Disconnect {
                        device: device_path,
                    }),
                    (true, false) => Some(match self.activatable(device) {
                        Some(profile) => Request::Activate {
                            profile: profile.to_owned(),
                            device: device_path,
                        },
                        None => Request::AutoConnect {
                            device: device_path,
                        },
                    }),
                })
            }
        }
    }

    /// The device GNOME's toggle for `kind` stands for. Port of
    /// `NMToggle._getPrimaryItem` and `NMWirelessToggle`'s: a hotspot, else
    /// the first device with a connection, else the first. (GNOME orders
    /// devices by name; this follows NetworkManager's order.)
    fn toggle_device(&self, kind: u32) -> Option<(&str, &Device)> {
        if !self.manager.networking_enabled {
            return None;
        }
        let shown = || {
            self.manager.devices.iter().filter_map(|path| {
                let device = self.devices.get(path)?;
                (device.kind == kind && device.is_shown()).then_some((path.as_str(), device))
            })
        };
        shown()
            .find(|(_, device)| self.link(device).hotspot)
            .or_else(|| {
                // `NMMenuItem.is_active`.
                shown().find(|(_, device)| {
                    self.active_of(device)
                        .is_some_and(|active| active.state <= ACTIVATED)
                })
            })
            .or_else(|| shown().next())
    }

    fn active_of(&self, device: &Device) -> Option<&Active> {
        self.active.get(object(&device.active_connection)?)
    }

    fn link(&self, device: &Device) -> Link {
        let Some(active) = self.active_of(device) else {
            return Link::default();
        };
        let status = match active.state {
            ACTIVATING => Status::Connecting,
            ACTIVATED => Status::Connected,
            // The Wi-Fi icon spins on the way down too; the wired one does
            // not.
            DEACTIVATING if device.kind == WIFI => Status::Connecting,
            _ => Status::Idle,
        };
        // Port of `_canReachInternet`: only the primary connection can be
        // told to be cut off.
        let is_primary =
            object(&self.manager.primary_connection) == object(&device.active_connection);
        Link {
            status,
            // Port of `NMWirelessDeviceItem.is_hotspot`.
            hotspot: device.kind == WIFI
                && self
                    .profiles
                    .get(&active.profile)
                    .is_some_and(|profile| profile.shared),
            limited: status == Status::Connected && is_primary && self.manager.connectivity != FULL,
            strength: object(&device.active_access_point)
                .and_then(|path| self.access_points.get(path))
                .map(|access_point| access_point.strength),
        }
    }

    fn wifi(&self, path: &str, device: &Device) -> Wifi {
        let link = self.link(device);
        let enabled = self.manager.wireless_enabled;
        Wifi {
            enabled,
            hardware_enabled: self.manager.wireless_hardware_enabled,
            networks: self
                .rows(device)
                .into_iter()
                .map(|(network, _)| network)
                .collect(),
            scanning: enabled
                && self
                    .scan
                    .as_ref()
                    .is_some_and(|(scanned, at)| scanned == path && *at == device.last_scan),
            hotspot: link.hotspot,
            limited: link.limited,
            active_strength: link.strength,
        }
    }

    /// Port of `Indicator._syncMainConnection` and `_updateIcon`.
    fn primary(
        &self,
        wifi: Option<(&str, &Device)>,
        wired: Option<(&str, &Device)>,
    ) -> Option<Connection> {
        let main = object(&self.manager.primary_connection)
            .or(object(&self.manager.activating_connection))
            .and_then(|path| self.active.get(path));
        let device = main
            .and_then(|main| main.devices.first())
            .and_then(|path| self.devices.get(path));
        // The icon is that of the type's toggle, which with several devices
        // of a type need not stand for this one.
        match device {
            Some(device) if device.kind == WIFI => {
                let link = self.link(wifi.map_or(device, |(_, toggle)| toggle));
                Some(Connection::Wifi {
                    strength: link.strength.unwrap_or(0),
                    status: link.status,
                    limited: link.limited,
                    hotspot: link.hotspot,
                })
            }
            Some(device) if device.kind == ETHERNET => {
                let link = self.link(wired.map_or(device, |(_, toggle)| toggle));
                Some(Connection::Wired {
                    status: link.status,
                    limited: link.limited,
                })
            }
            // No toggle to take the icon from: GNOME shows one only once
            // NetworkManager calls itself connected. (It has toggles for
            // modems and Bluetooth tethers as well; those are not ported,
            // so their connections end up here.)
            _ => (self.manager.state == CONNECTED_GLOBAL).then_some(Connection::Other {
                limited: self.manager.connectivity != FULL,
            }),
        }
    }

    /// The Wi-Fi menu: its rows in order, each with the access points
    /// behind it. Port of `NMWirelessDeviceItem._addAccessPoint` and
    /// `WirelessNetwork`.
    fn rows<'a>(&'a self, device: &'a Device) -> Vec<(Network, Group<'a>)> {
        let mut groups: Vec<Group<'a>> = Vec::new();
        for path in &device.access_points {
            let Some(access_point) = self.access_points.get(path) else {
                continue;
            };
            let Some(name) = access_point.name() else {
                continue;
            };
            let Some(security) = Security::of(access_point.security_flags(), device.capabilities)
            else {
                continue;
            };
            let found = groups.iter_mut().find(|group| {
                group.ssid == access_point.ssid
                    && group.mode == access_point.mode
                    && group.security == security
            });
            match found {
                Some(group) => {
                    group.access_points.push(path);
                    if access_point.strength > group.strength {
                        group.strength = access_point.strength;
                        group.best = path;
                    }
                }
                None => groups.push(Group {
                    ssid: &access_point.ssid,
                    name,
                    mode: access_point.mode,
                    security,
                    access_points: vec![path],
                    strength: access_point.strength,
                    best: path,
                    profiles: Vec::new(),
                    active: false,
                }),
            }
        }

        let link = self.link(device);
        let active_profile = self.active_of(device).map(|active| active.profile.as_str());
        let mut rows: Vec<(Network, Group<'a>)> = groups
            .into_iter()
            .map(|mut group| {
                group.profiles = device
                    .available_connections
                    .iter()
                    .filter(|path| {
                        self.profiles.get(*path).is_some_and(|profile| {
                            group.access_points.iter().any(|access_point| {
                                self.access_points
                                    .get(*access_point)
                                    .is_some_and(|access_point| profile.fits(access_point))
                            })
                        })
                    })
                    .map(String::as_str)
                    .collect();
                // Port of `WirelessNetwork.is_active`.
                group.active = group
                    .access_points
                    .contains(&device.active_access_point.as_str())
                    || active_profile.is_some_and(|profile| group.profiles.contains(&profile));
                let network = Network {
                    ssid: group.name.clone(),
                    strength: group.strength,
                    secure: group.security.is_secure(),
                    enterprise: !group.security.can_autoconnect(),
                    known: !group.profiles.is_empty(),
                    status: if group.active {
                        link.status
                    } else {
                        Status::Idle
                    },
                };
                (network, group)
            })
            .collect();
        arrange(&mut rows, |(network, _)| network);
        rows
    }

    /// The profile the wired toggle brings up. Port of
    /// `NMDeviceItem._getActivatableItem`: the one used last, else the
    /// first by name.
    fn activatable<'a>(&self, device: &'a Device) -> Option<&'a str> {
        let profiles = || {
            device.available_connections.iter().filter_map(|path| {
                let profile = self.profiles.get(path)?;
                (!profile.id.is_empty()).then_some((path.as_str(), profile))
            })
        };
        profiles()
            .filter(|(_, profile)| profile.timestamp > 0)
            .max_by_key(|(_, profile)| profile.timestamp)
            .or_else(|| profiles().min_by(|(_, one), (_, two)| collate(&one.id, &two.id)))
            .map(|(path, _)| path)
    }
}

#[cfg(test)]
mod tests {
    use zbus::ObjectPath;

    use super::*;

    const WLAN: &str = "/device/wlan";
    const ETH: &str = "/device/eth";
    /// What an adapter that can do everything reports.
    const CAPABILITIES: u32 = 0x203f;
    /// `RsnFlags` of a WPA2 network with a password.
    const WPA2: u32 = 0x188;
    const WPA2_ENTERPRISE: u32 = 0x288;

    fn text(text: &str) -> String {
        text.to_owned()
    }

    /// NetworkManager running, with nothing connected.
    fn system() -> System {
        System {
            running: true,
            manager: Manager {
                networking_enabled: true,
                wireless_enabled: true,
                wireless_hardware_enabled: true,
                connectivity: FULL,
                state: 20,
                ..Manager::default()
            },
            ..System::default()
        }
    }

    fn add_device(system: &mut System, path: &str, kind: u32, state: u32) {
        system.manager.devices.push(text(path));
        system.devices.insert(
            text(path),
            Device {
                kind,
                state,
                active_connection: text("/"),
                active_access_point: text("/"),
                capabilities: CAPABILITIES,
                ..Device::default()
            },
        );
    }

    /// An access point on [`WLAN`]; `rsn` 0 makes it open.
    fn add_access_point(system: &mut System, path: &str, ssid: &str, strength: u8, rsn: u32) {
        system.access_points.insert(
            text(path),
            AccessPoint {
                ssid: ssid.as_bytes().to_vec(),
                hw_address: format!("00:00:00:00:00:{:02X}", system.access_points.len()),
                strength,
                flags: u32::from(rsn != 0),
                wpa_flags: 0,
                rsn_flags: rsn,
                mode: MODE_INFRA,
            },
        );
        let device = system.devices.get_mut(WLAN).unwrap();
        device.access_points.push(text(path));
    }

    /// A saved Wi-Fi connection that the adapter can use.
    fn add_profile(system: &mut System, path: &str, ssid: &str) {
        system.profiles.insert(
            text(path),
            Profile {
                id: text(ssid),
                kind: text("802-11-wireless"),
                ssid: ssid.as_bytes().to_vec(),
                ..Profile::default()
            },
        );
        let device = system.devices.get_mut(WLAN).unwrap();
        device.available_connections.push(text(path));
    }

    fn activate(system: &mut System, device: &str, active: &str, profile: &str, state: u32) {
        system.active.insert(
            text(active),
            Active {
                state,
                profile: text(profile),
                devices: vec![text(device)],
            },
        );
        system.devices.get_mut(device).unwrap().active_connection = text(active);
    }

    /// A Wi-Fi adapter in range of a known network with two access points,
    /// an open one, and a locked one.
    fn neighbourhood() -> System {
        let mut system = system();
        add_device(&mut system, WLAN, WIFI, DISCONNECTED);
        add_access_point(&mut system, "/ap/home-far", "home", 45, WPA2);
        add_access_point(&mut system, "/ap/cafe", "cafe", 80, 0);
        add_access_point(&mut system, "/ap/home-near", "home", 62, WPA2);
        add_access_point(&mut system, "/ap/office", "office", 30, WPA2);
        add_profile(&mut system, "/profile/home", "home");
        system
    }

    /// The same, connected to the known network through the farther access
    /// point, which carries the default route.
    fn at_home() -> System {
        let mut system = neighbourhood();
        activate(&mut system, WLAN, "/active/1", "/profile/home", ACTIVATED);
        system.devices.get_mut(WLAN).unwrap().active_access_point = text("/ap/home-far");
        system.manager.primary_connection = text("/active/1");
        system.manager.state = CONNECTED_GLOBAL;
        system
    }

    fn wifi(system: &System) -> Wifi {
        system.state().wifi.expect("a Wi-Fi adapter")
    }

    fn names(wifi: &Wifi) -> Vec<&str> {
        wifi.networks
            .iter()
            .map(|network| network.ssid.as_str())
            .collect()
    }

    #[test]
    fn nothing_without_network_manager_or_adapters() {
        assert_eq!(System::default().state(), State::default());
        assert_eq!(system().state(), State::default());
        // What was known of a NetworkManager that has gone does not count.
        let mut gone = at_home();
        gone.running = false;
        assert_eq!(gone.state(), State::default());
    }

    #[test]
    fn the_toggles_are_there_as_gnome_shows_them() {
        let shown = |kind, state| {
            let mut system = system();
            add_device(&mut system, "/device", kind, state);
            let state = system.state();
            state.wifi.is_some() || state.wired.is_some()
        };
        // Unmanaged, then unavailable: the radio is off, or no cable.
        assert!(!shown(WIFI, 10));
        assert!(shown(WIFI, UNAVAILABLE));
        assert!(!shown(ETHERNET, 10));
        assert!(!shown(ETHERNET, UNAVAILABLE));
        for state in [DISCONNECTED, 40, 60, 100, 110, FAILED] {
            assert!(shown(WIFI, state));
            assert!(shown(ETHERNET, state));
        }

        // Networking off, and a device NetworkManager no longer lists.
        let mut system = neighbourhood();
        system.manager.networking_enabled = false;
        assert_eq!(system.state().wifi, None);
        let mut system = neighbourhood();
        system.manager.devices.clear();
        assert_eq!(system.state().wifi, None);
    }

    #[test]
    fn access_points_become_the_rows_of_the_menu() {
        let system = neighbourhood();
        let wifi = wifi(&system);
        assert!(wifi.enabled && wifi.hardware_enabled && !wifi.scanning);
        assert_eq!(names(&wifi), ["home", "cafe", "office"]);
        assert_eq!(
            wifi.networks[0],
            Network {
                ssid: text("home"),
                // Of the nearer of its two access points.
                strength: 62,
                secure: true,
                enterprise: false,
                known: true,
                status: Status::Idle,
            }
        );
        assert_eq!(
            wifi.networks[1],
            Network {
                ssid: text("cafe"),
                strength: 80,
                secure: false,
                enterprise: false,
                known: false,
                status: Status::Idle,
            }
        );
        assert!(wifi.networks[2].secure && !wifi.networks[2].known);
        assert_eq!(wifi.current(), None);
        assert_eq!(wifi.active_strength, None);
    }

    #[test]
    fn access_points_the_menu_leaves_out() {
        let mut system = neighbourhood();
        // Hidden: no name, or one of zero bytes.
        add_access_point(&mut system, "/ap/hidden", "", 90, WPA2);
        add_access_point(&mut system, "/ap/zeros", "\0\0\0", 90, WPA2);
        // SAE over TKIP alone: libnm finds no security type for it.
        add_access_point(&mut system, "/ap/invalid", "invalid", 90, 0x444);
        // Listed by the adapter but never described.
        let device = system.devices.get_mut(WLAN).unwrap();
        device.access_points.push(text("/ap/unknown"));
        assert_eq!(names(&wifi(&system)), ["home", "cafe", "office"]);

        // An adapter that cannot do WPA2 sees only the open network.
        system.devices.get_mut(WLAN).unwrap().capabilities = 0x1f;
        assert_eq!(names(&wifi(&system)), ["cafe"]);
    }

    #[test]
    fn one_name_with_two_kinds_of_security_is_one_row() {
        let mut system = neighbourhood();
        add_access_point(&mut system, "/ap/cafe-locked", "cafe", 85, WPA2);
        let wifi = wifi(&system);
        assert_eq!(names(&wifi), ["home", "cafe", "office"]);
        // The stronger row is the one GNOME lists first.
        let cafe = &wifi.networks[1];
        assert_eq!((cafe.strength, cafe.secure), (85, true));

        // Joining goes by the row that is listed.
        assert_eq!(
            system.plan(&Command::ConnectWifi(text("cafe"))),
            Ok(Some(Request::Join {
                device: text(WLAN),
                access_point: text("/ap/cafe-locked"),
            }))
        );
    }

    #[test]
    fn a_profile_makes_a_network_known_only_if_it_is_for_it() {
        let known = |change: fn(&mut Profile)| {
            let mut system = neighbourhood();
            change(system.profiles.get_mut("/profile/home").unwrap());
            wifi(&system).networks.iter().any(|network| network.known)
        };
        assert!(known(|_| {}));
        assert!(known(|profile| profile.mode = text("infrastructure")));
        // Tied to the nearer access point, then to one that is not there.
        assert!(known(|profile| profile.bssid = vec![0, 0, 0, 0, 0, 2]));
        assert!(!known(|profile| profile.bssid = vec![0, 0, 0, 0, 0, 9]));
        assert!(!known(|profile| profile.ssid = b"elsewhere".to_vec()));
        assert!(!known(|profile| profile.mode = text("adhoc")));
        // A hotspot of the same name.
        assert!(!known(|profile| profile.mode = text("ap")));
        assert!(!known(|profile| profile.kind = text("802-3-ethernet")));

        // Saved, but not for this adapter as far as NetworkManager says.
        let mut system = neighbourhood();
        let device = system.devices.get_mut(WLAN).unwrap();
        device.available_connections.clear();
        assert!(wifi(&system).networks.iter().all(|network| !network.known));
    }

    #[test]
    fn the_network_in_use() {
        let system = at_home();
        let wifi = wifi(&system);
        let current = wifi.current().unwrap();
        assert_eq!(current.ssid, "home");
        assert_eq!(current.status, Status::Connected);
        let active = wifi.networks.iter().filter(|network| network.is_active());
        assert_eq!(active.count(), 1);
        // The row shows the nearer access point, the toggle the one in use.
        assert_eq!(current.strength, 62);
        assert_eq!(wifi.active_strength, Some(45));
        assert_eq!(wifi.icon_name(), "network-wireless-signal-ok-symbolic");
        assert_eq!(wifi.subtitle(), Some("home"));
        assert!(!wifi.limited && !wifi.hotspot);
    }

    #[test]
    fn the_network_in_use_is_found_by_access_point_or_by_profile() {
        // By access point alone: its profile is not among those the adapter
        // lists.
        let mut system = neighbourhood();
        activate(&mut system, WLAN, "/active/1", "/profile/cafe", ACTIVATED);
        system.devices.get_mut(WLAN).unwrap().active_access_point = text("/ap/cafe");
        assert_eq!(wifi(&system).subtitle(), Some("cafe"));

        // By profile alone: no access point chosen yet.
        let mut system = neighbourhood();
        activate(&mut system, WLAN, "/active/1", "/profile/home", ACTIVATING);
        let wifi = wifi(&system);
        assert_eq!(wifi.subtitle(), Some("home"));
        assert_eq!(wifi.current().unwrap().status, Status::Connecting);
        assert_eq!(wifi.active_strength, None);
        assert_eq!(wifi.icon_name(), "network-wireless-acquiring-symbolic");
    }

    #[test]
    fn connections_on_their_way_up_and_down() {
        let status = |kind, state| {
            let mut system = system();
            add_device(&mut system, "/device", kind, 100);
            activate(&mut system, "/device", "/active/1", "/profile", state);
            system.link(&system.devices["/device"]).status
        };
        for kind in [WIFI, ETHERNET] {
            assert_eq!(status(kind, 0), Status::Idle);
            assert_eq!(status(kind, ACTIVATING), Status::Connecting);
            assert_eq!(status(kind, ACTIVATED), Status::Connected);
            assert_eq!(status(kind, 4), Status::Idle);
        }
        assert_eq!(status(WIFI, DEACTIVATING), Status::Connecting);
        assert_eq!(status(ETHERNET, DEACTIVATING), Status::Idle);

        // An active connection NetworkManager has not described yet.
        let mut system = neighbourhood();
        system.devices.get_mut(WLAN).unwrap().active_connection = text("/active/9");
        assert_eq!(wifi(&system).current(), None);
    }

    #[test]
    fn only_the_primary_connection_is_called_cut_off() {
        let mut system = at_home();
        system.manager.connectivity = 3;
        system.manager.state = 60;
        let state = system.state();
        assert!(state.wifi.as_ref().unwrap().limited);
        assert_eq!(
            state.panel_icon_name(),
            Some("network-wireless-no-route-symbolic")
        );
        assert_eq!(
            state.wifi.unwrap().icon_name(),
            "network-wireless-no-route-symbolic"
        );

        // A wire has the route: nothing is known about Wi-Fi's reach.
        add_device(&mut system, ETH, ETHERNET, 100);
        activate(&mut system, ETH, "/active/2", "/profile/wire", ACTIVATED);
        system.manager.primary_connection = text("/active/2");
        let state = system.state();
        assert!(!state.wifi.as_ref().unwrap().limited);
        let cut_off = Wired {
            status: Status::Connected,
            limited: true,
        };
        assert_eq!(state.wired, Some(cut_off));
        assert_eq!(
            state.primary,
            Some(Connection::Wired {
                status: Status::Connected,
                limited: true,
            })
        );

        // Not while it is still connecting.
        system.active.get_mut("/active/2").unwrap().state = ACTIVATING;
        let connecting = Wired {
            status: Status::Connecting,
            limited: false,
        };
        assert_eq!(system.state().wired, Some(connecting));
    }

    #[test]
    fn a_hotspot() {
        let mut system = system();
        add_device(&mut system, WLAN, WIFI, 100);
        // NetworkManager lists the adapter's own network.
        add_access_point(&mut system, "/ap/own", "laptop", 0, WPA2);
        system.access_points.get_mut("/ap/own").unwrap().mode = 3;
        activate(
            &mut system,
            WLAN,
            "/active/1",
            "/profile/hotspot",
            ACTIVATED,
        );
        system.devices.get_mut(WLAN).unwrap().active_access_point = text("/ap/own");
        system.manager.primary_connection = text("/active/1");
        // Before the settings are read it looks like any connection.
        assert_eq!(
            system.wanted_profiles(),
            BTreeSet::from([text("/profile/hotspot")])
        );
        assert!(!wifi(&system).hotspot);

        system.profiles.insert(
            text("/profile/hotspot"),
            Profile {
                kind: text("802-11-wireless"),
                ssid: b"laptop".to_vec(),
                mode: text("ap"),
                shared: true,
                ..Profile::default()
            },
        );
        let state = system.state();
        let wifi = state.wifi.as_ref().unwrap();
        assert!(wifi.hotspot);
        assert_eq!(wifi.icon_name(), "network-wireless-hotspot-symbolic");
        assert_eq!(wifi.subtitle(), Some("Wi-Fi Hotspot"));
        assert_eq!(
            state.panel_icon_name(),
            Some("network-wireless-hotspot-symbolic")
        );

        // Stopping it deactivates the connection rather than the adapter.
        let stop = Request::Deactivate {
            active: text("/active/1"),
        };
        assert_eq!(system.plan(&Command::DisconnectWifi), Ok(Some(stop)));
    }

    #[test]
    fn the_panel_follows_the_primary_then_the_activating_connection() {
        let mut system = at_home();
        assert_eq!(
            system.state().primary,
            Some(Connection::Wifi {
                strength: 45,
                status: Status::Connected,
                limited: false,
                hotspot: false,
            })
        );

        // A wire coming up beside it does not take the icon yet.
        add_device(&mut system, ETH, ETHERNET, 70);
        activate(&mut system, ETH, "/active/2", "/profile/wire", ACTIVATING);
        system.manager.activating_connection = text("/active/2");
        assert!(matches!(
            system.state().primary,
            Some(Connection::Wifi { .. })
        ));

        // Without a primary connection it does.
        system.manager.primary_connection = text("/");
        assert_eq!(
            system.state().primary,
            Some(Connection::Wired {
                status: Status::Connecting,
                limited: false,
            })
        );
        assert_eq!(
            system.state().panel_icon_name(),
            Some("network-wired-acquiring-symbolic")
        );

        system.manager.activating_connection = text("/");
        system.manager.state = 20;
        assert_eq!(system.state().primary, None);
    }

    #[test]
    fn a_connection_without_a_toggle_shows_once_network_manager_is_connected() {
        let mut system = system();
        // A WireGuard tunnel.
        add_device(&mut system, "/device/wg", 29, 100);
        activate(
            &mut system,
            "/device/wg",
            "/active/1",
            "/profile/wg",
            ACTIVATED,
        );
        system.manager.primary_connection = text("/active/1");
        system.manager.state = 60;
        assert_eq!(system.state().primary, None);
        system.manager.state = CONNECTED_GLOBAL;
        let other = |limited| Some(Connection::Other { limited });
        assert_eq!(system.state().primary, other(false));
        system.manager.connectivity = 3;
        assert_eq!(system.state().primary, other(true));

        // Connected with nothing NetworkManager calls primary.
        let mut system = self::system();
        system.manager.state = CONNECTED_GLOBAL;
        assert_eq!(
            system.state().panel_icon_name(),
            Some("network-wired-symbolic")
        );
        system.manager.state = 20;
        assert_eq!(system.state().panel_icon_name(), None);
    }

    #[test]
    fn the_toggle_stands_for_the_adapter_that_is_connected() {
        let mut system = neighbourhood();
        add_device(&mut system, "/device/dongle", WIFI, 100);
        assert_eq!(system.toggle_device(WIFI).unwrap().0, WLAN);
        activate(
            &mut system,
            "/device/dongle",
            "/active/1",
            "/profile/x",
            ACTIVATED,
        );
        assert_eq!(system.toggle_device(WIFI).unwrap().0, "/device/dongle");
        // The dongle sees no networks of its own here.
        assert!(wifi(&system).networks.is_empty());
    }

    #[test]
    fn a_scan_lasts_until_the_adapter_reports_one() {
        let mut system = neighbourhood();
        system.devices.get_mut(WLAN).unwrap().last_scan = 1000;
        let scan = Request::Scan { device: text(WLAN) };
        assert_eq!(system.plan(&Command::ScanWifi), Ok(Some(scan)));
        system.scan = Some((text(WLAN), 1000));
        assert!(wifi(&system).scanning);
        system.devices.get_mut(WLAN).unwrap().last_scan = 2000;
        assert!(!wifi(&system).scanning);

        // Nothing is scanned for, or said to be, with the radio off.
        system.scan = Some((text(WLAN), 2000));
        system.manager.wireless_enabled = false;
        assert!(!wifi(&system).scanning);
        assert_eq!(system.plan(&Command::ScanWifi), Ok(None));
    }

    #[test]
    fn joining_a_network() {
        let system = neighbourhood();
        let join = |ssid: &str| system.plan(&Command::ConnectWifi(text(ssid)));
        assert_eq!(
            join("home"),
            Ok(Some(Request::Activate {
                profile: text("/profile/home"),
                device: text(WLAN),
            }))
        );
        assert_eq!(
            join("cafe"),
            Ok(Some(Request::Join {
                device: text(WLAN),
                access_point: text("/ap/cafe"),
            }))
        );
        // A password is asked for by NetworkManager, through the agent.
        assert_eq!(
            join("office"),
            Ok(Some(Request::Join {
                device: text(WLAN),
                access_point: text("/ap/office"),
            }))
        );
        assert!(join("nowhere").unwrap_err().contains("not in range"));

        let mut system = neighbourhood();
        add_access_point(&mut system, "/ap/campus", "campus", 50, WPA2_ENTERPRISE);
        let plan = system.plan(&Command::ConnectWifi(text("campus")));
        assert!(plan.unwrap_err().contains("enterprise"));
        let campus = |system: &System| {
            let wifi = wifi(system);
            let row = wifi.networks.iter().find(|row| row.ssid == "campus");
            row.cloned().unwrap()
        };
        assert!(campus(&system).enterprise && !campus(&system).can_connect());
        // With a profile it is a network like any other.
        add_profile(&mut system, "/profile/campus", "campus");
        assert!(campus(&system).can_connect());
        assert!(matches!(
            system.plan(&Command::ConnectWifi(text("campus"))),
            Ok(Some(Request::Activate { .. }))
        ));

        // An open network is joined through its strongest access point.
        add_access_point(&mut system, "/ap/cafe-near", "cafe", 95, 0);
        assert_eq!(
            system.plan(&Command::ConnectWifi(text("cafe"))),
            Ok(Some(Request::Join {
                device: text(WLAN),
                access_point: text("/ap/cafe-near"),
            }))
        );
    }

    #[test]
    fn leaving_a_network_and_switching_the_radio() {
        let leave = Request::Disconnect { device: text(WLAN) };
        assert_eq!(at_home().plan(&Command::DisconnectWifi), Ok(Some(leave)));
        assert_eq!(neighbourhood().plan(&Command::DisconnectWifi), Ok(None));
        assert_eq!(
            neighbourhood().plan(&Command::SetWifiEnabled(false)),
            Ok(Some(Request::SetWirelessEnabled(false)))
        );
        for command in [
            Command::ConnectWifi(text("home")),
            Command::DisconnectWifi,
            Command::ScanWifi,
            Command::SetWiredConnected(true),
        ] {
            assert!(system().plan(&command).unwrap_err().contains("adapter"));
        }
    }

    #[test]
    fn the_wire_comes_up_with_the_profile_used_last() {
        let mut system = system();
        add_device(&mut system, ETH, ETHERNET, DISCONNECTED);
        let up = Command::SetWiredConnected(true);
        let down = Command::SetWiredConnected(false);
        let auto = Request::AutoConnect { device: text(ETH) };
        assert_eq!(system.plan(&up), Ok(Some(auto)));
        assert_eq!(system.plan(&down), Ok(None));

        let add = |system: &mut System, path: &str, id: &str| {
            system.profiles.insert(
                text(path),
                Profile {
                    id: text(id),
                    kind: text("802-3-ethernet"),
                    ..Profile::default()
                },
            );
            let device = system.devices.get_mut(ETH).unwrap();
            device.available_connections.push(text(path));
        };
        add(&mut system, "/profile/office", "Office");
        add(&mut system, "/profile/dock", "dock");
        let brought_up = |system: &System| match system.plan(&up) {
            Ok(Some(Request::Activate { profile, device })) if device == ETH => profile,
            other => panic!("not an activation: {other:?}"),
        };
        // None was ever used: the first by name.
        assert_eq!(brought_up(&system), "/profile/dock");
        system
            .profiles
            .get_mut("/profile/office")
            .unwrap()
            .timestamp = 1_700_000_000;
        assert_eq!(brought_up(&system), "/profile/office");
        system.profiles.get_mut("/profile/dock").unwrap().timestamp = 1_800_000_000;
        assert_eq!(brought_up(&system), "/profile/dock");
        assert_eq!(
            system.wanted_profiles(),
            BTreeSet::from([text("/profile/dock"), text("/profile/office")])
        );

        activate(&mut system, ETH, "/active/1", "/profile/dock", ACTIVATED);
        assert_eq!(system.plan(&up), Ok(None));
        let unplug = Request::Disconnect { device: text(ETH) };
        assert_eq!(system.plan(&down), Ok(Some(unplug)));
    }

    #[test]
    fn properties_are_read_as_network_manager_sends_them() {
        let path = |path: &'static str| Variant::from(ObjectPath::from_static_str_unchecked(path));
        let mut system = System::default();
        assert!(system.set("/", MANAGER, "WirelessEnabled", &Variant::from(true)));
        // The same again is no change.
        assert!(!system.set("/", MANAGER, "WirelessEnabled", &Variant::from(true)));
        assert!(system.set("/", MANAGER, "Connectivity", &Variant::from(4u32)));
        assert!(system.set("/", MANAGER, "PrimaryConnection", &path("/active/1")));
        let devices = Variant::from(vec![ObjectPath::from_static_str_unchecked(WLAN)]);
        assert!(system.set("/", MANAGER, "Devices", &devices));
        // Not something the shell shows, and a value of the wrong type.
        assert!(!system.set("/", MANAGER, "Version", &Variant::from("1.58")));
        assert!(!system.set("/", MANAGER, "State", &Variant::from("seventy")));
        assert!(system.manager.wireless_enabled);
        assert_eq!(system.manager.connectivity, 4);
        assert_eq!(system.manager.primary_connection, "/active/1");
        assert_eq!(system.manager.devices, [WLAN]);

        assert!(system.set(WLAN, DEVICE, "DeviceType", &Variant::from(2u32)));
        assert!(system.set(WLAN, WIRELESS, "LastScan", &Variant::from(123_456i64)));
        let capabilities = Variant::from(0x3fu32);
        assert!(system.set(WLAN, WIRELESS, "WirelessCapabilities", &capabilities));
        // One interface's property under another's name is not taken, nor
        // is an interface that is not followed.
        assert!(!system.set(WLAN, DEVICE, "LastScan", &Variant::from(5i64)));
        let statistics = "org.freedesktop.NetworkManager.Device.Statistics";
        assert!(!system.set(WLAN, statistics, "State", &Variant::from(5u32)));
        let device = &system.devices[WLAN];
        assert_eq!(device.kind, 2);
        assert_eq!(device.last_scan, 123_456);
        assert_eq!(device.capabilities, 0x3f);

        let ssid = Variant::from(b"home".to_vec());
        assert!(system.set("/ap/1", ACCESS_POINT, "Ssid", &ssid));
        assert!(system.set("/ap/1", ACCESS_POINT, "Strength", &Variant::from(61u8)));
        let address = Variant::from("00:1A:2b:03:04:05");
        assert!(system.set("/ap/1", ACCESS_POINT, "HwAddress", &address));
        let access_point = &system.access_points["/ap/1"];
        assert_eq!(access_point.name().as_deref(), Some("home"));
        assert_eq!(access_point.strength, 61);
        assert_eq!(access_point.bssid(), Some(vec![0, 0x1a, 0x2b, 3, 4, 5]));

        assert!(system.set("/active/1", ACTIVE, "State", &Variant::from(2u32)));
        assert!(system.set("/active/1", ACTIVE, "Connection", &path("/profile/home")));
        assert_eq!(system.active["/active/1"].profile, "/profile/home");

        assert!(system.remove("/ap/1", ACCESS_POINT));
        assert!(!system.remove("/ap/1", ACCESS_POINT));
        assert!(system.remove("/active/1", ACTIVE));
        assert!(system.remove(WLAN, DEVICE));
        assert!(!system.remove(WLAN, WIRELESS));
        assert!(system.devices.is_empty() && system.active.is_empty());
    }

    #[test]
    fn a_profile_is_read_from_its_settings() {
        let group = |entries: Vec<(&str, Variant<'_>)>| -> HashMap<String, OwnedVariant> {
            entries
                .into_iter()
                .map(|(key, value)| (text(key), OwnedVariant::try_from(value).unwrap()))
                .collect()
        };
        let settings = HashMap::from([
            (
                text("connection"),
                group(vec![
                    ("id", Variant::from("Home")),
                    ("type", Variant::from("802-11-wireless")),
                    ("timestamp", Variant::from(1_700_000_000u64)),
                ]),
            ),
            (
                text("802-11-wireless"),
                group(vec![
                    ("ssid", Variant::from(b"home".to_vec())),
                    ("mode", Variant::from("infrastructure")),
                    ("bssid", Variant::from(vec![0u8, 1, 2, 3, 4, 5])),
                ]),
            ),
            (
                text("ipv4"),
                group(vec![("method", Variant::from("shared"))]),
            ),
        ]);
        assert_eq!(
            Profile::from_settings(&settings),
            Profile {
                id: text("Home"),
                kind: text("802-11-wireless"),
                timestamp: 1_700_000_000,
                ssid: b"home".to_vec(),
                mode: text("infrastructure"),
                bssid: vec![0, 1, 2, 3, 4, 5],
                shared: true,
            }
        );
        assert_eq!(Profile::from_settings(&HashMap::new()), Profile::default());
    }

    #[test]
    fn only_the_profiles_the_adapters_can_use_are_wanted() {
        let mut system = at_home();
        system.profiles.clear();
        assert_eq!(
            system.wanted_profiles(),
            BTreeSet::from([text("/profile/home")])
        );
        // The profile in use is wanted even when the adapter's list has
        // moved on.
        let device = system.devices.get_mut(WLAN).unwrap();
        device.available_connections = vec![text("/profile/a")];
        assert_eq!(
            system.wanted_profiles(),
            BTreeSet::from([text("/profile/a"), text("/profile/home")])
        );
        system.manager.networking_enabled = false;
        assert!(system.wanted_profiles().is_empty());
    }
}
