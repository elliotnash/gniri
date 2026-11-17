// SPDX-License-Identifier: GPL-3.0-or-later

//! The secret agent: when NetworkManager needs a password it does not have
//! (a Wi-Fi network joined for the first time, a password that stopped
//! working), it asks the session's agent, and the agent asks the user.
//!
//! Port of `js/ui/components/networkAgent.js` and of the part GNOME Shell
//! has in C, `src/shell-network-agent.c`. What to ask for is decided here,
//! from the connection's settings alone; [`backend`] is the object on the
//! bus.
//!
//! Not ported: VPN connections, whose plugins bring a program of their own
//! to do the asking, and the keyring, where GNOME's agent keeps the secrets
//! NetworkManager is told not to store itself.

pub mod backend;

use gniri_core::{fill, tr};
use std::collections::HashMap;

use serde::Deserialize;
use zbus::{OwnedVariant, Variant};

use crate::model::Model;

/// A connection's settings as NetworkManager sends them: groups of keys.
pub type Settings = HashMap<String, HashMap<String, OwnedVariant>>;

// `NMSecretAgentGetSecretsFlags`.
/// The user may be asked.
pub const ALLOW_INTERACTION: u32 = 0x1;
/// What is stored did not work: ask again.
pub const REQUEST_NEW: u32 = 0x2;
/// The user started the connection themselves.
pub const USER_REQUESTED: u32 = 0x4;
/// The router's WPS button would do as well.
pub const WPS_PBC_ACTIVE: u32 = 0x8;

// `NMWepKeyType`.
const WEP_KEY: u32 = 1;
const WEP_PASSPHRASE: u32 = 2;

/// What a field's text has to be like before it can be sent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Check {
    /// Anything but nothing.
    #[default]
    NotEmpty,
    /// `_validateWpaPsk`: 8 to 63 characters, or the 64 hexadecimal digits
    /// of the key itself.
    WpaPsk,
    /// `_validateStaticWep` for a key: 10 or 26 hexadecimal digits, or 5
    /// or 13 letters.
    WepKey,
    /// The same for a passphrase: at most 64 characters.
    WepPassphrase,
    /// A WEP key of a kind the profile does not say.
    Anything,
}

impl Check {
    pub fn accepts(self, value: &str) -> bool {
        // JavaScript counts a string's length as this does.
        let length = value.encode_utf16().count();
        let all = |accepted: fn(&char) -> bool| value.chars().all(|character| accepted(&character));
        match self {
            Self::NotEmpty => length > 0,
            Self::WpaPsk => {
                if length == 64 {
                    all(char::is_ascii_hexdigit)
                } else {
                    (8..=63).contains(&length)
                }
            }
            Self::WepKey => match length {
                10 | 26 => all(char::is_ascii_hexdigit),
                // GNOME takes letters only here, though such a key may hold
                // any character.
                5 | 13 => all(char::is_ascii_alphabetic),
                _ => false,
            },
            Self::WepPassphrase => length <= 64,
            Self::Anything => true,
        }
    }
}

/// One line of the dialog.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Field {
    /// The entry's hint, such as "Password".
    pub label: String,
    /// The setting's key the answer is for. `None` for a line that is only
    /// shown: the user name a password belongs to.
    pub key: Option<String>,
    /// What NetworkManager already has for it.
    pub value: String,
    /// Typed unseen.
    pub password: bool,
    pub check: Check,
}

impl Field {
    /// Whether `value` can be sent for this field.
    pub fn accepts(&self, value: &str) -> bool {
        self.key.is_none() || self.check.accepts(value)
    }
}

/// One question to the user, as the dialog shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Request {
    pub title: String,
    pub message: Option<String>,
    pub fields: Vec<Field>,
    /// Say that pressing the router's WPS button would do as well.
    pub wps: bool,
}

impl Request {
    /// Whether `values`, one per field, can be sent. Port of
    /// `_updateOkButton`.
    pub fn accepts(&self, values: &[String]) -> bool {
        values.len() == self.fields.len()
            && self
                .fields
                .iter()
                .zip(values)
                .all(|(field, value)| field.accepts(value))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct State {
    /// The question being asked. Others wait their turn.
    pub request: Option<Request>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Begin(Request),
    /// The question is over: answered, cancelled or withdrawn.
    End,
}

#[derive(Clone, PartialEq, Eq)]
pub enum Command {
    /// What the user typed, one text per field of the request.
    Respond(Vec<String>),
    Cancel,
}

/// Never prints the answers: commands are logged.
impl std::fmt::Debug for Command {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Respond(_) => formatter.write_str("Respond(…)"),
            Self::Cancel => formatter.write_str("Cancel"),
        }
    }
}

impl Model for State {
    type Event = Event;
    type Command = Command;

    fn apply(&mut self, event: Event) {
        match event {
            Event::Begin(request) => self.request = Some(request),
            Event::End => self.request = None,
        }
    }

    /// Either way the dialog closes at once, as GNOME's does; whether the
    /// answer was right shows in the connection.
    fn predict(&mut self, command: &Command) {
        match command {
            Command::Respond(_) | Command::Cancel => self.request = None,
        }
    }
}

fn value<'a>(settings: &'a Settings, group: &str, key: &str) -> Option<&'a Variant<'static>> {
    settings.get(group)?.get(key).map(|value| &**value)
}

fn text(settings: &Settings, group: &str, key: &str) -> String {
    match value(settings, group, key) {
        Some(Variant::Str(text)) => text.as_str().to_owned(),
        _ => String::new(),
    }
}

fn number(settings: &Settings, group: &str, key: &str) -> u32 {
    match value(settings, group, key) {
        Some(Variant::U32(number)) => *number,
        Some(Variant::I32(number)) => u32::try_from(*number).unwrap_or(0),
        _ => 0,
    }
}

/// The first of a list of strings.
fn first(settings: &Settings, group: &str, key: &str) -> String {
    match value(settings, group, key) {
        Some(Variant::Array(items)) => match items.inner().first() {
            Some(Variant::Str(text)) => text.as_str().to_owned(),
            _ => String::new(),
        },
        _ => String::new(),
    }
}

/// A network's name for showing. Stands in for `nm_utils_ssid_to_utf8`,
/// which also guesses at the encoding of a name that is not UTF-8.
fn ssid(settings: &Settings) -> String {
    let bytes: Vec<u8> = match value(settings, "802-11-wireless", "ssid") {
        Some(Variant::Array(items)) => items
            .inner()
            .iter()
            .filter_map(|item| match item {
                Variant::U8(byte) => Some(*byte),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    String::from_utf8_lossy(&bytes).into_owned()
}

/// A field that is only shown. `label` is GNOME's English, as for
/// [`asked`]; the field carries it in the user's language.
fn shown(label: &str, value: String) -> Field {
    Field {
        label: tr(label),
        key: None,
        value,
        password: false,
        check: Check::NotEmpty,
    }
}

fn asked(label: &str, key: &str, value: String, password: bool) -> Field {
    Field {
        label: tr(label),
        key: Some(key.to_owned()),
        value,
        password,
        check: Check::NotEmpty,
    }
}

/// Port of `_get8021xSecrets`.
fn enterprise_fields(
    settings: &Settings,
    setting_name: &str,
    hints: &[String],
) -> Result<Vec<Field>, String> {
    const GROUP: &str = "802-1x";
    let hinted = |hint: &str| hints.iter().any(|other| other == hint);
    let identity = || text(settings, GROUP, "identity");
    let password = || {
        asked(
            "Password",
            "password",
            text(settings, GROUP, "password"),
            true,
        )
    };
    let key_password = || {
        asked(
            "Private key password",
            "private-key-password",
            text(settings, GROUP, "private-key-password"),
            true,
        )
    };

    // "If hints were given we know exactly what we need to ask"
    if setting_name == GROUP && !hints.is_empty() {
        let mut fields = Vec::new();
        if hinted("identity") {
            fields.push(asked("Username", "identity", identity(), false));
        }
        if hinted("password") {
            fields.push(password());
        }
        if hinted("private-key-password") {
            fields.push(key_password());
        }
        return Ok(fields);
    }

    let method = first(settings, GROUP, "eap");
    match method.as_str() {
        // "TTLS and PEAP are actually much more complicated, but this
        // complication is not visible here since we only care about phase2
        // authentication (and don't even care of which one)"
        "md5" | "leap" | "ttls" | "peap" | "fast" => {
            Ok(vec![shown("Username", identity()), password()])
        }
        "tls" => Ok(vec![shown("Identity", identity()), key_password()]),
        other => Err(format!("{other:?} is no 802.1X method gniri can ask for")),
    }
}

/// Port of `_getWirelessSecrets`.
fn wireless_fields(
    settings: &Settings,
    setting_name: &str,
    hints: &[String],
) -> Result<Vec<Field>, String> {
    const GROUP: &str = "802-11-wireless-security";
    if setting_name == "802-1x" {
        return enterprise_fields(settings, setting_name, hints);
    }
    let key_management = text(settings, GROUP, "key-mgmt");
    match key_management.as_str() {
        "wpa-none" | "wpa-psk" | "sae" => Ok(vec![Field {
            check: Check::WpaPsk,
            ..asked("Password", "psk", text(settings, GROUP, "psk"), true)
        }]),
        // Static WEP.
        "none" => {
            let key = format!("wep-key{}", number(settings, GROUP, "wep-tx-keyidx"));
            let check = match number(settings, GROUP, "wep-key-type") {
                WEP_KEY => Check::WepKey,
                WEP_PASSPHRASE => Check::WepPassphrase,
                _ => Check::Anything,
            };
            Ok(vec![Field {
                check,
                ..asked("Key", &key, text(settings, GROUP, &key), true)
            }])
        }
        // Cisco LEAP, else dynamic WEP.
        "ieee8021x" if text(settings, GROUP, "auth-alg") == "leap" => Ok(vec![asked(
            "Password",
            "leap-password",
            text(settings, GROUP, "leap-password"),
            true,
        )]),
        "ieee8021x" | "wpa-eap" => enterprise_fields(settings, setting_name, hints),
        other => Err(format!("{other:?} is no key management gniri can ask for")),
    }
}

/// What to ask the user so that NetworkManager gets the secrets of
/// `setting_name` for the connection with these settings. Port of
/// `NetworkSecretDialog._getContent`.
///
/// `Err` says why nothing can be asked: the answer to NetworkManager is
/// then that this agent has no secrets, and it tries the next.
pub fn request(
    settings: &Settings,
    setting_name: &str,
    hints: &[String],
    flags: u32,
) -> Result<Request, String> {
    let id = text(settings, "connection", "id");
    let kind = text(settings, "connection", "type");
    let hinted = |hint: &str| hints.iter().any(|other| other == hint);
    let password_for = |group: &str| {
        vec![asked(
            "Password",
            "password",
            text(settings, group, "password"),
            true,
        )]
    };
    let needs_password = || {
        Some(fill(
            &tr("A password is required to connect to “%s”"),
            &[&id],
        ))
    };

    let (title, message, fields) = match kind.as_str() {
        "802-11-wireless" => (
            "Authentication required",
            Some(fill(
                &tr(
                    "Passwords or encryption keys are required to access the wireless network “%s”",
                ),
                &[&ssid(settings)],
            )),
            wireless_fields(settings, setting_name, hints)?,
        ),
        "802-3-ethernet" => {
            let mut fields = vec![shown("Network name", id.clone())];
            fields.extend(enterprise_fields(settings, setting_name, hints)?);
            ("Wired 802.1X authentication", None, fields)
        }
        "pppoe" => (
            "DSL authentication",
            None,
            vec![
                asked(
                    "Username",
                    "username",
                    text(settings, "pppoe", "username"),
                    false,
                ),
                asked(
                    "Service",
                    "service",
                    text(settings, "pppoe", "service"),
                    false,
                ),
                asked(
                    "Password",
                    "password",
                    text(settings, "pppoe", "password"),
                    true,
                ),
            ],
        ),
        "gsm" if hinted("pin") => (
            "PIN code required",
            Some(tr("PIN code is needed for the mobile broadband device")),
            vec![asked("PIN", "pin", text(settings, "gsm", "pin"), true)],
        ),
        "gsm" | "cdma" => (
            "Authentication required",
            needs_password(),
            password_for(&kind),
        ),
        "bluetooth" => {
            let group = if settings.contains_key("cdma") {
                "cdma"
            } else {
                "gsm"
            };
            (
                "Authentication required",
                needs_password(),
                password_for(group),
            )
        }
        "vpn" => {
            return Err("gniri does not ask for VPN secrets".to_owned());
        }
        other => return Err(format!("{other:?} is no connection type gniri can ask for")),
    };
    if !fields.iter().any(|field| field.key.is_some()) {
        return Err("there is nothing to ask for".to_owned());
    }
    Ok(Request {
        title: tr(title),
        message,
        fields,
        wps: flags & WPS_PBC_ACTIVE != 0,
    })
}

/// The answer to NetworkManager: `values`, one per field, under the keys
/// of the fields that have one. Port of `shell_network_agent_set_password`
/// and `shell_network_agent_respond`.
pub fn secrets(setting_name: &str, fields: &[Field], values: &[String]) -> Settings {
    let setting: HashMap<String, OwnedVariant> = fields
        .iter()
        .zip(values)
        .filter_map(|(field, value)| {
            let key = field.key.clone()?;
            let value = OwnedVariant::try_from(Variant::from(value.as_str())).ok()?;
            Some((key, value))
        })
        .collect();
    HashMap::from([(setting_name.to_owned(), setting)])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(groups: &[(&str, Vec<(&str, Variant<'_>)>)]) -> Settings {
        groups
            .iter()
            .map(|(group, entries)| {
                let entries = entries
                    .iter()
                    .map(|(key, value)| ((*key).to_owned(), value.try_to_owned().unwrap()))
                    .collect();
                ((*group).to_owned(), entries)
            })
            .collect()
    }

    /// A Wi-Fi profile with this key management.
    fn wifi(key_management: &str, more: Vec<(&'static str, Variant<'static>)>) -> Settings {
        let mut security = vec![("key-mgmt", Variant::from(key_management.to_owned()))];
        security.extend(more);
        settings(&[
            (
                "connection",
                vec![
                    ("id", Variant::from("Cafe 1")),
                    ("type", Variant::from("802-11-wireless")),
                ],
            ),
            (
                "802-11-wireless",
                vec![("ssid", Variant::from(b"Caf\xc3\xa9".to_vec()))],
            ),
            ("802-11-wireless-security", security),
        ])
    }

    fn ask(settings: &Settings, setting_name: &str, hints: &[&str]) -> Request {
        let hints: Vec<String> = hints.iter().map(|hint| (*hint).to_owned()).collect();
        request(settings, setting_name, &hints, ALLOW_INTERACTION).unwrap()
    }

    fn labels(request: &Request) -> Vec<(&str, Option<&str>)> {
        request
            .fields
            .iter()
            .map(|field| (field.label.as_str(), field.key.as_deref()))
            .collect()
    }

    #[test]
    fn a_wifi_password() {
        let request = ask(&wifi("wpa-psk", vec![]), "802-11-wireless-security", &[]);
        assert_eq!(request.title, "Authentication required");
        assert_eq!(
            request.message.as_deref(),
            Some("Passwords or encryption keys are required to access the wireless network “Café”")
        );
        assert_eq!(
            request.fields,
            [Field {
                label: "Password".to_owned(),
                key: Some("psk".to_owned()),
                value: String::new(),
                password: true,
                check: Check::WpaPsk,
            }]
        );
        assert!(!request.wps);

        // WPA3 asks the same; what NetworkManager has is filled in.
        let known = wifi("sae", vec![("psk", Variant::from("old password"))]);
        let request = ask(&known, "802-11-wireless-security", &[]);
        assert_eq!(request.fields[0].value, "old password");
    }

    #[test]
    fn the_wps_button_is_mentioned_when_it_would_do() {
        let request = request(
            &wifi("wpa-psk", vec![]),
            "802-11-wireless-security",
            &[],
            ALLOW_INTERACTION | USER_REQUESTED | WPS_PBC_ACTIVE,
        )
        .unwrap();
        assert!(request.wps);
    }

    #[test]
    fn a_wep_key_goes_under_the_index_in_use() {
        let profile = wifi(
            "none",
            vec![
                ("wep-tx-keyidx", Variant::from(2u32)),
                ("wep-key-type", Variant::from(1u32)),
            ],
        );
        let request = ask(&profile, "802-11-wireless-security", &[]);
        assert_eq!(labels(&request), [("Key", Some("wep-key2"))]);
        assert_eq!(request.fields[0].check, Check::WepKey);

        let passphrase = wifi("none", vec![("wep-key-type", Variant::from(2u32))]);
        let request = ask(&passphrase, "802-11-wireless-security", &[]);
        assert_eq!(labels(&request), [("Key", Some("wep-key0"))]);
        assert_eq!(request.fields[0].check, Check::WepPassphrase);
        let unknown = ask(&wifi("none", vec![]), "802-11-wireless-security", &[]);
        assert_eq!(unknown.fields[0].check, Check::Anything);
    }

    #[test]
    fn leap_and_enterprise_networks() {
        let leap = wifi("ieee8021x", vec![("auth-alg", Variant::from("leap"))]);
        let request = ask(&leap, "802-11-wireless-security", &[]);
        assert_eq!(labels(&request), [("Password", Some("leap-password"))]);

        let mut peap = wifi("wpa-eap", vec![]);
        peap.extend(settings(&[(
            "802-1x",
            vec![
                ("eap", Variant::from(vec!["peap"])),
                ("identity", Variant::from("ada")),
            ],
        )]));
        let request = ask(&peap, "802-1x", &[]);
        // The name is shown, the password asked for.
        assert_eq!(
            labels(&request),
            [("Username", None), ("Password", Some("password"))]
        );
        assert_eq!(request.fields[0].value, "ada");

        // With hints, exactly what they name.
        let request = ask(&peap, "802-1x", &["identity", "password"]);
        assert_eq!(
            labels(&request),
            [
                ("Username", Some("identity")),
                ("Password", Some("password"))
            ]
        );

        let mut tls = wifi("wpa-eap", vec![]);
        tls.extend(settings(&[(
            "802-1x",
            vec![("eap", Variant::from(vec!["tls"]))],
        )]));
        let request = ask(&tls, "802-1x", &[]);
        assert_eq!(
            labels(&request),
            [
                ("Identity", None),
                ("Private key password", Some("private-key-password"))
            ]
        );
    }

    #[test]
    fn other_kinds_of_connection() {
        let wired = settings(&[
            (
                "connection",
                vec![
                    ("id", Variant::from("Office")),
                    ("type", Variant::from("802-3-ethernet")),
                ],
            ),
            ("802-1x", vec![("eap", Variant::from(vec!["md5"]))]),
        ]);
        let request = ask(&wired, "802-1x", &[]);
        assert_eq!(request.title, "Wired 802.1X authentication");
        assert_eq!(request.message, None);
        assert_eq!(
            labels(&request),
            [
                ("Network name", None),
                ("Username", None),
                ("Password", Some("password"))
            ]
        );
        assert_eq!(request.fields[0].value, "Office");

        let dsl = settings(&[
            (
                "connection",
                vec![
                    ("id", Variant::from("Line")),
                    ("type", Variant::from("pppoe")),
                ],
            ),
            ("pppoe", vec![("username", Variant::from("ada"))]),
        ]);
        let request = ask(&dsl, "pppoe", &[]);
        assert_eq!(request.title, "DSL authentication");
        assert_eq!(
            labels(&request),
            [
                ("Username", Some("username")),
                ("Service", Some("service")),
                ("Password", Some("password"))
            ]
        );
        assert_eq!(request.fields[0].value, "ada");

        let modem = settings(&[(
            "connection",
            vec![
                ("id", Variant::from("Mobile")),
                ("type", Variant::from("gsm")),
            ],
        )]);
        let request = ask(&modem, "gsm", &["pin"]);
        assert_eq!(request.title, "PIN code required");
        assert_eq!(labels(&request), [("PIN", Some("pin"))]);
        let request = ask(&modem, "gsm", &[]);
        assert_eq!(
            request.message.as_deref(),
            Some("A password is required to connect to “Mobile”")
        );
        assert_eq!(labels(&request), [("Password", Some("password"))]);
    }

    #[test]
    fn what_cannot_be_asked_for() {
        let refused = |settings: &Settings, setting_name: &str| {
            request(settings, setting_name, &[], ALLOW_INTERACTION).unwrap_err()
        };
        let vpn = settings(&[(
            "connection",
            vec![
                ("id", Variant::from("Work")),
                ("type", Variant::from("vpn")),
            ],
        )]);
        assert!(refused(&vpn, "vpn").contains("VPN"));
        assert!(refused(&Settings::new(), "802-1x").contains("connection type"));
        assert!(
            refused(
                &wifi("wpa-eap-suite-b-192", vec![]),
                "802-11-wireless-security"
            )
            .contains("key management")
        );
        // An enterprise network whose method is not set.
        assert!(refused(&wifi("wpa-eap", vec![]), "802-1x").contains("802.1X"));
        // Hints that name nothing gniri knows leave nothing to ask.
        let hints = ["certificate".to_owned()];
        let nothing = request(&wifi("wpa-eap", vec![]), "802-1x", &hints, 0);
        assert!(nothing.unwrap_err().contains("nothing to ask"));
    }

    #[test]
    fn passwords_are_checked_as_gnome_checks_them() {
        let psk = |value: &str| Check::WpaPsk.accepts(value);
        assert!(!psk(""));
        assert!(!psk("short12"));
        assert!(psk("eight ch"));
        assert!(psk(&"x".repeat(63)));
        assert!(!psk(&"x".repeat(64)));
        assert!(psk(&"0123456789abcdefABCDEF".repeat(3)[..64]));
        assert!(!psk(&"x".repeat(65)));

        let key = |value: &str| Check::WepKey.accepts(value);
        assert!(key("0123456789"));
        assert!(key(&"aF".repeat(13)));
        assert!(!key("012345678g"));
        assert!(key("abcde"));
        assert!(key("abcdefghijklm"));
        assert!(!key("abc12"));
        assert!(!key("abcdef"));

        assert!(Check::WepPassphrase.accepts(""));
        assert!(!Check::WepPassphrase.accepts(&"x".repeat(65)));
        assert!(Check::Anything.accepts(""));
        assert!(!Check::NotEmpty.accepts(""));
        assert!(Check::NotEmpty.accepts("x"));
    }

    #[test]
    fn a_request_is_ready_when_every_asked_field_is() {
        let request = Request {
            fields: vec![
                shown("Username", String::new()),
                asked("Password", "password", String::new(), true),
            ],
            ..Request::default()
        };
        let values = |password: &str| vec![String::new(), password.to_owned()];
        assert!(!request.accepts(&values("")));
        assert!(request.accepts(&values("x")));
        // One text per field, no fewer.
        assert!(!request.accepts(&["x".to_owned()]));
    }

    #[test]
    fn the_answer_holds_the_asked_keys_only() {
        let fields = vec![
            shown("Username", "ada".to_owned()),
            asked("Password", "password", String::new(), true),
        ];
        let answer = secrets("802-1x", &fields, &["ada".to_owned(), "sesame".to_owned()]);
        assert_eq!(answer.len(), 1);
        let setting = &answer["802-1x"];
        assert_eq!(setting.len(), 1);
        assert_eq!(*setting["password"], Variant::from("sesame"));
    }

    #[test]
    fn a_request_comes_and_goes_and_an_answer_is_never_printed() {
        let mut state = State::default();
        let request = ask(&wifi("wpa-psk", vec![]), "802-11-wireless-security", &[]);
        state.apply(Event::Begin(request.clone()));
        assert_eq!(state.request, Some(request.clone()));
        state.predict(&Command::Respond(vec!["hunter2 hunter2".to_owned()]));
        assert_eq!(state, State::default());

        state.apply(Event::Begin(request));
        state.predict(&Command::Cancel);
        assert_eq!(state, State::default());
        state.apply(Event::End);
        assert_eq!(state, State::default());

        let printed = format!("{:?}", Command::Respond(vec!["hunter2".to_owned()]));
        assert!(!printed.contains("hunter2"), "{printed}");
    }

    #[test]
    fn a_scene_can_describe_a_request() {
        let state: State = toml::from_str(
            r#"
            [request]
            title = "Authentication required"
            message = "A password"
            fields = [{ label = "Password", key = "psk", password = true, check = "wpa-psk" }]
            "#,
        )
        .unwrap();
        let request = state.request.unwrap();
        assert_eq!(request.fields[0].check, Check::WpaPsk);
        assert!(request.fields[0].password);
    }
}
