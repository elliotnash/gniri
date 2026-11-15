//! One set of surfaces per output, kept in step with outputs coming and
//! going.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio};

use crate::ready::ReadyTracker;
use crate::services::Services;
use crate::surfaces::banner::{self, Banner, Tray};
use crate::surfaces::osd::{self, Osd};
use crate::surfaces::overview::{self, Overview};
use crate::surfaces::panel::{self, Panel};
use crate::surfaces::stage::{self, Stage};
use crate::surfaces::wallpaper::{self, WallpaperSource};

/// What gniri shows on one output.
struct Output {
    monitor: gdk::Monitor,
    panel: Option<Panel>,
    wallpaper: Vec<gtk::ApplicationWindow>,
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
        self.panel
            .iter()
            .map(|panel| &panel.window)
            .chain(&self.wallpaper)
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

pub struct Outputs {
    app: adw::Application,
    services: Rc<Services>,
    wallpaper: Option<WallpaperSource>,
    outputs: RefCell<Vec<Output>>,
    /// Shows notification banners on the focused output.
    tray: RefCell<Option<Tray>>,
}

impl Outputs {
    /// Builds the surfaces for every output there is now and keeps doing so
    /// as outputs appear and disappear. `ready` hears about the surfaces of
    /// the outputs present at the start.
    pub fn start(
        app: &adw::Application,
        services: &Rc<Services>,
        wallpaper: Option<WallpaperSource>,
        ready: &ReadyTracker,
    ) -> Rc<Self> {
        let outputs = Rc::new(Self {
            app: app.clone(),
            services: Rc::clone(services),
            wallpaper,
            outputs: RefCell::new(Vec::new()),
            tray: RefCell::new(None),
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
        outputs
    }

    fn sync(&self, monitors: &gio::ListModel) {
        let present: Vec<gdk::Monitor> = monitors
            .iter::<gdk::Monitor>()
            .filter_map(Result::ok)
            .collect();

        let mut outputs = self.outputs.borrow_mut();
        outputs.retain(|output| present.contains(&output.monitor));
        for monitor in present {
            if outputs.iter().all(|output| output.monitor != monitor) {
                outputs.push(self.build(monitor));
            }
        }
    }

    fn build(&self, monitor: gdk::Monitor) -> Output {
        let (wallpaper, cover) = match &self.wallpaper {
            Some(source) => {
                let wallpaper = wallpaper::build(&self.app, &monitor, source);
                let cover = wallpaper.cover();
                (wallpaper.windows.to_vec(), cover)
            }
            None => (Vec::new(), wallpaper::cover_without_wallpaper()),
        };
        // An output without a connector name cannot be excluded by name.
        let wants_panel = monitor
            .connector()
            .is_none_or(|name| self.services.config.panel.shows_on(&name));
        let panel = wants_panel.then(|| panel::build(&self.app, &monitor, &self.services));
        let panel_window = panel.as_ref().map(|panel| panel.window.downgrade());
        let stage = stage::build(&self.app, &monitor, &self.services, move || {
            panel_window
                .as_ref()
                .and_then(|window| window.upgrade())
                .map_or(0, |window| window.height())
        });
        let osd = osd::build(&self.app, &monitor);
        let banner = banner::build(&self.app, &monitor);
        // The dash and the search belong with the panel: an output without
        // one shows niri's overview as it is.
        let overview =
            wants_panel.then(|| overview::build(&self.app, &monitor, &self.services, cover));
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
        output.monitor.connector().map(Into::into)
    }
}
