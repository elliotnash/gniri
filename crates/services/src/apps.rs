//! Installed applications: what the dash, the app grid and the search
//! show. Port of the parts of `ShellAppSystem`, `ShellWindowTracker` and
//! GIO's desktop file index that the overview uses.
//!
//! The list comes from the desktop entries ([`crate::desktop_entry`]).
//! Which applications run is not kept here: the compositor says which
//! windows are open, and [`State::dash`] puts the two together.

pub mod backend;

use serde::Deserialize;

use crate::desktop_entry::{self, DesktopEntry};
use crate::model::Model;
use crate::niri::Window;

/// One installed application.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct App {
    /// Its desktop entry's name without ".desktop".
    pub id: String,
    pub name: String,
    /// An icon name, or the path of an image file.
    pub icon: Option<String>,
    /// The entry's `Exec` key as written.
    pub exec: Option<String>,
    pub generic_name: Option<String>,
    pub comment: Option<String>,
    pub keywords: Vec<String>,
    /// The application id of its windows, where that is not `id`.
    pub startup_wm_class: Option<String>,
    /// The command wants a terminal to run in.
    pub terminal: bool,
}

impl From<DesktopEntry> for App {
    fn from(entry: DesktopEntry) -> Self {
        Self {
            id: entry.id,
            name: entry.name,
            icon: entry.icon,
            exec: entry.exec,
            generic_name: entry.generic_name,
            comment: entry.comment,
            keywords: entry.keywords,
            startup_wm_class: entry.startup_wm_class,
            terminal: entry.terminal,
        }
    }
}

impl App {
    /// The command that starts the application.
    pub fn command(&self) -> Option<Vec<String>> {
        desktop_entry::split_exec(self.exec.as_deref()?, &self.name)
    }

    /// The program the command runs, without its directory: what a search
    /// for "nautilus" finds Files by.
    fn program(&self) -> Option<String> {
        let command = self.command()?;
        // `env VAR=value program` and the like start with something that
        // is not the program.
        let mut words = command.iter().map(String::as_str);
        let mut word = words.next()?;
        if word.rsplit('/').next() == Some("env") {
            word = words.find(|word| !word.contains('=') && !word.starts_with('-'))?;
        }
        word.rsplit('/').next().map(str::to_owned)
    }
}

/// Where in an entry a search term was found, best first
/// (`desktop_key_match_category` in GIO's gdesktopappinfo.c).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Field {
    Name,
    Exec,
    Keywords,
    GenericName,
    Comment,
}

/// How well an application matches a search: how its worst term matched.
/// A term at the start of a word beats one inside a word, whatever the
/// field (GIO 2.80's match types).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Rank {
    inside_a_word: bool,
    field: Field,
}

/// Lower-case words of `text`: runs of letters and digits
/// (`g_str_tokenize_and_fold`, without its transliteration).
fn words(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// The terms of a search as typed: lower-case words.
pub fn terms(query: &str) -> Vec<String> {
    query.split_whitespace().flat_map(words).collect()
}

impl App {
    /// The words a search looks at, with where each is from.
    fn index(&self) -> Vec<(Field, String)> {
        fn add(index: &mut Vec<(Field, String)>, field: Field, text: &str) {
            index.extend(words(text).into_iter().map(|word| (field, word)));
        }
        let mut index = Vec::new();
        add(&mut index, Field::Name, &self.name);
        if let Some(program) = self.program() {
            // The program's name counts as one word, dashes and all.
            index.push((Field::Exec, program.to_lowercase()));
        }
        for keyword in &self.keywords {
            add(&mut index, Field::Keywords, keyword);
        }
        if let Some(generic_name) = &self.generic_name {
            add(&mut index, Field::GenericName, generic_name);
        }
        if let Some(comment) = &self.comment {
            add(&mut index, Field::Comment, comment);
        }
        index
    }

    /// How well the application matches every one of `terms`, or `None` if
    /// one of them is nowhere in it.
    fn rank(&self, terms: &[String]) -> Option<Rank> {
        let index = self.index();
        let mut worst = None;
        for term in terms {
            let best = index
                .iter()
                .filter_map(|(field, word)| {
                    let inside_a_word = if word.starts_with(term.as_str()) {
                        false
                    } else if word.contains(term.as_str()) {
                        true
                    } else {
                        return None;
                    };
                    Some(Rank {
                        inside_a_word,
                        field: *field,
                    })
                })
                .min()?;
            worst = worst.max(Some(best));
        }
        worst
    }
}

/// An item of the dash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DashItem {
    /// The application's id, or for windows of an application that is not
    /// installed, their application id.
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    /// Its open windows, the one used last first.
    pub windows: Vec<u64>,
    /// Whether one of its windows has the keyboard focus.
    pub focused: bool,
}

impl DashItem {
    pub fn running(&self) -> bool {
        !self.windows.is_empty()
    }
}

/// What the dash shows: the favourites, then the other running
/// applications (`Dash._redisplay`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dash {
    pub favorites: Vec<DashItem>,
    pub running: Vec<DashItem>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct State {
    /// Every application that is to be shown, by name.
    pub installed: Vec<App>,
}

#[derive(Debug)]
pub enum Event {
    /// The applications that are installed now.
    Installed(Vec<App>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Look again: something may have been installed or removed.
    Refresh,
}

impl Model for State {
    type Event = Event;
    type Command = Command;

    fn apply(&mut self, event: Event) {
        match event {
            Event::Installed(mut installed) => {
                sort_by_name(&mut installed);
                self.installed = installed;
            }
        }
    }

    fn predict(&mut self, _command: &Command) {}
}

/// Orders applications as GNOME's app grid does before anyone has moved an
/// icon: by name, whatever the case.
fn sort_by_name(apps: &mut [App]) {
    apps.sort_by(|a, b| (a.name.to_lowercase(), &a.id).cmp(&(b.name.to_lowercase(), &b.id)));
}

/// A desktop entry id as written in a list of favourites: with or without
/// ".desktop".
fn bare_id(id: &str) -> &str {
    id.strip_suffix(".desktop").unwrap_or(id)
}

impl State {
    pub fn app(&self, id: &str) -> Option<&App> {
        let id = bare_id(id);
        self.installed.iter().find(|app| app.id == id)
    }

    /// The application a window with this Wayland application id belongs
    /// to (`ShellWindowTracker`'s `get_app_from_window_wmclass` and
    /// `get_app_from_id`): the entry of that name, else the entry that
    /// names it as its window class, else either of those in another case.
    pub fn app_for_window(&self, app_id: &str) -> Option<&App> {
        let app_id = bare_id(app_id);
        let by_id = |app: &&App| app.id == app_id;
        let by_class = |app: &&App| app.startup_wm_class.as_deref() == Some(app_id);
        let lower = app_id.to_lowercase();
        let by_id_folded = |app: &&App| app.id.to_lowercase() == lower;
        let by_class_folded = |app: &&App| {
            app.startup_wm_class
                .as_ref()
                .is_some_and(|class| class.to_lowercase() == lower)
        };
        let apps = &self.installed;
        apps.iter()
            .find(by_id)
            .or_else(|| apps.iter().find(by_class))
            .or_else(|| apps.iter().find(by_id_folded))
            .or_else(|| apps.iter().find(by_class_folded))
    }

    /// The dash for these favourites and open windows.
    ///
    /// A favourite that is not installed is left out. The other running
    /// applications follow in the order their first windows opened; GNOME
    /// orders them by when they were started, which comes to the same
    /// until one is restarted.
    pub fn dash(&self, favorites: &[String], windows: &[Window]) -> Dash {
        // Windows by application, most recently focused first.
        let mut running: Vec<(String, Vec<&Window>)> = Vec::new();
        for window in windows {
            let Some(app_id) = window.app_id.as_deref() else {
                continue;
            };
            let key = self
                .app_for_window(app_id)
                .map_or(app_id, |app| app.id.as_str());
            match running.iter_mut().find(|(id, _)| id == key) {
                Some((_, windows)) => windows.push(window),
                None => running.push((key.to_owned(), vec![window])),
            }
        }
        for (_, windows) in &mut running {
            windows.sort_by_key(|window| std::cmp::Reverse((window.focused, window.focus_time)));
        }

        let item = |id: &str, name: &str, icon: Option<&String>, windows: Option<&Vec<&Window>>| {
            DashItem {
                id: id.to_owned(),
                name: name.to_owned(),
                icon: icon.cloned(),
                windows: windows
                    .map(|windows| windows.iter().map(|window| window.id).collect())
                    .unwrap_or_default(),
                focused: windows.is_some_and(|windows| windows.iter().any(|window| window.focused)),
            }
        };
        let windows_of = |id: &str| {
            running
                .iter()
                .find(|(running_id, _)| running_id == id)
                .map(|(_, windows)| windows)
        };

        let mut dash = Dash::default();
        for favorite in favorites {
            let Some(app) = self.app(favorite) else {
                continue;
            };
            if dash.favorites.iter().any(|item| item.id == app.id) {
                continue;
            }
            dash.favorites.push(item(
                &app.id,
                &app.name,
                app.icon.as_ref(),
                windows_of(&app.id),
            ));
        }
        for (id, windows) in &running {
            if dash.favorites.iter().any(|item| &item.id == id) {
                continue;
            }
            dash.running.push(match self.app(id) {
                Some(app) => item(&app.id, &app.name, app.icon.as_ref(), Some(windows)),
                // No entry: the window's own title is the best name there
                // is, as in GNOME's window-backed applications.
                None => {
                    let title = windows
                        .first()
                        .and_then(|window| window.title.as_deref())
                        .filter(|title| !title.is_empty())
                        .unwrap_or(id);
                    item(id, title, None, Some(windows))
                }
            });
        }
        dash
    }

    /// What the app grid shows: every application but the favourites, which
    /// are in the dash already (`AppDisplay._loadApps`).
    pub fn grid(&self, favorites: &[String]) -> Vec<&App> {
        self.installed
            .iter()
            .filter(|app| !favorites.iter().any(|favorite| bare_id(favorite) == app.id))
            .collect()
    }

    /// The applications matching every one of `terms`, best match first.
    /// GNOME orders equal matches by how often each is used; here they are
    /// in name order.
    pub fn search(&self, terms: &[String]) -> Vec<&App> {
        if terms.is_empty() {
            return Vec::new();
        }
        let mut found: Vec<(Rank, &App)> = self
            .installed
            .iter()
            .filter_map(|app| Some((app.rank(terms)?, app)))
            .collect();
        // Stable: `installed` is in name order already.
        found.sort_by_key(|(rank, _)| *rank);
        found.into_iter().map(|(_, app)| app).collect()
    }
}

/// The command a terminal application's own command is put after: the
/// first terminal emulator found, `xdg-terminal-exec` (which knows the
/// user's choice) before any other. `has` says whether a program exists.
pub fn terminal_prefix(has: impl Fn(&str) -> bool) -> Option<Vec<String>> {
    const TERMINALS: [&[&str]; 10] = [
        &["xdg-terminal-exec"],
        &["ptyxis", "--"],
        &["kgx", "-e"],
        &["gnome-terminal", "--"],
        &["ghostty", "-e"],
        &["foot"],
        &["alacritty", "-e"],
        &["kitty"],
        &["wezterm", "start", "--"],
        &["xterm", "-e"],
    ];
    TERMINALS
        .iter()
        .find(|terminal| has(terminal[0]))
        .map(|terminal| terminal.iter().map(|&word| word.to_owned()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, name: &str) -> App {
        App {
            id: id.to_owned(),
            name: name.to_owned(),
            ..App::default()
        }
    }

    fn state(apps: Vec<App>) -> State {
        let mut state = State::default();
        state.apply(Event::Installed(apps));
        state
    }

    fn window(id: u64, app_id: &str) -> Window {
        Window {
            id,
            app_id: Some(app_id.to_owned()),
            ..Window::default()
        }
    }

    fn sample() -> State {
        state(vec![
            App {
                exec: Some("nautilus --new-window %U".to_owned()),
                keywords: vec!["folder".to_owned(), "manager".to_owned()],
                comment: Some("Access and organize files".to_owned()),
                ..app("org.gnome.Nautilus", "Files")
            },
            App {
                exec: Some("gnome-calculator".to_owned()),
                generic_name: Some("Calculator".to_owned()),
                keywords: vec!["calculation".to_owned(), "arithmetic".to_owned()],
                ..app("org.gnome.Calculator", "Calculator")
            },
            App {
                exec: Some("env LANG=C /usr/bin/pcbcalculator".to_owned()),
                ..app("org.kicad.pcbcalculator", "KiCad PCB Calculator")
            },
            App {
                exec: Some("gnome-text-editor %U".to_owned()),
                comment: Some("Edit text files, a calculator it is not".to_owned()),
                ..app("org.gnome.TextEditor", "Text Editor")
            },
            App {
                exec: Some("zen".to_owned()),
                startup_wm_class: Some("zen".to_owned()),
                ..app("app.zen_browser.zen", "Zen Browser")
            },
        ])
    }

    fn names(apps: Vec<&App>) -> Vec<&str> {
        apps.into_iter().map(|app| app.name.as_str()).collect()
    }

    #[test]
    fn applications_are_kept_in_name_order_whatever_the_case() {
        let state = state(vec![app("b", "beta"), app("c", "Gamma"), app("a", "Alpha")]);
        assert_eq!(
            names(state.installed.iter().collect()),
            ["Alpha", "beta", "Gamma"]
        );
    }

    #[test]
    fn a_search_finds_names_before_keywords_before_comments() {
        let state = sample();
        // Name, then name again (a later word), then the comment.
        assert_eq!(
            names(state.search(&terms("calc"))),
            ["Calculator", "KiCad PCB Calculator", "Text Editor"]
        );
        assert_eq!(names(state.search(&terms("folder"))), ["Files"]);
        // The program's name, though the entry is called something else.
        assert_eq!(names(state.search(&terms("nautilus"))), ["Files"]);
        assert_eq!(
            names(state.search(&terms("pcbcalc"))),
            ["KiCad PCB Calculator"]
        );
        assert_eq!(names(state.search(&terms("ARITH"))), ["Calculator"]);
    }

    #[test]
    fn every_term_must_match_and_the_worst_one_ranks() {
        let state = sample();
        assert_eq!(
            names(state.search(&terms("kicad calc"))),
            ["KiCad PCB Calculator"]
        );
        assert!(state.search(&terms("kicad folder")).is_empty());
        assert!(state.search(&terms("")).is_empty());
        assert!(state.search(&terms("   ")).is_empty());
        // "text" is in Text Editor's name, "files" only in its comment: the
        // comment ranks it. Files has "files" in its name but no "text".
        assert_eq!(names(state.search(&terms("text files"))), ["Text Editor"]);
    }

    #[test]
    fn a_term_inside_a_word_matches_after_every_word_start() {
        let state = sample();
        // "ditor" is inside "Editor"; "edit" starts a word of the comment
        // of the same entry and of nothing else.
        assert_eq!(names(state.search(&terms("ditor"))), ["Text Editor"]);
        // "alc" is inside names; nothing starts with it.
        assert_eq!(
            names(state.search(&terms("alc"))),
            ["Calculator", "KiCad PCB Calculator", "Text Editor"]
        );
    }

    #[test]
    fn a_window_finds_its_application() {
        let state = sample();
        let found = |app_id: &str| state.app_for_window(app_id).map(|app| app.id.as_str());
        assert_eq!(found("org.gnome.Nautilus"), Some("org.gnome.Nautilus"));
        assert_eq!(
            found("org.gnome.Nautilus.desktop"),
            Some("org.gnome.Nautilus")
        );
        assert_eq!(found("zen"), Some("app.zen_browser.zen"));
        assert_eq!(found("org.gnome.nautilus"), Some("org.gnome.Nautilus"));
        assert_eq!(found("Zen"), Some("app.zen_browser.zen"));
        assert_eq!(found("com.example.Unknown"), None);
    }

    #[test]
    fn the_dash_has_the_favourites_then_whatever_else_runs() {
        let state = sample();
        let favorites = [
            "app.zen_browser.zen.desktop".to_owned(),
            "org.gnome.Nautilus".to_owned(),
            "org.example.NotInstalled.desktop".to_owned(),
            "org.gnome.Nautilus.desktop".to_owned(),
        ];
        let windows = [
            window(1, "com.mitchellh.ghostty"),
            Window {
                title: Some("A terminal".to_owned()),
                focus_time: Some((10, 0)),
                ..window(2, "com.mitchellh.ghostty")
            },
            Window {
                focused: true,
                ..window(3, "zen")
            },
            window(4, "org.gnome.Calculator"),
            Window {
                app_id: None,
                ..window(5, "")
            },
        ];
        let dash = state.dash(&favorites, &windows);
        let ids = |items: &[DashItem]| -> Vec<(String, bool)> {
            items
                .iter()
                .map(|item| (item.id.clone(), item.running()))
                .collect()
        };
        assert_eq!(
            ids(&dash.favorites),
            [
                ("app.zen_browser.zen".to_owned(), true),
                ("org.gnome.Nautilus".to_owned(), false),
            ]
        );
        assert!(dash.favorites[0].focused);
        assert_eq!(
            ids(&dash.running),
            [
                ("com.mitchellh.ghostty".to_owned(), true),
                ("org.gnome.Calculator".to_owned(), true),
            ]
        );
        // No entry: named after the window, the one used last first.
        assert_eq!(dash.running[0].name, "A terminal");
        assert_eq!(dash.running[0].windows, [2, 1]);
        assert_eq!(dash.running[1].name, "Calculator");
    }

    #[test]
    fn the_grid_leaves_the_favourites_to_the_dash() {
        let state = sample();
        let favorites = ["org.gnome.Nautilus.desktop".to_owned()];
        assert_eq!(
            names(state.grid(&favorites)),
            [
                "Calculator",
                "KiCad PCB Calculator",
                "Text Editor",
                "Zen Browser"
            ]
        );
    }

    #[test]
    fn a_terminal_application_runs_in_the_first_terminal_there_is() {
        assert_eq!(terminal_prefix(|_| false), None);
        assert_eq!(
            terminal_prefix(|program| program == "kgx" || program == "xterm").unwrap(),
            ["kgx", "-e"]
        );
        assert_eq!(terminal_prefix(|_| true).unwrap(), ["xdg-terminal-exec"]);
    }
}
