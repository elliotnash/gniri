// SPDX-License-Identifier: GPL-3.0-or-later

//! The Bluetooth backend against python-dbusmock's BlueZ.
//!
//! Every test runs the backend through `run_with`, with an ordinary file
//! standing in for `/dev/rfkill`: `run` would write to the real device and
//! switch the machine's Bluetooth off.

mod common;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::{Follower, MockBus, TIMEOUT};
use gniri_services::bluetooth::{self, Adapter, Command, State, Status};
use gniri_services::rfkill::backend::{Record, TYPE_BLUETOOTH};
use zbus::{ObjectPath, OwnedVariant, Variant};

const BLUEZ: &str = "org.bluez";
const BLUEZ_MOCK: &str = "org.bluez.Mock";
const MOCK: &str = "org.freedesktop.DBus.Mock";
const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";
const BATTERY: &str = "org.bluez.Battery1";
const OBJECT_MANAGER: &str = "org.freedesktop.DBus.ObjectManager";

const HCI0: &str = "/org/bluez/hci0";
const PHONE: &str = "AA:BB:CC:00:00:01";
const PHONE_PATH: &str = "/org/bluez/hci0/dev_AA_BB_CC_00_00_01";
const TABLET: &str = "AA:BB:CC:00:00:02";
const TABLET_PATH: &str = "/org/bluez/hci0/dev_AA_BB_CC_00_00_02";

const ON: Adapter = Adapter {
    powered: true,
    changing: false,
};
const OFF: Adapter = Adapter {
    powered: false,
    changing: false,
};

/// A file for the backend to take for `/dev/rfkill`, gone with the test.
struct KillSwitch {
    directory: PathBuf,
}

impl KillSwitch {
    fn new(test: &str) -> Self {
        let directory =
            std::env::temp_dir().join(format!("gniri-bluetooth-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let switch = Self { directory };
        std::fs::write(switch.device(), []).unwrap();
        switch
    }

    fn device(&self) -> PathBuf {
        self.directory.join("rfkill")
    }

    /// The last request written to it.
    fn written(&self) -> Vec<u8> {
        std::fs::read(self.device()).unwrap()
    }
}

impl Drop for KillSwitch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn follow(bus: &MockBus, switch: &KillSwitch) -> Follower<State> {
    let device = switch.device();
    Follower::new(bus.run(move |connection, events, commands| async move {
        bluetooth::backend::run_with(connection, events, commands, &device).await
    }))
}

/// Applies the backend's next report, which must not take long.
fn next_report(bluetooth: &mut Follower<State>) {
    use gniri_services::Model;

    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Ok(event) = bluetooth.link.events.try_recv() {
            bluetooth.state.apply(event);
            return;
        }
        assert!(Instant::now() < deadline, "the backend reported nothing");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn add_adapter(bus: &MockBus) {
    bus.call(BLUEZ, "/", BLUEZ_MOCK, "AddAdapter", &("hci0", "laptop"));
}

/// A device that is paired, as dbusmock's phone: its profiles include an
/// audio source, so the menu lists it.
fn add_paired(bus: &MockBus, address: &str, alias: &str) {
    bus.call(
        BLUEZ,
        "/",
        BLUEZ_MOCK,
        "AddDevice",
        &("hci0", address, alias),
    );
    bus.call(BLUEZ, "/", BLUEZ_MOCK, "PairDevice", &("hci0", address));
}

/// Replaces a method of the mock at `path` with Python code.
fn script(bus: &MockBus, path: &str, method: &str, code: &str) {
    bus.call(
        BLUEZ,
        path,
        MOCK,
        "AddMethod",
        &(DEVICE, method, "", "", code),
    );
}

/// How often `method` was called on the mock at `path`.
fn calls(bus: &MockBus, path: &str, method: &str) -> usize {
    let reply = bus.call(BLUEZ, path, MOCK, "GetMethodCalls", &(method,));
    let calls: Vec<(u64, Vec<OwnedVariant>)> = reply.body().deserialize().unwrap();
    calls.len()
}

fn status(state: &State, address: &str) -> Option<Status> {
    state
        .devices
        .iter()
        .find(|device| device.address == address)
        .map(|device| device.status)
}

fn names(state: &State) -> Vec<&str> {
    state
        .devices
        .iter()
        .map(|device| device.name.as_str())
        .collect()
}

#[test]
fn bluez_comes_and_goes() {
    let Some(mut bus) = MockBus::start() else {
        return;
    };
    // No BlueZ on the bus at all.
    let switch = KillSwitch::new("comes-and-goes");
    let mut bluetooth = follow(&bus, &switch);
    next_report(&mut bluetooth);
    assert_eq!(bluetooth.state, State::default());
    assert_eq!(bluetooth.state.icon_name(), "bluetooth-disabled-symbolic");

    // It starts, without an adapter at first.
    bus.template("bluez5", BLUEZ, "{}");
    add_adapter(&bus);
    bluetooth.wait("the adapter", |state| state.adapter.is_some());
    assert_eq!(bluetooth.state.adapter, Some(ON));
    assert_eq!(bluetooth.state.icon_name(), "bluetooth-active-symbolic");
    assert_eq!(
        bluetooth.state.placeholder(),
        Some("No available or connected devices")
    );

    add_paired(&bus, PHONE, "Phone");
    bluetooth.wait("the phone", |state| !state.devices.is_empty());

    // It crashes.
    bus.stop_templates();
    bluetooth.wait("no adapter", |state| state.adapter.is_none());
    assert_eq!(bluetooth.state, State::default());

    // And is started again, knowing what it knew.
    bus.template("bluez5", BLUEZ, "{}");
    add_adapter(&bus);
    add_paired(&bus, PHONE, "Phone");
    bluetooth.wait("the phone again", |state| !state.devices.is_empty());
    assert_eq!(bluetooth.state.adapter, Some(ON));
}

#[test]
fn the_adapter_and_its_devices_follow_the_bus() {
    let Some(mut bus) = MockBus::start() else {
        return;
    };
    bus.template("bluez5", BLUEZ, "{}");
    let switch = KillSwitch::new("follows");
    let mut bluetooth = follow(&bus, &switch);
    next_report(&mut bluetooth);
    assert_eq!(bluetooth.state, State::default());

    add_adapter(&bus);
    bluetooth.wait("the adapter", |state| state.adapter == Some(ON));

    // A device that was only seen is none of the menu's business.
    bus.call(
        BLUEZ,
        "/",
        BLUEZ_MOCK,
        "AddDevice",
        &("hci0", PHONE, "Phone"),
    );
    add_paired(&bus, TABLET, "Tablet");
    bluetooth.wait("the paired device", |state| !state.devices.is_empty());
    assert_eq!(names(&bluetooth.state), ["Tablet"]);
    let tablet = bluetooth.state.devices[0].clone();
    assert_eq!(tablet.address, TABLET);
    assert_eq!(tablet.status, Status::Disconnected);
    assert_eq!(tablet.icon_name(), "phone-symbolic");
    assert_eq!(tablet.battery, None);
    assert!(tablet.connectable);
    assert_eq!(bluetooth.state.placeholder(), None);
    assert_eq!(bluetooth.state.subtitle(), None);
    assert!(!bluetooth.state.shows_indicator());

    // Pairing the other one, by name before the first.
    bus.call(BLUEZ, "/", BLUEZ_MOCK, "PairDevice", &("hci0", PHONE));
    bluetooth.wait("both devices", |state| state.devices.len() == 2);
    assert_eq!(names(&bluetooth.state), ["Phone", "Tablet"]);

    // Someone else connects the tablet: it moves to the top.
    bus.call(BLUEZ, "/", BLUEZ_MOCK, "ConnectDevice", &("hci0", TABLET));
    bluetooth.wait("the tablet connected", |state| {
        status(state, TABLET) == Some(Status::Connected)
    });
    assert_eq!(names(&bluetooth.state), ["Tablet", "Phone"]);
    assert_eq!(bluetooth.state.subtitle().as_deref(), Some("Tablet"));
    assert!(bluetooth.state.shows_indicator());

    bus.call(BLUEZ, "/", BLUEZ_MOCK, "ConnectDevice", &("hci0", PHONE));
    bluetooth.wait("both connected", |state| {
        status(state, PHONE) == Some(Status::Connected)
    });
    assert_eq!(bluetooth.state.subtitle().as_deref(), Some("2 Connected"));

    // Renaming a device.
    bus.set_property(BLUEZ, PHONE_PATH, DEVICE, "Alias", "Zed");
    bluetooth.wait("the new name", |state| names(state) == ["Tablet", "Zed"]);

    // Switching off: the kill switch first, then the adapter.
    bluetooth.send(Command::SetPowered(false));
    bluetooth.wait("turning off", |state| {
        state
            .adapter
            .as_ref()
            .is_some_and(|adapter| adapter.changing)
    });
    assert!(!bluetooth.state.powered());
    assert_eq!(bluetooth.state.icon_name(), "bluetooth-acquiring-symbolic");
    bluetooth.wait("off", |state| state.adapter == Some(OFF));
    assert!(!bus.property::<bool>(BLUEZ, HCI0, ADAPTER, "Powered"));
    assert_eq!(
        switch.written(),
        Record::change_all(TYPE_BLUETOOTH, true).bytes()
    );
    assert!(bluetooth.state.devices.is_empty());
    assert_eq!(bluetooth.state.icon_name(), "bluetooth-disabled-symbolic");
    assert_eq!(
        bluetooth.state.placeholder(),
        Some("Turn on Bluetooth to connect to devices")
    );
    assert!(!bluetooth.state.shows_indicator());

    // And on again.
    bluetooth.send(Command::SetPowered(true));
    bluetooth.wait("turning on", |state| {
        state
            .adapter
            .as_ref()
            .is_some_and(|adapter| adapter.changing)
    });
    assert!(bluetooth.state.powered());
    bluetooth.wait("on", |state| state.adapter == Some(ON));
    assert!(bus.property::<bool>(BLUEZ, HCI0, ADAPTER, "Powered"));
    assert_eq!(
        switch.written(),
        Record::change_all(TYPE_BLUETOOTH, false).bytes()
    );
    assert_eq!(names(&bluetooth.state), ["Tablet", "Zed"]);

    // Someone else switching it off.
    bus.set_property(BLUEZ, HCI0, ADAPTER, "Powered", false);
    bluetooth.wait("off from outside", |state| state.adapter == Some(OFF));

    // Forgetting a device; unplugging the adapter.
    bus.set_property(BLUEZ, HCI0, ADAPTER, "Powered", true);
    bluetooth.wait("on from outside", |state| state.devices.len() == 2);
    let phone = ObjectPath::try_from(PHONE_PATH).unwrap();
    bus.call(BLUEZ, HCI0, ADAPTER, "RemoveDevice", &(phone,));
    bluetooth.wait("the phone forgotten", |state| names(state) == ["Tablet"]);
    bus.call(BLUEZ, "/", BLUEZ_MOCK, "RemoveAdapter", &("hci0",));
    bluetooth.wait("no adapter", |state| state.adapter.is_none());
    assert_eq!(bluetooth.state, State::default());
}

#[test]
fn devices_connect_and_disconnect_on_command() {
    let Some(mut bus) = MockBus::start() else {
        return;
    };
    bus.template("bluez5", BLUEZ, "{}");
    // Everything is there before the backend starts.
    add_adapter(&bus);
    add_paired(&bus, PHONE, "Phone");
    add_paired(&bus, TABLET, "Tablet");
    let switch = KillSwitch::new("connects");
    let mut bluetooth = follow(&bus, &switch);
    next_report(&mut bluetooth);
    assert_eq!(bluetooth.state.adapter, Some(ON));
    assert_eq!(names(&bluetooth.state), ["Phone", "Tablet"]);

    let phone = bluetooth.state.devices[0].clone();
    assert_eq!(phone.activated(), Some(Command::Connect(PHONE.to_owned())));
    bluetooth.send(phone.activated().unwrap());
    bluetooth.wait("connecting", |state| {
        status(state, PHONE) == Some(Status::Connecting)
    });
    assert_eq!(bluetooth.state.devices[0].activated(), None);
    assert!(!bluetooth.state.shows_indicator());
    bluetooth.wait("connected", |state| {
        status(state, PHONE) == Some(Status::Connected)
    });
    assert_eq!(calls(&bus, PHONE_PATH, "Connect"), 1);
    assert_eq!(bluetooth.state.subtitle().as_deref(), Some("Phone"));
    assert!(bluetooth.state.shows_indicator());

    let phone = bluetooth.state.devices[0].clone();
    assert_eq!(
        phone.activated(),
        Some(Command::Disconnect(PHONE.to_owned()))
    );
    bluetooth.send(phone.activated().unwrap());
    bluetooth.wait("disconnecting", |state| {
        status(state, PHONE) == Some(Status::Disconnecting)
    });
    // The link is still up.
    assert!(bluetooth.state.shows_indicator());
    bluetooth.wait("disconnected", |state| {
        status(state, PHONE) == Some(Status::Disconnected)
    });
    assert_eq!(calls(&bus, PHONE_PATH, "Disconnect"), 1);
    assert!(!bluetooth.state.shows_indicator());

    // An address nothing has: nothing is called, and the shell hears so.
    bluetooth.send(Command::Connect("AA:BB:CC:00:00:09".to_owned()));
    next_report(&mut bluetooth);
    assert_eq!(status(&bluetooth.state, PHONE), Some(Status::Disconnected));
    assert_eq!(status(&bluetooth.state, TABLET), Some(Status::Disconnected));
}

#[test]
fn a_connection_that_fails_falls_back() {
    let Some(mut bus) = MockBus::start() else {
        return;
    };
    bus.template("bluez5", BLUEZ, "{}");
    add_adapter(&bus);
    add_paired(&bus, PHONE, "Phone");
    // The device is out of range.
    script(
        &bus,
        PHONE_PATH,
        "Connect",
        "raise dbus.exceptions.DBusException('Page Timeout', name='org.bluez.Error.Failed')",
    );
    let switch = KillSwitch::new("fails");
    let mut bluetooth = follow(&bus, &switch);
    bluetooth.wait("the phone", |state| !state.devices.is_empty());

    bluetooth.send(Command::Connect(PHONE.to_owned()));
    bluetooth.wait("connecting", |state| {
        status(state, PHONE) == Some(Status::Connecting)
    });
    bluetooth.wait("disconnected again", |state| {
        status(state, PHONE) == Some(Status::Disconnected)
    });
    assert_eq!(calls(&bus, PHONE_PATH, "Connect"), 1);
    assert_eq!(bluetooth.state.devices[0].action_label(), "Connect");
}

#[test]
fn a_slow_connection_holds_nothing_up() {
    let Some(mut bus) = MockBus::start() else {
        return;
    };
    bus.template("bluez5", BLUEZ, "{}");
    add_adapter(&bus);
    add_paired(&bus, PHONE, "Phone");
    add_paired(&bus, TABLET, "Tablet");
    // The phone takes its time, and the whole mock with it.
    script(
        &bus,
        PHONE_PATH,
        "Connect",
        "time.sleep(1)\n\
         self.EmitSignal('org.freedesktop.DBus.Properties', 'PropertiesChanged', 'sa{sv}as', \
         ['org.bluez.Device1', {'Connected': dbus.Boolean(True)}, []])",
    );
    let switch = KillSwitch::new("slow");
    let mut bluetooth = follow(&bus, &switch);
    bluetooth.wait("the devices", |state| state.devices.len() == 2);

    let started = Instant::now();
    bluetooth.send(Command::Connect(PHONE.to_owned()));
    bluetooth.send(Command::Connect(TABLET.to_owned()));
    // The second command is taken up while the first is unanswered.
    bluetooth.wait("both connecting", |state| {
        status(state, PHONE) == Some(Status::Connecting)
            && status(state, TABLET) == Some(Status::Connecting)
    });
    assert!(started.elapsed() < Duration::from_millis(900));

    bluetooth.wait("both connected", |state| {
        status(state, PHONE) == Some(Status::Connected)
            && status(state, TABLET) == Some(Status::Connected)
    });
    assert!(started.elapsed() >= Duration::from_secs(1));
    assert_eq!(bluetooth.state.subtitle().as_deref(), Some("2 Connected"));
}

#[test]
fn a_device_reports_its_battery() {
    let Some(mut bus) = MockBus::start() else {
        return;
    };
    bus.template("bluez5", BLUEZ, "{}");
    add_adapter(&bus);
    add_paired(&bus, PHONE, "Phone");
    add_paired(&bus, TABLET, "Tablet");
    // The template has no batteries; the interface is BlueZ's.
    let battery = |percentage: u8| HashMap::from([("Percentage", Variant::from(percentage))]);
    bus.call(
        BLUEZ,
        PHONE_PATH,
        MOCK,
        "AddProperties",
        &(BATTERY, battery(80)),
    );

    let switch = KillSwitch::new("battery");
    let mut bluetooth = follow(&bus, &switch);
    bluetooth.wait("the devices", |state| state.devices.len() == 2);
    assert_eq!(bluetooth.state.devices[0].battery, Some(80));
    assert_eq!(bluetooth.state.devices[1].battery, None);

    bus.call(
        BLUEZ,
        PHONE_PATH,
        MOCK,
        "UpdateProperties",
        &(BATTERY, battery(79)),
    );
    bluetooth.wait("the battery draining", |state| {
        state.devices[0].battery == Some(79)
    });

    // The tablet connects and starts reporting.
    bus.call(
        BLUEZ,
        TABLET_PATH,
        MOCK,
        "AddProperties",
        &(BATTERY, battery(45)),
    );
    let added: Vec<Variant<'_>> = vec![
        ObjectPath::try_from(TABLET_PATH).unwrap().into(),
        HashMap::from([(BATTERY, battery(45))]).into(),
    ];
    bus.call(
        BLUEZ,
        "/",
        MOCK,
        "EmitSignal",
        &(OBJECT_MANAGER, "InterfacesAdded", "oa{sa{sv}}", added),
    );
    bluetooth.wait("the tablet's battery", |state| {
        state.devices[1].battery == Some(45)
    });

    // And stops.
    let removed: Vec<Variant<'_>> = vec![
        ObjectPath::try_from(TABLET_PATH).unwrap().into(),
        vec![BATTERY].into(),
    ];
    bus.call(
        BLUEZ,
        "/",
        MOCK,
        "EmitSignal",
        &(OBJECT_MANAGER, "InterfacesRemoved", "oas", removed),
    );
    bluetooth.wait("no battery", |state| state.devices[1].battery.is_none());
    assert_eq!(bluetooth.state.devices[0].battery, Some(79));
}
