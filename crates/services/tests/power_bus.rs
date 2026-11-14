//! The power backend against python-dbusmock's UPower and power-profiles
//! daemon.

mod common;

use common::{Follower, MockBus};
use gniri_services::power::{self, Charge, Command, Profile};

const UPOWER: &str = "org.freedesktop.UPower";
const UPOWER_PATH: &str = "/org/freedesktop/UPower";
const PROFILES: &str = "org.freedesktop.UPower.PowerProfiles";
const PROFILES_PATH: &str = "/org/freedesktop/UPower/PowerProfiles";
const MOCK: &str = "org.freedesktop.DBus.Mock";

/// `SetupDisplayDevice`: type, state, percentage, three energy values, two
/// times, is-present, icon name, warning level.
fn display_device(bus: &MockBus, state: u32, percentage: f64, present: bool) {
    bus.call(
        UPOWER,
        UPOWER_PATH,
        MOCK,
        "SetupDisplayDevice",
        &(
            2u32, state, percentage, 0.0, 0.0, 0.0, 0i64, 0i64, present, "", 0u32,
        ),
    );
}

#[test]
fn battery_and_profiles_follow_the_bus() {
    let Some(mut bus) = MockBus::start() else {
        return;
    };
    bus.template("upower", UPOWER, "{}");
    bus.template(
        "upower_power_profiles_daemon",
        PROFILES,
        r#"{"ActiveProfile": "balanced"}"#,
    );
    display_device(&bus, 2, 64.2, true);

    let mut power: Follower<power::State> = Follower::new(bus.run(power::backend::run));

    power.wait("the first battery reading", |state| state.battery.is_some());
    let battery = power.state.battery.clone().unwrap();
    assert_eq!(battery.charge, Charge::Discharging);
    assert_eq!(battery.icon_name(), "battery-level-60-symbolic");
    assert_eq!(battery.label(), "64%");

    power.wait("the profiles", |state| state.profile.is_some());
    assert_eq!(power.state.profile, Some(Profile::Balanced));
    assert_eq!(
        power.state.profiles,
        [Profile::Performance, Profile::Balanced, Profile::PowerSaver]
    );

    // Plugging in.
    display_device(&bus, 1, 65.0, true);
    power.wait("charging", |state| {
        state.battery.as_ref().map(|battery| battery.charge) == Some(Charge::Charging)
    });

    // The battery going away (a desktop, or a removable pack).
    display_device(&bus, 0, 0.0, false);
    power.wait("no battery", |state| state.battery.is_none());

    // A command reaches the daemon and comes back as an event.
    power.send(Command::SetProfile(Profile::Performance));
    power.wait("the performance profile", |state| {
        state.profile == Some(Profile::Performance)
    });
    let active: String = bus.property(PROFILES, PROFILES_PATH, PROFILES, "ActiveProfile");
    assert_eq!(active, "performance");
    assert_eq!(power.state.last_profile, Profile::Performance);

    // Someone else changing the profile.
    bus.set_property(
        PROFILES,
        PROFILES_PATH,
        PROFILES,
        "ActiveProfile",
        "power-saver",
    );
    power.wait("the power-saver profile", |state| {
        state.profile == Some(Profile::PowerSaver)
    });
}
