//! The network backend against python-dbusmock's NetworkManager.
//!
//! The template leaves out some of what the real service has: the
//! `PrimaryConnection`, `ActivatingConnection`, `ActiveAccessPoint` and
//! `LastScan` properties and the device's `Disconnect` method. The tests add
//! them to the mock where they need them.

mod common;

use common::{Follower, MockBus};
use gniri_services::Model;
use gniri_services::network::{self, Command, Connection, Status};
use zbus::{ObjectPath, OwnedVariant, Variant};

const NM: &str = "org.freedesktop.NetworkManager";
/// The mock's control object, which is also NetworkManager's object manager.
const MAIN: &str = "/org/freedesktop";
const MANAGER: &str = "/org/freedesktop/NetworkManager";
const SETTINGS: &str = "/org/freedesktop/NetworkManager/Settings";
const DEVICE: &str = "org.freedesktop.NetworkManager.Device";
const WIRELESS: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const ACCESS_POINT: &str = "org.freedesktop.NetworkManager.AccessPoint";
const ACTIVE: &str = "org.freedesktop.NetworkManager.Connection.Active";
const MOCK: &str = "org.freedesktop.DBus.Mock";

// `NMDeviceState`.
const UNAVAILABLE: i32 = 20;
const DISCONNECTED: i32 = 30;

// `NMActiveConnectionState`.
const ACTIVATING: u32 = 1;
const ACTIVATED: u32 = 2;

/// `NM_CONNECTIVITY_LIMITED` and `_FULL`.
const LIMITED: u32 = 3;
const FULL: u32 = 4;

/// WPA2 with a password: CCMP ciphers and PSK key management.
const WPA2: u32 = 0x188;
/// WPA2 with a user's own credentials: 802.1X key management.
const WPA2_ENTERPRISE: u32 = 0x288;

fn path(path: &str) -> Variant<'_> {
    Variant::from(ObjectPath::try_from(path).expect("a valid object path"))
}

/// `NM_STATE_DISCONNECTED`. The template starts out connected to the
/// internet with no connection to show for it.
const OFFLINE: &str = r#"{"State": 20}"#;
const CONNECTED_GLOBAL: u32 = 70;

fn start() -> Option<MockBus> {
    let mut bus = MockBus::start()?;
    bus.template("networkmanager", NM, OFFLINE);
    for property in ["PrimaryConnection", "ActivatingConnection"] {
        bus.call(NM, MANAGER, MOCK, "AddProperty", &(NM, property, path("/")));
    }
    Some(bus)
}

fn returned_path(reply: &zbus::Message) -> String {
    reply.body().deserialize().expect("the mock returns a path")
}

/// A Wi-Fi adapter with the properties the template forgets.
fn wifi_device(bus: &MockBus, state: i32) -> String {
    let reply = bus.call(NM, MAIN, MOCK, "AddWiFiDevice", &("wifi", "wlan0", state));
    let device = returned_path(&reply);
    bus.call(
        NM,
        &device,
        MOCK,
        "AddProperty",
        &(WIRELESS, "ActiveAccessPoint", path("/")),
    );
    bus.call(
        NM,
        &device,
        MOCK,
        "AddProperty",
        &(WIRELESS, "LastScan", Variant::from(1000i64)),
    );
    disconnectable(bus, &device);
    device
}

fn wired_device(bus: &MockBus, state: i32) -> String {
    let reply = bus.call(
        NM,
        MAIN,
        MOCK,
        "AddEthernetDevice",
        &("wired", "eth0", state),
    );
    let device = returned_path(&reply);
    disconnectable(bus, &device);
    device
}

/// Gives a device the `Disconnect` method, doing what the template's
/// `DeactivateConnection` does.
fn disconnectable(bus: &MockBus, device: &str) {
    let code = "objects['/org/freedesktop'].RemoveActiveConnection(\
                self.path, self.Get('org.freedesktop.NetworkManager.Device', 'ActiveConnection'))";
    bus.call(
        NM,
        device,
        MOCK,
        "AddMethod",
        &(DEVICE, "Disconnect", "", "", code),
    );
}

fn access_point(
    bus: &MockBus,
    device: &str,
    name: &str,
    ssid: &str,
    strength: u8,
    security: u32,
) -> String {
    let address = format!("00:11:22:33:44:{strength:02X}");
    let reply = bus.call(
        NM,
        MAIN,
        MOCK,
        "AddAccessPoint",
        // Mode (infrastructure), frequency and bit rate come before the
        // strength.
        &(
            device,
            name,
            ssid,
            address.as_str(),
            2u32,
            2437u32,
            54000u32,
            strength,
            security,
        ),
    );
    let access_point = returned_path(&reply);
    // The template copies the security flags into the WPA flags as well,
    // and can only make a profile for an access point without any there.
    bus.set_property(NM, &access_point, ACCESS_POINT, "WpaFlags", 0u32);
    access_point
}

/// A saved connection for an access point that is in range.
fn profile(bus: &MockBus, device: &str, name: &str, ssid: &str) -> String {
    let reply = bus.call(
        NM,
        MAIN,
        MOCK,
        "AddWiFiConnection",
        &(device, name, ssid, ""),
    );
    returned_path(&reply)
}

/// The calls of `method` the mock object at `path` has logged.
fn calls(bus: &MockBus, path: &str, method: &str) -> Vec<Vec<OwnedVariant>> {
    let reply = bus.call(NM, path, MOCK, "GetMethodCalls", &(method,));
    let calls: Vec<(u64, Vec<OwnedVariant>)> = reply.body().deserialize().expect("a list of calls");
    calls.into_iter().map(|(_, arguments)| arguments).collect()
}

fn text(value: &OwnedVariant) -> String {
    match &**value {
        Variant::Str(text) => text.as_str().to_owned(),
        Variant::ObjectPath(path) => path.as_str().to_owned(),
        other => panic!("not text: {other:?}"),
    }
}

fn follow(bus: &MockBus) -> Follower<network::State> {
    Follower::new(bus.run(network::backend::run))
}

fn ssids(state: &network::State) -> Vec<&str> {
    state
        .wifi
        .iter()
        .flat_map(|wifi| &wifi.networks)
        .map(|network| network.ssid.as_str())
        .collect()
}

/// A known network with two access points, an open one, and one that hides
/// its name.
fn neighbourhood(bus: &MockBus) -> String {
    let device = wifi_device(bus, DISCONNECTED);
    access_point(bus, &device, "home_far", "home", 45, WPA2);
    access_point(bus, &device, "cafe", "cafe", 80, 0);
    access_point(bus, &device, "home_near", "home", 62, WPA2);
    access_point(bus, &device, "hidden", "", 90, WPA2);
    profile(bus, &device, "home_profile", "home");
    device
}

#[test]
fn the_networks_in_range_are_listed_as_the_menu_has_them() {
    let Some(bus) = start() else { return };
    let device = neighbourhood(&bus);
    let mut network = follow(&bus);

    network.wait("the networks", |state| ssids(state).len() == 2);
    let wifi = network.state.wifi.clone().unwrap();
    assert!(wifi.enabled);
    assert!(wifi.hardware_enabled);
    assert!(!wifi.scanning);
    // The known network first although the open one is stronger; its two
    // access points are one row with the strength of the nearer.
    assert_eq!(ssids(&network.state), ["home", "cafe"]);
    let home = &wifi.networks[0];
    assert_eq!((home.strength, home.secure, home.known), (62, true, true));
    assert_eq!(home.status, Status::Idle);
    let cafe = &wifi.networks[1];
    assert_eq!((cafe.strength, cafe.secure, cafe.known), (80, false, false));
    assert_eq!(wifi.icon_name(), "network-wireless-signal-none-symbolic");
    assert_eq!(wifi.subtitle(), None);
    assert_eq!(network.state.primary, None);
    assert_eq!(network.state.panel_icon_name(), None);
    assert_eq!(network.state.wired, None);

    // An access point appearing.
    let library = access_point(&bus, &device, "library", "library", 70, WPA2);
    network.wait("a third network", |state| ssids(state).len() == 3);
    assert_eq!(ssids(&network.state), ["home", "cafe", "library"]);
    assert!(network.state.wifi.as_ref().unwrap().networks[2].secure);

    // Its signal getting stronger than the open network's.
    bus.set_property(NM, &library, ACCESS_POINT, "Strength", 85u8);
    network.wait("the library first of the unknown", |state| {
        ssids(state) == ["home", "library", "cafe"]
    });

    // And going out of range.
    bus.call(
        NM,
        MAIN,
        MOCK,
        "RemoveAccessPoint",
        &(device.as_str(), library.as_str()),
    );
    network.wait("two networks again", |state| {
        ssids(state) == ["home", "cafe"]
    });
}

#[test]
fn wifi_is_switched_by_command_and_by_others() {
    let Some(bus) = start() else { return };
    neighbourhood(&bus);
    let mut network = follow(&bus);
    network.wait("Wi-Fi on", |state| {
        state.wifi.as_ref().is_some_and(|wifi| wifi.enabled)
    });

    network.send(Command::SetWifiEnabled(false));
    network.wait("Wi-Fi off", |state| {
        state.wifi.as_ref().is_some_and(|wifi| !wifi.enabled)
    });
    let enabled: bool = bus.property(NM, MANAGER, NM, "WirelessEnabled");
    assert!(!enabled);
    assert_eq!(
        network.state.wifi.as_ref().unwrap().icon_name(),
        "network-wireless-disabled-symbolic"
    );

    network.send(Command::SetWifiEnabled(true));
    network.wait("Wi-Fi on again", |state| {
        state.wifi.as_ref().is_some_and(|wifi| wifi.enabled)
    });
    let enabled: bool = bus.property(NM, MANAGER, NM, "WirelessEnabled");
    assert!(enabled);

    // Someone else switching it off, then the hardware switch.
    bus.set_property(NM, MANAGER, NM, "WirelessEnabled", false);
    network.wait("Wi-Fi off", |state| {
        state.wifi.as_ref().is_some_and(|wifi| !wifi.enabled)
    });
    bus.set_property(NM, MANAGER, NM, "WirelessHardwareEnabled", false);
    network.wait("the switch off", |state| {
        state
            .wifi
            .as_ref()
            .is_some_and(|wifi| !wifi.hardware_enabled)
    });
}

#[test]
fn a_known_network_is_joined_and_left_by_command() {
    let Some(bus) = start() else { return };
    let device = neighbourhood(&bus);
    let mut network = follow(&bus);
    network.wait("the networks", |state| ssids(state).len() == 2);

    network.send(Command::ConnectWifi("home".to_owned()));
    network.wait("home connected", |state| {
        state
            .wifi
            .as_ref()
            .and_then(|wifi| wifi.current())
            .is_some_and(|current| current.ssid == "home" && current.status == Status::Connected)
    });
    // The saved profile was activated on the adapter, with no access point
    // named.
    let activations = calls(&bus, MANAGER, "ActivateConnection");
    assert_eq!(activations.len(), 1);
    let arguments: Vec<String> = activations[0].iter().map(text).collect();
    assert_eq!(
        arguments,
        [
            "/org/freedesktop/NetworkManager/Settings/home_profile",
            device.as_str(),
            "/"
        ]
    );
    let wifi = network.state.wifi.as_ref().unwrap();
    assert_eq!(wifi.subtitle(), Some("home"));
    assert_eq!(wifi.icon_name(), "network-wireless-signal-good-symbolic");
    // Nothing carries the default route yet as far as the mock says.
    assert_eq!(network.state.primary, None);

    // The adapter settles on the farther access point.
    bus.set_property(
        NM,
        &device,
        WIRELESS,
        "ActiveAccessPoint",
        path("/org/freedesktop/NetworkManager/AccessPoint/home_far"),
    );
    network.wait("the strength in use", |state| {
        state.wifi.as_ref().unwrap().active_strength == Some(45)
    });
    let wifi = network.state.wifi.as_ref().unwrap();
    assert_eq!(wifi.icon_name(), "network-wireless-signal-ok-symbolic");
    assert_eq!(wifi.networks[0].strength, 62);

    network.send(Command::DisconnectWifi);
    network.wait("home left", |state| {
        state.wifi.as_ref().unwrap().current().is_none()
    });
    assert_eq!(calls(&bus, &device, "Disconnect").len(), 1);
}

#[test]
fn a_network_is_joined_unless_it_needs_setting_up() {
    let Some(bus) = start() else { return };
    let device = neighbourhood(&bus);
    access_point(&bus, &device, "library", "library", 30, WPA2);
    access_point(&bus, &device, "campus", "campus", 20, WPA2_ENTERPRISE);
    let mut network = follow(&bus);
    network.wait("the networks", |state| ssids(state).len() == 4);

    // A user name, a certificate: nothing a password dialog can ask for,
    // so nothing is asked of NetworkManager. The scan after it shows the
    // command has been dealt with.
    network.send(Command::ConnectWifi("campus".to_owned()));
    network.send(Command::ScanWifi);
    network.wait("the scan", |state| state.wifi.as_ref().unwrap().scanning);
    assert_eq!(calls(&bus, &device, "RequestScan").len(), 1);
    assert!(calls(&bus, MANAGER, "ActivateConnection").is_empty());
    assert!(calls(&bus, MANAGER, "AddAndActivateConnection").is_empty());
    assert_eq!(network.state.wifi.as_ref().unwrap().current(), None);

    // The scan ends when the adapter says when it last finished one.
    bus.set_property(NM, &device, WIRELESS, "LastScan", 2000i64);
    network.wait("the scan to end", |state| {
        !state.wifi.as_ref().unwrap().scanning
    });

    network.send(Command::ConnectWifi("cafe".to_owned()));
    network.wait("the cafe connected", |state| {
        state
            .wifi
            .as_ref()
            .and_then(|wifi| wifi.current())
            .is_some_and(|current| current.ssid == "cafe" && current.status == Status::Connected)
    });
    let joins = calls(&bus, MANAGER, "AddAndActivateConnection");
    assert_eq!(joins.len(), 1);
    assert_eq!(text(&joins[0][1]), device);
    assert_eq!(
        text(&joins[0][2]),
        "/org/freedesktop/NetworkManager/AccessPoint/cafe"
    );
    // NetworkManager saved a profile for it.
    network.wait("the cafe known", |state| {
        state.wifi.as_ref().unwrap().current().unwrap().known
    });

    // One with a password is joined the same way: NetworkManager makes
    // the profile and asks its secret agent for the password.
    network.send(Command::ConnectWifi("library".to_owned()));
    network.wait("the library joined", |state| {
        state
            .wifi
            .as_ref()
            .and_then(|wifi| wifi.current())
            .is_some_and(|current| current.ssid == "library")
    });
    let joins = calls(&bus, MANAGER, "AddAndActivateConnection");
    assert_eq!(joins.len(), 2);
    assert_eq!(
        text(&joins[1][2]),
        "/org/freedesktop/NetworkManager/AccessPoint/library"
    );
}

#[test]
fn a_command_that_fails_undoes_the_prediction() {
    let Some(bus) = start() else { return };
    neighbourhood(&bus);
    bus.call(
        NM,
        MANAGER,
        MOCK,
        "AddMethod",
        &(
            NM,
            "ActivateConnection",
            "ooo",
            "o",
            "raise dbus.exceptions.DBusException('no', name='org.freedesktop.NetworkManager.Failed')",
        ),
    );
    let mut network = follow(&bus);
    network.wait("the networks", |state| ssids(state).len() == 2);

    // What the shell does: show the guess, then send the command.
    let command = Command::ConnectWifi("home".to_owned());
    network.state.predict(&command);
    assert_eq!(
        network.state.wifi.as_ref().unwrap().icon_name(),
        "network-wireless-acquiring-symbolic"
    );
    network.send(command);
    network.wait("the guess to be undone", |state| {
        state.wifi.as_ref().unwrap().current().is_none() && state.primary.is_none()
    });
}

#[test]
fn the_panel_follows_the_primary_connection() {
    let Some(bus) = start() else { return };
    let wifi = neighbourhood(&bus);
    // No cable yet: GNOME shows no wired toggle.
    let wired = wired_device(&bus, UNAVAILABLE);
    let mut network = follow(&bus);
    network.wait("the networks", |state| ssids(state).len() == 2);
    assert_eq!(network.state.wired, None);

    // Wi-Fi coming up: the panel shows it while it connects.
    let home = "/org/freedesktop/NetworkManager/Settings/home_profile";
    let far = "/org/freedesktop/NetworkManager/AccessPoint/home_far";
    let reply = bus.call(
        NM,
        MAIN,
        MOCK,
        "AddActiveConnection",
        &(vec![wifi.as_str()], home, far, "wifi", ACTIVATING),
    );
    let wifi_active = returned_path(&reply);
    bus.set_property(NM, &wifi, WIRELESS, "ActiveAccessPoint", path(far));
    bus.set_property(NM, MANAGER, NM, "ActivatingConnection", path(&wifi_active));
    network.wait("Wi-Fi connecting", |state| {
        state.panel_icon_name() == Some("network-wireless-acquiring-symbolic")
    });
    let toggle = network.state.wifi.as_ref().unwrap();
    assert_eq!(toggle.icon_name(), "network-wireless-acquiring-symbolic");
    assert_eq!(toggle.subtitle(), Some("home"));
    assert!(toggle.networks[0].is_active());

    bus.set_property(NM, &wifi_active, ACTIVE, "State", ACTIVATED);
    bus.set_property(NM, MANAGER, NM, "ActivatingConnection", path("/"));
    bus.set_property(NM, MANAGER, NM, "PrimaryConnection", path(&wifi_active));
    network.wait("Wi-Fi primary", |state| {
        state.primary
            == Some(Connection::Wifi {
                strength: 45,
                status: Status::Connected,
                limited: false,
                hotspot: false,
            })
    });
    assert_eq!(
        network.state.panel_icon_name(),
        Some("network-wireless-signal-ok-symbolic")
    );

    // The internet cannot be reached through it, then it can again.
    bus.call(NM, MAIN, MOCK, "SetConnectivity", &(LIMITED,));
    network.wait("no route", |state| {
        state.panel_icon_name() == Some("network-wireless-no-route-symbolic")
    });
    assert_eq!(
        network.state.wifi.as_ref().unwrap().icon_name(),
        "network-wireless-no-route-symbolic"
    );
    bus.call(NM, MAIN, MOCK, "SetConnectivity", &(FULL,));
    network.wait("a route", |state| {
        state.panel_icon_name() == Some("network-wireless-signal-ok-symbolic")
    });

    // A cable is plugged in; its profile is brought up by command and takes
    // the default route.
    let settings = std::collections::HashMap::from([(
        "connection",
        std::collections::HashMap::from([
            ("id", Variant::from("Wired connection 1")),
            ("type", Variant::from("802-3-ethernet")),
        ]),
    )]);
    let reply = bus.call(
        NM,
        SETTINGS,
        "org.freedesktop.NetworkManager.Settings",
        "AddConnection",
        &(settings,),
    );
    let wired_profile: zbus::OwnedObjectPath = reply.body().deserialize().unwrap();
    bus.set_property(NM, &wired, DEVICE, "State", DISCONNECTED as u32);
    bus.set_property(
        NM,
        &wired,
        DEVICE,
        "AvailableConnections",
        Variant::from(vec![ObjectPath::from(wired_profile.clone())]),
    );
    network.wait("the wired toggle", |state| state.wired.is_some());
    let toggle = network.state.wired.as_ref().unwrap();
    assert_eq!(toggle.icon_name(), "network-wired-disconnected-symbolic");
    assert!(!toggle.is_active());

    network.send(Command::SetWiredConnected(true));
    network.wait("the wire up", |state| {
        state.wired.as_ref().unwrap().status == Status::Connected
    });
    let activations = calls(&bus, MANAGER, "ActivateConnection");
    assert_eq!(activations.len(), 1);
    assert_eq!(text(&activations[0][0]), wired_profile.as_str());
    assert_eq!(text(&activations[0][1]), wired);
    // Still Wi-Fi in the panel until NetworkManager moves the route.
    assert_eq!(
        network.state.panel_icon_name(),
        Some("network-wireless-signal-ok-symbolic")
    );
    let wired_active: String = {
        let active: zbus::OwnedObjectPath = bus.property(NM, &wired, DEVICE, "ActiveConnection");
        active.as_str().to_owned()
    };
    bus.set_property(NM, MANAGER, NM, "PrimaryConnection", path(&wired_active));
    network.wait("the wire primary", |state| {
        state.primary
            == Some(Connection::Wired {
                status: Status::Connected,
                limited: false,
            })
    });
    assert_eq!(
        network.state.panel_icon_name(),
        Some("network-wired-symbolic")
    );
    // Wi-Fi is still up beside it.
    assert_eq!(
        network.state.wifi.as_ref().unwrap().icon_name(),
        "network-wireless-signal-ok-symbolic"
    );

    // The wire is taken down by command; the route goes back to Wi-Fi.
    network.send(Command::SetWiredConnected(false));
    network.wait("the wire down", |state| {
        state.wired.as_ref().unwrap().status == Status::Idle
    });
    assert_eq!(calls(&bus, &wired, "Disconnect").len(), 1);
    bus.set_property(NM, MANAGER, NM, "PrimaryConnection", path(&wifi_active));
    network.wait("Wi-Fi primary again", |state| {
        state.panel_icon_name() == Some("network-wireless-signal-ok-symbolic")
    });

    // Nothing left: no icon.
    bus.set_property(NM, MANAGER, NM, "PrimaryConnection", path("/"));
    network.wait("no icon", |state| state.primary.is_none());

    // NetworkManager is online through something GNOME has no toggle for.
    bus.call(
        NM,
        MAIN,
        MOCK,
        "SetGlobalConnectionState",
        &(CONNECTED_GLOBAL,),
    );
    network.wait("the fallback icon", |state| {
        state.primary == Some(Connection::Other { limited: false })
    });
    assert_eq!(
        network.state.panel_icon_name(),
        Some("network-wired-symbolic")
    );
}

#[test]
fn network_manager_may_start_late_and_restart() {
    let Some(mut bus) = MockBus::start() else {
        return;
    };
    // Nothing on the bus yet: the backend waits.
    let mut network = follow(&bus);

    bus.template("networkmanager", NM, OFFLINE);
    let device = wifi_device(&bus, DISCONNECTED);
    access_point(&bus, &device, "cafe", "cafe", 80, 0);
    network.wait("the network", |state| ssids(state) == ["cafe"]);
    assert_eq!(network.state.primary, None);

    bus.stop_templates();
    network.wait("nothing", |state| *state == network::State::default());

    bus.template("networkmanager", NM, r#"{"WirelessEnabled": false}"#);
    let device = wifi_device(&bus, UNAVAILABLE);
    network.wait("the new adapter", |state| {
        state.wifi.as_ref().is_some_and(|wifi| !wifi.enabled)
    });
    assert!(ssids(&network.state).is_empty());
    access_point(&bus, &device, "library", "library", 50, WPA2);
    network.wait("its network", |state| ssids(state) == ["library"]);
}
