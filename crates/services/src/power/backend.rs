//! UPower and the power-profiles daemon, over the system bus.

use std::collections::HashMap;

use async_channel::{Receiver, Sender};
use futures_lite::StreamExt;
use zbus::OwnedVariant;

use super::{Battery, Charge, Command, Event, Profile};
use crate::bus::{merge, wake};

/// The display device is UPower's summary of every battery in the machine.
#[zbus::proxy(
    interface = "org.freedesktop.UPower.Device",
    default_service = "org.freedesktop.UPower",
    default_path = "/org/freedesktop/UPower/devices/DisplayDevice"
)]
trait Device {
    #[zbus(property)]
    fn is_present(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn percentage(&self) -> zbus::Result<f64>;
    #[zbus(property)]
    fn state(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn charge_threshold_enabled(&self) -> zbus::Result<bool>;
}

#[zbus::proxy(
    interface = "org.freedesktop.UPower.PowerProfiles",
    default_service = "org.freedesktop.UPower.PowerProfiles",
    default_path = "/org/freedesktop/UPower/PowerProfiles"
)]
trait PowerProfiles {
    #[zbus(property)]
    fn active_profile(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn set_active_profile(&self, profile: &str) -> zbus::Result<()>;
    #[zbus(property)]
    fn profiles(&self) -> zbus::Result<Vec<HashMap<String, OwnedVariant>>>;
}

// `UpDeviceState`.
const CHARGING: u32 = 1;
const FULLY_CHARGED: u32 = 4;
const PENDING_CHARGE: u32 = 5;

/// Port of the state handling in `PowerToggle._sync` (`status/system.js`).
pub fn charge(state: u32, threshold_enabled: bool) -> Charge {
    match state {
        CHARGING => Charge::Charging,
        FULLY_CHARGED => Charge::Full,
        PENDING_CHARGE if threshold_enabled => Charge::PluggedIn,
        _ => Charge::Discharging,
    }
}

async fn read_battery(device: &DeviceProxy<'_>) -> Option<Battery> {
    if !device.is_present().await.ok()? {
        return None;
    }
    Some(Battery {
        percentage: device.percentage().await.ok()?,
        charge: charge(
            device.state().await.ok()?,
            // Older UPower versions do not have the property.
            device.charge_threshold_enabled().await.unwrap_or(false),
        ),
    })
}

async fn read_profiles(profiles: &PowerProfilesProxy<'_>) -> Event {
    let active = profiles.active_profile().await.ok();
    // The daemon lists them slowest first; the menu shows the fastest first.
    let available = profiles
        .profiles()
        .await
        .unwrap_or_default()
        .iter()
        .rev()
        .filter_map(|entry| {
            let name: &str = entry.get("Profile")?.downcast_ref().ok()?;
            Profile::from_id(name)
        })
        .collect();
    Event::Profiles {
        active: active.as_deref().and_then(Profile::from_id),
        available,
    }
}

#[derive(Clone)]
enum Wake {
    Battery,
    Profiles,
    Command(Command),
}

pub async fn run(
    connection: zbus::Connection,
    events: Sender<Event>,
    commands: Receiver<Command>,
) -> zbus::Result<()> {
    let device = DeviceProxy::new(&connection).await?;
    let profiles = PowerProfilesProxy::new(&connection).await?;

    let mut wakes = merge(vec![
        wake(device.receive_is_present_changed().await, Wake::Battery),
        wake(device.receive_percentage_changed().await, Wake::Battery),
        wake(device.receive_state_changed().await, Wake::Battery),
        wake(
            device.receive_charge_threshold_enabled_changed().await,
            Wake::Battery,
        ),
        // The services coming and going: read everything again.
        wake(device.inner().receive_owner_changed().await?, Wake::Battery),
        wake(
            profiles.receive_active_profile_changed().await,
            Wake::Profiles,
        ),
        wake(profiles.receive_profiles_changed().await, Wake::Profiles),
        wake(
            profiles.inner().receive_owner_changed().await?,
            Wake::Profiles,
        ),
        Box::pin(commands.map(Wake::Command)),
    ]);

    // The property streams also yield each property's first value, so the
    // initial state arrives through the loop like any later change.
    while let Some(wake) = wakes.next().await {
        let sent = match wake {
            Wake::Battery => {
                events
                    .send(Event::Battery(read_battery(&device).await))
                    .await
            }
            Wake::Profiles => events.send(read_profiles(&profiles).await).await,
            Wake::Command(Command::SetProfile(profile)) => {
                if let Err(error) = profiles.set_active_profile(profile.id()).await {
                    eprintln!("gniri: power: cannot set the profile: {error}");
                    // Undo the shell's prediction.
                    events.send(read_profiles(&profiles).await).await
                } else {
                    Ok(())
                }
            }
        };
        if sent.is_err() {
            break; // The shell went away.
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upower_states_map_to_charge() {
        assert_eq!(charge(1, false), Charge::Charging);
        assert_eq!(charge(2, false), Charge::Discharging);
        assert_eq!(charge(4, false), Charge::Full);
        assert_eq!(charge(5, true), Charge::PluggedIn);
        assert_eq!(charge(5, false), Charge::Discharging);
        assert_eq!(charge(6, true), Charge::Discharging);
    }
}
