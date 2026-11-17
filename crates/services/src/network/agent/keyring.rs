// SPDX-License-Identifier: GPL-3.0-or-later

//! Where the agent keeps the secrets NetworkManager leaves to it: the
//! keyring, through the Secret Service (`org.freedesktop.secrets`, which
//! gnome-keyring provides). The part of `src/shell-network-agent.c` that
//! uses libsecret.
//!
//! The items are GNOME's: the schema `org.freedesktop.NetworkManager.Connection`
//! and the attributes `connection-uuid`, `setting-name` and `setting-key`,
//! which nm-applet uses too. A password either of them kept is found here,
//! and the other way round.
//!
//! Secrets cross the session bus as they are (the Secret Service's "plain"
//! session), which only the user's own processes can listen to. libsecret
//! encrypts them on the way.

use std::collections::HashMap;

use futures_lite::StreamExt;
use zbus::{ObjectPath, OwnedObjectPath, OwnedVariant, Variant};

use super::Settings;

const SCHEMA: &str = "org.freedesktop.NetworkManager.Connection";
const UUID: &str = "connection-uuid";
const SETTING_NAME: &str = "setting-name";
const SETTING_KEY: &str = "setting-key";
/// The attribute libsecret adds with the schema's name.
const SCHEMA_ATTRIBUTE: &str = "xdg:schema";
/// `SECRET_COLLECTION_DEFAULT`
const DEFAULT: &str = "/org/freedesktop/secrets/aliases/default";
const NO_SUCH_OBJECT: &str = "org.freedesktop.Secret.Error.NoSuchObject";
/// What a call that needs no prompt answers instead of one.
const NO_PROMPT: &str = "/";

// `NMSettingSecretFlags`
/// The agent keeps it.
const AGENT_OWNED: u32 = 0x1;
/// Nobody keeps it: it is asked for every time.
const NOT_SAVED: u32 = 0x2;

/// A secret as the Secret Service passes it: the session, the session's
/// parameters, the value and its content type.
type Secret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

#[zbus::proxy(
    interface = "org.freedesktop.Secret.Service",
    default_service = "org.freedesktop.secrets",
    default_path = "/org/freedesktop/secrets"
)]
trait SecretService {
    fn open_session(
        &self,
        algorithm: &str,
        input: &Variant<'_>,
    ) -> zbus::Result<(OwnedVariant, OwnedObjectPath)>;

    fn search_items(
        &self,
        attributes: HashMap<&str, &str>,
    ) -> zbus::Result<(Vec<OwnedObjectPath>, Vec<OwnedObjectPath>)>;

    fn unlock(
        &self,
        objects: &[ObjectPath<'_>],
    ) -> zbus::Result<(Vec<OwnedObjectPath>, OwnedObjectPath)>;

    fn get_secrets(
        &self,
        items: &[ObjectPath<'_>],
        session: &ObjectPath<'_>,
    ) -> zbus::Result<HashMap<OwnedObjectPath, Secret>>;

    fn create_collection(
        &self,
        properties: HashMap<&str, Variant<'_>>,
        alias: &str,
    ) -> zbus::Result<(OwnedObjectPath, OwnedObjectPath)>;
}

#[zbus::proxy(
    interface = "org.freedesktop.Secret.Collection",
    default_service = "org.freedesktop.secrets"
)]
trait Collection {
    fn create_item(
        &self,
        properties: HashMap<&str, Variant<'_>>,
        secret: &(ObjectPath<'_>, Vec<u8>, Vec<u8>, &str),
        replace: bool,
    ) -> zbus::Result<(OwnedObjectPath, OwnedObjectPath)>;
}

#[zbus::proxy(
    interface = "org.freedesktop.Secret.Item",
    default_service = "org.freedesktop.secrets"
)]
trait Item {
    fn delete(&self) -> zbus::Result<OwnedObjectPath>;

    #[zbus(property)]
    fn attributes(&self) -> zbus::Result<HashMap<String, String>>;
}

#[zbus::proxy(
    interface = "org.freedesktop.Secret.Prompt",
    default_service = "org.freedesktop.secrets"
)]
trait Prompt {
    fn prompt(&self, window_id: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    fn completed(&self, dismissed: bool, result: Variant<'_>) -> zbus::Result<()>;
}

/// A string from a connection's settings, or nothing.
pub fn text(settings: &Settings, group: &str, key: &str) -> Option<String> {
    let value = settings.get(group)?.get(key)?;
    String::try_from(value.try_clone().ok()?).ok()
}

fn flags(group: &HashMap<String, OwnedVariant>, key: &str) -> u32 {
    group
        .get(key)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(0)
}

/// The key that holds the flags of the secret `key`: the four WEP keys
/// share one.
fn flags_key(key: &str) -> String {
    match key {
        "wep-key0" | "wep-key1" | "wep-key2" | "wep-key3" => "wep-key-flags".to_owned(),
        _ => format!("{key}-flags"),
    }
}

/// `is_connection_always_ask`: some secret of the connection is never
/// kept, so there is no use looking for any; the user is asked.
pub fn always_ask(settings: &Settings) -> bool {
    let kind = text(settings, "connection", "type").unwrap_or_default();
    // "Be a bit paranoid and only consider secrets from settings relevant
    // to the connection type."
    let groups: &[&str] = match kind.as_str() {
        "802-11-wireless" => &["802-11-wireless-security", "802-1x"],
        "802-3-ethernet" => &["pppoe", "802-1x"],
        _ => &[],
    };
    std::iter::once(kind.as_str())
        .chain(groups.iter().copied())
        .filter_map(|name| settings.get(name))
        .any(|group| {
            group
                .keys()
                .filter(|key| key.ends_with("-flags"))
                .any(|key| flags(group, key) & NOT_SAVED != 0)
        })
}

/// One secret to keep: `save_one_secret`'s attributes and label.
#[derive(Debug, PartialEq, Eq)]
pub struct Kept {
    pub setting_name: String,
    pub key: String,
    pub value: String,
}

/// The secrets of `settings` that are the agent's to keep: those whose
/// flags say exactly "agent-owned", and that are text and not empty
/// (`write_one_secret_to_keyring` and `save_one_secret`). VPN secrets are
/// not among them: gniri does not ask for those.
pub fn to_keep(settings: &Settings) -> Vec<Kept> {
    let mut kept: Vec<Kept> = settings
        .iter()
        .filter(|(setting_name, _)| setting_name.as_str() != "vpn")
        .flat_map(|(setting_name, group)| {
            group.iter().filter_map(move |(key, value)| {
                if key.ends_with("-flags") || flags(group, &flags_key(key)) != AGENT_OWNED {
                    return None;
                }
                let value = String::try_from(value.try_clone().ok()?).ok()?;
                (!value.is_empty()).then(|| Kept {
                    setting_name: setting_name.clone(),
                    key: key.clone(),
                    value,
                })
            })
        })
        .collect();
    kept.sort_by(|a, b| (&a.setting_name, &a.key).cmp(&(&b.setting_name, &b.key)));
    kept
}

/// `settings` with the secrets of `answer` put in.
pub fn with_secrets(settings: &Settings, answer: &Settings) -> Settings {
    let mut merged: Settings = settings
        .iter()
        .map(|(name, group)| {
            let group = group
                .iter()
                .filter_map(|(key, value)| Some((key.clone(), value.try_clone().ok()?)))
                .collect();
            (name.clone(), group)
        })
        .collect();
    for (name, group) in answer {
        let into = merged.entry(name.clone()).or_default();
        for (key, value) in group {
            if let Ok(value) = value.try_clone() {
                into.insert(key.clone(), value);
            }
        }
    }
    merged
}

/// The keyring, reached over the session bus.
#[derive(Clone)]
pub struct Keyring {
    connection: zbus::Connection,
}

impl Keyring {
    pub fn new(connection: zbus::Connection) -> Self {
        Self { connection }
    }

    /// Runs `future` apart, on a thread of its own: what the keyring does
    /// may wait on the user, and the agent goes on meanwhile.
    pub fn spawn(&self, future: impl std::future::Future<Output = ()> + Send + 'static) {
        let spawned = std::thread::Builder::new()
            .name("keyring".to_owned())
            .spawn(move || futures_lite::future::block_on(future));
        if let Err(error) = spawned {
            eprintln!("gniri: network agent: cannot start a thread: {error}");
        }
    }

    async fn service(&self) -> zbus::Result<SecretServiceProxy<'static>> {
        SecretServiceProxy::new(&self.connection).await
    }

    /// Runs the prompt at `path`, if there is one: gnome-keyring asks the
    /// user (to unlock a keyring, or for the password of a new one). True
    /// if it was not dismissed.
    async fn prompt(&self, path: &ObjectPath<'_>) -> zbus::Result<bool> {
        if path.as_str() == NO_PROMPT {
            return Ok(true);
        }
        let prompt = PromptProxy::builder(&self.connection)
            .path(path.to_owned())
            .build()
            .await?;
        let mut completed = prompt.receive_completed().await?;
        prompt.prompt("").await?;
        match completed.next().await {
            Some(signal) => Ok(!signal.args()?.dismissed),
            None => Ok(false),
        }
    }

    /// The secrets of `setting_name` kept for the connection `uuid`, by
    /// key. Locked items are unlocked first, which may ask the user
    /// (`SECRET_SEARCH_ALL | SECRET_SEARCH_UNLOCK | SECRET_SEARCH_LOAD_SECRETS`).
    pub async fn find(
        &self,
        uuid: &str,
        setting_name: &str,
    ) -> zbus::Result<HashMap<String, String>> {
        let service = self.service().await?;
        let (mut unlocked, locked) = service
            .search_items(HashMap::from([(UUID, uuid), (SETTING_NAME, setting_name)]))
            .await?;
        if !locked.is_empty() {
            let paths: Vec<ObjectPath<'_>> = locked.iter().map(|path| path.as_ref()).collect();
            let (now, prompt) = service.unlock(&paths).await?;
            unlocked.extend(now);
            // "This can happen if the user denied a request to unlock":
            // what is still locked is left out.
            if self.prompt(&prompt.as_ref()).await? {
                let (again, _) = service
                    .search_items(HashMap::from([(UUID, uuid), (SETTING_NAME, setting_name)]))
                    .await?;
                unlocked = again;
            }
        }
        if unlocked.is_empty() {
            return Ok(HashMap::new());
        }
        let (_, session) = service.open_session("plain", &Variant::from("")).await?;
        let paths: Vec<ObjectPath<'_>> = unlocked.iter().map(|path| path.as_ref()).collect();
        let secrets = service.get_secrets(&paths, &session.as_ref()).await?;
        let mut found = HashMap::new();
        for (path, (_, _, value, _)) in secrets {
            let item = ItemProxy::builder(&self.connection)
                .path(path)
                .build()
                .await?;
            let Some(key) = item.attributes().await?.remove(SETTING_KEY) else {
                continue;
            };
            if let Ok(value) = String::from_utf8(value) {
                found.insert(key, value);
            }
        }
        Ok(found)
    }

    /// Forgets every secret kept for the connection `uuid`
    /// (`secret_password_clear`).
    pub async fn delete(&self, uuid: &str) -> zbus::Result<()> {
        let service = self.service().await?;
        let (unlocked, locked) = service.search_items(HashMap::from([(UUID, uuid)])).await?;
        for path in unlocked.into_iter().chain(locked) {
            let item = ItemProxy::builder(&self.connection)
                .path(path)
                .build()
                .await?;
            let prompt = item.delete().await?;
            self.prompt(&prompt.as_ref()).await?;
        }
        Ok(())
    }

    /// Keeps what of `settings` is the agent's to keep, in place of what
    /// was kept for the connection before (`shell_network_agent_save_secrets`).
    pub async fn save(&self, settings: &Settings) -> zbus::Result<()> {
        let uuid = text(settings, "connection", "uuid").unwrap_or_default();
        if uuid.is_empty() {
            return Ok(());
        }
        // "First delete any existing items in the keyring." Its errors are
        // ignored, as there.
        let _ = self.delete(&uuid).await;
        let kept = to_keep(settings);
        if kept.is_empty() {
            return Ok(());
        }
        let id = text(settings, "connection", "id").unwrap_or_default();
        let service = self.service().await?;
        let (_, session) = service.open_session("plain", &Variant::from("")).await?;
        let mut collection = self.collection(DEFAULT).await?;
        for (index, secret) in kept.iter().enumerate() {
            let label = format!(
                "Network secret for {id}/{}/{}",
                secret.setting_name, secret.key
            );
            let attributes = HashMap::from([
                (SCHEMA_ATTRIBUTE, SCHEMA),
                (UUID, uuid.as_str()),
                (SETTING_NAME, secret.setting_name.as_str()),
                (SETTING_KEY, secret.key.as_str()),
            ]);
            let properties = HashMap::from([
                ("org.freedesktop.Secret.Item.Label", Variant::from(label)),
                (
                    "org.freedesktop.Secret.Item.Attributes",
                    Variant::from(attributes),
                ),
            ]);
            let value = (
                session.as_ref(),
                Vec::new(),
                secret.value.as_bytes().to_vec(),
                "text/plain",
            );
            let created = collection
                .create_item(properties.clone(), &value, true)
                .await;
            let (_, prompt) = match created {
                // libsecret's `on_store_create`: with no default keyring
                // there is one made, which asks the user for its password.
                Err(zbus::Error::MethodError(name, _, _))
                    if index == 0 && name.as_str() == NO_SUCH_OBJECT =>
                {
                    let made = HashMap::from([(
                        "org.freedesktop.Secret.Collection.Label",
                        Variant::from("Default keyring"),
                    )]);
                    let (path, prompt) = service.create_collection(made, "default").await?;
                    if !self.prompt(&prompt.as_ref()).await? {
                        return Ok(());
                    }
                    // The new keyring is at the alias now, whichever way
                    // the service answered.
                    let path = if path.as_str() == NO_PROMPT {
                        DEFAULT.to_owned()
                    } else {
                        path.as_str().to_owned()
                    };
                    collection = self.collection(&path).await?;
                    collection.create_item(properties, &value, true).await?
                }
                other => other?,
            };
            self.prompt(&prompt.as_ref()).await?;
        }
        Ok(())
    }

    async fn collection(&self, path: &str) -> zbus::Result<CollectionProxy<'static>> {
        CollectionProxy::builder(&self.connection)
            .path(path.to_owned())
            .build()
            .await
    }
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

    #[test]
    fn only_what_is_left_to_the_agent_is_kept() {
        let settings = settings(&[
            (
                "connection",
                vec![
                    ("id", Variant::from("Cafe")),
                    ("uuid", Variant::from("1234")),
                    ("type", Variant::from("802-11-wireless")),
                ],
            ),
            (
                "802-11-wireless-security",
                vec![
                    ("key-mgmt", Variant::from("none")),
                    ("psk", Variant::from("kept by NetworkManager")),
                    ("psk-flags", Variant::from(0u32)),
                    ("wep-key0", Variant::from("abcde")),
                    ("wep-key1", Variant::from("")),
                    ("wep-key-flags", Variant::from(AGENT_OWNED)),
                ],
            ),
            (
                "802-1x",
                vec![
                    ("password", Variant::from("asked every time")),
                    ("password-flags", Variant::from(NOT_SAVED)),
                    ("private-key-password", Variant::from("mine")),
                    ("private-key-password-flags", Variant::from(AGENT_OWNED)),
                    // Not text.
                    ("password-raw", Variant::from(vec![1u8, 2])),
                    ("password-raw-flags", Variant::from(AGENT_OWNED)),
                ],
            ),
        ]);
        let kept = |setting_name: &str, key: &str, value: &str| Kept {
            setting_name: setting_name.to_owned(),
            key: key.to_owned(),
            value: value.to_owned(),
        };
        assert_eq!(
            to_keep(&settings),
            [
                kept("802-11-wireless-security", "wep-key0", "abcde"),
                kept("802-1x", "private-key-password", "mine"),
            ]
        );
        // A secret that is never kept means the user is always asked.
        assert!(always_ask(&settings));
    }

    #[test]
    fn a_password_left_to_the_agent_is_looked_up_first() {
        let wifi = |flags: u32| {
            settings(&[
                (
                    "connection",
                    vec![("type", Variant::from("802-11-wireless"))],
                ),
                (
                    "802-11-wireless-security",
                    vec![
                        ("key-mgmt", Variant::from("wpa-psk")),
                        ("psk-flags", Variant::from(flags)),
                    ],
                ),
            ])
        };
        assert!(!always_ask(&wifi(AGENT_OWNED)));
        assert!(always_ask(&wifi(NOT_SAVED)));
        // Only the settings that belong to the connection's type count.
        let ethernet = settings(&[
            (
                "connection",
                vec![("type", Variant::from("802-3-ethernet"))],
            ),
            (
                "802-11-wireless-security",
                vec![("psk-flags", Variant::from(NOT_SAVED))],
            ),
        ]);
        assert!(!always_ask(&ethernet));
    }

    #[test]
    fn the_answer_goes_into_the_settings() {
        let before = settings(&[(
            "802-11-wireless-security",
            vec![
                ("key-mgmt", Variant::from("wpa-psk")),
                ("psk-flags", Variant::from(AGENT_OWNED)),
            ],
        )]);
        let answer = settings(&[(
            "802-11-wireless-security",
            vec![("psk", Variant::from("secret"))],
        )]);
        let merged = with_secrets(&before, &answer);
        let group = &merged["802-11-wireless-security"];
        assert_eq!(group.len(), 3);
        assert_eq!(
            text(&merged, "802-11-wireless-security", "psk").as_deref(),
            Some("secret")
        );
    }
}
