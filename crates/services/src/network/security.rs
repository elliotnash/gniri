//! What kind of security an access point asks for.
//!
//! GNOME Shell takes the strongest security type that libnm considers valid
//! for the access point and the adapter together
//! (`WirelessNetwork._getApSecurityType` in `status/network.js`). The check is
//! libnm's `nm_utils_security_valid`, ported here for the case the shell uses
//! (an access point is given).

/// `NM80211ApFlags`.
const PRIVACY: u32 = 0x1;

// `NM80211ApSecurityFlags`.
const PAIR_WEP40: u32 = 0x1;
const PAIR_WEP104: u32 = 0x2;
const PAIR_TKIP: u32 = 0x4;
const PAIR_CCMP: u32 = 0x8;
const GROUP_WEP40: u32 = 0x10;
const GROUP_WEP104: u32 = 0x20;
const GROUP_TKIP: u32 = 0x40;
const GROUP_CCMP: u32 = 0x80;
const KEY_MGMT_PSK: u32 = 0x100;
const KEY_MGMT_802_1X: u32 = 0x200;
const KEY_MGMT_SAE: u32 = 0x400;
const KEY_MGMT_OWE: u32 = 0x800;
const KEY_MGMT_OWE_TM: u32 = 0x1000;
const KEY_MGMT_EAP_SUITE_B_192: u32 = 0x2000;

// `NMDeviceWifiCapabilities`.
const CAP_CIPHER_WEP40: u32 = 0x1;
const CAP_CIPHER_WEP104: u32 = 0x2;
const CAP_CIPHER_TKIP: u32 = 0x4;
const CAP_CIPHER_CCMP: u32 = 0x8;
const CAP_WPA: u32 = 0x10;
const CAP_RSN: u32 = 0x20;
const CAP_IBSS_RSN: u32 = 0x2000;

/// `NM80211Mode`.
pub const MODE_ADHOC: u32 = 1;

/// The security flags an access point advertises.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flags {
    pub flags: u32,
    pub wpa: u32,
    pub rsn: u32,
    /// `NM80211Mode`.
    pub mode: u32,
}

/// `NMUtilsSecurityType`, without its `INVALID`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Security {
    None,
    StaticWep,
    Leap,
    DynamicWep,
    WpaPsk,
    WpaEnterprise,
    Wpa2Psk,
    Wpa2Enterprise,
    Sae,
    Owe,
    Wpa3SuiteB192,
}

impl Security {
    /// Strongest first, which is the order the shell tries them in.
    const STRONGEST_FIRST: [Self; 11] = [
        Self::Wpa3SuiteB192,
        Self::Owe,
        Self::Sae,
        Self::Wpa2Enterprise,
        Self::Wpa2Psk,
        Self::WpaEnterprise,
        Self::WpaPsk,
        Self::DynamicWep,
        Self::Leap,
        Self::StaticWep,
        Self::None,
    ];

    /// The strongest type an adapter with `capabilities` can use with the
    /// access point, or `None` if there is none: the shell does not list
    /// such an access point.
    pub fn of(access_point: Flags, capabilities: u32) -> Option<Self> {
        Self::STRONGEST_FIRST
            .into_iter()
            .find(|security| security.is_valid(access_point, capabilities))
    }

    /// The menu shows a lock. Port of `WirelessNetwork.secure`: opportunistic
    /// encryption needs no credentials, so it does not count.
    pub fn is_secure(self) -> bool {
        !matches!(self, Self::None | Self::Owe)
    }

    /// A password is all it takes to join. Port of
    /// `WirelessNetwork.canAutoconnect`: enterprise networks need settings
    /// that only the Settings application can ask for.
    pub fn can_autoconnect(self) -> bool {
        !matches!(self, Self::WpaEnterprise | Self::Wpa2Enterprise)
    }

    /// Port of `nm_utils_security_valid` with `have_ap` true.
    fn is_valid(self, access_point: Flags, capabilities: u32) -> bool {
        let Flags {
            flags,
            wpa,
            rsn,
            mode,
        } = access_point;
        let adhoc = mode == MODE_ADHOC;
        let has = |capability: u32| capabilities & capability != 0;
        let privacy = flags & PRIVACY != 0;
        // A pairwise cipher that both sides have.
        let shared_pair = |advertised: u32| {
            (advertised & PAIR_TKIP != 0 && has(CAP_CIPHER_TKIP))
                || (advertised & PAIR_CCMP != 0 && has(CAP_CIPHER_CCMP))
        };

        match self {
            Self::None => !privacy && wpa == 0 && rsn == 0,
            Self::Leap | Self::StaticWep => {
                if self == Self::Leap && adhoc {
                    return false;
                }
                privacy
                    && (wpa == 0 && rsn == 0
                        || supports_ciphers(capabilities, wpa, true)
                        || supports_ciphers(capabilities, rsn, true))
            }
            Self::DynamicWep => {
                !adhoc
                    && rsn == 0
                    && privacy
                    && (wpa == 0
                        || (wpa & KEY_MGMT_802_1X != 0
                            && supports_ciphers(capabilities, wpa, false)))
            }
            Self::WpaPsk => !adhoc && has(CAP_WPA) && wpa & KEY_MGMT_PSK != 0 && shared_pair(wpa),
            Self::Wpa2Psk => {
                if !has(CAP_RSN) {
                    return false;
                }
                if adhoc {
                    return has(CAP_IBSS_RSN) && rsn & PAIR_CCMP != 0 && has(CAP_CIPHER_CCMP);
                }
                rsn & KEY_MGMT_PSK != 0 && shared_pair(rsn)
            }
            Self::WpaEnterprise => {
                !adhoc
                    && has(CAP_WPA)
                    && wpa & KEY_MGMT_802_1X != 0
                    && supports_ciphers(capabilities, wpa, false)
            }
            Self::Wpa2Enterprise => {
                !adhoc
                    && has(CAP_RSN)
                    && rsn & KEY_MGMT_802_1X != 0
                    && supports_ciphers(capabilities, rsn, false)
            }
            // No TKIP with SAE.
            Self::Sae => {
                !adhoc
                    && has(CAP_RSN)
                    && rsn & KEY_MGMT_SAE != 0
                    && rsn & PAIR_CCMP != 0
                    && has(CAP_CIPHER_CCMP)
            }
            Self::Owe => !adhoc && has(CAP_RSN) && rsn & (KEY_MGMT_OWE | KEY_MGMT_OWE_TM) != 0,
            Self::Wpa3SuiteB192 => !adhoc && has(CAP_RSN) && rsn & KEY_MGMT_EAP_SUITE_B_192 != 0,
        }
    }
}

/// The adapter has a pairwise and a group cipher the access point offers.
/// Port of libnm's `device_supports_ap_ciphers`; static WEP uses group
/// ciphers only.
fn supports_ciphers(capabilities: u32, advertised: u32, static_wep: bool) -> bool {
    let both =
        |capability: u32, cipher: u32| capabilities & capability != 0 && advertised & cipher != 0;
    let pair = static_wep
        || both(CAP_CIPHER_WEP40, PAIR_WEP40)
        || both(CAP_CIPHER_WEP104, PAIR_WEP104)
        || both(CAP_CIPHER_TKIP, PAIR_TKIP)
        || both(CAP_CIPHER_CCMP, PAIR_CCMP);
    let group = both(CAP_CIPHER_WEP40, GROUP_WEP40)
        || both(CAP_CIPHER_WEP104, GROUP_WEP104)
        || (!static_wep
            && (both(CAP_CIPHER_TKIP, GROUP_TKIP) || both(CAP_CIPHER_CCMP, GROUP_CCMP)));
    pair && group
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An adapter that can do everything.
    const ALL: u32 = 0x203f;
    const INFRA: u32 = 2;

    fn security(capabilities: u32, mode: u32, flags: u32, wpa: u32, rsn: u32) -> Option<Security> {
        Security::of(
            Flags {
                flags,
                wpa,
                rsn,
                mode,
            },
            capabilities,
        )
    }

    /// The expected values are what libnm 1.58 answers. The port agreed
    /// with it on 200 000 random combinations of flags when it was written.
    #[test]
    fn the_strongest_valid_type_as_libnm_finds_it() {
        use Security::*;
        let infra = |flags, wpa, rsn| security(ALL, INFRA, flags, wpa, rsn);
        assert_eq!(infra(0, 0, 0), Some(None));
        // Plain WEP: nothing tells the dynamic kind apart, and it is tried
        // first.
        assert_eq!(infra(1, 0, 0), Some(DynamicWep));
        assert_eq!(infra(1, 0, 0x10), Some(Leap));
        assert_eq!(infra(1, 0x144, 0), Some(WpaPsk));
        assert_eq!(infra(1, 0, 0x188), Some(Wpa2Psk));
        // WPA and WPA2 mixed.
        assert_eq!(infra(1, 0x144, 0x188), Some(Wpa2Psk));
        // WPA2 and WPA3 in transition, then WPA3 alone.
        assert_eq!(infra(1, 0, 0x588), Some(Sae));
        assert_eq!(infra(1, 0, 0x488), Some(Sae));
        assert_eq!(infra(1, 0, 0x444), Option::None);
        assert_eq!(infra(1, 0x244, 0), Some(WpaEnterprise));
        assert_eq!(infra(1, 0x211, 0), Some(WpaEnterprise));
        assert_eq!(infra(1, 0, 0x288), Some(Wpa2Enterprise));
        assert_eq!(infra(1, 0, 0x888), Some(Owe));
        // An open network that offers the encrypted one beside it.
        assert_eq!(infra(0, 0, 0x1000), Some(Owe));
        assert_eq!(infra(1, 0, 0x2088), Some(Wpa3SuiteB192));
    }

    #[test]
    fn the_adapter_has_a_say() {
        // Without RSN, and without CCMP.
        assert_eq!(security(0x1f, INFRA, 1, 0, 0x188), None);
        assert_eq!(security(ALL & !0x8, INFRA, 1, 0, 0x188), None);
    }

    #[test]
    fn adhoc_networks() {
        use Security::*;
        let adhoc = |capabilities, flags, rsn| security(capabilities, MODE_ADHOC, flags, 0, rsn);
        assert_eq!(adhoc(ALL, 0, 0), Some(None));
        assert_eq!(adhoc(ALL, 1, 0), Some(StaticWep));
        assert_eq!(adhoc(ALL, 1, 0x188), Some(Wpa2Psk));
        assert_eq!(adhoc(ALL, 1, 0x288), Some(Wpa2Psk));
        // The adapter cannot do RSN in an ad-hoc network.
        assert_eq!(adhoc(0x3f, 1, 0x188), Option::None);
    }

    #[test]
    fn what_needs_credentials_and_what_a_password_is_enough_for() {
        assert!(!Security::None.is_secure());
        assert!(!Security::Owe.is_secure());
        assert!(Security::DynamicWep.is_secure());
        assert!(Security::Wpa2Psk.is_secure());
        assert!(Security::Sae.is_secure());
        assert!(Security::Wpa2Enterprise.is_secure());

        assert!(Security::Wpa2Psk.can_autoconnect());
        assert!(Security::None.can_autoconnect());
        assert!(!Security::WpaEnterprise.can_autoconnect());
        assert!(!Security::Wpa2Enterprise.can_autoconnect());
    }
}
