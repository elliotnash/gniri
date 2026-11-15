//! Network connections: what the panel icon shows, Wi-Fi and wired.
//!
//! The state and what widgets derive from it, ported from GNOME Shell's
//! `js/ui/status/network.js`. [`backend`] fills it in from NetworkManager.

pub mod backend;
mod security;
mod system;

use std::cmp::Ordering;
use std::time::Duration;

use serde::Deserialize;

use crate::model::Model;

/// How many networks the Wi-Fi menu lists (`MAX_VISIBLE_NETWORKS`). The
/// state keeps them all; see [`Wifi::visible_networks`].
pub const MAX_VISIBLE_NETWORKS: usize = 8;

/// How often GNOME asks for a scan while the Wi-Fi menu is open
/// (`WIFI_SCAN_FREQUENCY`); it also asks once as the menu opens.
pub const WIFI_SCAN_INTERVAL: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct State {
    /// The connection the panel icon stands for: the one that carries the
    /// default route or, without one, the one being set up.
    pub primary: Option<Connection>,
    /// The Wi-Fi adapter, if the machine has one that NetworkManager runs.
    pub wifi: Option<Wifi>,
    /// The wired adapter, while a cable is plugged in: GNOME hides the
    /// toggle otherwise.
    pub wired: Option<Wired>,
}

/// A connection as the panel shows it.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Connection {
    Wired {
        #[serde(default = "connected")]
        status: Status,
        /// It is up, but the internet cannot be reached through it.
        #[serde(default)]
        limited: bool,
    },
    Wifi {
        /// Signal strength of the access point in use, 0 to 100.
        strength: u8,
        #[serde(default = "connected")]
        status: Status,
        /// It is up, but the internet cannot be reached through it.
        #[serde(default)]
        limited: bool,
        /// The adapter is sharing a connection instead of using one.
        #[serde(default)]
        hotspot: bool,
    },
    /// Anything GNOME has no toggle for: a VPN, a bridge, a tether.
    Other {
        #[serde(default)]
        limited: bool,
    },
}

/// A scene that names a primary connection means one that is up.
fn connected() -> Status {
    Status::Connected
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Wifi {
    /// The radio is on.
    pub enabled: bool,
    /// No hardware switch holds the radio off. GNOME makes the toggle
    /// insensitive otherwise.
    pub hardware_enabled: bool,
    /// Networks in range, in the order the menu lists them, one per SSID.
    pub networks: Vec<Network>,
    /// A scan the shell asked for is under way (the menu's spinner).
    pub scanning: bool,
    /// The adapter is sharing a connection as a hotspot.
    pub hotspot: bool,
    /// The network in use is up, but the internet cannot be reached
    /// through it.
    pub limited: bool,
    /// Signal strength of the access point in use. It can be lower than
    /// its network's, which is that of the strongest access point with the
    /// same name. `None` falls back on the network's.
    pub active_strength: Option<u8>,
}

impl Default for Wifi {
    fn default() -> Self {
        Self {
            enabled: false,
            hardware_enabled: true,
            networks: Vec::new(),
            scanning: false,
            hotspot: false,
            limited: false,
            active_strength: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Network {
    pub ssid: String,
    /// Signal strength of its strongest access point, 0 to 100.
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
    /// It is up, but the internet cannot be reached through it.
    pub limited: bool,
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
    /// Take the Wi-Fi adapter off its network, or stop its hotspot.
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

/// Port of `NMWirelessDeviceItem.icon_name` for a radio that is on. GNOME
/// has one more icon, `network-wireless-offline-symbolic`, for a connection
/// that is up without an access point; it calls that state unexpected and
/// it is not told apart here.
fn wifi_icon_name(status: Status, hotspot: bool, limited: bool, strength: u8) -> &'static str {
    match status {
        Status::Connecting => "network-wireless-acquiring-symbolic",
        Status::Connected if hotspot => "network-wireless-hotspot-symbolic",
        Status::Connected if limited => "network-wireless-no-route-symbolic",
        Status::Connected => signal_icon_name(strength),
        Status::Idle => "network-wireless-signal-none-symbolic",
    }
}

/// Port of `NMWiredDeviceItem.icon_name`.
fn wired_icon_name(status: Status, limited: bool) -> &'static str {
    match status {
        Status::Connecting => "network-wired-acquiring-symbolic",
        Status::Connected if limited => "network-wired-no-route-symbolic",
        Status::Connected => "network-wired-symbolic",
        Status::Idle => "network-wired-disconnected-symbolic",
    }
}

impl Connection {
    /// The panel icon. GNOME binds it to the icon of the toggle the
    /// connection belongs to; the icons for a connection without a toggle
    /// are from `Indicator._updateIcon`.
    pub fn icon_name(&self) -> &'static str {
        match *self {
            Self::Wired { status, limited } => wired_icon_name(status, limited),
            Self::Wifi {
                strength,
                status,
                limited,
                hotspot,
            } => wifi_icon_name(status, hotspot, limited, strength),
            Self::Other { limited: false } => "network-wired-symbolic",
            Self::Other { limited: true } => "network-wired-no-route-symbolic",
        }
    }
}

impl Network {
    /// Port of `WirelessNetwork.icon_name`.
    pub fn icon_name(&self) -> &'static str {
        signal_icon_name(self.strength)
    }

    /// The menu marks it with a check: it is in use or being joined. Port
    /// of `WirelessNetwork.is_active`.
    pub fn is_active(&self) -> bool {
        self.status != Status::Idle
    }

    /// [`Command::ConnectWifi`] can join it without asking for anything.
    pub fn can_connect(&self) -> bool {
        self.known || !self.secure
    }

    /// Port of `WirelessNetwork.compare`: known networks first, then the
    /// stronger, then the secure, then by name.
    pub fn menu_order(&self, other: &Self) -> Ordering {
        other
            .known
            .cmp(&self.known)
            .then(other.strength.cmp(&self.strength))
            .then(other.secure.cmp(&self.secure))
            .then_with(|| collate(&self.ssid, &other.ssid))
    }
}

/// Stands in for `g_utf8_collate`, which sorts by the locale's rules: like
/// those, letters come before their case does.
fn collate(one: &str, two: &str) -> Ordering {
    let fold = |text: &str| {
        text.chars()
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    fold(one).cmp(&fold(two)).then_with(|| one.cmp(two))
}

/// Puts rows in the order the Wi-Fi menu lists them and keeps one per
/// SSID. `network` is the network a row stands for.
///
/// GNOME lists the same name twice when access points with one SSID differ
/// in security. Commands tell networks apart by SSID alone, so only the row
/// in use, or else the one GNOME lists first, is kept.
pub fn arrange<T>(rows: &mut Vec<T>, network: impl Fn(&T) -> &Network) {
    rows.sort_by(|one, two| network(one).menu_order(network(two)));
    let mut kept: Vec<T> = Vec::with_capacity(rows.len());
    for row in rows.drain(..) {
        let twin = kept
            .iter_mut()
            .find(|other| network(other).ssid == network(&row).ssid);
        match twin {
            Some(other) if network(&row).is_active() && !network(other).is_active() => {
                *other = row;
            }
            Some(_) => {}
            None => kept.push(row),
        }
    }
    // A row that took its twin's place may be out of order there.
    kept.sort_by(|one, two| network(one).menu_order(network(two)));
    *rows = kept;
}

impl Wifi {
    /// The network in use or being joined.
    pub fn current(&self) -> Option<&Network> {
        self.networks.iter().find(|network| network.is_active())
    }

    /// The networks the menu shows: GNOME stops at
    /// [`MAX_VISIBLE_NETWORKS`] and leaves the rest to Settings.
    pub fn visible_networks(&self) -> &[Network] {
        &self.networks[..self.networks.len().min(MAX_VISIBLE_NETWORKS)]
    }

    /// The quick toggle's icon. Port of `NMWirelessDeviceItem.icon_name`.
    pub fn icon_name(&self) -> &'static str {
        if !self.enabled {
            return "network-wireless-disabled-symbolic";
        }
        match self.current() {
            Some(network) => wifi_icon_name(
                network.status,
                self.hotspot,
                self.limited,
                self.active_strength.unwrap_or(network.strength),
            ),
            None if self.hotspot => "network-wireless-hotspot-symbolic",
            None => "network-wireless-signal-none-symbolic",
        }
    }

    /// The quick toggle's second line: the network's name. Port of
    /// `NMWirelessDeviceItem.name`; GNOME drops it when it would repeat the
    /// title, which is what is left without a network.
    pub fn subtitle(&self) -> Option<&str> {
        if self.hotspot {
            // "%s Hotspot" with the adapter's name, which is the title.
            return Some("Wi-Fi Hotspot");
        }
        self.current().map(|network| network.ssid.as_str())
    }

    /// Forgets the connection, as when it is dropped.
    fn disconnect(&mut self) {
        for network in &mut self.networks {
            network.status = Status::Idle;
        }
        self.hotspot = false;
        self.limited = false;
        self.active_strength = None;
    }
}

impl Wired {
    /// The quick toggle's icon. Port of `NMWiredDeviceItem.icon_name`. The
    /// toggle has no second line: GNOME would show the adapter's name,
    /// which with one adapter repeats the title ("Wired").
    pub fn icon_name(&self) -> &'static str {
        wired_icon_name(self.status, self.limited)
    }

    /// The toggle is checked. Port of `NMMenuItem.is_active`.
    pub fn is_active(&self) -> bool {
        self.status != Status::Idle
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
                let Some(wifi) = &mut self.wifi else { return };
                wifi.enabled = *enabled;
                if !enabled {
                    // The adapter forgets what it saw.
                    wifi.networks.clear();
                    wifi.scanning = false;
                    wifi.disconnect();
                    if matches!(self.primary, Some(Connection::Wifi { .. })) {
                        self.primary = None;
                    }
                }
            }
            Command::ConnectWifi(ssid) => {
                let Some(wifi) = &mut self.wifi else { return };
                let joinable = wifi
                    .networks
                    .iter()
                    .find(|network| network.ssid == *ssid)
                    .filter(|network| network.can_connect());
                let Some(strength) = joinable.map(|network| network.strength) else {
                    return;
                };
                wifi.disconnect();
                for network in &mut wifi.networks {
                    if network.ssid == *ssid {
                        network.status = Status::Connecting;
                    }
                }
                // The panel follows a connection that is being set up
                // unless another one carries the default route.
                if matches!(self.primary, None | Some(Connection::Wifi { .. })) {
                    self.primary = Some(Connection::Wifi {
                        strength,
                        status: Status::Connecting,
                        limited: false,
                        hotspot: false,
                    });
                }
            }
            Command::DisconnectWifi => {
                let Some(wifi) = &mut self.wifi else { return };
                wifi.disconnect();
                if matches!(self.primary, Some(Connection::Wifi { .. })) {
                    self.primary = None;
                }
            }
            Command::ScanWifi => {
                if let Some(wifi) = &mut self.wifi {
                    wifi.scanning = wifi.enabled;
                }
            }
            Command::SetWiredConnected(connected) => {
                let Some(wired) = &mut self.wired else { return };
                if *connected == wired.is_active() {
                    return;
                }
                wired.limited = false;
                if *connected {
                    wired.status = Status::Connecting;
                    if self.primary.is_none() {
                        self.primary = Some(Connection::Wired {
                            status: Status::Connecting,
                            limited: false,
                        });
                    }
                } else {
                    wired.status = Status::Idle;
                    if matches!(self.primary, Some(Connection::Wired { .. })) {
                        self.primary = None;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wifi_connection(strength: u8) -> Connection {
        Connection::Wifi {
            strength,
            status: Status::Connected,
            limited: false,
            hotspot: false,
        }
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

    fn wifi(networks: Vec<Network>) -> Wifi {
        Wifi {
            enabled: true,
            networks,
            ..Wifi::default()
        }
    }

    #[test]
    fn wifi_icons_step_with_strength() {
        let icon = |strength| wifi_connection(strength).icon_name();
        assert_eq!(icon(0), "network-wireless-signal-none-symbolic");
        assert_eq!(icon(19), "network-wireless-signal-none-symbolic");
        assert_eq!(icon(20), "network-wireless-signal-weak-symbolic");
        assert_eq!(icon(39), "network-wireless-signal-weak-symbolic");
        assert_eq!(icon(40), "network-wireless-signal-ok-symbolic");
        assert_eq!(icon(49), "network-wireless-signal-ok-symbolic");
        assert_eq!(icon(50), "network-wireless-signal-good-symbolic");
        assert_eq!(icon(79), "network-wireless-signal-good-symbolic");
        assert_eq!(icon(80), "network-wireless-signal-excellent-symbolic");
        assert_eq!(icon(100), "network-wireless-signal-excellent-symbolic");
    }

    #[test]
    fn the_panel_shows_how_far_the_connection_has_got() {
        let wifi = |status, limited, hotspot| {
            Connection::Wifi {
                strength: 70,
                status,
                limited,
                hotspot,
            }
            .icon_name()
        };
        assert_eq!(
            wifi(Status::Connecting, false, false),
            "network-wireless-acquiring-symbolic"
        );
        assert_eq!(
            wifi(Status::Connected, false, false),
            "network-wireless-signal-good-symbolic"
        );
        assert_eq!(
            wifi(Status::Connected, true, false),
            "network-wireless-no-route-symbolic"
        );
        // A hotspot has no internet of its own to be cut off from.
        assert_eq!(
            wifi(Status::Connected, true, true),
            "network-wireless-hotspot-symbolic"
        );
        assert_eq!(
            wifi(Status::Connecting, false, true),
            "network-wireless-acquiring-symbolic"
        );
        assert_eq!(
            wifi(Status::Idle, false, false),
            "network-wireless-signal-none-symbolic"
        );

        let wired = |status, limited| Connection::Wired { status, limited }.icon_name();
        assert_eq!(
            wired(Status::Connecting, false),
            "network-wired-acquiring-symbolic"
        );
        assert_eq!(wired(Status::Connected, false), "network-wired-symbolic");
        assert_eq!(
            wired(Status::Connected, true),
            "network-wired-no-route-symbolic"
        );
        assert_eq!(
            wired(Status::Idle, false),
            "network-wired-disconnected-symbolic"
        );

        assert_eq!(
            Connection::Other { limited: false }.icon_name(),
            "network-wired-symbolic"
        );
        assert_eq!(
            Connection::Other { limited: true }.icon_name(),
            "network-wired-no-route-symbolic"
        );
    }

    #[test]
    fn scenes_name_a_connection_that_is_up() {
        #[derive(Deserialize)]
        struct Scene {
            network: State,
        }
        let state = |text: &str| toml::from_str::<Scene>(text).unwrap().network;

        let scene = state("network = { primary = { kind = \"wifi\", strength = 70 } }");
        assert_eq!(scene.primary, Some(wifi_connection(70)));
        let scene = state("network = { primary = { kind = \"wired\" } }");
        assert_eq!(scene.panel_icon_name(), Some("network-wired-symbolic"));
        let scene = state(
            "network = { primary = { kind = \"wired\", status = \"connecting\" }, \
             wifi = { enabled = true, networks = [{ ssid = \"home\", status = \"connected\" }] } }",
        );
        assert_eq!(
            scene.panel_icon_name(),
            Some("network-wired-acquiring-symbolic")
        );
        let wifi = scene.wifi.unwrap();
        assert!(wifi.hardware_enabled);
        assert_eq!(wifi.subtitle(), Some("home"));
        let scene = state("network = { primary = { kind = \"other\" } }");
        assert_eq!(scene.primary, Some(Connection::Other { limited: false }));
    }

    #[test]
    fn the_toggle_shows_the_current_network() {
        let mut wifi = wifi(vec![
            network("home", Status::Connected),
            network("cafe", Status::Idle),
        ]);
        assert_eq!(wifi.subtitle(), Some("home"));
        assert_eq!(wifi.icon_name(), "network-wireless-signal-good-symbolic");

        // The access point in use is not the strongest of its network.
        wifi.active_strength = Some(30);
        assert_eq!(wifi.icon_name(), "network-wireless-signal-weak-symbolic");
        assert_eq!(
            wifi.networks[0].icon_name(),
            "network-wireless-signal-good-symbolic"
        );

        wifi.limited = true;
        assert_eq!(wifi.icon_name(), "network-wireless-no-route-symbolic");
        wifi.networks[0].status = Status::Connecting;
        assert_eq!(wifi.icon_name(), "network-wireless-acquiring-symbolic");
        wifi.enabled = false;
        assert_eq!(wifi.icon_name(), "network-wireless-disabled-symbolic");
    }

    #[test]
    fn the_toggle_without_a_network() {
        let mut wifi = wifi(vec![network("cafe", Status::Idle)]);
        assert_eq!(wifi.subtitle(), None);
        assert_eq!(wifi.icon_name(), "network-wireless-signal-none-symbolic");
        wifi.hotspot = true;
        assert_eq!(wifi.subtitle(), Some("Wi-Fi Hotspot"));
        assert_eq!(wifi.icon_name(), "network-wireless-hotspot-symbolic");
    }

    #[test]
    fn the_wired_toggle() {
        let icon = |status, limited| Wired { status, limited }.icon_name();
        assert_eq!(
            icon(Status::Idle, false),
            "network-wired-disconnected-symbolic"
        );
        assert_eq!(
            icon(Status::Connecting, false),
            "network-wired-acquiring-symbolic"
        );
        assert_eq!(icon(Status::Connected, false), "network-wired-symbolic");
        assert_eq!(
            icon(Status::Connected, true),
            "network-wired-no-route-symbolic"
        );
        assert!(!Wired::default().is_active());
    }

    fn row(ssid: &str, strength: u8, secure: bool, known: bool) -> Network {
        Network {
            ssid: ssid.to_owned(),
            strength,
            secure,
            known,
            status: Status::Idle,
        }
    }

    fn names(networks: &[Network]) -> Vec<&str> {
        networks
            .iter()
            .map(|network| network.ssid.as_str())
            .collect()
    }

    #[test]
    fn the_menu_lists_known_then_strong_then_secure_then_by_name() {
        let mut networks = vec![
            row("open", 60, false, false),
            row("zebra", 60, true, false),
            row("Alpha", 60, true, false),
            row("beta", 60, true, false),
            row("strong", 90, false, false),
            row("known-weak", 10, true, true),
            row("known", 40, true, true),
        ];
        arrange(&mut networks, |network| network);
        assert_eq!(
            names(&networks),
            [
                "known",
                "known-weak",
                "strong",
                "Alpha",
                "beta",
                "zebra",
                "open"
            ]
        );
    }

    #[test]
    fn the_network_in_use_is_not_moved_to_the_top() {
        let mut networks = vec![
            Network {
                status: Status::Connected,
                ..row("in-use", 40, true, true)
            },
            row("nearer", 80, true, true),
        ];
        arrange(&mut networks, |network| network);
        assert_eq!(names(&networks), ["nearer", "in-use"]);
    }

    #[test]
    fn one_row_per_ssid() {
        let mut networks = vec![
            row("twin", 50, false, false),
            row("twin", 40, true, false),
            row("other", 45, true, false),
        ];
        arrange(&mut networks, |network| network);
        assert_eq!(names(&networks), ["twin", "other"]);
        assert!(!networks[0].secure);

        // The one in use wins over the one listed first, and is listed
        // where it belongs.
        let mut networks = vec![
            row("twin", 50, false, false),
            Network {
                status: Status::Connected,
                ..row("twin", 40, true, false)
            },
            row("other", 45, true, false),
        ];
        arrange(&mut networks, |network| network);
        assert_eq!(names(&networks), ["other", "twin"]);
        assert!(networks[1].secure);
    }

    #[test]
    fn the_menu_shows_the_first_eight() {
        let networks: Vec<Network> = (0..12)
            .map(|index| row(&format!("network {index}"), 50, true, false))
            .collect();
        let mut wifi = wifi(networks);
        assert_eq!(wifi.visible_networks().len(), MAX_VISIBLE_NETWORKS);
        assert_eq!(wifi.networks.len(), 12);
        wifi.networks.truncate(3);
        assert_eq!(wifi.visible_networks().len(), 3);
    }

    fn connected_state() -> State {
        State {
            primary: Some(wifi_connection(70)),
            wifi: Some(wifi(vec![
                network("home", Status::Connected),
                network("cafe", Status::Idle),
                Network {
                    known: false,
                    ..network("locked", Status::Idle)
                },
                Network {
                    known: false,
                    secure: false,
                    ..network("open", Status::Idle)
                },
            ])),
            wired: None,
        }
    }

    #[test]
    fn joining_a_network_is_predicted() {
        let mut state = connected_state();
        state.predict(&Command::ConnectWifi("cafe".to_owned()));
        let wifi = state.wifi.as_ref().unwrap();
        assert_eq!(wifi.subtitle(), Some("cafe"));
        assert_eq!(wifi.icon_name(), "network-wireless-acquiring-symbolic");
        assert_eq!(wifi.networks[0].status, Status::Idle);
        assert_eq!(
            state.panel_icon_name(),
            Some("network-wireless-acquiring-symbolic")
        );

        state.predict(&Command::ConnectWifi("open".to_owned()));
        assert_eq!(state.wifi.as_ref().unwrap().subtitle(), Some("open"));
    }

    #[test]
    fn a_network_that_needs_a_password_is_not_predicted_to_connect() {
        let mut state = connected_state();
        state.predict(&Command::ConnectWifi("locked".to_owned()));
        assert_eq!(state, connected_state());
        state.predict(&Command::ConnectWifi("out of range".to_owned()));
        assert_eq!(state, connected_state());
    }

    #[test]
    fn the_panel_stays_on_the_wire_while_wifi_changes() {
        let mut state = connected_state();
        state.primary = Some(Connection::Wired {
            status: Status::Connected,
            limited: false,
        });
        state.predict(&Command::ConnectWifi("cafe".to_owned()));
        assert_eq!(state.panel_icon_name(), Some("network-wired-symbolic"));
        state.predict(&Command::DisconnectWifi);
        assert_eq!(state.panel_icon_name(), Some("network-wired-symbolic"));
        assert_eq!(state.wifi.as_ref().unwrap().current(), None);
    }

    #[test]
    fn leaving_and_turning_wifi_off_are_predicted() {
        let mut state = connected_state();
        state.predict(&Command::DisconnectWifi);
        assert_eq!(state.primary, None);
        let wifi = state.wifi.as_ref().unwrap();
        assert_eq!(wifi.subtitle(), None);
        assert_eq!(wifi.networks.len(), 4);

        let mut state = connected_state();
        state.predict(&Command::ScanWifi);
        assert!(state.wifi.as_ref().unwrap().scanning);
        state.predict(&Command::SetWifiEnabled(false));
        assert_eq!(state.primary, None);
        let wifi = state.wifi.as_ref().unwrap();
        assert_eq!(wifi.subtitle(), None);
        assert_eq!(wifi.icon_name(), "network-wireless-disabled-symbolic");
        assert!(wifi.networks.is_empty());
        assert!(!wifi.scanning);
        assert_eq!(state.panel_icon_name(), None);

        // Nothing is scanned for while the radio is off.
        state.predict(&Command::ScanWifi);
        assert!(!state.wifi.as_ref().unwrap().scanning);
        state.predict(&Command::SetWifiEnabled(true));
        assert!(state.wifi.as_ref().unwrap().enabled);
    }

    #[test]
    fn the_wire_is_predicted() {
        let mut state = State {
            wired: Some(Wired::default()),
            ..State::default()
        };
        state.predict(&Command::SetWiredConnected(true));
        assert_eq!(state.wired.as_ref().unwrap().status, Status::Connecting);
        assert_eq!(
            state.panel_icon_name(),
            Some("network-wired-acquiring-symbolic")
        );

        state.wired = Some(Wired {
            status: Status::Connected,
            limited: true,
        });
        // Already so: nothing changes.
        let before = state.clone();
        state.predict(&Command::SetWiredConnected(true));
        assert_eq!(state, before);

        state.predict(&Command::SetWiredConnected(false));
        assert_eq!(state.wired, Some(Wired::default()));
        assert_eq!(state.primary, None);
    }

    #[test]
    fn nothing_is_predicted_without_adapters() {
        let mut state = State::default();
        for command in [
            Command::SetWifiEnabled(true),
            Command::ConnectWifi("home".to_owned()),
            Command::DisconnectWifi,
            Command::ScanWifi,
            Command::SetWiredConnected(true),
        ] {
            state.predict(&command);
        }
        assert_eq!(state, State::default());
    }
}
