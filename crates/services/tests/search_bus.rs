//! The search backend against python-dbusmock objects that stand in for
//! search providers, with their key files and desktop entries in a
//! directory of the test's own.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::{Follower, MockBus, TIMEOUT};
use gniri_services::desktop_entry::DesktopEntry;
use gniri_services::search::backend::{self, Options, PROVIDER_TIMEOUT, ProviderSpec};
use gniri_services::search::{self, Command, Event, Image};
use gniri_services::{Link, Model};

const INTERFACE: &str = "org.gnome.Shell.SearchProvider2";
const MOCK: &str = "org.freedesktop.DBus.Mock";

/// Seven results named after the first term; a first term of "slow" takes
/// its time, as a provider that is only just starting does.
const INITIAL_RESULTS: &str = "\
import time
if args[0][0] == 'slow':
    time.sleep(1.5)
ret = ['%s-%d' % (args[0][0], number) for number in range(7)]
";

/// What a well-behaved provider says of each result, the icon as GIO
/// serialises a themed one: `('themed', <['name', ...]>)`. The names carry
/// their variant themselves, since dbus-python works a structure's type
/// out from what is in it.
const RESULT_METAS: &str = "\
ret = [{
    'id': name,
    'name': 'Result ' + name,
    'description': 'About ' + name,
    'icon': dbus.Struct((
        'themed',
        dbus.Array(['folder-symbolic', 'folder'], signature='s', variant_level=1),
    )),
} for name in args[0]]
";

/// A directory of the test's own with the providers' key files and their
/// applications' desktop entries.
struct Installed {
    root: PathBuf,
}

impl Installed {
    fn new(test: &str) -> Self {
        let root = std::env::temp_dir().join(format!("gniri-search-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("providers")).unwrap();
        std::fs::create_dir_all(root.join("applications")).unwrap();
        Self { root }
    }

    /// Installs an application called `name` with the desktop entry `id`
    /// and its provider's key file, to which `extra` is added.
    fn provider(&self, id: &str, name: &str, extra: &str) {
        std::fs::write(
            self.root.join(format!("applications/{id}.desktop")),
            format!("[Desktop Entry]\nType=Application\nName={name}\nIcon={id}\nExec=true\n"),
        )
        .unwrap();
        std::fs::write(
            self.root.join(format!("providers/{id}.ini")),
            format!(
                "[Shell Search Provider]\nDesktopId={id}.desktop\nBusName={}\n\
                 ObjectPath={}\nVersion=2\n{extra}",
                bus_name(id),
                object_path(id),
            ),
        )
        .unwrap();
    }

    /// What the backend finds installed.
    fn load(&self, options: &Options) -> Vec<ProviderSpec> {
        let applications = [self.root.join("applications")];
        backend::load_providers(
            &[self.root.join("providers")],
            |id| DesktopEntry::find_in(id, &applications),
            &["niri".to_owned()],
            options,
        )
    }
}

impl Drop for Installed {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn bus_name(id: &str) -> String {
    format!("{id}.SearchProvider")
}

fn object_path(id: &str) -> String {
    format!("/{}/SearchProvider", id.replace('.', "/"))
}

/// Puts a provider for the application `id` on the bus.
fn add_provider(bus: &mut MockBus, id: &str, initial_results: &str) {
    let (name, path) = (bus_name(id), object_path(id));
    bus.object(&name, &path, INTERFACE);
    let methods = [
        ("GetInitialResultSet", "as", "as", initial_results),
        ("GetResultMetas", "as", "aa{sv}", RESULT_METAS),
        ("ActivateResult", "sasu", "", ""),
        ("LaunchSearch", "asu", "", ""),
    ];
    for (method, input, output, code) in methods {
        bus.call(
            &name,
            &path,
            MOCK,
            "AddMethod",
            &(INTERFACE, method, input, output, code),
        );
    }
}

/// Runs the backend with `providers` on the test's bus.
fn serve(
    bus: &MockBus,
    providers: Vec<ProviderSpec>,
    timeout: Duration,
) -> Follower<search::State> {
    let link: Link<search::State> = bus.run(move |connection, events, commands| {
        backend::serve(connection, events, commands, providers, timeout)
    });
    Follower::new(link)
}

fn terms(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_owned).collect()
}

/// Searches as the shell does: the state hears of the command first.
fn search_for(follower: &mut Follower<search::State>, text: &str) {
    let command = Command::Search(terms(text));
    follower.state.predict(&command);
    follower.send(command);
}

fn provider<'a>(state: &'a search::State, id: &str) -> &'a search::Provider {
    state
        .providers
        .iter()
        .find(|provider| provider.id == id)
        .unwrap_or_else(|| panic!("no provider {id} in {state:#?}"))
}

/// The arguments of every call of `method` the mock for `id` has had.
fn calls(bus: &MockBus, id: &str, method: &str) -> Vec<Vec<zbus::OwnedVariant>> {
    let calls: Vec<(u64, Vec<zbus::OwnedVariant>)> = bus
        .call(
            &bus_name(id),
            &object_path(id),
            MOCK,
            "GetMethodCalls",
            &(method),
        )
        .body()
        .deserialize()
        .expect("the calls so far");
    calls.into_iter().map(|(_, arguments)| arguments).collect()
}

/// Waits until the mock for `id` has been called `method`, and returns the
/// first call's arguments.
fn wait_for_call(bus: &MockBus, id: &str, method: &str) -> Vec<zbus::OwnedVariant> {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some(arguments) = calls(bus, id, method).into_iter().next() {
            return arguments;
        }
        assert!(Instant::now() < deadline, "{method} was never called");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn strings(value: &zbus::OwnedVariant) -> Vec<String> {
    value
        .try_clone()
        .unwrap()
        .try_into()
        .expect("a list of strings")
}

#[test]
fn providers_are_announced_in_gnomes_order() {
    // Nothing is asked: the bus alone will do.
    let Some(bus) = MockBus::start_empty() else {
        return;
    };
    let installed = Installed::new("order");
    installed.provider("org.example.Zebra", "Zebra", "");
    installed.provider("org.gnome.Nautilus", "Files", "");
    installed.provider("org.example.Notes", "Alpha Notes", "");
    installed.provider("org.example.Off", "Off", "");
    installed.provider("org.example.Later", "Later", "DefaultDisabled=true\n");
    let options = Options {
        disabled: vec!["org.example.Off".to_owned()],
    };

    let mut search = serve(&bus, installed.load(&options), PROVIDER_TIMEOUT);
    search.wait("the providers", |state| !state.providers.is_empty());
    let announced: Vec<(&str, &str, Option<&str>)> = search
        .state
        .providers
        .iter()
        .map(|provider| {
            (
                provider.id.as_str(),
                provider.name.as_str(),
                provider.icon.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        announced,
        [
            ("org.gnome.Nautilus", "Files", Some("org.gnome.Nautilus")),
            (
                "org.example.Notes",
                "Alpha Notes",
                Some("org.example.Notes")
            ),
            ("org.example.Zebra", "Zebra", Some("org.example.Zebra")),
        ]
    );
    assert!(!search.state.searching());
}

#[test]
fn a_search_is_answered_with_the_first_results_and_their_number() {
    let Some(mut bus) = MockBus::start() else {
        return;
    };
    const FILES: &str = "org.example.Files";
    let installed = Installed::new("answer");
    installed.provider(FILES, "Files", "");
    add_provider(&mut bus, FILES, INITIAL_RESULTS);

    let mut search = serve(&bus, installed.load(&Options::default()), PROVIDER_TIMEOUT);
    search.wait("the provider", |state| state.providers.len() == 1);
    search_for(&mut search, "calc ulator");
    search.wait("the search to start", search::State::searching);
    assert_eq!(search.state.shown().count(), 0);

    search.wait("the answer", |state| !state.searching());
    let files = provider(&search.state, FILES);
    assert_eq!(files.total, 7);
    assert_eq!(files.more(), 2);
    let names: Vec<&str> = files
        .results
        .iter()
        .map(|result| result.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "Result calc-0",
            "Result calc-1",
            "Result calc-2",
            "Result calc-3",
            "Result calc-4",
        ]
    );
    let first = &files.results[0];
    assert_eq!(first.id, "calc-0");
    assert_eq!(first.description.as_deref(), Some("About calc-0"));
    assert_eq!(first.icon, Some(Image::Name("folder-symbolic".to_owned())));

    // The provider was handed every term, and asked about five results.
    let asked = wait_for_call(&bus, FILES, "GetInitialResultSet");
    assert_eq!(strings(&asked[0]), ["calc", "ulator"]);
    let described = wait_for_call(&bus, FILES, "GetResultMetas");
    assert_eq!(strings(&described[0]).len(), 5);

    // Ending the search asks nobody anything.
    search_for(&mut search, "");
    assert_eq!(search.state.shown().count(), 0);
    std::thread::sleep(Duration::from_millis(200));
    assert!(search.link.events.is_empty());
    assert_eq!(calls(&bus, FILES, "GetInitialResultSet").len(), 1);
}

#[test]
fn a_new_search_replaces_one_that_is_still_being_answered() {
    let Some(mut bus) = MockBus::start() else {
        return;
    };
    const FILES: &str = "org.example.Files";
    let installed = Installed::new("replace");
    installed.provider(FILES, "Files", "");
    add_provider(&mut bus, FILES, INITIAL_RESULTS);

    let mut search = serve(&bus, installed.load(&Options::default()), PROVIDER_TIMEOUT);
    search.wait("the provider", |state| state.providers.len() == 1);
    search_for(&mut search, "slow");
    search.wait("the first search to start", search::State::searching);

    // The provider now sits on the first question for a second and a
    // half. The backend must hear the second one all the same.
    let asked = Instant::now();
    search_for(&mut search, "quick");
    let next = search
        .link
        .events
        .recv_blocking()
        .expect("the backend is running");
    match &next {
        Event::Started { terms } => assert_eq!(terms, &["quick"]),
        other => panic!("expected the new search to start, got {other:?}"),
    }
    assert!(
        asked.elapsed() < Duration::from_secs(1),
        "the backend waited for the first answer: {:?}",
        asked.elapsed()
    );
    search.state.apply(next);

    // Only answers to the new search follow.
    let deadline = Instant::now() + TIMEOUT;
    while search.state.searching() {
        assert!(Instant::now() < deadline, "the second search never ended");
        let event = search
            .link
            .events
            .recv_blocking()
            .expect("the backend is running");
        match &event {
            Event::Results { terms, .. } => assert_eq!(terms, &["quick"]),
            other => panic!("unexpected {other:?}"),
        }
        search.state.apply(event);
    }
    let files = provider(&search.state, FILES);
    assert_eq!(files.results[0].id, "quick-0");
    assert_eq!(search.state.terms, ["quick"]);
}

#[test]
fn opening_a_result_and_the_provider_reach_the_application() {
    let Some(mut bus) = MockBus::start() else {
        return;
    };
    const FILES: &str = "org.example.Files";
    let installed = Installed::new("open");
    installed.provider(FILES, "Files", "");
    add_provider(&mut bus, FILES, INITIAL_RESULTS);

    let search = serve(&bus, installed.load(&Options::default()), PROVIDER_TIMEOUT);
    search.send(Command::Activate {
        provider: FILES.to_owned(),
        result: "calc-3".to_owned(),
        terms: terms("calc ulator"),
    });
    search.send(Command::LaunchSearch {
        provider: FILES.to_owned(),
        terms: terms("calc"),
    });
    // A provider nobody knows is asked nothing, and breaks nothing.
    search.send(Command::LaunchSearch {
        provider: "org.example.Unknown".to_owned(),
        terms: terms("calc"),
    });

    let activated = wait_for_call(&bus, FILES, "ActivateResult");
    let result: String = activated[0].try_clone().unwrap().try_into().unwrap();
    assert_eq!(result, "calc-3");
    assert_eq!(strings(&activated[1]), ["calc", "ulator"]);
    let timestamp: u32 = activated[2].try_clone().unwrap().try_into().unwrap();
    assert_eq!(timestamp, 0);

    let launched = wait_for_call(&bus, FILES, "LaunchSearch");
    assert_eq!(strings(&launched[0]), ["calc"]);
    assert_eq!(calls(&bus, FILES, "LaunchSearch").len(), 1);
}

#[test]
fn a_provider_that_is_not_there_has_found_nothing() {
    let Some(mut bus) = MockBus::start() else {
        return;
    };
    const FILES: &str = "org.example.Files";
    const ABSENT: &str = "org.example.Absent";
    const ASLEEP: &str = "org.example.Asleep";
    let installed = Installed::new("absent");
    installed.provider(FILES, "Files", "");
    // Nobody owns its bus name, and the bus cannot start it.
    installed.provider(ABSENT, "Absent", "");
    // It does not want to be started, and does not run.
    installed.provider(ASLEEP, "Asleep", "AutoStart=false\n");
    add_provider(&mut bus, FILES, INITIAL_RESULTS);

    let providers = installed.load(&Options::default());
    assert!(
        providers
            .iter()
            .all(|provider| provider.auto_start == (provider.info.id != ASLEEP))
    );
    let mut search = serve(&bus, providers, PROVIDER_TIMEOUT);
    search.wait("the providers", |state| state.providers.len() == 3);
    let asked = Instant::now();
    search_for(&mut search, "calc");
    search.wait("the search to start", search::State::searching);
    search.wait("every answer", |state| !state.searching());
    assert!(
        asked.elapsed() < PROVIDER_TIMEOUT / 2,
        "the answers took a timeout to come: {:?}",
        asked.elapsed()
    );
    let shown: Vec<&str> = search
        .state
        .shown()
        .map(|provider| provider.id.as_str())
        .collect();
    assert_eq!(shown, [FILES]);
    assert_eq!(provider(&search.state, ABSENT).total, 0);

    // A provider that does not want to be started is not told to open
    // anything either; one that runs is.
    search.send(Command::LaunchSearch {
        provider: ASLEEP.to_owned(),
        terms: terms("calc"),
    });
    search.send(Command::LaunchSearch {
        provider: FILES.to_owned(),
        terms: terms("calc"),
    });
    wait_for_call(&bus, FILES, "LaunchSearch");
}

#[test]
fn a_provider_that_never_answers_is_given_up_on() {
    let Some(mut bus) = MockBus::start() else {
        return;
    };
    const FILES: &str = "org.example.Files";
    let installed = Installed::new("timeout");
    installed.provider(FILES, "Files", "");
    add_provider(&mut bus, FILES, INITIAL_RESULTS);

    let providers = installed.load(&Options::default());
    let mut search = serve(&bus, providers, Duration::from_millis(300));
    search.wait("the provider", |state| state.providers.len() == 1);
    let asked = Instant::now();
    search_for(&mut search, "slow");
    search.wait("the search to start", search::State::searching);
    search.wait("the backend to give up", |state| !state.searching());
    assert!(
        asked.elapsed() < Duration::from_millis(1200),
        "gave up only when the answer came: {:?}",
        asked.elapsed()
    );
    assert_eq!(search.state.shown().count(), 0);

    // The answer that comes after all is not passed on, and once the
    // provider is done with it the next search is answered as usual.
    std::thread::sleep(Duration::from_millis(1700).saturating_sub(asked.elapsed()));
    assert!(search.link.events.is_empty());
    search_for(&mut search, "quick");
    search.wait("the next search to start", search::State::searching);
    search.wait("its answer", |state| !state.searching());
    assert_eq!(provider(&search.state, FILES).results[0].id, "quick-0");
}
