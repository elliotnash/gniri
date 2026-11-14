//! Network connection.
//!
//! M1 has the state and the indicator logic for an established connection
//! only; fixture scenes fill it in. The NetworkManager backend, and states
//! such as connecting, limited connectivity and VPN, arrive in M2.

use serde::Deserialize;

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct State {
    /// The connection that carries the default route.
    pub primary: Option<Connection>,
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

impl Connection {
    /// The panel icon. The Wi-Fi steps are `signalToIcon` in
    /// `status/network.js`.
    pub fn icon_name(&self) -> &'static str {
        match self {
            Self::Wired => "network-wired-symbolic",
            Self::Wifi { strength } => match strength {
                0..20 => "network-wireless-signal-none-symbolic",
                20..40 => "network-wireless-signal-weak-symbolic",
                40..50 => "network-wireless-signal-ok-symbolic",
                50..80 => "network-wireless-signal-good-symbolic",
                _ => "network-wireless-signal-excellent-symbolic",
            },
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
}
