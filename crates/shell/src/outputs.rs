//! One set of surfaces per output, kept in step with outputs coming and
//! going.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio};

use crate::ready::ReadyTracker;
use crate::services::Services;
use crate::surfaces::panel::{self, Panel};
use crate::surfaces::wallpaper::{self, WallpaperSource};

/// What gniri shows on one output.
struct Output {
    monitor: gdk::Monitor,
    panel: Option<Panel>,
    wallpaper: Vec<gtk::ApplicationWindow>,
}

impl Output {
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
    }
}

pub struct Outputs {
    app: adw::Application,
    services: Rc<Services>,
    wallpaper: Option<WallpaperSource>,
    outputs: RefCell<Vec<Output>>,
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
        let wallpaper = match &self.wallpaper {
            Some(source) => wallpaper::build(&self.app, &monitor, source).to_vec(),
            None => Vec::new(),
        };
        // An output without a connector name cannot be excluded by name.
        let wants_panel = monitor
            .connector()
            .is_none_or(|name| self.services.config.panel.shows_on(&name));
        let panel = wants_panel.then(|| panel::build(&self.app, &monitor, &self.services));
        Output {
            monitor,
            panel,
            wallpaper,
        }
    }
}
