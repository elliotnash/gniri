//! Desktop entries: the name, icon and command of an installed application.
//!
//! A notification or a media player names its application by desktop entry;
//! GNOME Shell shows that application's name and icon and starts it on a
//! click. This reads the few keys that takes straight from the `.desktop`
//! file. Names are not translated, like the rest of gniri's text.
//!
//! The overview lists every installed application ([`DesktopEntry::all`]),
//! which takes a few more keys: whether an entry is meant to be shown, and
//! what a search looks at.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DesktopEntry {
    /// The file's name without ".desktop".
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    /// The `Exec` key as written.
    pub exec: Option<String>,
    pub generic_name: Option<String>,
    pub comment: Option<String>,
    pub keywords: Vec<String>,
    /// The window class (on Wayland the application id) of the windows it
    /// opens, where that is not the entry's own id.
    pub startup_wm_class: Option<String>,
    /// `NoDisplay`: installed, but not for menus.
    pub no_display: bool,
    /// `Terminal`: the command wants a terminal to run in.
    pub terminal: bool,
    /// `OnlyShowIn` and `NotShowIn`: desktop names.
    pub only_show_in: Vec<String>,
    pub not_show_in: Vec<String>,
    /// `TryExec`: a program that must exist for the entry to count.
    pub try_exec: Option<String>,
}

/// A `;`-separated list, as the specification writes them.
fn list(value: &str) -> Vec<String> {
    value
        .split(';')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The desktop names an entry's `OnlyShowIn` and `NotShowIn` are held
/// against: `$XDG_CURRENT_DESKTOP`.
pub fn current_desktops() -> Vec<String> {
    std::env::var("XDG_CURRENT_DESKTOP")
        .map(|names| {
            names
                .split(':')
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// True if `program` is an executable on `PATH` (or a path to one).
fn on_path(program: &str) -> bool {
    if program.contains('/') {
        return Path::new(program).is_file();
    }
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

/// The directories applications install their entries in, most specific
/// first (the XDG base directory specification).
pub fn application_dirs() -> Vec<PathBuf> {
    let var = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
    let mut dirs = Vec::new();
    match var("XDG_DATA_HOME") {
        Some(home) => dirs.push(PathBuf::from(home)),
        None => {
            if let Some(home) = var("HOME") {
                dirs.push(PathBuf::from(home).join(".local/share"));
            }
        }
    }
    match var("XDG_DATA_DIRS") {
        Some(list) => dirs.extend(std::env::split_paths(&list)),
        None => dirs.extend(["/usr/local/share", "/usr/share"].map(PathBuf::from)),
    }
    dirs.into_iter()
        .map(|dir| dir.join("applications"))
        .collect()
}

impl DesktopEntry {
    /// Reads the `[Desktop Entry]` group of a file's text.
    pub fn parse(id: &str, text: &str) -> Option<Self> {
        let mut entry = Self {
            id: id.to_owned(),
            ..Self::default()
        };
        let mut in_group = false;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_group = line == "[Desktop Entry]";
                continue;
            }
            if !in_group || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            let text = || Some(value.to_owned()).filter(|value| !value.is_empty());
            match key.trim() {
                "Name" => entry.name = value.to_owned(),
                "Icon" => entry.icon = text(),
                "Exec" => entry.exec = text(),
                "GenericName" => entry.generic_name = text(),
                "Comment" => entry.comment = text(),
                "Keywords" => entry.keywords = list(value),
                "StartupWMClass" => entry.startup_wm_class = text(),
                "NoDisplay" => entry.no_display = value == "true",
                "Terminal" => entry.terminal = value == "true",
                "OnlyShowIn" => entry.only_show_in = list(value),
                "NotShowIn" => entry.not_show_in = list(value),
                "TryExec" => entry.try_exec = text(),
                "Type" if value != "Application" => return None,
                "Hidden" if value == "true" => return None,
                _ => {}
            }
        }
        (!entry.name.is_empty()).then_some(entry)
    }

    /// Finds `<id>.desktop` in `dirs`; the first directory that has it wins.
    pub fn find_in(id: &str, dirs: &[PathBuf]) -> Option<Self> {
        // An id is a file name, never a path.
        if id.is_empty() || id.contains('/') {
            return None;
        }
        let id = id.strip_suffix(".desktop").unwrap_or(id);
        dirs.iter().find_map(|dir| {
            let path: &Path = dir;
            let text = std::fs::read_to_string(path.join(format!("{id}.desktop"))).ok()?;
            Self::parse(id, &text)
        })
    }

    /// Finds an installed application's entry.
    pub fn find(id: &str) -> Option<Self> {
        Self::find_in(id, &application_dirs())
    }

    /// Every entry in `dirs`, by id. A directory earlier in the list hides
    /// the entries of the same id in later ones, also where its own file
    /// says `Hidden` or is no application: that is how a user removes an
    /// application the system installs. A file in a subdirectory has the
    /// directory's name in its id, joined with a dash.
    pub fn all_in(dirs: &[PathBuf]) -> Vec<Self> {
        fn visit(
            dir: &Path,
            prefix: &str,
            seen: &mut std::collections::HashSet<String>,
            found: &mut Vec<DesktopEntry>,
            fresh: &mut Vec<String>,
        ) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                if path.is_dir() {
                    visit(&path, &format!("{prefix}{name}-"), seen, found, fresh);
                } else if let Some(stem) = name.strip_suffix(".desktop") {
                    let id = format!("{prefix}{stem}");
                    if seen.contains(&id) {
                        continue;
                    }
                    fresh.push(id.clone());
                    if let Some(entry) = std::fs::read_to_string(&path)
                        .ok()
                        .and_then(|text| DesktopEntry::parse(&id, &text))
                    {
                        found.push(entry);
                    }
                }
            }
        }

        let mut seen = std::collections::HashSet::new();
        let mut found = Vec::new();
        for dir in dirs {
            // Ids only count as seen once their directory is done: two
            // files of one directory never hide each other.
            let mut fresh = Vec::new();
            visit(dir, "", &mut seen, &mut found, &mut fresh);
            seen.extend(fresh);
        }
        found.sort_by(|a, b| a.id.cmp(&b.id));
        found
    }

    /// Every installed application's entry.
    pub fn all() -> Vec<Self> {
        Self::all_in(&application_dirs())
    }

    /// Whether the entry belongs in a list of applications on a desktop
    /// called any of `desktops`: `g_app_info_should_show`, and GIO's rule
    /// that an entry whose `TryExec` program is missing does not exist.
    ///
    /// gniri stands in for GNOME Shell and starts GNOME's own programs
    /// (Settings is `OnlyShowIn=GNOME`), so an entry made for GNOME alone
    /// is shown as well. `NotShowIn` goes by the real desktop names only.
    pub fn should_show(&self, desktops: &[String]) -> bool {
        let named = |list: &[String]| list.iter().any(|name| desktops.contains(name));
        !self.no_display
            && (self.only_show_in.is_empty()
                || named(&self.only_show_in)
                || self.only_show_in.iter().any(|name| name == "GNOME"))
            && !named(&self.not_show_in)
            && self.try_exec.as_deref().is_none_or(on_path)
    }

    /// The command that starts the application with no files: `Exec` split
    /// into arguments as the desktop entry specification says, without its
    /// field codes.
    pub fn command(&self) -> Option<Vec<String>> {
        split_exec(self.exec.as_deref()?, &self.name)
    }
}

/// Splits an `Exec` value into arguments as the desktop entry specification
/// says, leaving out the field codes that stand for files. `name` is what
/// `%c` stands for.
pub fn split_exec(exec: &str, name: &str) -> Option<Vec<String>> {
    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut has_current = false;
    let mut quoted = false;
    let mut characters = exec.chars();
    while let Some(character) = characters.next() {
        match character {
            '"' => {
                quoted = !quoted;
                has_current = true;
            }
            '\\' if quoted => {
                if let Some(next) = characters.next() {
                    current.push(next);
                }
            }
            ' ' | '\t' if !quoted => {
                if has_current {
                    arguments.push(std::mem::take(&mut current));
                    has_current = false;
                }
            }
            '%' if !quoted => match characters.next() {
                Some('%') => {
                    current.push('%');
                    has_current = true;
                }
                Some('c') => {
                    current.push_str(name);
                    has_current = true;
                }
                // %f, %u, %i and the rest: no files, no icon argument.
                _ => {}
            },
            other => {
                current.push(other);
                has_current = true;
            }
        }
    }
    if quoted {
        return None;
    }
    if has_current {
        arguments.push(current);
    }
    (!arguments.is_empty()).then_some(arguments)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOFTWARE: &str = "\
# A comment
[Desktop Entry]
Name=Software
Name[de]=Software-Verwaltung
Icon=org.gnome.Software
Exec=gnome-software %U
Type=Application

[Desktop Action search]
Name=Search
Exec=gnome-software --search
";

    #[test]
    fn reads_the_main_group_only() {
        let entry = DesktopEntry::parse("org.gnome.Software", SOFTWARE).unwrap();
        assert_eq!(entry.name, "Software");
        assert_eq!(entry.icon.as_deref(), Some("org.gnome.Software"));
        assert_eq!(entry.command().unwrap(), ["gnome-software"]);
    }

    #[test]
    fn rejects_what_is_not_an_application() {
        assert_eq!(
            DesktopEntry::parse("x", "[Desktop Entry]\nType=Link\nName=X"),
            None
        );
        assert_eq!(
            DesktopEntry::parse("x", "[Desktop Entry]\nName=X\nHidden=true"),
            None
        );
        assert_eq!(DesktopEntry::parse("x", "[Desktop Entry]\nIcon=x"), None);
    }

    #[test]
    fn commands_follow_the_quoting_rules() {
        let entry = |exec: &str| DesktopEntry {
            name: "My App".to_owned(),
            exec: Some(exec.to_owned()),
            ..DesktopEntry::default()
        };
        assert_eq!(
            entry("env A=1 \"/opt/my app/run\" --title %c %F")
                .command()
                .unwrap(),
            ["env", "A=1", "/opt/my app/run", "--title", "My App"]
        );
        assert_eq!(
            entry("sh -c \"echo \\\"hi\\\" 100%%\"").command().unwrap(),
            ["sh", "-c", "echo \"hi\" 100%%"]
        );
        assert_eq!(
            entry("printf 100%% \"\"").command().unwrap(),
            ["printf", "100%", ""]
        );
        assert_eq!(entry("\"unterminated").command(), None);
        assert_eq!(entry("%U").command(), None);
    }

    #[test]
    fn reads_what_the_overview_needs() {
        let entry = DesktopEntry::parse(
            "org.example.Editor",
            "[Desktop Entry]\nName=Editor\nGenericName=Text Editor\n\
             Comment=Edit text files\nKeywords=write;notes;\n\
             StartupWMClass=editor-window\nTerminal=true\nNoDisplay=true\n\
             OnlyShowIn=GNOME;Unity;\nNotShowIn=KDE;\nTryExec=/nonexistent/editor\n",
        )
        .unwrap();
        assert_eq!(entry.generic_name.as_deref(), Some("Text Editor"));
        assert_eq!(entry.comment.as_deref(), Some("Edit text files"));
        assert_eq!(entry.keywords, ["write", "notes"]);
        assert_eq!(entry.startup_wm_class.as_deref(), Some("editor-window"));
        assert!(entry.terminal && entry.no_display);
        assert_eq!(entry.only_show_in, ["GNOME", "Unity"]);
        assert_eq!(entry.not_show_in, ["KDE"]);
        assert_eq!(entry.try_exec.as_deref(), Some("/nonexistent/editor"));
    }

    #[test]
    fn an_entry_is_shown_unless_it_says_otherwise() {
        let niri = ["niri".to_owned()];
        let entry = |text: &str| {
            DesktopEntry::parse("x", &format!("[Desktop Entry]\nName=X\n{text}")).unwrap()
        };
        assert!(entry("").should_show(&niri));
        assert!(!entry("NoDisplay=true").should_show(&niri));
        assert!(!entry("OnlyShowIn=KDE;").should_show(&niri));
        assert!(entry("OnlyShowIn=KDE;niri;").should_show(&niri));
        // Made for GNOME: gniri shows it too.
        assert!(entry("OnlyShowIn=GNOME;").should_show(&niri));
        assert!(!entry("NotShowIn=niri;").should_show(&niri));
        assert!(entry("NotShowIn=GNOME;").should_show(&niri));
        assert!(!entry("TryExec=/nonexistent/program").should_show(&niri));
        assert!(entry("TryExec=/bin/sh").should_show(&niri));
        assert!(entry("TryExec=sh").should_show(&niri));
    }

    #[test]
    fn lists_every_entry_once_and_lets_a_user_hide_one() {
        let root = std::env::temp_dir().join(format!("gniri-desktop-all-{}", std::process::id()));
        let (user, system) = (root.join("user"), root.join("system"));
        std::fs::create_dir_all(&user).unwrap();
        std::fs::create_dir_all(system.join("kde4")).unwrap();
        let write = |dir: &Path, name: &str, text: &str| {
            std::fs::write(dir.join(name), format!("[Desktop Entry]\n{text}")).unwrap();
        };
        write(&system, "a.desktop", "Name=System A");
        write(&system, "b.desktop", "Name=B");
        write(&system, "c.desktop", "Name=C");
        write(&system.join("kde4"), "d.desktop", "Name=D");
        write(&system, "readme.txt", "Name=Not an entry");
        write(&user, "a.desktop", "Name=User A");
        write(&user, "b.desktop", "Name=B\nHidden=true");

        let entries = DesktopEntry::all_in(&[user, system]);
        let found: Vec<(&str, &str)> = entries
            .iter()
            .map(|entry| (entry.id.as_str(), entry.name.as_str()))
            .collect();
        assert_eq!(found, [("a", "User A"), ("c", "C"), ("kde4-d", "D")]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn finds_the_first_matching_file() {
        let root = std::env::temp_dir().join(format!("gniri-desktop-{}", std::process::id()));
        let (first, second) = (root.join("a"), root.join("b"));
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(second.join("app.desktop"), "[Desktop Entry]\nName=Second").unwrap();
        let dirs = [first.clone(), second];
        assert_eq!(DesktopEntry::find_in("app", &dirs).unwrap().name, "Second");
        assert_eq!(
            DesktopEntry::find_in("app.desktop", &dirs).unwrap().id,
            "app"
        );
        std::fs::write(first.join("app.desktop"), "[Desktop Entry]\nName=First").unwrap();
        assert_eq!(DesktopEntry::find_in("app", &dirs).unwrap().name, "First");
        assert_eq!(DesktopEntry::find_in("../app", &dirs), None);
        assert_eq!(DesktopEntry::find_in("missing", &dirs), None);
        std::fs::remove_dir_all(root).unwrap();
    }
}
