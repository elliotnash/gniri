//! Desktop entries: the name, icon and command of an installed application.
//!
//! A notification or a media player names its application by desktop entry;
//! GNOME Shell shows that application's name and icon and starts it on a
//! click. This reads the few keys that takes straight from the `.desktop`
//! file. Names are not translated, like the rest of gniri's text.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DesktopEntry {
    /// The file's name without ".desktop".
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    /// The `Exec` key as written.
    pub exec: Option<String>,
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
            match key.trim() {
                "Name" => entry.name = value.to_owned(),
                "Icon" => entry.icon = Some(value.to_owned()).filter(|icon| !icon.is_empty()),
                "Exec" => entry.exec = Some(value.to_owned()).filter(|exec| !exec.is_empty()),
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

    /// The command that starts the application with no files: `Exec` split
    /// into arguments as the desktop entry specification says, without its
    /// field codes.
    pub fn command(&self) -> Option<Vec<String>> {
        let exec = self.exec.as_deref()?;
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
                        current.push_str(&self.name);
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
