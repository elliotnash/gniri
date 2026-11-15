//! What gniri adds to niri's overview: the search entry, the dash, the app
//! grid and the search results. Port of `ControlsManager`
//! (overviewControls.js) and `SearchController` (searchController.js).
//!
//! niri owns the overview: it zooms the workspaces out, and says when it
//! has done so. This module only keeps what is shown on top of that.

pub mod app_grid;
pub mod controls;
pub mod dash;
pub mod icon;
pub mod layout;
pub mod search_entry;
pub mod search_results;

use std::rc::Rc;

use gniri_services::apps::{self, App, DashItem};
use gniri_services::search;

use crate::services::Services;

/// What the overview shows beside the dash and the search entry. One piece
/// of state for every output: niri opens its overview on all of them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Overview {
    /// The app grid shows in place of the windows.
    pub apps: bool,
    /// What is typed in the search entry.
    pub query: String,
    /// The page of the app grid that shows.
    pub page: usize,
    /// The item the arrow keys have moved to: an index into the search
    /// results, or into the app grid when nothing is searched for.
    pub selected: Option<usize>,
    /// The search was set by a script and nobody is typing: the entry looks
    /// as it does without the keyboard focus and no result is highlighted,
    /// which is how GNOME Shell shows a search that its own scripting
    /// interface started. The reference captures were taken that way.
    pub quiet: bool,
}

impl Overview {
    /// The terms of the search, as `getTermsForSearchString` splits them.
    pub fn terms(&self) -> Vec<String> {
        self.query.split_whitespace().map(str::to_owned).collect()
    }

    /// Whether the search results show.
    pub fn searching(&self) -> bool {
        !self.query.trim().is_empty()
    }

    /// Whether something covers the windows.
    pub fn covered(&self) -> bool {
        self.apps || self.searching()
    }

    pub fn set_query(&mut self, query: &str) {
        if self.query != query {
            query.clone_into(&mut self.query);
            self.selected = None;
            self.quiet = false;
        }
    }

    /// What the dash's last button does: shows the app grid, or the windows
    /// again. A search that was running ends.
    pub fn toggle_apps(&mut self) {
        self.set_apps(!self.apps || self.searching());
    }

    pub fn set_apps(&mut self, apps: bool) {
        self.query.clear();
        self.quiet = false;
        self.selected = None;
        if self.apps != apps {
            self.apps = apps;
            self.page = 0;
        }
    }

    /// What Escape does (`SearchController._onStageKeyPress`): ends the
    /// search, or else leaves the app grid. False if there was neither,
    /// which is when Escape closes the overview.
    pub fn back(&mut self) -> bool {
        if self.searching() || !self.query.is_empty() {
            self.set_query("");
            true
        } else if self.apps {
            self.set_apps(false);
            true
        } else {
            false
        }
    }
}

/// One thing a search found, in the order the arrow keys go through them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    App(String),
    Result { provider: String, result: String },
}

/// Which arrow key was pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// Everything a search found that is on screen: the applications, at most
/// `app_columns` of them, then each provider's results.
pub fn found(
    apps: &apps::State,
    search: &search::State,
    terms: &[String],
    app_columns: usize,
) -> Vec<Found> {
    let app_terms = apps::terms(&terms.join(" "));
    let mut found: Vec<Found> = apps
        .search(&app_terms)
        .into_iter()
        .take(app_columns)
        .map(|app| Found::App(app.id.clone()))
        .collect();
    for provider in search.shown() {
        found.extend(provider.results.iter().map(|result| Found::Result {
            provider: provider.id.clone(),
            result: result.id.clone(),
        }));
    }
    found
}

/// Where an arrow key takes the selection among what a search found. The
/// applications are one row; under it the other results are one column.
pub fn move_among_results(
    found: &[Found],
    selected: Option<usize>,
    direction: Direction,
) -> Option<usize> {
    if found.is_empty() {
        return None;
    }
    let apps = found
        .iter()
        .take_while(|found| matches!(found, Found::App(_)))
        .count();
    let last = found.len() - 1;
    // Nothing chosen yet: the first result is the one Enter would take,
    // and an arrow moves on from it.
    let current = selected.unwrap_or(0).min(last);
    Some(match direction {
        Direction::Left if current < apps => current.saturating_sub(1),
        Direction::Right if current < apps => (current + 1).min(apps.saturating_sub(1)),
        Direction::Left | Direction::Right => current,
        // Down from the row of applications: the first of the others.
        Direction::Down if current < apps => apps.min(last),
        Direction::Down => (current + 1).min(last),
        // Up from the first of the others: back to the row.
        Direction::Up if current <= apps => 0,
        Direction::Up => current - 1,
    })
}

/// Where an arrow key takes the selection in an app grid of `count` icons
/// in pages of `columns` by `rows`.
pub fn move_in_grid(
    count: usize,
    (columns, rows): (usize, usize),
    selected: Option<usize>,
    direction: Direction,
) -> Option<usize> {
    if count == 0 || columns == 0 || rows == 0 {
        return None;
    }
    let Some(current) = selected else {
        return Some(0);
    };
    let current = current.min(count - 1);
    let per_page = columns * rows;
    let (page, position) = (current / per_page, current % per_page);
    let (row, column) = (position / columns, position % columns);
    let target = match direction {
        // Off the edge of a page: the same row of the page beside it.
        Direction::Left if column == 0 => {
            if page == 0 {
                return Some(current);
            }
            (page - 1) * per_page + row * columns + columns - 1
        }
        Direction::Left => current - 1,
        Direction::Right if column == columns - 1 => (page + 1) * per_page + row * columns,
        Direction::Right => current + 1,
        Direction::Up if row == 0 => return Some(current),
        Direction::Up => current - columns,
        Direction::Down if row == rows - 1 => return Some(current),
        Direction::Down => current + columns,
    };
    Some(if target < count { target } else { current })
}

/// Opens the application's window that was used last, or starts it, and
/// leaves the overview (`AppIcon.activate`).
pub fn activate_dash_item(services: &Rc<Services>, item: &DashItem) {
    match item.windows.first() {
        Some(&window) => services.focus_window(window),
        None => {
            services.launch_app(&item.id);
        }
    }
    services.close_overview();
}

/// Starts an application, or shows it if it is running, and leaves the
/// overview.
pub fn activate_app(services: &Rc<Services>, app: &App) {
    let favorites = &services.config.dash.favorites;
    let window = services.niri.with(|niri| {
        let dash = services
            .apps
            .state
            .with(|apps| apps.dash(favorites, &niri.windows));
        dash.favorites
            .iter()
            .chain(&dash.running)
            .find(|item| item.id == app.id)
            .and_then(|item| item.windows.first().copied())
    });
    match window {
        Some(window) => services.focus_window(window),
        None => {
            services.launch_app(&app.id);
        }
    }
    services.close_overview();
}

/// Does what a click on `found` does.
pub fn activate_found(services: &Rc<Services>, found: &Found, terms: &[String]) {
    match found {
        Found::App(id) => {
            let app = services.apps.state.with(|apps| apps.app(id).cloned());
            if let Some(app) = app {
                activate_app(services, &app);
            }
        }
        Found::Result { provider, result } => {
            services.search.send(search::Command::Activate {
                provider: provider.clone(),
                result: result.clone(),
                terms: terms.to_vec(),
            });
            services.close_overview();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_ends_the_search_then_leaves_the_grid_then_gives_up() {
        let mut overview = Overview::default();
        overview.toggle_apps();
        overview.set_query("calc");
        assert!(overview.searching() && overview.apps);
        assert!(overview.back());
        assert!(!overview.searching() && overview.apps, "the grid is back");
        assert!(overview.back());
        assert_eq!(overview, Overview::default());
        assert!(!overview.back());
    }

    #[test]
    fn spaces_alone_search_for_nothing_but_escape_still_clears_them() {
        let mut overview = Overview::default();
        overview.set_query("  ");
        assert!(!overview.searching() && !overview.covered());
        assert!(overview.terms().is_empty());
        assert!(overview.back());
        assert!(overview.query.is_empty());
        overview.set_query(" text   editor ");
        assert_eq!(overview.terms(), ["text", "editor"]);
    }

    #[test]
    fn the_apps_button_ends_a_search_and_shows_the_grid() {
        let mut overview = Overview::default();
        overview.set_query("calc");
        overview.toggle_apps();
        assert!(overview.apps && !overview.searching());
        overview.page = 2;
        overview.toggle_apps();
        assert_eq!(overview, Overview::default());
    }

    #[test]
    fn typing_forgets_the_selection_and_the_script() {
        let mut overview = Overview {
            query: "cal".to_owned(),
            selected: Some(3),
            quiet: true,
            ..Overview::default()
        };
        overview.set_query("cal");
        assert!(overview.quiet, "nothing changed");
        overview.set_query("calc");
        assert_eq!((overview.selected, overview.quiet), (None, false));
    }

    fn sample() -> Vec<Found> {
        let app = |id: &str| Found::App(id.to_owned());
        let result = |id: &str| Found::Result {
            provider: "files".to_owned(),
            result: id.to_owned(),
        };
        vec![app("a"), app("b"), app("c"), result("x"), result("y")]
    }

    #[test]
    fn arrows_go_along_the_applications_and_down_the_list() {
        let found = sample();
        let step = |selected, direction| move_among_results(&found, selected, direction);
        assert_eq!(step(None, Direction::Right), Some(1));
        assert_eq!(step(Some(2), Direction::Right), Some(2), "end of the row");
        assert_eq!(step(Some(0), Direction::Left), Some(0));
        assert_eq!(step(Some(1), Direction::Down), Some(3));
        assert_eq!(step(Some(3), Direction::Down), Some(4));
        assert_eq!(step(Some(4), Direction::Down), Some(4));
        assert_eq!(step(Some(4), Direction::Up), Some(3));
        assert_eq!(step(Some(3), Direction::Up), Some(0));
        assert_eq!(step(Some(3), Direction::Right), Some(3));
        assert_eq!(move_among_results(&[], None, Direction::Down), None);
        // Only a list: down from nothing chosen is its second entry.
        assert_eq!(
            move_among_results(&found[3..], None, Direction::Down),
            Some(1)
        );
    }

    #[test]
    fn arrows_move_through_the_grid_and_across_its_pages() {
        // 30 icons in pages of 8 by 3: a full page and six more.
        let step = |selected, direction| move_in_grid(30, (8, 3), selected, direction);
        assert_eq!(step(None, Direction::Down), Some(0));
        assert_eq!(step(Some(0), Direction::Right), Some(1));
        assert_eq!(step(Some(0), Direction::Left), Some(0));
        assert_eq!(step(Some(0), Direction::Up), Some(0));
        assert_eq!(step(Some(0), Direction::Down), Some(8));
        assert_eq!(step(Some(16), Direction::Down), Some(16), "last row");
        // Off the right edge of the first row: the next page's first row.
        assert_eq!(step(Some(7), Direction::Right), Some(24));
        assert_eq!(step(Some(24), Direction::Left), Some(7));
        // The second row of the next page does not exist.
        assert_eq!(step(Some(15), Direction::Right), Some(15));
        assert_eq!(step(Some(29), Direction::Right), Some(29));
        assert_eq!(move_in_grid(0, (8, 3), None, Direction::Down), None);
    }
}
