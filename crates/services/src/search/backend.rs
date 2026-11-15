//! Asks the installed search providers over the session bus. Port of
//! gnome-shell's `js/ui/remoteSearch.js`.
//!
//! A provider is an application that installs a key file under
//! `gnome-shell/search-providers/` naming a bus name and an object, on which
//! it answers `org.gnome.Shell.SearchProvider2`. Most are started by the bus
//! when first asked, and some take their time.
//!
//! So nothing here waits for one provider before it looks at the next, or
//! at the shell's commands: every question is a task of its own, and the
//! loop sleeps on the commands and all the tasks at once. A new search
//! drops the tasks of the one before it, whose answers nobody wants any
//! more (docs/DECISIONS.md, M2: the loop never sits in one reply).

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::{Duration, Instant};

use async_channel::{Receiver, Sender};
use futures_lite::future::or;
use zbus::{OwnedVariant, Variant};

use super::{Command, Event, Image, MAX_RESULTS, ProviderInfo, ResultItem, filter_results};
use crate::desktop_entry::{self, DesktopEntry};
use crate::notifications::Pixels;

const INTERFACE: &str = "org.gnome.Shell.SearchProvider2";
/// remoteSearch.js `KEY_FILE_GROUP`.
const KEY_FILE_GROUP: &str = "Shell Search Provider";
/// The default of GNOME's `org.gnome.desktop.search-providers sort-order`
/// setting: these come first, in this order.
const SORT_ORDER: [&str; 3] = [
    "org.gnome.Settings",
    "org.gnome.Contacts",
    "org.gnome.Nautilus",
];
/// How long a provider has to answer a search. GNOME Shell waits as long as
/// GDBus does, 25 seconds; a search nobody has waited ten seconds for is
/// hard to imagine.
pub const PROVIDER_TIMEOUT: Duration = Duration::from_secs(10);

/// Which providers are asked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// Desktop entry ids (without ".desktop") of providers that are never
    /// asked.
    pub disabled: Vec<String>,
}

/// A provider and where it answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderSpec {
    pub info: ProviderInfo,
    pub bus_name: String,
    pub object_path: String,
    /// Whether asking may start it. One that says no is only asked while
    /// it runs anyway.
    pub auto_start: bool,
}

/// The directories providers install their key files in: GNOME Shell's
/// `collectFromDatadirs('search-providers', false)`, which looks in the
/// system's data directories and not in the user's own.
pub fn provider_dirs() -> Vec<PathBuf> {
    let dirs = match std::env::var_os("XDG_DATA_DIRS").filter(|value| !value.is_empty()) {
        Some(list) => std::env::split_paths(&list).collect(),
        None => vec![
            PathBuf::from("/usr/local/share"),
            PathBuf::from("/usr/share"),
        ],
    };
    dirs.into_iter()
        .map(|dir| dir.join("gnome-shell/search-providers"))
        .collect()
}

/// The keys of a key file's `[Shell Search Provider]` group, or `None` if
/// it has no such group.
fn provider_keys(text: &str) -> Option<HashMap<&str, &str>> {
    let mut keys = None;
    let mut in_group = false;
    for line in text.lines() {
        let line = line.trim();
        if let Some(group) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            in_group = group == KEY_FILE_GROUP;
            if in_group {
                keys.get_or_insert_with(HashMap::new);
            }
            continue;
        }
        if !in_group || line.starts_with('#') {
            continue;
        }
        if let (Some((key, value)), Some(keys)) = (line.split_once('='), keys.as_mut()) {
            keys.insert(key.trim(), value.trim());
        }
    }
    keys
}

/// A desktop entry id as a key file or a setting writes it: with or
/// without ".desktop".
fn bare_id(id: &str) -> &str {
    id.strip_suffix(".desktop").unwrap_or(id)
}

/// One key file's provider (`loadRemoteSearchProvider`). `None` for a file
/// that is not a provider's, for a provider of the first version of the
/// interface, for one that is off unless the user turns it on (there is no
/// setting to turn one on here), and for one whose application is not
/// installed or not shown.
fn provider(
    text: &str,
    find: &impl Fn(&str) -> Option<DesktopEntry>,
    desktops: &[String],
) -> Option<ProviderSpec> {
    let keys = provider_keys(text)?;
    let flag = |key: &str, default: bool| match keys.get(key) {
        Some(&"true") => true,
        Some(&"false") => false,
        _ => default,
    };
    let bus_name = *keys.get("BusName")?;
    let object_path = *keys.get("ObjectPath")?;
    zbus::names::BusName::try_from(bus_name).ok()?;
    zbus::ObjectPath::try_from(object_path).ok()?;
    let version: u32 = keys.get("Version").and_then(|text| text.parse().ok())?;
    if version < 2 || flag("DefaultDisabled", false) {
        return None;
    }
    let entry = find(keys.get("DesktopId")?).filter(|entry| entry.should_show(desktops))?;
    Some(ProviderSpec {
        info: ProviderInfo {
            id: entry.id,
            name: entry.name,
            icon: entry.icon,
        },
        bus_name: bus_name.to_owned(),
        object_path: object_path.to_owned(),
        auto_start: flag("AutoStart", true),
    })
}

/// The providers installed in `dirs`, in the order their results are
/// listed (`loadRemoteSearchProviders`): the ones GNOME puts first, then
/// the rest by name. `find` looks an application's desktop entry up.
///
/// Where two files name the same object on the same bus name, the first
/// counts.
pub fn load_providers(
    dirs: &[PathBuf],
    find: impl Fn(&str) -> Option<DesktopEntry>,
    desktops: &[String],
    options: &Options,
) -> Vec<ProviderSpec> {
    let mut providers: Vec<ProviderSpec> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "ini"))
            .collect();
        // A directory lists its files in no order; the result must not
        // depend on it.
        files.sort();
        for file in files {
            let Some(provider) = read_provider(&file, &find, desktops) else {
                continue;
            };
            let known = providers.iter().any(|other| {
                other.bus_name == provider.bus_name && other.object_path == provider.object_path
            });
            let disabled = options
                .disabled
                .iter()
                .any(|id| bare_id(id) == provider.info.id);
            if !known && !disabled {
                providers.push(provider);
            }
        }
    }

    let place = |provider: &ProviderSpec| {
        SORT_ORDER
            .iter()
            .position(|id| *id == provider.info.id)
            .unwrap_or(SORT_ORDER.len())
    };
    providers.sort_by(|a, b| {
        (place(a), a.info.name.to_lowercase(), &a.info.id).cmp(&(
            place(b),
            b.info.name.to_lowercase(),
            &b.info.id,
        ))
    });
    providers
}

fn read_provider(
    file: &Path,
    find: &impl Fn(&str) -> Option<DesktopEntry>,
    desktops: &[String],
) -> Option<ProviderSpec> {
    provider(&std::fs::read_to_string(file).ok()?, find, desktops)
}

/// The picture a serialised `GIcon` stands for (`g_icon_serialize`): a
/// themed icon's first name, or a file. Emblems and embedded image files
/// are not shown.
fn serialized_icon(value: &Variant<'_>) -> Option<Image> {
    // The dictionary's own variant may or may not have been taken off.
    let value = match value {
        Variant::Variant(inner) => inner,
        other => other,
    };
    let Variant::Structure(icon) = value else {
        return None;
    };
    let [Variant::Str(kind), Variant::Variant(data)] = icon.fields() else {
        return None;
    };
    match (kind.as_str(), &**data) {
        ("themed", Variant::Array(names)) => names.iter().find_map(|name| match name {
            Variant::Str(name) if !name.is_empty() => Some(Image::Name(name.to_string())),
            _ => None,
        }),
        ("file", Variant::Str(address)) => Image::from_hint(address.as_str()),
        _ => None,
    }
}

/// The picture a `gicon` string stands for (`g_icon_new_for_string`): a
/// path, a `file:` address, an icon name, or GIO's own way of writing a
/// themed icon with several names, of which the first counts.
fn icon_from_string(text: &str) -> Option<Image> {
    match text.strip_prefix(". GThemedIcon ") {
        Some(names) => names
            .split_whitespace()
            .next()
            .map(|name| Image::Name(name.to_owned())),
        None => Image::from_hint(text),
    }
}

/// The `icon-data` of a result: width, height, rowstride, has-alpha, bits
/// per sample, channels and the pixels. `None` for anything GdkPixbuf's
/// RGB format could not hold.
fn icon_pixels(value: &OwnedVariant) -> Option<Pixels> {
    let (width, height, rowstride, has_alpha, bits_per_sample, channels, data) =
        <(i32, i32, i32, bool, i32, i32, Vec<u8>)>::try_from(value.try_clone().ok()?).ok()?;
    let expected_channels = if has_alpha { 4 } else { 3 };
    if width <= 0 || height <= 0 || bits_per_sample != 8 || channels != expected_channels {
        return None;
    }
    // The last row may stop at its last pixel, short of the rowstride.
    let needed = i64::from(height - 1) * i64::from(rowstride) + i64::from(width * channels);
    if rowstride < width * channels || (data.len() as i64) < needed {
        return None;
    }
    Some(Pixels {
        width,
        height,
        rowstride,
        has_alpha,
        data,
    })
}

type Meta = HashMap<String, OwnedVariant>;

fn meta_text(meta: &Meta, key: &str) -> Option<String> {
    let text: &str = meta.get(key)?.downcast_ref().ok()?;
    Some(text.to_owned()).filter(|text| !text.is_empty())
}

/// `RemoteSearchProvider.createIcon`: the first of `icon`, `gicon` and
/// `icon-data` that is there decides.
fn meta_icon(meta: &Meta) -> Option<Image> {
    if let Some(icon) = meta.get("icon") {
        return serialized_icon(icon);
    }
    if let Some(text) = meta_text(meta, "gicon") {
        return icon_from_string(&text);
    }
    meta.get("icon-data")
        .and_then(icon_pixels)
        .map(|pixels| Image::Pixels(Arc::new(pixels)))
}

/// The results `metas` describe, one for each of `ids` in order
/// (`SearchResultsBase._ensureResultActors`). As in GNOME Shell an answer
/// with the wrong number of entries, or with an entry that has no name or
/// no id, is no answer at all.
pub fn results(ids: &[String], metas: &[Meta]) -> Result<Vec<ResultItem>, String> {
    if metas.len() != ids.len() {
        return Err(format!(
            "expected {} result metas but got {}",
            ids.len(),
            metas.len()
        ));
    }
    ids.iter()
        .zip(metas)
        .map(|(id, meta)| {
            let name = meta_text(meta, "name");
            match (name, meta_text(meta, "id")) {
                (Some(name), Some(_)) => Ok(ResultItem {
                    id: id.clone(),
                    name,
                    description: meta_text(meta, "description"),
                    icon: meta_icon(meta),
                }),
                _ => Err("a result meta without a name or an id".to_owned()),
            }
        })
        .collect()
}

/// Tasks that run side by side, in whatever order they finish.
struct Tasks<T> {
    running: Vec<Pin<Box<dyn Future<Output = T>>>>,
}

impl<T> Tasks<T> {
    fn new() -> Self {
        Self {
            running: Vec::new(),
        }
    }

    fn add(&mut self, task: impl Future<Output = T> + 'static) {
        self.running.push(Box::pin(task));
    }

    /// Drops every task where it stands.
    fn clear(&mut self) {
        self.running.clear();
    }

    /// What the next task to finish made. Waits for ever while there is
    /// none.
    async fn next(&mut self) -> T {
        std::future::poll_fn(|context| {
            for index in 0..self.running.len() {
                if let Poll::Ready(output) = self.running[index].as_mut().poll(context) {
                    drop(self.running.swap_remove(index));
                    return Poll::Ready(output);
                }
            }
            Poll::Pending
        })
        .await
    }
}

/// One alarm at a time, kept by a thread that sleeps: the bus thread has
/// no clock of its own to wait on.
struct Alarm {
    set: std::sync::mpsc::Sender<Option<(u64, Instant)>>,
    /// Yields the number an alarm was set with, once its time has come.
    rang: Receiver<u64>,
}

impl Alarm {
    fn start() -> Option<Self> {
        let (set, changes) = std::sync::mpsc::channel::<Option<(u64, Instant)>>();
        let (ring, rang) = async_channel::unbounded();
        let spawned = std::thread::Builder::new()
            .name("search-alarm".to_owned())
            .spawn(move || {
                let mut pending: Option<(u64, Instant)> = None;
                loop {
                    let change = match pending {
                        Some((number, at)) => {
                            let left = at.saturating_duration_since(Instant::now());
                            match changes.recv_timeout(left) {
                                Ok(change) => change,
                                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                                    if ring.send_blocking(number).is_err() {
                                        return;
                                    }
                                    None
                                }
                                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                            }
                        }
                        None => match changes.recv() {
                            Ok(change) => change,
                            Err(_) => return,
                        },
                    };
                    pending = change;
                }
            });
        match spawned {
            Ok(_) => Some(Self { set, rang }),
            Err(error) => {
                eprintln!("gniri: search: no timeouts, cannot start their thread: {error}");
                None
            }
        }
    }

    /// Replaces whatever alarm is set.
    fn set(&self, number: u64, after: Duration) {
        let _ = self.set.send(Some((number, Instant::now() + after)));
    }

    fn cancel(&self) {
        let _ = self.set.send(None);
    }
}

/// What a provider said to one search.
struct Answer {
    provider: String,
    results: Vec<ResultItem>,
    total: usize,
}

/// A bus error that only says the provider is not there to be asked.
fn absent(error: &zbus::Error) -> bool {
    matches!(
        error,
        zbus::Error::MethodError(name, _, _) if matches!(
            name.as_str(),
            "org.freedesktop.DBus.Error.ServiceUnknown" | "org.freedesktop.DBus.Error.NameHasNoOwner"
        )
    )
}

async fn has_owner(connection: &zbus::Connection, bus_name: &str) -> zbus::Result<bool> {
    connection
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "NameHasOwner",
            &(bus_name),
        )
        .await?
        .body()
        .deserialize()
}

/// Whether `provider` may be called now: always, unless it does not want
/// to be started and is not running.
async fn callable(connection: &zbus::Connection, provider: &ProviderSpec) -> zbus::Result<bool> {
    if provider.auto_start {
        return Ok(true);
    }
    has_owner(connection, &provider.bus_name).await
}

/// Asks one provider: what it found, and what the first few of those are.
async fn ask(
    connection: &zbus::Connection,
    provider: &ProviderSpec,
    terms: &[String],
) -> zbus::Result<(Vec<ResultItem>, usize)> {
    if !callable(connection, provider).await? {
        return Ok((Vec::new(), 0));
    }
    let call = |method: &'static str, argument: Vec<String>| async move {
        connection
            .call_method(
                Some(provider.bus_name.as_str()),
                provider.object_path.as_str(),
                Some(INTERFACE),
                method,
                &(argument),
            )
            .await
    };
    let found: Vec<String> = call("GetInitialResultSet", terms.to_vec())
        .await?
        .body()
        .deserialize()?;
    let shown = filter_results(&found, MAX_RESULTS);
    if shown.is_empty() {
        return Ok((Vec::new(), 0));
    }
    let metas: Vec<Meta> = call("GetResultMetas", shown.clone())
        .await?
        .body()
        .deserialize()?;
    let results = results(&shown, &metas).map_err(zbus::Error::Failure)?;
    Ok((results, found.len()))
}

/// A provider's answer to a search; a failure is an answer of nothing.
async fn answer(
    connection: zbus::Connection,
    provider: ProviderSpec,
    terms: Vec<String>,
) -> Answer {
    let (results, total) = match ask(&connection, &provider, &terms).await {
        Ok(found) => found,
        Err(error) => {
            if !absent(&error) {
                eprintln!("gniri: search: {}: {error}", provider.info.id);
            }
            (Vec::new(), 0)
        }
    };
    Answer {
        provider: provider.info.id,
        results,
        total,
    }
}

/// Calls a method nobody waits for the reply of: opening a result.
async fn tell<B>(
    connection: zbus::Connection,
    provider: ProviderSpec,
    method: &'static str,
    body: B,
) where
    B: serde::Serialize + zbus::DynamicType,
{
    let called = async {
        if !callable(&connection, &provider).await? {
            return Ok(());
        }
        connection
            .call_method(
                Some(provider.bus_name.as_str()),
                provider.object_path.as_str(),
                Some(INTERFACE),
                method,
                &body,
            )
            .await
            .map(drop)
    };
    let result: zbus::Result<()> = called.await;
    if let Err(error) = result {
        eprintln!("gniri: search: {}: {method}: {error}", provider.info.id);
    }
}

enum Wake {
    Command(Command),
    /// The shell went away.
    Closed,
    Answer(Answer),
    /// The alarm set for the search of this number rang.
    Late(u64),
    /// A call nobody waited for is through.
    Told,
}

/// Announces `providers` and answers the shell's commands with them, until
/// the shell's ends of the channels close. A provider that has not answered
/// a search after `timeout` is taken to have found nothing.
pub async fn serve(
    connection: zbus::Connection,
    events: Sender<Event>,
    commands: Receiver<Command>,
    providers: Vec<ProviderSpec>,
    timeout: Duration,
) -> zbus::Result<()> {
    let infos = providers
        .iter()
        .map(|provider| provider.info.clone())
        .collect();
    if events.send(Event::Providers(infos)).await.is_err() {
        return Ok(());
    }

    let alarm = Alarm::start();
    // The search that is running: its number, its terms, the tasks asking
    // each provider, and the providers that have yet to answer.
    let mut number = 0;
    let mut terms: Vec<String> = Vec::new();
    let mut asking: Tasks<Answer> = Tasks::new();
    let mut awaited: Vec<String> = Vec::new();
    let mut telling: Tasks<()> = Tasks::new();

    loop {
        let wake = or(
            or(
                async { commands.recv().await.map_or(Wake::Closed, Wake::Command) },
                async {
                    match &alarm {
                        Some(alarm) => match alarm.rang.recv().await {
                            Ok(number) => Wake::Late(number),
                            // The thread is gone; there will be no alarms.
                            Err(_) => std::future::pending().await,
                        },
                        None => std::future::pending().await,
                    }
                },
            ),
            or(async { Wake::Answer(asking.next().await) }, async {
                telling.next().await;
                Wake::Told
            }),
        )
        .await;

        let finished = |awaited: &[String]| {
            if awaited.is_empty()
                && let Some(alarm) = &alarm
            {
                alarm.cancel();
            }
        };
        match wake {
            Wake::Closed => return Ok(()),
            Wake::Told => {}
            Wake::Command(Command::Search(new_terms)) => {
                // Whatever the last search still waits for is of no use.
                asking.clear();
                awaited.clear();
                number += 1;
                terms = new_terms;
                if terms.is_empty() {
                    finished(&awaited);
                    continue;
                }
                let started = Event::Started {
                    terms: terms.clone(),
                };
                if events.send(started).await.is_err() {
                    return Ok(());
                }
                for provider in &providers {
                    awaited.push(provider.info.id.clone());
                    asking.add(answer(connection.clone(), provider.clone(), terms.clone()));
                }
                match &alarm {
                    Some(alarm) if !awaited.is_empty() => alarm.set(number, timeout),
                    _ => {}
                }
            }
            Wake::Command(Command::Activate {
                provider,
                result,
                terms,
            }) => {
                if let Some(provider) = providers.iter().find(|other| other.info.id == provider) {
                    telling.add(tell(
                        connection.clone(),
                        provider.clone(),
                        "ActivateResult",
                        (result, terms, 0u32),
                    ));
                }
            }
            Wake::Command(Command::LaunchSearch { provider, terms }) => {
                if let Some(provider) = providers.iter().find(|other| other.info.id == provider) {
                    telling.add(tell(
                        connection.clone(),
                        provider.clone(),
                        "LaunchSearch",
                        (terms, 0u32),
                    ));
                }
            }
            Wake::Answer(answer) => {
                awaited.retain(|provider| *provider != answer.provider);
                finished(&awaited);
                let event = Event::Results {
                    terms: terms.clone(),
                    provider: answer.provider,
                    results: answer.results,
                    total: answer.total,
                };
                if events.send(event).await.is_err() {
                    return Ok(());
                }
            }
            Wake::Late(late) => {
                // An alarm of a search that has been replaced since.
                if late != number {
                    continue;
                }
                asking.clear();
                for provider in std::mem::take(&mut awaited) {
                    eprintln!("gniri: search: {provider} did not answer in {timeout:?}");
                    let event = Event::Results {
                        terms: terms.clone(),
                        provider,
                        results: Vec::new(),
                        total: 0,
                    };
                    if events.send(event).await.is_err() {
                        return Ok(());
                    }
                }
            }
        }
    }
}

/// Runs until the shell's ends of the channels close.
pub async fn run(
    connection: zbus::Connection,
    events: Sender<Event>,
    commands: Receiver<Command>,
    options: Options,
) -> zbus::Result<()> {
    let providers = load_providers(
        &provider_dirs(),
        DesktopEntry::find,
        &desktop_entry::current_desktops(),
        &options,
    );
    serve(connection, events, commands, providers, PROVIDER_TIMEOUT).await
}

#[cfg(test)]
mod tests {
    use zbus::{Array, Structure};

    use super::*;

    const FILES: &str = "\
[Shell Search Provider]
DesktopId=org.gnome.Nautilus.desktop
BusName=org.gnome.Nautilus
ObjectPath=/org/gnome/Nautilus/SearchProvider
Version=2
";

    fn entry(id: &str, name: &str) -> DesktopEntry {
        DesktopEntry {
            id: id.to_owned(),
            name: name.to_owned(),
            icon: Some(id.to_owned()),
            ..DesktopEntry::default()
        }
    }

    /// Entries for every id but "org.example.Missing"; "org.example.Hidden"
    /// is not for menus.
    fn find(id: &str) -> Option<DesktopEntry> {
        let id = bare_id(id);
        let name = match id {
            "org.example.Missing" => return None,
            "org.gnome.Nautilus" => "Files",
            "org.gnome.Settings" => "Settings",
            other => other.rsplit('.').next().unwrap_or(other),
        };
        Some(DesktopEntry {
            no_display: id == "org.example.Hidden",
            ..entry(id, name)
        })
    }

    fn key_file(id: &str, extra: &str) -> String {
        format!(
            "[Shell Search Provider]\nDesktopId={id}.desktop\nBusName={id}.Search\n\
             ObjectPath=/search/{}\nVersion=2\n{extra}",
            id.replace('.', "_")
        )
    }

    #[test]
    fn reads_a_provider_from_its_key_file() {
        let niri = ["niri".to_owned()];
        let files = provider(FILES, &find, &niri).unwrap();
        assert_eq!(
            files,
            ProviderSpec {
                info: ProviderInfo {
                    id: "org.gnome.Nautilus".to_owned(),
                    name: "Files".to_owned(),
                    icon: Some("org.gnome.Nautilus".to_owned()),
                },
                bus_name: "org.gnome.Nautilus".to_owned(),
                object_path: "/org/gnome/Nautilus/SearchProvider".to_owned(),
                auto_start: true,
            }
        );

        let quiet = provider(
            &format!(
                "# A comment\n[Other]\nVersion=1\n\n{FILES}AutoStart=false\n[More]\nBusName=x\n"
            ),
            &find,
            &niri,
        )
        .unwrap();
        assert!(!quiet.auto_start);
        assert_eq!(quiet.bus_name, "org.gnome.Nautilus");
    }

    #[test]
    fn leaves_out_what_is_no_usable_provider() {
        let niri = ["niri".to_owned()];
        let read = |text: &str| provider(text, &find, &niri);
        assert_eq!(read("[Desktop Entry]\nName=Not a provider"), None);
        assert_eq!(read(&FILES.replace("Version=2\n", "")), None, "version 1");
        assert_eq!(read(&FILES.replace("Version=2", "Version=1")), None);
        assert_eq!(read(&format!("{FILES}DefaultDisabled=true\n")), None);
        assert!(read(&format!("{FILES}DefaultDisabled=false\n")).is_some());
        assert_eq!(read(&FILES.replace("BusName=", "Bus=")), None);
        assert_eq!(read(&FILES.replace("ObjectPath=/", "ObjectPath=")), None);
        assert_eq!(read(&FILES.replace("DesktopId=", "Desktop=")), None);
        assert_eq!(read(&key_file("org.example.Missing", "")), None);
        assert_eq!(read(&key_file("org.example.Hidden", "")), None);
        assert!(read(&key_file("org.example.Shown", "")).is_some());
    }

    #[test]
    fn providers_are_ordered_as_gnome_orders_them() {
        let root = std::env::temp_dir().join(format!("gniri-providers-{}", std::process::id()));
        let (first, second) = (root.join("first"), root.join("second"));
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        let write =
            |dir: &Path, file: &str, text: &str| std::fs::write(dir.join(file), text).unwrap();
        write(&first, "zebra.ini", &key_file("org.example.Zebra", ""));
        write(&first, "files.ini", FILES);
        write(&first, "alpha.ini", &key_file("org.example.alpha", ""));
        write(&first, "off.ini", &key_file("org.example.Off", ""));
        write(&first, "notes.txt", &key_file("org.example.Text", ""));
        write(&second, "settings.ini", &key_file("org.gnome.Settings", ""));
        // The same object again, under another application's name.
        write(
            &second,
            "copy.ini",
            &FILES.replace("org.gnome.Nautilus.desktop", "org.example.Copy.desktop"),
        );
        write(&second, "beta.ini", &key_file("org.example.Beta", ""));

        let options = Options {
            disabled: vec!["org.example.Off.desktop".to_owned()],
        };
        let dirs = [first, root.join("not-there"), second];
        let providers = load_providers(&dirs, find, &["niri".to_owned()], &options);
        let ids: Vec<&str> = providers
            .iter()
            .map(|provider| provider.info.id.as_str())
            .collect();
        assert_eq!(
            ids,
            [
                "org.gnome.Settings",
                "org.gnome.Nautilus",
                "org.example.alpha",
                "org.example.Beta",
                "org.example.Zebra",
            ]
        );

        let all = load_providers(&dirs, find, &["niri".to_owned()], &Options::default());
        assert!(
            all.iter()
                .any(|provider| provider.info.id == "org.example.Off")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    fn themed(names: &[&str]) -> Variant<'static> {
        let names: Vec<String> = names.iter().map(|&name| name.to_owned()).collect();
        Variant::from(Structure::from((
            "themed".to_owned(),
            Variant::from(Array::from(names)),
        )))
    }

    fn owned(value: Variant<'static>) -> OwnedVariant {
        value.try_into().unwrap()
    }

    fn meta(entries: Vec<(&str, Variant<'static>)>) -> Meta {
        entries
            .into_iter()
            .map(|(key, value)| (key.to_owned(), owned(value)))
            .collect()
    }

    #[test]
    fn decodes_serialised_icons() {
        let name = |name: &str| Some(Image::Name(name.to_owned()));
        assert_eq!(
            serialized_icon(&themed(&["folder-symbolic", "folder"])),
            name("folder-symbolic")
        );
        // Still wrapped in the dictionary's variant.
        assert_eq!(
            serialized_icon(&Variant::Variant(Box::new(themed(&["folder"])))),
            name("folder")
        );
        assert_eq!(serialized_icon(&themed(&[])), None);
        let file = Variant::from(Structure::from((
            "file".to_owned(),
            Variant::from("file:///tmp/a%20picture.png".to_owned()),
        )));
        assert_eq!(
            serialized_icon(&file),
            Some(Image::File("/tmp/a picture.png".into()))
        );
        let bytes = Variant::from(Structure::from((
            "bytes".to_owned(),
            Variant::from(Array::from(vec![1u8, 2, 3])),
        )));
        assert_eq!(serialized_icon(&bytes), None);
        assert_eq!(serialized_icon(&Variant::from("folder".to_owned())), None);
    }

    #[test]
    fn decodes_icons_written_as_text() {
        let name = |name: &str| Some(Image::Name(name.to_owned()));
        assert_eq!(
            icon_from_string("org.gnome.Software"),
            name("org.gnome.Software")
        );
        assert_eq!(
            icon_from_string(". GThemedIcon text-x-generic text-x-generic-symbolic"),
            name("text-x-generic")
        );
        assert_eq!(icon_from_string(". GThemedIcon "), None);
        assert_eq!(
            icon_from_string("/usr/share/icons/a.png"),
            Some(Image::File("/usr/share/icons/a.png".into()))
        );
        assert_eq!(
            icon_from_string("file:///tmp/b.png"),
            Some(Image::File("/tmp/b.png".into()))
        );
        assert_eq!(icon_from_string(""), None);
    }

    fn icon_data(width: i32, bits: i32, data: Vec<u8>) -> Variant<'static> {
        Variant::from(Structure::from((
            width,
            1i32,
            width * 4,
            true,
            bits,
            4i32,
            data,
        )))
    }

    #[test]
    fn the_first_kind_of_icon_a_result_has_decides() {
        let pixels = meta(vec![("icon-data", icon_data(2, 8, vec![0; 8]))]);
        let Some(Image::Pixels(decoded)) = meta_icon(&pixels) else {
            panic!("no pixels from {pixels:?}");
        };
        assert_eq!(
            (decoded.width, decoded.height, decoded.rowstride),
            (2, 1, 8)
        );
        assert!(decoded.has_alpha);

        assert_eq!(
            meta_icon(&meta(vec![("icon-data", icon_data(2, 16, vec![0; 8]))])),
            None,
            "16 bits per sample"
        );
        assert_eq!(
            meta_icon(&meta(vec![("icon-data", icon_data(2, 8, vec![0; 7]))])),
            None,
            "too few bytes"
        );

        let all = meta(vec![
            ("icon", themed(&["first"])),
            ("gicon", Variant::from("second".to_owned())),
            ("icon-data", icon_data(2, 8, vec![0; 8])),
        ]);
        assert_eq!(meta_icon(&all), Some(Image::Name("first".to_owned())));
        let two = meta(vec![
            ("gicon", Variant::from("second".to_owned())),
            ("icon-data", icon_data(2, 8, vec![0; 8])),
        ]);
        assert_eq!(meta_icon(&two), Some(Image::Name("second".to_owned())));
        assert_eq!(meta_icon(&Meta::new()), None);
    }

    #[test]
    fn results_follow_the_ids_and_a_faulty_answer_gives_none() {
        let text = |text: &str| Variant::from(text.to_owned());
        let ids = ["a".to_owned(), "b".to_owned()];
        let metas = [
            meta(vec![
                ("id", text("a")),
                ("name", text("First")),
                ("description", text("Of two")),
                ("gicon", text("folder")),
            ]),
            meta(vec![
                ("id", text("b")),
                ("name", text("Second")),
                ("description", text("")),
            ]),
        ];
        assert_eq!(
            results(&ids, &metas).unwrap(),
            [
                ResultItem {
                    id: "a".to_owned(),
                    name: "First".to_owned(),
                    description: Some("Of two".to_owned()),
                    icon: Some(Image::Name("folder".to_owned())),
                },
                ResultItem {
                    id: "b".to_owned(),
                    name: "Second".to_owned(),
                    description: None,
                    icon: None,
                },
            ]
        );

        assert!(results(&ids, &metas[..1]).is_err(), "one meta short");
        let nameless = [metas[0].clone(), meta(vec![("id", text("b"))])];
        assert!(results(&ids, &nameless).is_err());
        let without_id = [metas[0].clone(), meta(vec![("name", text("Second"))])];
        assert!(results(&ids, &without_id).is_err());
        assert_eq!(results(&[], &[]).unwrap(), []);
    }

    #[test]
    fn tasks_finish_in_their_own_order_and_can_be_dropped() {
        futures_lite::future::block_on(async {
            let (first, wait_first) = async_channel::bounded::<()>(1);
            let (second, wait_second) = async_channel::bounded::<()>(1);
            let mut tasks: Tasks<u8> = Tasks::new();
            tasks.add(async move {
                let _ = wait_first.recv().await;
                1
            });
            tasks.add(async move {
                let _ = wait_second.recv().await;
                2
            });
            second.send(()).await.unwrap();
            assert_eq!(tasks.next().await, 2);
            first.send(()).await.unwrap();
            assert_eq!(tasks.next().await, 1);

            // With none left, or all dropped, `next` waits for ever.
            tasks.add(std::future::pending());
            tasks.clear();
            let waited = or(async { Some(tasks.next().await) }, async { None }).await;
            assert_eq!(waited, None);
        });
    }

    #[test]
    fn the_alarm_rings_once_for_the_last_time_set() {
        let alarm = Alarm::start().unwrap();
        alarm.set(1, Duration::from_secs(60));
        alarm.set(2, Duration::from_millis(20));
        assert_eq!(alarm.rang.recv_blocking().unwrap(), 2);
        alarm.set(3, Duration::from_millis(20));
        alarm.cancel();
        alarm.set(4, Duration::from_millis(40));
        assert_eq!(alarm.rang.recv_blocking().unwrap(), 4);
        assert!(alarm.rang.is_empty());
    }
}
