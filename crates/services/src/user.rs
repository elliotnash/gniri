// SPDX-License-Identifier: GPL-3.0-or-later

//! Who is logged in: the name, the full name and the picture the lock
//! screen and the authentication dialog show.
//!
//! Read from the files GNOME's account service keeps, not asked of it over
//! the bus: the lock screen should need as little as possible to come up.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct User {
    /// The login name.
    pub name: String,
    /// The full name, empty if the account has none.
    pub real_name: String,
    /// A picture of the user, if there is one.
    pub avatar: Option<PathBuf>,
}

impl User {
    /// What to call the user: the full name where there is one.
    pub fn display_name(&self) -> &str {
        if self.real_name.is_empty() {
            &self.name
        } else {
            &self.real_name
        }
    }

    /// The user this process runs as.
    pub fn current() -> Self {
        // The owner of this process's own entry in /proc.
        let uid = std::fs::metadata("/proc/self")
            .map(|metadata| metadata.uid())
            .ok();
        let account = uid.and_then(|uid| {
            let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
            passwd
                .lines()
                .find_map(|line| account(line, Some(uid), None))
        });
        let (name, real_name) = account.unwrap_or_else(|| {
            // Not in the local file (a directory service): the login name
            // from the environment has to do.
            (std::env::var("USER").unwrap_or_default(), String::new())
        });
        let avatar = avatar(
            &name,
            std::env::var_os("HOME").map(PathBuf::from).as_deref(),
        );
        Self {
            name,
            real_name,
            avatar,
        }
    }

    /// The local account called `name`, for the authentication dialog,
    /// which may ask for another user's password.
    pub fn named(name: &str) -> Self {
        let real_name = std::fs::read_to_string("/etc/passwd")
            .ok()
            .and_then(|passwd| {
                passwd
                    .lines()
                    .find_map(|line| account(line, None, Some(name)))
            })
            .map(|(_, real_name)| real_name)
            .unwrap_or_default();
        Self {
            name: name.to_owned(),
            real_name,
            avatar: avatar(name, None),
        }
    }
}

/// The login name of the local account with this id.
pub fn login_name(uid: u32) -> Option<String> {
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    passwd
        .lines()
        .find_map(|line| account(line, Some(uid), None))
        .map(|(name, _)| name)
}

/// The login name and full name on one line of `/etc/passwd`, if the line
/// is for `uid` or `name` (whichever is given).
fn account(line: &str, uid: Option<u32>, name: Option<&str>) -> Option<(String, String)> {
    let mut fields = line.split(':');
    let login = fields.next()?;
    let _password = fields.next()?;
    let line_uid: u32 = fields.next()?.parse().ok()?;
    let _gid = fields.next()?;
    let gecos = fields.next()?;
    if uid.is_some_and(|uid| uid != line_uid) || name.is_some_and(|name| name != login) {
        return None;
    }
    // The comment field is the full name, then office and phone numbers
    // after commas.
    let real_name = gecos.split(',').next().unwrap_or_default().trim();
    Some((login.to_owned(), real_name.to_owned()))
}

/// The picture GNOME Settings saved for `name`, or the `~/.face` older
/// programs use.
fn avatar(name: &str, home: Option<&Path>) -> Option<PathBuf> {
    let candidates = [
        (!name.is_empty()).then(|| Path::new("/var/lib/AccountsService/icons").join(name)),
        home.map(|home| home.join(".face")),
    ];
    candidates.into_iter().flatten().find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_passwd_line() {
        let line = "ada:x:1000:1000:Ada Lovelace,Room 1,555:/home/ada:/bin/bash";
        assert_eq!(
            account(line, Some(1000), None),
            Some(("ada".to_owned(), "Ada Lovelace".to_owned()))
        );
        assert_eq!(account(line, None, Some("ada")).unwrap().1, "Ada Lovelace");
        assert_eq!(account(line, Some(1001), None), None);
        assert_eq!(account(line, None, Some("ad")), None);
        assert_eq!(
            account("svc:x:990:990::/var/lib/svc:/sbin/nologin", Some(990), None),
            Some(("svc".to_owned(), String::new()))
        );
        assert_eq!(account("not a passwd line", Some(0), None), None);
    }

    #[test]
    fn the_full_name_is_preferred() {
        let mut user = User {
            name: "ada".to_owned(),
            ..User::default()
        };
        assert_eq!(user.display_name(), "ada");
        user.real_name = "Ada Lovelace".to_owned();
        assert_eq!(user.display_name(), "Ada Lovelace");
    }

    #[test]
    fn the_current_user_has_a_name() {
        assert!(!User::current().name.is_empty());
    }
}
