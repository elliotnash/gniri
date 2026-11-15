// SPDX-License-Identifier: GPL-3.0-or-later

//! What a search found. Port of `SearchResultsView`, `GridSearchResults`,
//! `ListSearchResults` and `ProviderInfo` in `js/ui/search.js`.
//!
//! Widget tree, with the actors it stands for:
//!
//! ```text
//! box#searchResults                     SearchResultsView
//!   scrolledwindow.search-display       its scroll view
//!     clamp                             MaxWidthBox's limit
//!       box#searchResultsContent        MaxWidthBox
//!       box.search-section              GridSearchResults (applications)
//!         box.grid-search-results
//!           button.overview-tile.grid-search-result   (overview/icon.rs)
//!         box.search-section-separator
//!       box.search-section              ListSearchResults, one a provider
//!         box.search-section-content
//!           button.search-provider-icon   ProviderInfo
//!             box.list-search-provider-content
//!               image
//!               box.list-search-provider-details
//!                 label                     the application's name
//!                 label                     "3 more"
//!           box.list-search-results
//!             button.list-search-result     ListSearchResult
//!               box.list-search-result-content
//!                 box.list-search-result-title
//!                   image
//!                   label
//!                 label.list-search-result-description
//!         box.search-section-separator
//!   box.search-statusbox                "Searching" or "No results"
//!     label.search-statustext
//! ```
//!
//! The applications are found by the shell itself; everything else by the
//! applications that answer searches over D-Bus.

use std::cell::RefCell;
use std::rc::Rc;

use gniri_core::Subscription;
use gniri_services::apps;
use gniri_services::search::{self, Provider, ResultItem};
use gtk::prelude::*;
use gtk::{glib, pango};

use super::icon::{self, Tile, set_class};
use super::{Found, layout};
use crate::services::Services;
use crate::widgets::picture;

/// `#searchResultsContent { max-width }`
const MAX_WIDTH: i32 = 1044;
/// `.grid-search-results { spacing }`
const GRID_SPACING: i32 = 30;
/// iconGrid.js `ICON_SIZE`: a result's icon is never made smaller.
const GRID_ICON_SIZE: i32 = 96;
/// `ListSearchResult.ICON_SIZE`
const LIST_ICON_SIZE: i32 = 24;
/// `ProviderInfo.PROVIDER_ICON_SIZE`
const PROVIDER_ICON_SIZE: i32 = 32;
/// What a result without a picture of its own shows.
const FALLBACK_ICON: &str = "application-x-executable";

/// `Highlighter.highlight` (js/misc/util.js): `text` as Pango markup with
/// every occurrence of a search term in bold, whatever its case.
pub fn highlight(text: &str, terms: &[String]) -> String {
    let escape = |text: &str| glib::markup_escape_text(text).to_string();
    let lower = text.to_lowercase();
    // Lower-casing can change a text's length; then the positions found in
    // one do not fit the other, and the text is shown plain.
    if lower.len() != text.len() {
        return escape(text);
    }
    let terms: Vec<String> = terms
        .iter()
        .map(|term| term.to_lowercase())
        .filter(|term| !term.is_empty())
        .collect();
    let mut markup = String::new();
    let mut at = 0;
    while at < text.len() {
        // The earliest match from here on; the longest if several start
        // at the same place.
        let next = terms
            .iter()
            .filter_map(|term| {
                lower[at..]
                    .find(term.as_str())
                    .map(|start| (at + start, term.len()))
            })
            .min_by_key(|&(start, length)| (start, std::cmp::Reverse(length)));
        let Some((start, length)) = next else {
            break;
        };
        if !text.is_char_boundary(start) || !text.is_char_boundary(start + length) {
            break;
        }
        markup.push_str(&escape(&text[at..start]));
        markup.push_str("<b>");
        markup.push_str(&escape(&text[start..start + length]));
        markup.push_str("</b>");
        at = start + length;
    }
    markup.push_str(&escape(&text[at..]));
    markup
}

struct Inner {
    services: Rc<Services>,
    scroll: gtk::ScrolledWindow,
    content: gtk::Box,
    status: gtk::Box,
    status_text: gtk::Label,
    /// Every result that is shown, in the order the arrow keys take, with
    /// the widget that shows it.
    shown: RefCell<Vec<(Found, gtk::Widget)>>,
}

pub struct SearchResults {
    pub widget: gtk::Box,
    inner: Rc<Inner>,
    _subscriptions: Vec<Subscription>,
}

/// One list of results in its box, with the gap under it.
fn section(child: &impl IsA<gtk::Widget>) -> gtk::Box {
    let section = gtk::Box::builder()
        .css_classes(["search-section"])
        .orientation(gtk::Orientation::Vertical)
        .build();
    section.append(child);
    section.append(
        &gtk::Box::builder()
            .css_classes(["search-section-separator"])
            .build(),
    );
    section
}

impl Inner {
    /// How many applications fit the row (`GridSearchResults.
    /// _getMaxDisplayedResults`).
    fn app_columns(&self) -> usize {
        let width = match self.content.width() {
            0 => MAX_WIDTH,
            width => width,
        };
        // A tile is its icon, the label under it and the tile's padding;
        // one is measured rather than the sum trusted.
        let tile = Tile::new(GRID_ICON_SIZE);
        tile.set("", None);
        let (_, item, _, _) = tile.button.measure(gtk::Orientation::Vertical, -1);
        layout::search_columns(width, item, GRID_SPACING)
    }

    fn found(&self) -> Vec<Found> {
        let terms = self.services.overview.with(super::Overview::terms);
        let columns = self.app_columns();
        self.services.apps.state.with(|apps| {
            self.services
                .search
                .state
                .with(|search| super::found(apps, search, &terms, columns))
        })
    }

    fn app_tile(self: &Rc<Self>, app: &apps::App) -> Tile {
        let tile = Tile::new(GRID_ICON_SIZE);
        tile.button.add_css_class("grid-search-result");
        tile.set(&app.name, app.icon.as_deref());
        tile.button.connect_clicked({
            let (services, app) = (Rc::clone(&self.services), app.clone());
            move |_| super::activate_app(&services, &app)
        });
        tile
    }

    fn provider_info(self: &Rc<Self>, provider: &Provider) -> gtk::Button {
        let image = icon::image(PROVIDER_ICON_SIZE);
        icon::set_app_icon(&image, provider.icon.as_deref());
        // The details are as tall as the icon; the name is in the middle
        // of what the line under it leaves.
        let details = gtk::Box::builder()
            .css_classes(["list-search-provider-details"])
            .orientation(gtk::Orientation::Vertical)
            .build();
        let label = |text: &str| {
            gtk::Label::builder()
                .label(text)
                .halign(gtk::Align::Start)
                .ellipsize(pango::EllipsizeMode::End)
                .build()
        };
        let name = label(&provider.name);
        name.set_vexpand(true);
        name.set_valign(gtk::Align::Center);
        details.append(&name);
        if provider.more() > 0 {
            details.append(&label(&format!("{} more", provider.more())));
        }
        let content = gtk::Box::builder()
            .css_classes(["list-search-provider-content"])
            .build();
        content.append(&image);
        content.append(&details);
        let button = gtk::Button::builder()
            .css_classes(["search-provider-icon"])
            .child(&content)
            .valign(gtk::Align::Start)
            .focusable(false)
            .build();
        button.update_property(&[gtk::accessible::Property::Label(&provider.name)]);
        // A click opens the application on the search.
        button.connect_clicked({
            let (services, provider) = (Rc::clone(&self.services), provider.id.clone());
            move |_| {
                let terms = services.overview.with(super::Overview::terms);
                services.search.send(search::Command::LaunchSearch {
                    provider: provider.clone(),
                    terms,
                });
                services.close_overview();
            }
        });
        button
    }

    fn list_result(
        self: &Rc<Self>,
        provider: &Provider,
        result: &ResultItem,
        terms: &[String],
    ) -> gtk::Button {
        let title = gtk::Box::builder()
            .css_classes(["list-search-result-title"])
            .valign(gtk::Align::Center)
            .build();
        // Without a picture GNOME shows none at all.
        if result.icon.is_some() {
            let image = icon::image(LIST_ICON_SIZE);
            picture::show(&image, result.icon.as_ref(), FALLBACK_ICON);
            title.append(&image);
        }
        title.append(
            &gtk::Label::builder()
                .label(&result.name)
                .valign(gtk::Align::Center)
                .ellipsize(pango::EllipsizeMode::End)
                .build(),
        );
        let content = gtk::Box::builder()
            .css_classes(["list-search-result-content"])
            .halign(gtk::Align::Start)
            .build();
        content.append(&title);
        // Only the first line of a description, with the terms in bold.
        let description = result
            .description
            .as_deref()
            .and_then(|description| description.lines().next())
            .filter(|line| !line.is_empty());
        if let Some(description) = description {
            content.append(
                &gtk::Label::builder()
                    .css_classes(["list-search-result-description"])
                    .label(highlight(description, terms))
                    .use_markup(true)
                    .valign(gtk::Align::Center)
                    .ellipsize(pango::EllipsizeMode::End)
                    .build(),
            );
        }
        let button = gtk::Button::builder()
            .css_classes(["list-search-result"])
            .child(&content)
            .focusable(false)
            .build();
        button.connect_clicked({
            let services = Rc::clone(&self.services);
            let found = Found::Result {
                provider: provider.id.clone(),
                result: result.id.clone(),
            };
            move |_| {
                let terms = services.overview.with(super::Overview::terms);
                super::activate_found(&services, &found, &terms);
            }
        });
        button
    }

    /// Builds the lists again for what was found.
    fn rebuild(self: &Rc<Self>) {
        while let Some(child) = self.content.first_child() {
            self.content.remove(&child);
        }
        let mut shown: Vec<(Found, gtk::Widget)> = Vec::new();
        let terms = self.services.overview.with(super::Overview::terms);
        let found = self.found();

        // The applications, in one row in the middle.
        let apps: Vec<apps::App> = self.services.apps.state.with(|apps| {
            found
                .iter()
                .filter_map(|found| match found {
                    Found::App(id) => apps.app(id).cloned(),
                    Found::Result { .. } => None,
                })
                .collect()
        });
        if !apps.is_empty() {
            let row = gtk::Box::builder()
                .css_classes(["grid-search-results"])
                .halign(gtk::Align::Center)
                .spacing(GRID_SPACING)
                .build();
            for app in &apps {
                let tile = self.app_tile(app);
                row.append(&tile.button);
                shown.push((Found::App(app.id.clone()), tile.button.upcast()));
            }
            self.content.append(&section(&row));
        }

        // Then each provider that found something.
        self.services.search.state.with(|search| {
            for provider in search.shown() {
                let results = gtk::Box::builder()
                    .css_classes(["list-search-results"])
                    .orientation(gtk::Orientation::Vertical)
                    .hexpand(true)
                    .build();
                for result in &provider.results {
                    let button = self.list_result(provider, result, &terms);
                    results.append(&button);
                    shown.push((
                        Found::Result {
                            provider: provider.id.clone(),
                            result: result.id.clone(),
                        },
                        button.upcast(),
                    ));
                }
                let content = gtk::Box::builder()
                    .css_classes(["search-section-content"])
                    .build();
                content.append(&self.provider_info(provider));
                content.append(&results);
                self.content.append(&section(&content));
            }
        });

        // `SearchResultsView._updateSearchProgress`.
        let have_results = !shown.is_empty();
        self.scroll.set_visible(have_results);
        self.status.set_visible(!have_results);
        if !have_results {
            let searching = self.services.search.state.with(search::State::searching);
            self.status_text
                .set_label(if searching { "Searching" } else { "No results" });
        }
        self.shown.replace(shown);
        self.select();
    }

    /// Shows which result Enter would take: the one the arrow keys are on,
    /// or the first (`SearchResultsView._maybeSetInitialSelection`).
    fn select(&self) {
        let (selected, quiet) = self
            .services
            .overview
            .with(|overview| (overview.selected, overview.quiet));
        let selected = if quiet {
            None
        } else {
            Some(selected.unwrap_or(0))
        };
        for (index, (_, widget)) in self.shown.borrow().iter().enumerate() {
            set_class(widget, "selected", selected == Some(index));
        }
    }
}

impl SearchResults {
    /// Everything that is shown, in the order the arrow keys take.
    pub fn found(&self) -> Vec<Found> {
        self.inner
            .shown
            .borrow()
            .iter()
            .map(|(found, _)| found.clone())
            .collect()
    }
}

pub fn build(services: &Rc<Services>) -> SearchResults {
    let content = gtk::Box::builder()
        .name("searchResultsContent")
        .orientation(gtk::Orientation::Vertical)
        .valign(gtk::Align::Start)
        .build();
    // `MaxWidthBox`: as wide as there is room, up to the theme's limit,
    // in the middle.
    let clamp = adw::Clamp::builder()
        .maximum_size(MAX_WIDTH)
        .tightening_threshold(MAX_WIDTH)
        .child(&content)
        .build();
    let scroll = gtk::ScrolledWindow::builder()
        .css_classes(["search-display"])
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&clamp)
        .hexpand(true)
        .vexpand(true)
        .build();
    let status_text = gtk::Label::builder()
        .css_classes(["search-statustext"])
        .build();
    let status = gtk::Box::builder()
        .css_classes(["search-statusbox"])
        .orientation(gtk::Orientation::Vertical)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .hexpand(true)
        .vexpand(true)
        .visible(false)
        .build();
    status.append(&status_text);

    let widget = gtk::Box::builder()
        .name("searchResults")
        .orientation(gtk::Orientation::Vertical)
        .build();
    widget.append(&scroll);
    widget.append(&status);

    let inner = Rc::new(Inner {
        services: Rc::clone(services),
        scroll,
        content,
        status,
        status_text,
        shown: RefCell::new(Vec::new()),
    });

    let rebuild = {
        let inner = Rc::downgrade(&inner);
        move || {
            if let Some(inner) = inner.upgrade() {
                inner.rebuild();
            }
        }
    };
    let subscriptions = vec![
        services.overview.select(|overview| overview.terms(), {
            let rebuild = rebuild.clone();
            move |_| rebuild()
        }),
        services.apps.state.select(|apps| apps.installed.clone(), {
            let rebuild = rebuild.clone();
            move |_| rebuild()
        }),
        services
            .search
            .state
            .select(|search| (search.providers.clone(), search.terms.clone()), {
                let rebuild = rebuild.clone();
                move |_| rebuild()
            }),
        services
            .overview
            .select(|overview| (overview.selected, overview.quiet), {
                let inner = Rc::downgrade(&inner);
                move |_| {
                    if let Some(inner) = inner.upgrade() {
                        inner.select();
                    }
                }
            }),
    ];

    SearchResults {
        widget,
        inner,
        _subscriptions: subscriptions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn terms_are_set_in_bold_whatever_their_case() {
        assert_eq!(
            highlight("A feature rich calculator", &terms("calculator")),
            "A feature rich <b>calculator</b>"
        );
        assert_eq!(
            highlight("AWESOME calculator program", &terms("CALC awe")),
            "<b>AWE</b>SOME <b>calc</b>ulator program"
        );
        assert_eq!(highlight("Nothing here", &terms("calc")), "Nothing here");
        assert_eq!(highlight("Plain", &[]), "Plain");
    }

    #[test]
    fn markup_in_a_description_is_shown_not_obeyed() {
        assert_eq!(
            highlight("a <b>bold</b> & b", &terms("bold")),
            "a &lt;b&gt;<b>bold</b>&lt;/b&gt; &amp; b"
        );
    }

    #[test]
    fn the_longer_of_two_terms_at_one_place_wins() {
        assert_eq!(
            highlight("calculator", &terms("calc calculator")),
            "<b>calculator</b>"
        );
        // Text whose lower case is longer is left alone.
        assert_eq!(highlight("İstanbul", &terms("i")), "İstanbul");
    }
}
