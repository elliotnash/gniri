// SPDX-License-Identifier: GPL-3.0-or-later

//! The search entry at the top of the overview. Port of `SearchEntry` in
//! `js/ui/search.js` and of the entry's part of `SearchController`
//! (searchController.js).
//!
//! Widget tree, with the actors it stands for:
//!
//! ```text
//! box.st-entry.search-entry           SearchEntry (an StEntry)
//!   image.search-entry-icon             its primary icon
//!   text                                its ClutterText
//!     placeholder                         StLabel.hint-text
//!   image.search-entry-icon             its secondary icon, to clear it
//! ```
//!
//! GNOME gives the entry the keyboard focus when the first letter is
//! typed. Here the overview's surface has the keyboard for as long as it
//! is open (docs/DECISIONS.md, S9) and the text field has the focus
//! inside it all along, so the focused look is a style class that goes by
//! whether anything is typed.

use gniri_core::tr;
use std::cell::Cell;
use std::rc::Rc;

use gniri_core::Subscription;
use gtk::prelude::*;

use super::icon::set_class;
use crate::services::Services;

pub struct SearchEntry {
    pub widget: gtk::Box,
    text: gtk::Text,
    _subscriptions: Vec<Subscription>,
}

impl SearchEntry {
    /// Puts the cursor in the entry: every key that is typed goes there.
    pub fn grab_focus(&self) {
        self.text.grab_focus_without_selecting();
    }

    /// Does what [`SearchEntry::grab_focus`] does, from anywhere.
    pub fn focuser(&self) -> impl Fn() + 'static {
        let text = self.text.downgrade();
        move || {
            if let Some(text) = text.upgrade() {
                text.grab_focus_without_selecting();
            }
        }
    }
}

fn icon(name: &str) -> gtk::Image {
    gtk::Image::builder()
        .icon_name(name)
        .css_classes(["search-entry-icon"])
        .valign(gtk::Align::Center)
        .build()
}

pub fn build(services: &Rc<Services>) -> SearchEntry {
    let text = gtk::Text::builder()
        .placeholder_text(tr("Type to search"))
        .input_hints(gtk::InputHints::NO_EMOJI | gtk::InputHints::NO_SPELLCHECK)
        .enable_emoji_completion(false)
        .hexpand(true)
        .valign(gtk::Align::Center)
        // The entry's width is the theme's alone to decide.
        .width_chars(1)
        .max_width_chars(1)
        .build();
    let clear = icon("edit-clear-symbolic");
    clear.set_visible(false);

    let widget = gtk::Box::builder()
        .css_classes(["st-entry", "search-entry"])
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Start)
        .build();
    widget.append(&icon("edit-find-symbolic"));
    widget.append(&text);
    widget.append(&clear);

    // What is typed is searched for. `setting` is true while the text is
    // being put there from the state, which must not come back as typing.
    let setting = Rc::new(Cell::new(false));
    text.connect_changed({
        let (services, setting) = (Rc::clone(services), Rc::clone(&setting));
        move |text| {
            if !setting.get() {
                let query = text.text();
                services
                    .overview
                    .update(|overview| overview.set_query(&query));
            }
        }
    });

    // A click on the second icon empties the entry.
    let click = gtk::GestureClick::new();
    click.connect_released({
        let services = Rc::clone(services);
        move |_, _, _, _| services.overview.update(|overview| overview.set_query(""))
    });
    clear.add_controller(click);

    let subscriptions = vec![
        // Escape, a new overview or a script changed the search.
        services
            .overview
            .select(|overview| overview.query.clone(), {
                let (text, clear) = (text.clone(), clear.clone());
                move |query: &String| {
                    if text.text().as_str() != query.as_str() {
                        setting.set(true);
                        text.set_text(query);
                        text.set_position(-1);
                        setting.set(false);
                    }
                    clear.set_visible(!query.is_empty());
                }
            }),
        services
            .overview
            .select(|overview| !overview.query.is_empty() && !overview.quiet, {
                let widget = widget.clone();
                move |focused| set_class(&widget, "focused", *focused)
            }),
    ];

    SearchEntry {
        widget,
        text,
        _subscriptions: subscriptions,
    }
}
