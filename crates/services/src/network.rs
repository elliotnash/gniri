//! Network connections: what the panel icon shows, Wi-Fi and wired.
//!
//! [`backend`] talks to NetworkManager.

pub mod backend;

use serde::Deserialize;

use crate::model::Model;

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct State {
    /// The connection that carries the default route.
    pub primary: Option<Connection>,
    /// The Wi-Fi adapter, if the machine has one.
    pub wifi: Option<Wifi>,
    /// The wired adapter, if the machine has one.
    pub wired: Option<Wired>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Connection {
    Wired,
    Wifi {
        /// Signal strength, 0 to 100.
        strength: u8,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Wifi {
    /// The radio is on.
    pub enabled: bool,
    /// Networks in range, in the order the menu lists them.
    pub networks: Vec<Network>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Network {
    pub ssid: String,
    /// Signal strength, 0 to 100.
    pub strength: u8,
    /// It asks for a password or other credentials.
    pub secure: bool,
    /// There is a saved connection for it, so no password is needed.
    pub known: bool,
    pub status: Status,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    #[default]
    Idle,
    Connecting,
    Connected,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Wired {
    pub status: Status,
}

/// What the backend reports.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Everything, as it is now.
    State(State),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    SetWifiEnabled(bool),
    /// Connect to the network with this SSID. Only networks that are known
    /// or open can be joined until the password dialog exists (M6).
    ConnectWifi(String),
    DisconnectWifi,
    /// Ask for a fresh list of networks in range.
    ScanWifi,
    SetWiredConnected(bool),
}

/// Port of `signalToIcon` in `status/network.js`.
pub fn signal_icon_name(strength: u8) -> &'static str {
    match strength {
        0..20 => "network-wireless-signal-none-symbolic",
        20..40 => "network-wireless-signal-weak-symbolic",
        40..50 => "network-wireless-signal-ok-symbolic",
        50..80 => "network-wireless-signal-good-symbolic",
        _ => "network-wireless-signal-excellent-symbolic",
    }
}

impl Connection {
    /// The panel icon.
    pub fn icon_name(&self) -> &'static str {
        match self {
            Self::Wired => "network-wired-symbolic",
            Self::Wifi { strength } => signal_icon_name(*strength),
        }
    }
}

impl Network {
    pub fn icon_name(&self) -> &'static str {
        signal_icon_name(self.strength)
    }
}

impl Wifi {
    /// The network in use or being joined.
    pub fn current(&self) -> Option<&Network> {
        self.networks
            .iter()
            .find(|network| network.status != Status::Idle)
    }

    /// The quick toggle's icon.
    pub fn icon_name(&self) -> &'static str {
        if !self.enabled {
            return "network-wireless-disabled-symbolic";
        }
        match self.current() {
            Some(network) if network.status == Status::Connecting => {
                "network-wireless-acquiring-symbolic"
            }
            Some(network) => network.icon_name(),
            None => "network-wireless-signal-none-symbolic",
        }
    }

    /// The quick toggle's second line: the network's name.
    pub fn subtitle(&self) -> Option<&str> {
        self.current().map(|network| network.ssid.as_str())
    }
}

impl Wired {
    pub fn icon_name(&self) -> &'static str {
        match self.status {
            Status::Idle => "network-wired-disconnected-symbolic",
            Status::Connecting => "network-wired-acquiring-symbolic",
            Status::Connected => "network-wired-symbolic",
        }
    }
}

impl State {
    /// The panel icon, or `None` to show no network indicator.
    pub fn panel_icon_name(&self) -> Option<&'static str> {
        self.primary.as_ref().map(Connection::icon_name)
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
        match command {
            Command::SetWifiEnabled(enabled) => {
                if let Some(wifi) = &mut self.wifi {
                    wifi.enabled = *enabled;
                    if !enabled {
                        wifi.networks.clear();
                        if matches!(self.primary, Some(Connection::Wifi { .. })) {
                            self.primary = None;
                        }
                    }
                }
            }
            Command::ConnectWifi(ssid) => {
                if let Some(wifi) = &mut self.wifi {
                    for network in &mut wifi.networks {
                        network.status = if network.ssid == *ssid {
                            Status::Connecting
                        } else {
                            Status::Idle
                        };
                    }
                }
            }
            Command::DisconnectWifi => {
                if let Some(wifi) = &mut self.wifi {
                    for network in &mut wifi.networks {
                        network.status = Status::Idle;
                    }
                }
            }
            Command::ScanWifi => {}
            Command::SetWiredConnected(connected) => {
                if let Some(wired) = &mut self.wired {
                    wired.status = if *connected {
                        Status::Connecting
                    } else {
                        Status::Idle
                    };
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wifi_icons_step_with_strength() {
        let icon = |strength| Connection::Wifi { strength }.icon_name();
        assert_eq!(icon(0), "network-wireless-signal-none-symbolic");
        assert_eq!(icon(19), "network-wireless-signal-none-symbolic");
        assert_eq!(icon(20), "network-wireless-signal-weak-symbolic");
        assert_eq!(icon(45), "network-wireless-signal-ok-symbolic");
        assert_eq!(icon(79), "network-wireless-signal-good-symbolic");
        assert_eq!(icon(80), "network-wireless-signal-excellent-symbolic");
        assert_eq!(icon(100), "network-wireless-signal-excellent-symbolic");
        assert_eq!(Connection::Wired.icon_name(), "network-wired-symbolic");
    }

    fn network(ssid: &str, status: Status) -> Network {
        Network {
            ssid: ssid.to_owned(),
            strength: 70,
            secure: true,
            known: true,
            status,
        }
    }

    #[test]
    fn the_toggle_shows_the_current_network() {
        let mut wifi = Wifi {
            enabled: true,
            networks: vec![
                network("home", Status::Connected),
                network("cafe", Status::Idle),
            ],
        };
        assert_eq!(wifi.subtitle(), Some("home"));
        assert_eq!(wifi.icon_name(), "network-wireless-signal-good-symbolic");
        wifi.networks[0].status = Status::Connecting;
        assert_eq!(wifi.icon_name(), "network-wireless-acquiring-symbolic");
        wifi.enabled = false;
        assert_eq!(wifi.icon_name(), "network-wireless-disabled-symbolic");
    }

    #[test]
    fn turning_wifi_off_is_predicted() {
        let mut state = State {
            primary: Some(Connection::Wifi { strength: 70 }),
            wifi: Some(Wifi {
                enabled: true,
                networks: vec![network("home", Status::Connected)],
            }),
            wired: None,
        };
        state.predict(&Command::ConnectWifi("home".to_owned()));
        assert_eq!(
            state.wifi.as_ref().unwrap().networks[0].status,
            Status::Connecting
        );
        state.predict(&Command::SetWifiEnabled(false));
        assert_eq!(state.primary, None);
        assert_eq!(state.wifi.as_ref().unwrap().subtitle(), None);
        assert_eq!(state.panel_icon_name(), None);
    }
}
