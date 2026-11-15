//! Search providers: applications that answer the overview's search over
//! D-Bus (`org.gnome.Shell.SearchProvider2`), such as Files, Calculator,
//! Characters and Software. Port of gnome-shell's `js/ui/remoteSearch.js`
//! and of the parts of `js/ui/search.js` that are not about widgets.
//!
//! Installed applications are not found through here: the shell searches
//! those itself ([`crate::apps::State::search`]).

pub mod backend;

use serde::Deserialize;

use crate::model::Model;
pub use crate::notifications::Image;

/// search.js `MAX_LIST_SEARCH_RESULTS_ROWS`: how many results of one
/// provider are shown.
pub const MAX_RESULTS: usize = 5;

/// One thing a provider found.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ResultItem {
    /// The provider's own name for it, handed back to activate it.
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub icon: Option<Image>,
}

/// An application that answers searches, and what it found last.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Provider {
    /// The application's desktop entry, without ".desktop".
    pub id: String,
    /// The application's name and icon.
    pub name: String,
    pub icon: Option<String>,
    /// The results that are shown: at most [`MAX_RESULTS`].
    pub results: Vec<ResultItem>,
    /// How many it found in all.
    pub total: usize,
    /// It has been asked and has not answered yet.
    pub searching: bool,
}

impl Provider {
    /// How many results there are beyond the ones shown: the "3 more"
    /// under the provider's name.
    pub fn more(&self) -> usize {
        self.total.saturating_sub(self.results.len())
    }
}

/// What is known of a provider before it is asked anything.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct State {
    /// What is being searched for; empty when nothing is.
    pub terms: Vec<String>,
    /// Every provider, in the order their results are listed.
    pub providers: Vec<Provider>,
}

#[derive(Debug)]
pub enum Event {
    /// The providers that are installed, in order. Those already known
    /// keep their results.
    Providers(Vec<ProviderInfo>),
    /// Every provider has been asked for `terms`.
    Started { terms: Vec<String> },
    /// A provider's answer for `terms`: the results shown, and how many it
    /// found in all.
    Results {
        terms: Vec<String>,
        provider: String,
        results: Vec<ResultItem>,
        total: usize,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Search for these terms; none ends the search.
    Search(Vec<String>),
    /// Open a result, as a click on it does.
    Activate {
        provider: String,
        result: String,
        terms: Vec<String>,
    },
    /// Open the provider's application on the search, as a click on the
    /// provider's own icon does.
    LaunchSearch {
        provider: String,
        terms: Vec<String>,
    },
}

impl State {
    /// The providers that have something to show, in order.
    pub fn shown(&self) -> impl Iterator<Item = &Provider> {
        self.providers
            .iter()
            .filter(|provider| !provider.results.is_empty())
    }

    /// Whether an answer is still awaited (`SearchResultsView`'s
    /// `searchInProgress`).
    pub fn searching(&self) -> bool {
        self.providers.iter().any(|provider| provider.searching)
    }
}

impl Model for State {
    type Event = Event;
    type Command = Command;

    fn apply(&mut self, event: Event) {
        match event {
            Event::Providers(infos) => {
                let mut old = std::mem::take(&mut self.providers);
                self.providers = infos
                    .into_iter()
                    .map(|info| {
                        let known = old
                            .iter()
                            .position(|provider| provider.id == info.id)
                            .map(|index| old.swap_remove(index));
                        Provider {
                            id: info.id,
                            name: info.name,
                            icon: info.icon,
                            ..known.unwrap_or_default()
                        }
                    })
                    .collect();
            }
            Event::Started { terms } => {
                if terms == self.terms {
                    for provider in &mut self.providers {
                        provider.searching = true;
                    }
                }
            }
            Event::Results {
                terms,
                provider,
                results,
                total,
            } => {
                // An answer to a search that has been replaced since.
                if terms != self.terms {
                    return;
                }
                if let Some(provider) = self.providers.iter_mut().find(|other| other.id == provider)
                {
                    provider.total = total.max(results.len());
                    provider.results = results;
                    provider.searching = false;
                }
            }
        }
    }

    fn predict(&mut self, command: &Command) {
        match command {
            // What was found for the terms before stays until the answers
            // for the new ones arrive, as in GNOME: the list does not
            // empty and fill again at every letter.
            Command::Search(terms) => {
                self.terms.clone_from(terms);
                if terms.is_empty() {
                    for provider in &mut self.providers {
                        provider.results.clear();
                        provider.total = 0;
                        provider.searching = false;
                    }
                }
            }
            Command::Activate { .. } | Command::LaunchSearch { .. } => {}
        }
    }
}

/// `RemoteSearchProvider.filterResults`: the first `limit` results, and
/// beside them the first `limit` of those a provider marks "special:"
/// (such as GNOME Software's offer to search for more).
pub fn filter_results(results: &[String], limit: usize) -> Vec<String> {
    if results.len() <= limit {
        return results.to_vec();
    }
    let special = |id: &&String| id.starts_with("special:");
    let regular = results.iter().filter(|id| !special(id)).take(limit);
    let special = results.iter().filter(special).take(limit);
    regular.chain(special).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(id: &str) -> ProviderInfo {
        ProviderInfo {
            id: id.to_owned(),
            name: id.to_uppercase(),
            icon: None,
        }
    }

    fn item(id: &str) -> ResultItem {
        ResultItem {
            id: id.to_owned(),
            name: id.to_owned(),
            ..ResultItem::default()
        }
    }

    fn terms(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_owned).collect()
    }

    fn searching_for(text: &str) -> State {
        let mut state = State::default();
        state.apply(Event::Providers(vec![info("files"), info("software")]));
        state.predict(&Command::Search(terms(text)));
        state.apply(Event::Started { terms: terms(text) });
        state
    }

    #[test]
    fn answers_fill_in_provider_by_provider() {
        let mut state = searching_for("calc");
        assert!(state.searching());
        assert_eq!(state.shown().count(), 0);

        state.apply(Event::Results {
            terms: terms("calc"),
            provider: "software".to_owned(),
            results: vec![item("a"), item("b")],
            total: 17,
        });
        assert!(state.searching(), "files has not answered");
        let shown: Vec<&Provider> = state.shown().collect();
        assert_eq!(shown.len(), 1);
        assert_eq!((shown[0].name.as_str(), shown[0].more()), ("SOFTWARE", 15));

        state.apply(Event::Results {
            terms: terms("calc"),
            provider: "files".to_owned(),
            results: Vec::new(),
            total: 0,
        });
        assert!(!state.searching());
        assert_eq!(state.shown().count(), 1);
    }

    #[test]
    fn an_answer_to_an_older_search_is_dropped() {
        let mut state = searching_for("calc");
        state.predict(&Command::Search(terms("calcu")));
        state.apply(Event::Results {
            terms: terms("calc"),
            provider: "files".to_owned(),
            results: vec![item("old")],
            total: 1,
        });
        assert_eq!(state.shown().count(), 0);
        // Nor does a late "started" for the old terms mark anything.
        let mut state = State::default();
        state.apply(Event::Providers(vec![info("files")]));
        state.predict(&Command::Search(terms("b")));
        state.apply(Event::Started { terms: terms("a") });
        assert!(!state.searching());
    }

    #[test]
    fn results_stay_while_the_next_search_runs_and_go_when_it_ends() {
        let mut state = searching_for("calc");
        state.apply(Event::Results {
            terms: terms("calc"),
            provider: "files".to_owned(),
            results: vec![item("a")],
            total: 1,
        });
        state.predict(&Command::Search(terms("calcu")));
        assert_eq!(state.shown().count(), 1, "kept until the new answer");
        state.predict(&Command::Search(Vec::new()));
        assert_eq!(state.shown().count(), 0);
        assert!(!state.searching());
        assert!(state.terms.is_empty());
    }

    #[test]
    fn a_new_list_of_providers_keeps_what_the_known_ones_found() {
        let mut state = searching_for("calc");
        state.apply(Event::Results {
            terms: terms("calc"),
            provider: "software".to_owned(),
            results: vec![item("a")],
            total: 1,
        });
        state.apply(Event::Providers(vec![info("software"), info("contacts")]));
        let ids: Vec<(&str, usize)> = state
            .providers
            .iter()
            .map(|provider| (provider.id.as_str(), provider.results.len()))
            .collect();
        assert_eq!(ids, [("software", 1), ("contacts", 0)]);
    }

    #[test]
    fn special_results_are_kept_beside_the_first_few() {
        let ids = |list: &[&str]| -> Vec<String> { list.iter().map(|&id| id.to_owned()).collect() };
        let results = ids(&["a", "b", "special:more", "c", "d"]);
        assert_eq!(filter_results(&results, 5), results);
        assert_eq!(
            filter_results(&results, 2),
            ids(&["a", "b", "special:more"])
        );
        assert_eq!(filter_results(&ids(&["a", "b", "c"]), 2), ids(&["a", "b"]));
    }
}
