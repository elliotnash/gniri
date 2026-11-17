// SPDX-License-Identifier: GPL-3.0-or-later

//! One set of surfaces per output, kept in step with outputs coming and
//! going and with the settings changing.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gniri_config::Config;
use gniri_core::Subscription;
use gtk::{gdk, gio};

use crate::ready::ReadyTracker;
use crate::services::Services;
use crate::surfaces::banner::{self, Banner, Tray};
use crate::surfaces::osd::{self, Osd};
use crate::surfaces::overview::{self, Overview};
use crate::surfaces::panel::{self, Panel};
use crate::surfaces::stage::{self, Stage};
use crate::surfaces::wallpaper::{self, Wallpaper, WallpaperSource};

/// What gniri shows on one output.
struct Output {
    monitor: gdk::Monitor,
    panel: Option<Panel>,
    wallpaper: Option<Wallpaper>,
    /// Unmapped until a menu opens.
    stage: Stage,
    /// Unmapped until a volume or brightness key is pressed.
    osd: Osd,
    /// Unmapped until a notification asks for a banner.
    banner: Banner,
    /// Unmapped until the compositor opens its overview.
    overview: Option<Overview>,
}

impl Output {
    /// The surfaces that are mapped from the start.
    fn windows(&self) -> impl Iterator<Item = &gtk::ApplicationWindow> {
        self.panel.iter().map(|panel| &panel.window).chain(
            self.wallpaper
                .iter()
                .flat_map(|wallpaper| &wallpaper.windows),
        )
    }

    /// Connector name, such as "eDP-1".
    fn name(&self) -> Option<String> {
        self.monitor.connector().map(Into::into)
    }

    fn set_wallpaper(&mut self, wallpaper: Option<Wallpaper>) {
        // The new one is mapped already: the old goes from under it.
        if let Some(old) = std::mem::replace(&mut self.wallpaper, wallpaper) {
            for window in &old.windows {
                window.destroy();
            }
        }
    }

    fn set_panel(&mut self, panel: Option<Panel>) {
        if let Some(old) = std::mem::replace(&mut self.panel, panel) {
            old.window.destroy();
        }
    }

    fn set_stage(&mut self, stage: Stage) {
        std::mem::replace(&mut self.stage, stage).window.destroy();
    }

    fn set_overview(&mut self, overview: Option<Overview>) {
        if let Some(old) = std::mem::replace(&mut self.overview, overview) {
            old.window.destroy();
        }
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        for window in self.windows() {
            window.destroy();
        }
        self.stage.window.destroy();
        self.osd.window.destroy();
        self.banner.window.destroy();
        if let Some(overview) = &self.overview {
            overview.window.destroy();
        }
    }
}

/// What a change of settings means for the surfaces there are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// Another picture or colour, or none.
    pub wallpaper: bool,
    /// Which outputs have a panel may have changed.
    pub panels: bool,
    /// The menus are built from settings that changed.
    pub menus: bool,
    /// So is what shows on the overview.
    pub overview: bool,
}

impl Changes {
    /// What changing the settings from `old` to `new` touches. The clock
    /// and the battery percentage are not here: the panel follows those
    /// by itself.
    pub fn between(old: &Config, new: &Config) -> Self {
        let panels = old.panel.outputs != new.panel.outputs;
        Self {
            wallpaper: old.wallpaper != new.wallpaper,
            panels,
            // The settings application decides which entries a menu has.
            menus: panels || old.date_menu != new.date_menu || old.commands != new.commands,
            overview: panels || old.dash != new.dash || old.wallpaper != new.wallpaper,
        }
    }
}

/// Says which wallpaper a set of settings asks for.
type WallpaperFor = Box<dyn Fn(&Config) -> Option<WallpaperSource>>;

pub struct Outputs {
    app: adw::Application,
    services: Rc<Services>,
    /// The wallpaper the settings ask for: the command line's, if it
    /// names one, wins over the config file's.
    wallpaper_for: WallpaperFor,
    /// The wallpaper that is drawn.
    wallpaper: RefCell<Option<WallpaperSource>>,
    /// The settings the surfaces were built from.
    applied: RefCell<Config>,
    outputs: RefCell<Vec<Output>>,
    /// Shows notification banners on the focused output.
    tray: RefCell<Option<Tray>>,
    config: RefCell<Option<Subscription>>,
    /// Set by a test: behave as if every output had been unplugged.
    unplugged: Cell<bool>,
}

impl Outputs {
    /// Builds the surfaces for every output there is now and keeps doing so
    /// as outputs appear and disappear. `ready` hears about the surfaces of
    /// the outputs present at the start.
    pub fn start(
        app: &adw::Application,
        services: &Rc<Services>,
        wallpaper_for: impl Fn(&Config) -> Option<WallpaperSource> + 'static,
        ready: &ReadyTracker,
    ) -> Rc<Self> {
        let config = services.config.get();
        let outputs = Rc::new(Self {
            app: app.clone(),
            services: Rc::clone(services),
            wallpaper: RefCell::new(wallpaper_for(&config)),
            wallpaper_for: Box::new(wallpaper_for),
            applied: RefCell::new(config),
            outputs: RefCell::new(Vec::new()),
            tray: RefCell::new(None),
            config: RefCell::new(None),
            unplugged: Cell::new(false),
        });

        let monitors = gdk::Display::default()
            .expect("GTK is initialised, so there is a display")
            .monitors();
        outputs.sync(&monitors);
        for output in outputs.outputs.borrow().iter() {
            for window in output.windows() {
                ready.watch(window);
            }
        }

        outputs.tray.replace(Some(Tray::start(services, {
            let outputs = Rc::downgrade(&outputs);
            move || outputs.upgrade().and_then(|outputs| outputs.banner())
        })));

        monitors.connect_items_changed({
            let outputs = Rc::downgrade(&outputs);
            move |monitors, _, _, _| {
                if let Some(outputs) = outputs.upgrade() {
                    outputs.sync(monitors);
                }
            }
        });
        outputs.config.replace(Some(services.config.subscribe({
            let outputs = Rc::downgrade(&outputs);
            move |config| {
                if let Some(outputs) = outputs.upgrade() {
                    outputs.reconfigure(config);
                }
            }
        })));
        outputs
    }

    fn sync(&self, monitors: &gio::ListModel) {
        let present: Vec<gdk::Monitor> = if self.unplugged.get() {
            Vec::new()
        } else {
            monitors
                .iter::<gdk::Monitor>()
                .filter_map(Result::ok)
                .collect()
        };

        // The surfaces of an output that went are taken out first and
        // dropped after: dropping them runs code that may ask for the
        // outputs there are.
        let (kept, gone): (Vec<Output>, Vec<Output>) = self
            .outputs
            .take()
            .into_iter()
            .partition(|output| present.contains(&output.monitor));
        self.outputs.replace(kept);
        drop(gone);
        for monitor in present {
            let known = self
                .outputs
                .borrow()
                .iter()
                .any(|output| output.monitor == monitor);
            if !known {
                let output = self.build(monitor);
                self.outputs.borrow_mut().push(output);
            }
        }

        // A menu that was open on an output that is gone has no surface to
        // show on, or to be closed from.
        let names: Vec<Option<String>> = self.outputs.borrow().iter().map(Output::name).collect();
        self.services.menus.update(|menus| {
            if menus
                .open
                .as_ref()
                .is_some_and(|open| !names.contains(&open.output))
            {
                menus.close();
            }
        });
        // Nor has a banner that was up there.
        if let Some(tray) = &*self.tray.borrow() {
            tray.outputs_changed();
        }
    }

    /// For tests, which have one output that cannot be unplugged: with
    /// `true`, takes every output's surfaces away as if all outputs had
    /// gone; with `false`, builds them again for the outputs there are.
    pub fn set_unplugged(&self, unplugged: bool) {
        self.unplugged.set(unplugged);
        if let Some(display) = gdk::Display::default() {
            self.sync(&display.monitors());
        }
    }

    /// Whether the settings give `monitor` a panel. An output without a
    /// connector name cannot be excluded by name.
    fn wants_panel(&self, monitor: &gdk::Monitor) -> bool {
        monitor
            .connector()
            .is_none_or(|name| self.applied.borrow().panel.shows_on(&name))
    }

    fn build_wallpaper(&self, monitor: &gdk::Monitor) -> Option<Wallpaper> {
        self.wallpaper
            .borrow()
            .as_ref()
            .map(|source| wallpaper::build(&self.app, monitor, source))
    }

    fn build_stage(&self, monitor: &gdk::Monitor, panel: Option<&Panel>) -> Stage {
        let panel_window = panel.map(|panel| panel.window.downgrade());
        stage::build(&self.app, monitor, &self.services, move || {
            panel_window
                .as_ref()
                .and_then(|window| window.upgrade())
                .map_or(0, |window| window.height())
        })
    }

    /// The dash and the search belong with the panel: an output without
    /// one shows niri's overview as it is. With one, the overview's cover
    /// goes on behind the panel too, which gets a cover from each overview
    /// built for it.
    fn build_overview(
        &self,
        monitor: &gdk::Monitor,
        wallpaper: Option<&Wallpaper>,
        panel: Option<&Panel>,
    ) -> Option<Overview> {
        let panel = panel?;
        let cover = || wallpaper.map_or_else(wallpaper::cover_without_wallpaper, Wallpaper::cover);
        let panel_cover = cover();
        let overview = overview::build(
            &self.app,
            monitor,
            &self.services,
            cover(),
            panel_cover.clone(),
        );
        panel.set_cover(&panel_cover);
        Some(overview)
    }

    fn build(&self, monitor: gdk::Monitor) -> Output {
        let wallpaper = self.build_wallpaper(&monitor);
        let panel = self
            .wants_panel(&monitor)
            .then(|| panel::build(&self.app, &monitor, &self.services));
        let stage = self.build_stage(&monitor, panel.as_ref());
        let osd = osd::build(&self.app, &monitor);
        let banner = banner::build(&self.app, &monitor);
        let overview = self.build_overview(&monitor, wallpaper.as_ref(), panel.as_ref());
        Output {
            monitor,
            panel,
            wallpaper,
            stage,
            osd,
            banner,
            overview,
        }
    }

    /// Brings the surfaces in line with changed settings, building anew
    /// only what was built from a setting that changed.
    fn reconfigure(&self, config: &Config) {
        let changes = Changes::between(&self.applied.borrow(), config);
        self.applied.replace(config.clone());
        let wallpaper = (self.wallpaper_for)(config);
        // The same file under another setting, or a command line that
        // wins either way, changes nothing that is drawn.
        let new_wallpaper = changes.wallpaper && wallpaper != *self.wallpaper.borrow();
        if new_wallpaper {
            if !matches!(wallpaper, Some(WallpaperSource::Colour(_))) {
                wallpaper::forget_colour();
            }
            self.wallpaper.replace(wallpaper);
        }
        if changes.menus {
            // A menu is built once and kept; none stays open over what it
            // was built from changing.
            self.services.menus.update(|menus| menus.close());
        }

        // Taken out while they are worked on: building and dropping
        // surfaces runs code that may ask for the outputs there are.
        let mut outputs = self.outputs.take();
        for output in &mut outputs {
            let monitor = output.monitor.clone();
            if new_wallpaper {
                output.set_wallpaper(self.build_wallpaper(&monitor));
            }
            let had_panel = output.panel.is_some();
            let wants_panel = self.wants_panel(&monitor);
            if had_panel != wants_panel {
                let panel = wants_panel.then(|| panel::build(&self.app, &monitor, &self.services));
                output.set_panel(panel);
            }
            if changes.menus || had_panel != wants_panel {
                output.set_stage(self.build_stage(&monitor, output.panel.as_ref()));
            }
            if changes.overview || new_wallpaper || had_panel != wants_panel {
                let overview =
                    self.build_overview(&monitor, output.wallpaper.as_ref(), output.panel.as_ref());
                output.set_overview(overview);
            }
        }
        self.outputs.replace(outputs);
    }

    /// Shows `content` on every output's on-screen display, as GNOME does
    /// for the volume and brightness keys.
    pub fn show_osd(&self, content: &osd::Content) {
        for output in self.outputs.borrow().iter() {
            output.osd.show(content);
        }
    }

    /// The banner surface of the output the user is looking at: the one
    /// with niri's focused workspace, or else the first with a panel. GNOME
    /// shows banners on its primary monitor, which niri does not have.
    fn banner(&self) -> Option<Banner> {
        let focused = self.services.niri.with(|niri| niri.focused_output.clone());
        let outputs = self.outputs.borrow();
        let output = outputs
            .iter()
            .find(|output| {
                focused.is_some() && output.monitor.connector().as_deref() == focused.as_deref()
            })
            .or_else(|| outputs.iter().find(|output| output.panel.is_some()))
            .or(outputs.first())?;
        Some(output.banner.clone())
    }

    /// Connector name of the first output that has a panel: where a menu
    /// opens when nobody says which output.
    pub fn default_output(&self) -> Option<String> {
        let outputs = self.outputs.borrow();
        let output = outputs
            .iter()
            .find(|output| output.panel.is_some())
            .or(outputs.first())?;
        output.name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Config {
        Config::from_toml(text, None).unwrap()
    }

    #[test]
    fn only_what_was_built_from_a_changed_setting_is_built_anew() {
        let before = Config::default();
        assert_eq!(Changes::between(&before, &before), Changes::default());

        // The panel shows these by itself; nothing is rebuilt for them,
        // nor for what only a backend reads.
        for unrelated in [
            "[clock]\nformat = \"12h\"\nshow_seconds = true",
            "[panel]\nshow_battery_percentage = true",
            "[idle]\nblank_after = 60",
            "[night_light]\ntemperature = 3000",
            "[search]\nproviders = false",
        ] {
            assert_eq!(
                Changes::between(&before, &parse(unrelated)),
                Changes::default(),
                "{unrelated}"
            );
        }

        let wallpaper = Changes::between(&before, &parse("[wallpaper]\ncolor = \"#241f31\""));
        assert_eq!(
            wallpaper,
            Changes {
                wallpaper: true,
                overview: true,
                ..Changes::default()
            }
        );
        let dash = Changes::between(&before, &parse("[dash]\nfavorites = []"));
        assert_eq!(
            dash,
            Changes {
                overview: true,
                ..Changes::default()
            }
        );
        for menus in [
            "[date_menu]\nweek_start = \"monday\"",
            "[commands]\nsettings = \"\"",
        ] {
            assert_eq!(
                Changes::between(&before, &parse(menus)),
                Changes {
                    menus: true,
                    ..Changes::default()
                },
                "{menus}"
            );
        }
        let panels = Changes::between(&before, &parse("[panel]\noutputs = [\"eDP-1\"]"));
        assert_eq!(
            panels,
            Changes {
                wallpaper: false,
                panels: true,
                menus: true,
                overview: true,
            }
        );
    }
}
