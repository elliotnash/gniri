// SPDX-License-Identifier: GPL-3.0-or-later

//! A user's picture in a circle. Port of `Avatar` in `js/ui/userWidget.js`.
//!
//! The theme gives it its size, which differs between the authentication
//! dialog and the lock screen. As in GNOME the picture is the widget's
//! background, scaled to cover it: a child picture would bring a size of
//! its own.

use gniri_services::user::User;
use gtk::gio;
use gtk::prelude::*;

use crate::style;

/// A style class for the picture of the user called `name`.
fn picture_class(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect();
    format!("user-avatar-of-{safe}")
}

/// Builds the avatar of `user`: the picture, or the icon GNOME shows for a
/// user without one.
pub fn build(user: &User) -> gtk::Box {
    let avatar = gtk::Box::builder()
        .css_classes(["user-icon", "user-avatar"])
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        // Said outright, so that nothing inside can make it grow.
        .hexpand(false)
        .vexpand(false)
        // The picture is cut to the circle the theme rounds the box to.
        .overflow(gtk::Overflow::Hidden)
        .build();
    match user.avatar.as_ref().filter(|path| path.is_file()) {
        Some(path) => {
            // `Avatar.update`: `background-image: url(...); background-size:
            // cover`. The file's address is escaped by GIO.
            let class = picture_class(&user.name);
            let address = gio::File::for_path(path).uri();
            style::add_rule(&format!(
                ".user-avatar.{class} {{ background-image: url(\"{address}\"); \
                 background-size: cover; background-position: center; }}"
            ));
            avatar.add_css_class(&class);
        }
        None => {
            let icon = gtk::Image::builder()
                .icon_name("avatar-default-symbolic")
                .hexpand(true)
                .build();
            avatar.append(&icon);
        }
    }
    avatar
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_login_name_becomes_a_plain_class() {
        assert_eq!(picture_class("ada"), "user-avatar-of-ada");
        assert_eq!(picture_class("a.b c\"}{"), "user-avatar-of-a-b-c---");
    }
}
