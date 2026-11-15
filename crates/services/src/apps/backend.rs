// SPDX-License-Identifier: GPL-3.0-or-later

//! Reads the installed applications from their desktop entries, on a thread
//! of its own: a few hundred small files, which must not hold up a frame.

use std::path::PathBuf;

use crate::desktop_entry::{self, DesktopEntry};
use crate::model::Link;

use super::{App, Command, Event, State};

/// The applications a list of entries stands for, on a desktop called any
/// of `desktops`.
pub fn shown(entries: Vec<DesktopEntry>, desktops: &[String]) -> Vec<App> {
    entries
        .into_iter()
        .filter(|entry| entry.should_show(desktops))
        .map(App::from)
        .collect()
}

/// Starts the backend for the directories applications install into.
pub fn spawn() -> Link<State> {
    spawn_for(
        desktop_entry::application_dirs(),
        desktop_entry::current_desktops(),
    )
}

/// The same for these directories and desktop names.
pub fn spawn_for(dirs: Vec<PathBuf>, desktops: Vec<String>) -> Link<State> {
    let (link, events, commands) = Link::new();
    let spawned = std::thread::Builder::new()
        .name("apps".to_owned())
        .spawn(move || {
            let mut last = None;
            loop {
                let apps = shown(DesktopEntry::all_in(&dirs), &desktops);
                // Nothing changed, nothing to say: most refreshes.
                if last.as_ref() != Some(&apps) {
                    last = Some(apps.clone());
                    if events.send_blocking(Event::Installed(apps)).is_err() {
                        return;
                    }
                }
                match commands.recv_blocking() {
                    Ok(Command::Refresh) => {
                        // Several may have queued up; one look answers all.
                        while commands.try_recv().is_ok() {}
                    }
                    Err(_) => return,
                }
            }
        });
    if let Err(error) = spawned {
        eprintln!("gniri: cannot start the applications thread: {error}");
    }
    link
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_what_is_installed_and_looks_again_when_asked() {
        let root = std::env::temp_dir().join(format!("gniri-apps-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let write = |name: &str, text: &str| {
            std::fs::write(root.join(name), format!("[Desktop Entry]\n{text}")).unwrap();
        };
        write("b.desktop", "Name=Beta\nExec=beta");
        write("hidden.desktop", "Name=Hidden\nNoDisplay=true");
        write("kde.desktop", "Name=For KDE\nOnlyShowIn=KDE;");

        let link = spawn_for(vec![root.clone()], vec!["niri".to_owned()]);
        let Event::Installed(apps) = link.events.recv_blocking().unwrap();
        assert_eq!(apps.len(), 1);
        assert_eq!((apps[0].id.as_str(), apps[0].name.as_str()), ("b", "Beta"));

        // Asked again with nothing new: no event. Then something new.
        link.commands.send_blocking(Command::Refresh).unwrap();
        write("a.desktop", "Name=Alpha");
        link.commands.send_blocking(Command::Refresh).unwrap();
        let Event::Installed(apps) = link.events.recv_blocking().unwrap();
        let mut names: Vec<&str> = apps.iter().map(|app| app.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["Alpha", "Beta"]);
        std::fs::remove_dir_all(root).unwrap();
    }
}
