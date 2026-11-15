//! Bluetooth: the default adapter and the devices set up with it.
//!
//! [`backend`] talks to BlueZ. What the quick settings show is a port of
//! `status/bluetooth.js`; where that leans on the gnome-bluetooth library
//! (which devices can be connected to, what state the adapter is in), the
//! library's rule is ported with it.

pub mod backend;

use std::cmp::Ordering;

use serde::Deserialize;

use crate::model::Model;

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct State {
    /// The default adapter, if the machine has one.
    pub adapter: Option<Adapter>,
    /// The paired or trusted devices of that adapter, connected ones first
    /// and then by name. [`State::menu_devices`] is what the menu lists.
    pub devices: Vec<Device>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Adapter {
    /// On, or on its way there.
    pub powered: bool,
    /// Still on its way to `powered`: GNOME's "turning on" and "turning
    /// off" adapter states.
    pub changing: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Device {
    /// Hardware address, such as "AA:BB:CC:DD:EE:FF"; what commands take.
    pub address: String,
    /// The name the user knows it by.
    pub name: String,
    /// Icon name without "-symbolic", such as "audio-headphones"; may be
    /// empty.
    pub icon: String,
    pub status: Status,
    /// Charge in percent, when the device reports it.
    pub battery: Option<u8>,
    /// The device offers something to connect to (audio, input, MIDI).
    /// GNOME's menu hides the others, though they count as connected.
    pub connectable: bool,
}

impl Default for Device {
    fn default() -> Self {
        Self {
            address: String::new(),
            name: String::new(),
            icon: String::new(),
            status: Status::default(),
            battery: None,
            // So that a fixture's devices show without saying so.
            connectable: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Disconnecting,
}

/// What the backend reports.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Everything, as it is now.
    State(State),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    SetPowered(bool),
    /// Connect to the paired device with this address.
    Connect(String),
    Disconnect(String),
}

impl Device {
    /// The menu icon: BlueZ's, as a symbolic icon, or a generic one.
    pub fn icon_name(&self) -> String {
        if self.icon.is_empty() {
            "bluetooth-symbolic".to_owned()
        } else {
            format!("{}-symbolic", self.icon)
        }
    }

    /// What GNOME counts as connected: the link is up. It still is while a
    /// disconnect is under way, and is not yet while connecting.
    pub fn connected(&self) -> bool {
        matches!(self.status, Status::Connected | Status::Disconnecting)
    }

    /// A request is under way: the row shows a spinner in place of
    /// [`Device::action_label`].
    pub fn busy(&self) -> bool {
        matches!(self.status, Status::Connecting | Status::Disconnecting)
    }

    /// The row's second label: what a click does. Port of the `connected`
    /// binding in `BluetoothDeviceItem`.
    pub fn action_label(&self) -> &'static str {
        if self.connected() {
            "Disconnect"
        } else {
            "Connect"
        }
    }

    /// Port of `BluetoothDeviceItem._updateAccessibleName`.
    pub fn accessible_name(&self) -> String {
        if self.connected() {
            format!("Disconnect {}", self.name)
        } else {
            format!("Connect to {}", self.name)
        }
    }

    /// What clicking the row asks for. Port of `BtClient.toggleDevice`,
    /// except while a request is under way: GNOME sends the same request
    /// again, which BlueZ refuses as already in progress.
    pub fn activated(&self) -> Option<Command> {
        match self.status {
            Status::Disconnected => Some(Command::Connect(self.address.clone())),
            Status::Connected => Some(Command::Disconnect(self.address.clone())),
            Status::Connecting | Status::Disconnecting => None,
        }
    }

    /// The menu's order: connected devices first, then by name. Port of
    /// `BluetoothToggle._getSortedDevices`.
    pub fn menu_order(&self, other: &Self) -> Ordering {
        other
            .connected()
            .cmp(&self.connected())
            .then_with(|| collate(&self.name, &other.name))
            // Only so that the order never depends on where it started.
            .then_with(|| self.address.cmp(&other.address))
    }
}

/// Stands in for JavaScript's `localeCompare`, which needs the Unicode
/// collation tables: letter case matters only between otherwise equal
/// names, and then lower case comes first. Accented letters sort after "z"
/// here instead of with their base letter.
fn collate(a: &str, b: &str) -> Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| b.cmp(a))
}

impl State {
    pub fn powered(&self) -> bool {
        self.adapter.as_ref().is_some_and(|adapter| adapter.powered)
    }

    /// Port of `BtClient.getDevices`: "Ignore any lingering device
    /// references when turned off".
    fn listed(&self) -> &[Device] {
        if self.powered() { &self.devices } else { &[] }
    }

    fn connected(&self) -> impl Iterator<Item = &Device> {
        self.listed().iter().filter(|device| device.connected())
    }

    /// The rows of the menu: the devices worth connecting to. Port of the
    /// `connectable` binding in `BluetoothDeviceItem`.
    pub fn menu_devices(&self) -> impl Iterator<Item = &Device> {
        self.listed().iter().filter(|device| device.connectable)
    }

    /// The text the menu shows instead of an empty list. Port of
    /// `BluetoothToggle._updatePlaceholder`.
    pub fn placeholder(&self) -> Option<&'static str> {
        if self.menu_devices().next().is_some() {
            None
        } else if self.powered() {
            Some("No available or connected devices")
        } else {
            Some("Turn on Bluetooth to connect to devices")
        }
    }

    /// The quick toggle's icon. Port of
    /// `BluetoothToggle._getIconNameFromState`.
    pub fn icon_name(&self) -> &'static str {
        match &self.adapter {
            Some(adapter) if adapter.changing => "bluetooth-acquiring-symbolic",
            Some(adapter) if adapter.powered => "bluetooth-active-symbolic",
            _ => "bluetooth-disabled-symbolic",
        }
    }

    /// The quick toggle's second line: the connected device, or how many.
    /// Port of `BluetoothToggle._sync`.
    pub fn subtitle(&self) -> Option<String> {
        let mut connected = self.connected();
        let first = connected.next()?;
        match connected.count() {
            0 => Some(first.name.clone()),
            others => Some(format!("{} Connected", others + 1)),
        }
    }

    /// GNOME shows the panel icon only while a device is connected. Port
    /// of `Indicator._sync`.
    pub fn shows_indicator(&self) -> bool {
        self.connected().next().is_some()
    }

    /// What clicking the toggle asks for. Port of `BtClient.toggleActive`.
    pub fn toggled(&self) -> Command {
        Command::SetPowered(!self.powered())
    }
}

impl Model for State {
    type Event = Event;
    type Command = Command;

    fn apply(&mut self, event: Event) {
        match event {
            Event::State(state) => *self = state,
        }
    }

    fn predict(&mut self, command: &Command) {
        let set = |devices: &mut [Device], address: &str, status| {
            for device in devices
                .iter_mut()
                .filter(|device| device.address == address)
            {
                device.status = status;
            }
        };
        match command {
            Command::SetPowered(powered) => {
                if let Some(adapter) = &mut self.adapter {
                    adapter.powered = *powered;
                    if !powered {
                        for device in &mut self.devices {
                            device.status = Status::Disconnected;
                        }
                    }
                }
            }
            Command::Connect(address) => set(&mut self.devices, address, Status::Connecting),
            Command::Disconnect(address) => set(&mut self.devices, address, Status::Disconnecting),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(address: &str, name: &str, status: Status) -> Device {
        Device {
            address: address.to_owned(),
            name: name.to_owned(),
            icon: "audio-headphones".to_owned(),
            status,
            ..Device::default()
        }
    }

    fn on(devices: Vec<Device>) -> State {
        State {
            adapter: Some(Adapter {
                powered: true,
                changing: false,
            }),
            devices,
        }
    }

    #[test]
    fn the_toggle_names_the_connected_device_or_counts_them() {
        let mut state = on(vec![
            device("1", "Buds", Status::Connected),
            device("2", "Mouse", Status::Disconnected),
            device("3", "Keys", Status::Disconnected),
        ]);
        assert_eq!(state.icon_name(), "bluetooth-active-symbolic");
        assert_eq!(state.subtitle().as_deref(), Some("Buds"));
        assert!(state.shows_indicator());
        state.devices[1].status = Status::Connected;
        assert_eq!(state.subtitle().as_deref(), Some("2 Connected"));
        state.devices[2].status = Status::Connected;
        assert_eq!(state.subtitle().as_deref(), Some("3 Connected"));
        assert_eq!(state.devices[0].icon_name(), "audio-headphones-symbolic");

        for device in &mut state.devices {
            device.status = Status::Disconnected;
        }
        assert_eq!(state.subtitle(), None);
        assert!(!state.shows_indicator());
    }

    #[test]
    fn a_device_counts_as_connected_while_its_link_is_up() {
        let mut state = on(vec![device("1", "Buds", Status::Connecting)]);
        assert_eq!(state.subtitle(), None);
        assert!(!state.shows_indicator());
        state.devices[0].status = Status::Disconnecting;
        assert_eq!(state.subtitle().as_deref(), Some("Buds"));
        assert!(state.shows_indicator());
    }

    #[test]
    fn the_icon_follows_the_adapter() {
        let mut state = State::default();
        assert_eq!(state.icon_name(), "bluetooth-disabled-symbolic");
        state.adapter = Some(Adapter::default());
        assert_eq!(state.icon_name(), "bluetooth-disabled-symbolic");
        // Turning on, on, turning off.
        state.adapter = Some(Adapter {
            powered: true,
            changing: true,
        });
        assert_eq!(state.icon_name(), "bluetooth-acquiring-symbolic");
        state.adapter = Some(Adapter {
            powered: true,
            changing: false,
        });
        assert_eq!(state.icon_name(), "bluetooth-active-symbolic");
        state.adapter = Some(Adapter {
            powered: false,
            changing: true,
        });
        assert_eq!(state.icon_name(), "bluetooth-acquiring-symbolic");
    }

    #[test]
    fn the_menu_lists_connectable_devices_or_a_placeholder() {
        let mut state = State::default();
        assert_eq!(
            state.placeholder(),
            Some("Turn on Bluetooth to connect to devices")
        );

        state = on(Vec::new());
        assert_eq!(
            state.placeholder(),
            Some("No available or connected devices")
        );

        // A paired device with nothing to connect to: hidden, but counted.
        state.devices.push(Device {
            connectable: false,
            ..device("1", "Tracker", Status::Connected)
        });
        assert_eq!(state.menu_devices().count(), 0);
        assert_eq!(
            state.placeholder(),
            Some("No available or connected devices")
        );
        assert_eq!(state.subtitle().as_deref(), Some("Tracker"));
        assert!(state.shows_indicator());

        state
            .devices
            .push(device("2", "Buds", Status::Disconnected));
        let listed: Vec<&str> = state
            .menu_devices()
            .map(|device| device.name.as_str())
            .collect();
        assert_eq!(listed, ["Buds"]);
        assert_eq!(state.placeholder(), None);
    }

    #[test]
    fn devices_linger_unseen_while_the_adapter_is_off() {
        let mut state = on(vec![device("1", "Buds", Status::Connected)]);
        state.adapter = Some(Adapter::default());
        assert_eq!(state.menu_devices().count(), 0);
        assert_eq!(state.subtitle(), None);
        assert!(!state.shows_indicator());
        assert_eq!(
            state.placeholder(),
            Some("Turn on Bluetooth to connect to devices")
        );
    }

    #[test]
    fn the_menu_order_is_connected_first_then_by_name() {
        let mut devices = [
            device("1", "mouse", Status::Disconnected),
            device("2", "Zebra", Status::Connecting),
            device("3", "Buds", Status::Disconnected),
            device("4", "Speaker", Status::Connected),
            device("5", "amp", Status::Disconnecting),
            device("6", "Mouse", Status::Disconnected),
            device("7", "Buds 2", Status::Disconnected),
        ];
        devices.sort_by(Device::menu_order);
        let names: Vec<&str> = devices.iter().map(|device| device.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "amp", "Speaker", "Buds", "Buds 2", "mouse", "Mouse", "Zebra"
            ]
        );

        // Two devices of the same name keep a fixed order.
        let mut twins = [
            device("B", "Buds", Status::Disconnected),
            device("A", "Buds", Status::Disconnected),
        ];
        twins.sort_by(Device::menu_order);
        assert_eq!(twins[0].address, "A");
    }

    #[test]
    fn a_click_connects_or_disconnects() {
        let mut buds = device("1", "Buds", Status::Disconnected);
        assert!(!buds.busy());
        assert_eq!(buds.action_label(), "Connect");
        assert_eq!(buds.accessible_name(), "Connect to Buds");
        assert_eq!(buds.activated(), Some(Command::Connect("1".to_owned())));

        buds.status = Status::Connecting;
        assert!(buds.busy());
        assert_eq!(buds.action_label(), "Connect");
        assert_eq!(buds.activated(), None);

        buds.status = Status::Connected;
        assert!(!buds.busy());
        assert_eq!(buds.action_label(), "Disconnect");
        assert_eq!(buds.accessible_name(), "Disconnect Buds");
        assert_eq!(buds.activated(), Some(Command::Disconnect("1".to_owned())));

        buds.status = Status::Disconnecting;
        assert!(buds.busy());
        assert_eq!(buds.action_label(), "Disconnect");
        assert_eq!(buds.activated(), None);
    }

    #[test]
    fn a_device_without_an_icon_gets_the_generic_one() {
        let device = Device::default();
        assert_eq!(device.icon_name(), "bluetooth-symbolic");
        assert!(device.connectable);
    }

    #[test]
    fn the_toggle_flips_the_adapter() {
        let mut state = on(Vec::new());
        assert_eq!(state.toggled(), Command::SetPowered(false));
        state.predict(&state.toggled());
        assert!(!state.powered());
        assert_eq!(state.toggled(), Command::SetPowered(true));
    }

    #[test]
    fn requests_are_predicted() {
        let mut state = on(vec![
            device("1", "Buds", Status::Connected),
            device("2", "Mouse", Status::Disconnected),
        ]);
        state.predict(&Command::Connect("2".to_owned()));
        assert_eq!(state.devices[1].status, Status::Connecting);
        assert_eq!(state.devices[0].status, Status::Connected);
        state.predict(&Command::Disconnect("1".to_owned()));
        assert_eq!(state.devices[0].status, Status::Disconnecting);
        // An address nothing has.
        let before = state.clone();
        state.predict(&Command::Connect("9".to_owned()));
        assert_eq!(state, before);
    }

    #[test]
    fn powering_off_is_predicted() {
        let mut state = on(vec![device("1", "Buds", Status::Connected)]);
        state.predict(&Command::Disconnect("1".to_owned()));
        assert_eq!(state.devices[0].status, Status::Disconnecting);
        state.predict(&Command::SetPowered(false));
        assert_eq!(state.icon_name(), "bluetooth-disabled-symbolic");
        assert_eq!(state.subtitle(), None);
        assert!(!state.shows_indicator());
        // No adapter: nothing to predict.
        let mut none = State::default();
        none.predict(&Command::SetPowered(true));
        assert_eq!(none, State::default());
    }
}
