//! Which menu is open.
//!
//! One piece of state shared by the panel (whose buttons show as pressed
//! while their menu is open), the stage (which shows the menu) and the debug
//! interface (which opens menus for screenshots).

use serde::{Deserialize, Serialize};

/// A menu that opens from the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
pub enum Menu {
    QuickSettings,
}

/// A menu inside the quick settings menu, opened from one of its items.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
pub enum Submenu {
    Wifi,
    Bluetooth,
    PowerMode,
    /// The output device list beside the volume slider.
    SoundOutput,
    SoundInput,
    /// Suspend, log out and so on, from the power button.
    Shutdown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Open {
    pub menu: Menu,
    /// Connector name of the output it is shown on. `None` matches an
    /// output without a name.
    pub output: Option<String>,
    pub submenu: Option<Submenu>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Menus {
    pub open: Option<Open>,
}

impl Menus {
    pub fn is_open(&self, menu: Menu, output: Option<&str>) -> bool {
        self.open
            .as_ref()
            .is_some_and(|open| open.menu == menu && open.output.as_deref() == output)
    }

    pub fn open(&mut self, menu: Menu, output: Option<&str>) {
        if !self.is_open(menu, output) {
            self.open = Some(Open {
                menu,
                output: output.map(str::to_owned),
                submenu: None,
            });
        }
    }

    pub fn close(&mut self) {
        self.open = None;
    }

    /// What clicking a panel button does: open its menu, or close it if it
    /// is the one showing.
    pub fn toggle(&mut self, menu: Menu, output: Option<&str>) {
        if self.is_open(menu, output) {
            self.close();
        } else {
            self.close();
            self.open(menu, output);
        }
    }

    pub fn submenu(&self) -> Option<Submenu> {
        self.open.as_ref().and_then(|open| open.submenu)
    }

    /// Opens `submenu` (closing any other), or closes it with `None`. Does
    /// nothing unless the quick settings menu is open.
    pub fn set_submenu(&mut self, submenu: Option<Submenu>) {
        if let Some(open) = &mut self.open
            && open.menu == Menu::QuickSettings
        {
            open.submenu = submenu;
        }
    }

    /// What Escape does: close the innermost thing that is open.
    pub fn back(&mut self) {
        if self.submenu().is_some() {
            self.set_submenu(None);
        } else {
            self.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_buttons_toggle_their_menu_per_output() {
        let mut menus = Menus::default();
        menus.toggle(Menu::QuickSettings, Some("eDP-1"));
        assert!(menus.is_open(Menu::QuickSettings, Some("eDP-1")));
        assert!(!menus.is_open(Menu::QuickSettings, Some("HDMI-A-1")));

        // The same button on another output moves the menu there.
        menus.toggle(Menu::QuickSettings, Some("HDMI-A-1"));
        assert!(menus.is_open(Menu::QuickSettings, Some("HDMI-A-1")));
        menus.toggle(Menu::QuickSettings, Some("HDMI-A-1"));
        assert_eq!(menus, Menus::default());
    }

    #[test]
    fn escape_closes_the_submenu_first() {
        let mut menus = Menus::default();
        menus.set_submenu(Some(Submenu::Wifi));
        assert_eq!(menus.submenu(), None, "no menu is open");

        menus.open(Menu::QuickSettings, None);
        menus.set_submenu(Some(Submenu::Wifi));
        menus.set_submenu(Some(Submenu::PowerMode));
        assert_eq!(menus.submenu(), Some(Submenu::PowerMode));
        menus.back();
        assert!(menus.is_open(Menu::QuickSettings, None));
        assert_eq!(menus.submenu(), None);
        menus.back();
        assert_eq!(menus, Menus::default());
    }

    #[test]
    fn reopening_keeps_the_submenu() {
        let mut menus = Menus::default();
        menus.open(Menu::QuickSettings, None);
        menus.set_submenu(Some(Submenu::Bluetooth));
        menus.open(Menu::QuickSettings, None);
        assert_eq!(menus.submenu(), Some(Submenu::Bluetooth));
    }
}
