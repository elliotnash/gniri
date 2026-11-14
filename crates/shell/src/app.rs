//! Application setup: one `adw::Application` owning every surface.

use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::Cli;
use crate::ready::ReadyTracker;
use crate::style;
use crate::surfaces::wallpaper::WallpaperSource;
use crate::surfaces::{panel, wallpaper};

/// Reverse-DNS application ID. Not final; change here before publishing.
const APP_ID: &str = "dev.gniri.Shell";

/// The ID of this process's application.
///
/// `GNIRI_INSTANCE` adds a suffix so a nested test instance can run beside a
/// real one without the two fighting over the same D-Bus name.
fn application_id() -> String {
    match std::env::var("GNIRI_INSTANCE") {
        Ok(instance) if !instance.is_empty() => format!("{APP_ID}.{instance}"),
        _ => APP_ID.to_owned(),
    }
}

pub fn run(cli: Cli) -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(application_id())
        // We parsed the command line ourselves; GApplication must not.
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();

    app.connect_startup(|_| {
        // GNOME Shell's own chrome is dark whatever the app colour scheme is.
        adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
        style::load();
    });
    app.connect_activate(move |app| activate(app, &cli));
    // Pass no arguments: clap already consumed them.
    app.run_with_args::<&str>(&[])
}

fn activate(app: &adw::Application, cli: &Cli) {
    if !gtk4_layer_shell::is_supported() {
        eprintln!("gniri: the compositor does not support layer-shell");
        app.quit();
        return;
    }

    let ready = Rc::new(ReadyTracker::new());

    if let Some(spec) = &cli.wallpaper {
        match WallpaperSource::parse(spec) {
            Ok(source) => ready.watch(&wallpaper::build(app, &source)),
            Err(error) => eprintln!("gniri: ignoring --wallpaper: {error}"),
        }
    }
    ready.watch(&panel::build(app));
    ready.arm();
}
