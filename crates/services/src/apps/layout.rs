// SPDX-License-Identifier: GPL-3.0-or-later

//! How the applications are arranged: the order of the app grid, its
//! folders, and the dash once its items have been dragged about.
//!
//! GNOME Shell keeps these in three settings (`app-picker-layout`,
//! `org.gnome.desktop.app-folders` and `favorite-apps`); here they are one
//! value, which the shell keeps in its state file. Port of the parts of
//! `AppDisplay`, `FolderView` and `PageManager` (appDisplay.js) and of
//! `AppFavorites` (appFavorites.js) that decide what is where.
//!
//! Nothing here moves anything on screen. A drag ends in one of the
//! methods of [`State`] that return the arrangement as it is afterwards.

use gniri_core::tr;
use serde::{Deserialize, Serialize};

use super::{App, State, bare_id};
use crate::desktop_entry;

/// One thing in the grid's order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Entry {
    /// An application, by desktop entry.
    App(String),
    Folder {
        /// The folder's id.
        folder: String,
    },
}

impl Entry {
    fn folder(id: &str) -> Self {
        Self::Folder {
            folder: id.to_owned(),
        }
    }
}

/// A folder of the app grid: `org.gnome.desktop.app-folders.folder`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Folder {
    pub id: String,
    /// Its name, or with `translate` the desktop directory file that has
    /// its name in every language ("X-GNOME-Shell-Utilities.directory").
    pub name: String,
    pub translate: bool,
    /// The applications in it, in order.
    pub apps: Vec<String>,
    /// Applications of these categories are in it too, after those.
    pub categories: Vec<String>,
    /// But not these, which were dragged out of it.
    pub excluded: Vec<String>,
}

impl Folder {
    /// The name under the folder's icon.
    pub fn display_name(&self) -> String {
        if !self.translate {
            return self.name.clone();
        }
        desktop_entry::directory_name(&self.name).unwrap_or_else(|| {
            // Not installed (gnome-menus brings the files): the two GNOME
            // makes are known, anything else is called by its file.
            match self.name.as_str() {
                "X-GNOME-Shell-Utilities.directory" => "Utilities".to_owned(),
                "X-GNOME-Shell-System.directory" => "System".to_owned(),
                other => other.trim_end_matches(".directory").to_owned(),
            }
        })
    }
}

/// The dash's applications after a drag, with the list of the config file
/// they were dragged from: a config file that has since been given another
/// list is the newer word, and counts again.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Favorites {
    pub from: Vec<String>,
    pub now: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Layout {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub favorites: Option<Favorites>,
    /// The grid's order. What is installed and not named here comes after
    /// everything that is, by name.
    pub grid: Vec<Entry>,
    pub folders: Vec<Folder>,
}

/// GNOME Shell's `data/default-apps/app-grid.txt`: the start of the grid
/// before anyone has moved an icon.
const DEFAULT_GRID: [&str; 16] = [
    "org.gnome.Geary",
    "org.gnome.Contacts",
    "org.gnome.Weather",
    "org.gnome.clocks",
    "org.gnome.Maps",
    "org.gnome.Music",
    "org.gnome.SimpleScan",
    "org.gnome.Settings",
    "org.gnome.Boxes",
    "org.gnome.Snapshot",
    "org.gnome.Characters",
    "folder:Utilities",
    "folder:System",
    "org.gnome.Console",
    "org.gnome.Tour",
    "org.gnome.Yelp",
];

/// `data/default-apps/utilities-folder.txt`.
const UTILITIES: [&str; 9] = [
    "org.gnome.Decibels",
    "org.gnome.Connections",
    "org.gnome.Papers",
    "org.gnome.FileRoller",
    "org.gnome.font-viewer",
    "org.gnome.Loupe",
    "org.gnome.seahorse.Application",
    "org.gnome.Seahorse",
    "org.gnome.Showtime",
];

/// `data/default-apps/system-folder.txt`.
const SYSTEM: [&str; 10] = [
    "nm-connection-editor",
    "org.gnome.DejaDup",
    "org.gnome.baobab",
    "org.gnome.DiskUtility",
    "org.gnome.Logs",
    "org.freedesktop.MalcontentControl",
    "org.freedesktop.GnomeAbrt",
    "org.gnome.Sysprof",
    "org.gnome.SystemMonitor",
    "org.gnome.tweaks",
];

impl Default for Layout {
    /// GNOME's own: its two folders (`AppDisplay._ensureDefaultFolders`)
    /// and the default of `app-picker-layout`.
    fn default() -> Self {
        let folder = |id: &str, name: &str, apps: &[&str]| Folder {
            id: id.to_owned(),
            name: name.to_owned(),
            translate: true,
            apps: apps.iter().map(|&app| app.to_owned()).collect(),
            ..Folder::default()
        };
        Self {
            favorites: None,
            grid: DEFAULT_GRID
                .iter()
                .map(|entry| match entry.strip_prefix("folder:") {
                    Some(id) => Entry::folder(id),
                    None => Entry::App((*entry).to_owned()),
                })
                .collect(),
            folders: vec![
                folder("System", "X-GNOME-Shell-System.directory", &SYSTEM),
                folder("Utilities", "X-GNOME-Shell-Utilities.directory", &UTILITIES),
            ],
        }
    }
}

impl Layout {
    /// Nothing arranged: every application by name, no folders.
    pub fn none() -> Self {
        Self {
            favorites: None,
            grid: Vec::new(),
            folders: Vec::new(),
        }
    }
}

/// A folder as the grid shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShownFolder<'a> {
    pub id: &'a str,
    pub name: String,
    /// Never empty: a folder with nothing to show is not shown.
    pub apps: Vec<&'a App>,
}

/// One item of the app grid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GridItem<'a> {
    App(&'a App),
    Folder(ShownFolder<'a>),
}

impl GridItem<'_> {
    pub fn entry(&self) -> Entry {
        match self {
            Self::App(app) => Entry::App(app.id.clone()),
            Self::Folder(folder) => Entry::folder(folder.id),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::App(app) => &app.name,
            Self::Folder(folder) => &folder.name,
        }
    }
}

/// The name GNOME gives a new folder: that of a category all its
/// applications share, if there is a desktop directory file for one
/// (`_findBestFolderName`).
fn best_folder_name(apps: &[&App]) -> Option<String> {
    let first = apps.first()?;
    first
        .categories
        .iter()
        .filter(|category| {
            !category.is_empty() && apps.iter().all(|app| app.categories.contains(category))
        })
        .find_map(|category| desktop_entry::directory_name(&format!("{category}.directory")))
}

impl State {
    /// The dash's applications: the config file's list (`config`), or what
    /// that list has been dragged into.
    pub fn favorites(&self, config: &[String]) -> Vec<String> {
        match &self.layout.favorites {
            Some(favorites) if favorites.from == config => favorites.now.clone(),
            _ => config.to_vec(),
        }
    }

    /// What `folder` shows: its applications that are installed and not in
    /// the dash, then those of its categories (`FolderView._loadApps`).
    fn folder_apps<'a>(&'a self, folder: &Folder, favorites: &[String]) -> Vec<&'a App> {
        let is_favorite = |id: &str| favorites.iter().any(|favorite| bare_id(favorite) == id);
        let excluded = |id: &str| folder.excluded.iter().any(|other| bare_id(other) == id);
        let mut apps: Vec<&App> = Vec::new();
        let named = folder.apps.iter().filter_map(|id| self.app(id));
        let by_category = self.installed.iter().filter(|app| {
            app.categories
                .iter()
                .any(|category| folder.categories.contains(category))
        });
        for app in named.chain(by_category) {
            let shown = apps.iter().any(|other| other.id == app.id);
            if !shown && !is_favorite(&app.id) && !excluded(&app.id) {
                apps.push(app);
            }
        }
        apps
    }

    /// The app grid: every application that is not in the dash, those in a
    /// folder inside it, in the order they were given or else by name
    /// (`AppDisplay._loadApps` and `_compareItems`).
    pub fn grid_items(&self, favorites: &[String]) -> Vec<GridItem<'_>> {
        let folders: Vec<ShownFolder> = self
            .layout
            .folders
            .iter()
            .map(|folder| ShownFolder {
                id: &folder.id,
                name: folder.display_name(),
                apps: self.folder_apps(folder, favorites),
            })
            .filter(|folder| !folder.apps.is_empty())
            .collect();
        let in_a_folder = |app: &App| {
            folders
                .iter()
                .any(|folder| folder.apps.iter().any(|other| other.id == app.id))
        };
        let mut items: Vec<GridItem> = self
            .grid(favorites)
            .into_iter()
            .filter(|app| !in_a_folder(app))
            .map(GridItem::App)
            .collect();
        items.extend(folders.into_iter().map(GridItem::Folder));

        let place = |item: &GridItem| {
            let entry = item.entry();
            self.layout
                .grid
                .iter()
                .position(|other| match (other, &entry) {
                    (Entry::App(a), Entry::App(b)) => bare_id(a) == b,
                    (a, b) => a == b,
                })
        };
        // Stable, so that what has no place stays in name order, and the
        // folders among the applications.
        items.sort_by_key(|item| item.name().to_lowercase());
        items.sort_by_key(|item| place(item).unwrap_or(usize::MAX));
        items
    }

    /// The arrangement with the grid's order written out as it shows now:
    /// what a change starts from, so that moving one icon fixes the places
    /// of all (`BaseAppView._savePages`).
    fn settled(&self, favorites: &[String]) -> Layout {
        Layout {
            grid: self
                .grid_items(favorites)
                .iter()
                .map(GridItem::entry)
                .collect(),
            ..self.layout.clone()
        }
    }

    /// Takes an application out of the folder `id`: off its list, and if
    /// one of its categories would bring it back, onto the list of those
    /// left out (`FolderView.removeApp`). A folder left with nothing to
    /// show goes.
    fn take_out(&self, layout: &mut Layout, app: &str, id: &str, favorites: &[String]) {
        let Some(folder) = layout.folders.iter_mut().find(|folder| folder.id == id) else {
            return;
        };
        folder.apps.retain(|other| bare_id(other) != app);
        let by_category = self.app(app).is_some_and(|app| {
            app.categories
                .iter()
                .any(|category| folder.categories.contains(category))
        });
        if by_category && !folder.excluded.iter().any(|other| bare_id(other) == app) {
            folder.excluded.push(app.to_owned());
        }
        let folder = folder.clone();
        if self.folder_apps(&folder, favorites).is_empty() {
            layout.folders.retain(|other| other.id != id);
            layout.grid.retain(|entry| *entry != Entry::folder(id));
        }
    }

    /// Takes an application out of wherever it is in the grid.
    fn lift(&self, layout: &mut Layout, app: &str, favorites: &[String]) {
        layout
            .grid
            .retain(|entry| *entry != Entry::App(app.to_owned()));
        let holding: Vec<String> = layout
            .folders
            .iter()
            .filter(|folder| {
                self.folder_apps(folder, favorites)
                    .iter()
                    .any(|other| other.id == app)
            })
            .map(|folder| folder.id.clone())
            .collect();
        for id in holding {
            self.take_out(layout, app, &id, favorites);
        }
    }

    /// Moves an item of the grid to `index`, counted among the others.
    pub fn move_in_grid(&self, entry: &Entry, index: usize, favorites: &[String]) -> Layout {
        let mut layout = self.settled(favorites);
        if let Some(from) = layout.grid.iter().position(|other| other == entry) {
            let entry = layout.grid.remove(from);
            layout.grid.insert(index.min(layout.grid.len()), entry);
        }
        layout
    }

    /// Makes a folder of two applications, where `onto` is: the one that
    /// was dropped and the one it was dropped on (`AppDisplay.createFolder`).
    pub fn create_folder(&self, dropped: &str, onto: &str, favorites: &[String]) -> Layout {
        let mut layout = self.settled(favorites);
        let (Some(dropped), Some(onto)) = (self.app(dropped), self.app(onto)) else {
            return layout;
        };
        if dropped.id == onto.id {
            return layout;
        }
        self.lift(&mut layout, &dropped.id, favorites);
        let Some(place) = layout
            .grid
            .iter()
            .position(|entry| *entry == Entry::App(onto.id.clone()))
        else {
            return self.settled(favorites);
        };
        // GNOME names a new folder by a random UUID; a number will do.
        let id = (1..)
            .map(|number| format!("folder-{number}"))
            .find(|id| !layout.folders.iter().any(|folder| folder.id == *id))
            .expect("there is always another number");
        layout.grid[place] = Entry::folder(&id);
        layout.folders.push(Folder {
            id,
            name: best_folder_name(&[onto, dropped]).unwrap_or_else(|| tr("Unnamed Folder")),
            apps: vec![onto.id.clone(), dropped.id.clone()],
            ..Folder::default()
        });
        layout
    }

    /// Puts an application into a folder, last (`FolderView.addApp`).
    pub fn add_to_folder(&self, app: &str, folder: &str, favorites: &[String]) -> Layout {
        let mut layout = self.settled(favorites);
        let app = bare_id(app);
        if !layout.folders.iter().any(|other| other.id == folder) {
            return layout;
        }
        self.lift(&mut layout, app, favorites);
        if let Some(folder) = layout.folders.iter_mut().find(|other| other.id == folder) {
            folder.excluded.retain(|other| bare_id(other) != app);
            folder.apps.retain(|other| bare_id(other) != app);
            folder.apps.push(app.to_owned());
        }
        layout
    }

    /// Takes an application out of a folder and puts it into the grid at
    /// `index`.
    pub fn remove_from_folder(
        &self,
        app: &str,
        folder: &str,
        index: usize,
        favorites: &[String],
    ) -> Layout {
        let mut layout = self.settled(favorites);
        let app = bare_id(app);
        self.take_out(&mut layout, app, folder, favorites);
        layout
            .grid
            .retain(|entry| *entry != Entry::App(app.to_owned()));
        layout
            .grid
            .insert(index.min(layout.grid.len()), Entry::App(app.to_owned()));
        layout
    }

    /// Moves an application within its folder to `index`, counted among
    /// the others there (`FolderView._redisplay` after a drop).
    pub fn move_in_folder(
        &self,
        app: &str,
        folder: &str,
        index: usize,
        favorites: &[String],
    ) -> Layout {
        let mut layout = self.settled(favorites);
        let app = bare_id(app);
        let Some(position) = layout.folders.iter().position(|other| other.id == folder) else {
            return layout;
        };
        // Every application it shows, written out: one of a category has
        // no place to move from otherwise.
        let mut apps: Vec<String> = self
            .folder_apps(&layout.folders[position], favorites)
            .iter()
            .map(|app| app.id.clone())
            .collect();
        if let Some(from) = apps.iter().position(|other| other == app) {
            let moved = apps.remove(from);
            apps.insert(index.min(apps.len()), moved);
            layout.folders[position].apps = apps;
        }
        layout
    }

    /// Gives a folder a name of the user's.
    pub fn rename_folder(&self, folder: &str, name: &str) -> Layout {
        let mut layout = self.layout.clone();
        if let Some(folder) = layout.folders.iter_mut().find(|other| other.id == folder) {
            name.clone_into(&mut folder.name);
            folder.translate = false;
        }
        layout
    }

    /// Puts an application into the dash at `index`, or moves it there if
    /// it is in it (`AppFavorites.addFavoriteAtPos` and `moveFavoriteToPos`).
    /// `config` is the config file's list.
    pub fn pin(&self, app: &str, index: usize, config: &[String]) -> Layout {
        let app = bare_id(app);
        let mut now: Vec<String> = self
            .favorites(config)
            .into_iter()
            .filter(|favorite| bare_id(favorite) != app)
            .collect();
        // Counted among the favourites that show: one that is not
        // installed takes no place in the dash.
        let shown: Vec<usize> = now
            .iter()
            .enumerate()
            .filter(|(_, favorite)| self.app(favorite).is_some())
            .map(|(position, _)| position)
            .collect();
        let at = match shown.get(index) {
            Some(at) => *at,
            // After the last that shows.
            None => shown.last().map_or(0, |last| last + 1),
        };
        now.insert(at, app.to_owned());
        let mut layout = self.layout.clone();
        layout
            .grid
            .retain(|entry| *entry != Entry::App(app.to_owned()));
        layout.favorites = Some(Favorites {
            from: config.to_vec(),
            now,
        });
        layout
    }

    /// Takes an application out of the dash and puts it into the grid at
    /// `index`, or with none where its name puts it.
    pub fn unpin(&self, app: &str, index: Option<usize>, config: &[String]) -> Layout {
        let app = bare_id(app);
        let now: Vec<String> = self
            .favorites(config)
            .into_iter()
            .filter(|favorite| bare_id(favorite) != app)
            .collect();
        let mut layout = match index {
            // The grid as it shows with the application still in the dash.
            Some(_) => self.settled(&self.favorites(config)),
            None => self.layout.clone(),
        };
        if let Some(index) = index {
            layout
                .grid
                .retain(|entry| *entry != Entry::App(app.to_owned()));
            layout
                .grid
                .insert(index.min(layout.grid.len()), Entry::App(app.to_owned()));
        }
        layout.favorites = Some(Favorites {
            from: config.to_vec(),
            now,
        });
        layout
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;

    fn app(id: &str, name: &str, categories: &[&str]) -> App {
        App {
            id: id.to_owned(),
            name: name.to_owned(),
            categories: categories
                .iter()
                .map(|&category| category.to_owned())
                .collect(),
            ..App::default()
        }
    }

    fn folder(id: &str, name: &str, apps: &[&str]) -> Folder {
        Folder {
            id: id.to_owned(),
            name: name.to_owned(),
            apps: apps.iter().map(|&app| app.to_owned()).collect(),
            ..Folder::default()
        }
    }

    fn state(layout: Layout) -> State {
        let mut state = State {
            layout,
            ..State::default()
        };
        state.apply(super::super::Event::Installed(vec![
            app("org.gnome.Boxes", "Boxes", &[]),
            app("org.gnome.Calculator", "Calculator", &["Utility"]),
            app("org.gnome.Logs", "Logs", &["System"]),
            app("org.gnome.Maps", "Maps", &[]),
            app("org.gnome.Nautilus", "Files", &[]),
            app("org.gnome.font-viewer", "Fonts", &["Utility"]),
            app("org.gnome.baobab", "Disk Usage Analyzer", &["System"]),
            app("org.example.Zebra", "Zebra", &[]),
        ]));
        state
    }

    fn empty() -> Layout {
        Layout::none()
    }

    /// The grid as a list of names, a folder's applications in brackets.
    fn shown(state: &State, favorites: &[String]) -> Vec<String> {
        state
            .grid_items(favorites)
            .iter()
            .map(|item| match item {
                GridItem::App(app) => app.name.clone(),
                GridItem::Folder(folder) => {
                    let apps: Vec<&str> = folder.apps.iter().map(|app| app.name.as_str()).collect();
                    format!("{}[{}]", folder.name, apps.join(" "))
                }
            })
            .collect()
    }

    fn after(state: &State, layout: Layout) -> State {
        State {
            layout,
            ..state.clone()
        }
    }

    const NONE: &[String] = &[];

    #[test]
    fn with_nothing_arranged_the_grid_is_in_name_order() {
        let state = state(empty());
        assert_eq!(
            shown(&state, NONE),
            [
                "Boxes",
                "Calculator",
                "Disk Usage Analyzer",
                "Files",
                "Fonts",
                "Logs",
                "Maps",
                "Zebra"
            ]
        );
        // What is in the dash is not in the grid.
        let favorites = ["org.gnome.Nautilus.desktop".to_owned()];
        assert!(!shown(&state, &favorites).contains(&"Files".to_owned()));
    }

    #[test]
    fn gnomes_own_arrangement_is_the_default() {
        let state = state(Layout::default());
        // Maps and Boxes are on GNOME's list, in that order; the two
        // folders follow, each with what is installed of its own; the rest
        // comes by name. Calculator is in GNOME's dash, not in a folder.
        let directories_installed =
            desktop_entry::directory_name("X-GNOME-Shell-Utilities.directory").is_some();
        let grid = shown(&state, NONE);
        assert_eq!(grid[..2], ["Maps", "Boxes"]);
        assert_eq!(grid[4..], ["Calculator", "Files", "Zebra"]);
        if !directories_installed {
            assert_eq!(
                grid[2..4],
                ["Utilities[Fonts]", "System[Disk Usage Analyzer Logs]"]
            );
        }
        assert!(grid[2].ends_with("[Fonts]"), "{grid:?}");
        assert!(grid[3].ends_with("[Disk Usage Analyzer Logs]"), "{grid:?}");
    }

    #[test]
    fn a_folder_has_its_own_and_those_of_its_categories() {
        let state = state(Layout {
            folders: vec![Folder {
                categories: vec!["Utility".to_owned()],
                excluded: vec!["org.gnome.Calculator".to_owned()],
                ..folder("tools", "Tools", &["org.gnome.Maps", "not.installed"])
            }],
            ..empty()
        });
        let grid = shown(&state, NONE);
        assert!(grid.contains(&"Tools[Maps Fonts]".to_owned()), "{grid:?}");
        assert!(grid.contains(&"Calculator".to_owned()), "left out of it");
        assert!(!grid.contains(&"Maps".to_owned()), "in the folder only");
        // A folder with nothing to show is not shown.
        let favorites = [
            "org.gnome.Maps".to_owned(),
            "org.gnome.font-viewer".to_owned(),
        ];
        assert!(
            !shown(&state, &favorites)
                .iter()
                .any(|item| item.starts_with("Tools"))
        );
    }

    #[test]
    fn moving_one_icon_fixes_the_place_of_every_other() {
        let state = state(empty());
        let moved = state.move_in_grid(&Entry::App("org.example.Zebra".to_owned()), 1, NONE);
        let state = after(&state, moved);
        assert_eq!(shown(&state, NONE)[..3], ["Boxes", "Zebra", "Calculator"]);
        assert_eq!(state.layout.grid.len(), 8, "every place is written down");
        // Past the end is the end.
        let moved = state.move_in_grid(&Entry::App("org.gnome.Boxes".to_owned()), 99, NONE);
        assert_eq!(shown(&after(&state, moved), NONE).last().unwrap(), "Boxes");
    }

    #[test]
    fn dropping_one_application_on_another_makes_a_folder_there() {
        let state = state(empty());
        let layout = state.create_folder("org.example.Zebra", "org.gnome.Calculator", NONE);
        let state = after(&state, layout);
        assert_eq!(
            shown(&state, NONE)[..3],
            [
                "Boxes",
                "Unnamed Folder[Calculator Zebra]",
                "Disk Usage Analyzer"
            ]
        );
        assert!(!shown(&state, NONE).contains(&"Zebra".to_owned()));
        // Another gets another id.
        let layout = state.create_folder("org.gnome.Maps", "org.gnome.Boxes", NONE);
        assert_eq!(
            layout
                .folders
                .iter()
                .map(|folder| &folder.id)
                .collect::<Vec<_>>(),
            ["folder-1", "folder-2"]
        );
        // On itself, or with something that is not there: nothing.
        let same = state.create_folder("org.gnome.Maps", "org.gnome.Maps", NONE);
        assert_eq!(same.folders.len(), 1);
        let unknown = state.create_folder("org.gnome.Maps", "not.installed", NONE);
        assert_eq!(unknown.folders.len(), 1);
    }

    #[test]
    fn applications_go_into_a_folder_and_come_out_again() {
        let state = state(Layout {
            folders: vec![folder("tools", "Tools", &["org.gnome.Maps"])],
            ..empty()
        });
        let state = after(
            &state,
            state.add_to_folder("org.gnome.Boxes.desktop", "tools", NONE),
        );
        assert!(shown(&state, NONE).contains(&"Tools[Maps Boxes]".to_owned()));

        let state = after(
            &state,
            state.move_in_folder("org.gnome.Boxes", "tools", 0, NONE),
        );
        assert!(shown(&state, NONE).contains(&"Tools[Boxes Maps]".to_owned()));

        let state = after(
            &state,
            state.remove_from_folder("org.gnome.Maps", "tools", 0, NONE),
        );
        assert_eq!(shown(&state, NONE)[..2], ["Maps", "Calculator"]);
        assert!(shown(&state, NONE).contains(&"Tools[Boxes]".to_owned()));

        // The last one out: the folder goes with it.
        let state = after(
            &state,
            state.remove_from_folder("org.gnome.Boxes", "tools", 1, NONE),
        );
        assert_eq!(shown(&state, NONE)[..3], ["Maps", "Boxes", "Calculator"]);
        assert!(state.layout.folders.is_empty());
        assert!(!state.layout.grid.contains(&Entry::folder("tools")));
    }

    #[test]
    fn what_a_category_brought_in_stays_out_once_taken_out() {
        let state = state(Layout {
            folders: vec![Folder {
                categories: vec!["Utility".to_owned()],
                ..folder("tools", "Tools", &[])
            }],
            ..empty()
        });
        assert!(shown(&state, NONE).contains(&"Tools[Calculator Fonts]".to_owned()));
        let state = after(
            &state,
            state.remove_from_folder("org.gnome.Calculator", "tools", 0, NONE),
        );
        assert_eq!(shown(&state, NONE)[0], "Calculator");
        assert!(shown(&state, NONE).contains(&"Tools[Fonts]".to_owned()));
        assert_eq!(state.layout.folders[0].excluded, ["org.gnome.Calculator"]);
        // Put back in by hand, it is in again.
        let state = after(
            &state,
            state.add_to_folder("org.gnome.Calculator", "tools", NONE),
        );
        assert!(shown(&state, NONE).contains(&"Tools[Calculator Fonts]".to_owned()));
        assert!(state.layout.folders[0].excluded.is_empty());
    }

    #[test]
    fn an_application_moves_from_one_folder_into_another() {
        let state = state(Layout {
            folders: vec![
                folder("a", "A", &["org.gnome.Maps", "org.gnome.Boxes"]),
                folder("b", "B", &["org.gnome.Logs"]),
            ],
            ..empty()
        });
        let state = after(&state, state.add_to_folder("org.gnome.Maps", "b", NONE));
        let grid = shown(&state, NONE);
        assert!(grid.contains(&"A[Boxes]".to_owned()), "{grid:?}");
        assert!(grid.contains(&"B[Logs Maps]".to_owned()), "{grid:?}");
    }

    #[test]
    fn a_renamed_folder_keeps_the_name_it_was_given() {
        let state = state(Layout::default());
        let layout = state.rename_folder("Utilities", "Odds and Ends");
        let renamed = layout
            .folders
            .iter()
            .find(|folder| folder.id == "Utilities")
            .unwrap();
        assert_eq!(renamed.display_name(), "Odds and Ends");
        assert!(!renamed.translate);
    }

    #[test]
    fn the_dash_is_the_config_files_until_it_is_dragged() {
        let config = ["org.gnome.Nautilus".to_owned(), "not.installed".to_owned()];
        let state = state(empty());
        assert_eq!(state.favorites(&config), config);

        // Into the dash, between the two: before the one that does not
        // show, which keeps its place in the list.
        let state = after(&state, state.pin("org.gnome.Maps.desktop", 1, &config));
        assert_eq!(
            state.favorites(&config),
            ["org.gnome.Nautilus", "org.gnome.Maps", "not.installed"]
        );
        assert!(!shown(&state, &state.favorites(&config)).contains(&"Maps".to_owned()));

        // Moved within it.
        let state = after(&state, state.pin("org.gnome.Maps", 0, &config));
        assert_eq!(
            state.favorites(&config),
            ["org.gnome.Maps", "org.gnome.Nautilus", "not.installed"]
        );

        // Out of it, to a place in the grid.
        let state = after(&state, state.unpin("org.gnome.Nautilus", Some(1), &config));
        assert_eq!(
            state.favorites(&config),
            ["org.gnome.Maps", "not.installed"]
        );
        assert_eq!(
            shown(&state, &state.favorites(&config))[..3],
            ["Boxes", "Files", "Calculator"]
        );

        // Another list in the config file is the newer word.
        let edited = ["org.gnome.Boxes".to_owned()];
        assert_eq!(state.favorites(&edited), edited);
    }

    #[test]
    fn unpinned_without_a_place_it_goes_where_its_name_puts_it() {
        let config = ["org.gnome.Maps".to_owned()];
        let state = state(empty());
        let state = after(&state, state.unpin("org.gnome.Maps", None, &config));
        assert!(state.favorites(&config).is_empty());
        assert_eq!(shown(&state, NONE)[6], "Maps");
    }

    #[test]
    fn the_arrangement_is_written_and_read_as_toml() {
        let state = state(empty());
        let layout = state.create_folder("org.example.Zebra", "org.gnome.Calculator", NONE);
        let layout = after(&state, layout).pin("org.gnome.Maps", 0, &["a".to_owned()]);
        let text = toml::to_string(&layout).unwrap();
        assert!(
            text.contains("{ folder = \"folder-1\" }") || text.contains("folder = \"folder-1\"")
        );
        assert_eq!(toml::from_str::<Layout>(&text).unwrap(), layout);
        // Nothing written: GNOME's own.
        assert_eq!(toml::from_str::<Layout>("").unwrap(), Layout::default());
    }
}
