//! Bluetooth: the adapter and the devices paired with it.
//!
//! [`backend`] talks to BlueZ.

pub mod backend;

use serde::Deserialize;

use crate::model::Model;

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct State {
    /// The default adapter, if the machine has one.
    pub adapter: Option<Adapter>,
    /// Paired devices, in the order the menu lists them.
    pub devices: Vec<Device>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Adapter {
    pub powered: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Device {
    /// Hardware address, such as "AA:BB:CC:DD:EE:FF"; what commands take.
    pub address: String,
    /// The name the user knows it by.
    pub name: String,
    /// Icon name from BlueZ, such as "audio-headphones"; may be empty.
    pub icon: String,
    pub status: Status,
    /// Charge in percent, when the device reports it.
    pub battery: Option<u8>,
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
}

impl State {
    pub fn powered(&self) -> bool {
        self.adapter.as_ref().is_some_and(|adapter| adapter.powered)
    }

    fn connected(&self) -> impl Iterator<Item = &Device> {
        self.devices
            .iter()
            .filter(|device| device.status == Status::Connected)
    }

    /// The quick toggle's icon. Port of `BluetoothToggle._sync` in
    /// `status/bluetooth.js`.
    pub fn icon_name(&self) -> &'static str {
        if self.powered() {
            "bluetooth-active-symbolic"
        } else {
            "bluetooth-disabled-symbolic"
        }
    }

    /// The quick toggle's second line: the connected device, or how many.
    pub fn subtitle(&self) -> Option<String> {
        let mut connected = self.connected();
        let first = connected.next()?;
        match connected.count() {
            0 => Some(first.name.clone()),
            others => Some(format!("{} Connected", others + 1)),
        }
    }

    /// GNOME shows the panel icon only while a device is connected.
    pub fn shows_indicator(&self) -> bool {
        self.connected().next().is_some()
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
            battery: None,
        }
    }

    #[test]
    fn the_toggle_names_the_connected_device_or_counts_them() {
        let mut state = State {
            adapter: Some(Adapter { powered: true }),
            devices: vec![
                device("1", "Buds", Status::Connected),
                device("2", "Mouse", Status::Disconnected),
            ],
        };
        assert_eq!(state.icon_name(), "bluetooth-active-symbolic");
        assert_eq!(state.subtitle().as_deref(), Some("Buds"));
        assert!(state.shows_indicator());
        state.devices[1].status = Status::Connected;
        assert_eq!(state.subtitle().as_deref(), Some("2 Connected"));
        assert_eq!(state.devices[0].icon_name(), "audio-headphones-symbolic");
    }

    #[test]
    fn powering_off_is_predicted() {
        let mut state = State {
            adapter: Some(Adapter { powered: true }),
            devices: vec![device("1", "Buds", Status::Connected)],
        };
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
