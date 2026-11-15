//! `gniri debug-window`: a plain flat-coloured window.
//!
//! Tests need an ordinary application window to give a workspace something
//! on it, to go fullscreen, or to sit under a blurred surface.

use adw::prelude::*;
use gtk::{gdk, gio, glib};

/// What the windows of a debug window call themselves when nothing else
/// is asked for.
const APP_ID: &str = "dev.gniri.DebugWindow";

/// `app_id` is the application the window says it belongs to: a test's
/// stand-in for a running application.
pub fn run(colour: &str, app_id: Option<&str>) -> glib::ExitCode {
    let Ok(colour) = gdk::RGBA::parse(colour) else {
        eprintln!("gniri: {colour:?} is not a colour");
        return glib::ExitCode::FAILURE;
    };
    let app = adw::Application::builder()
        .application_id(app_id.unwrap_or(APP_ID))
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_activate(move |app| {
        let provider = gtk::CssProvider::new();
        provider.load_from_string(&format!(
            "window.gniri-debug-window {{ background-color: {colour}; }}"
        ));
        gtk::style_context_add_provider_for_display(
            &gdk::Display::default().expect("GTK is initialised, so there is a display"),
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        gtk::ApplicationWindow::builder()
            .application(app)
            .title("gniri debug window")
            .css_classes(["gniri-debug-window"])
            .decorated(false)
            .build()
            .present();
    });
    app.run_with_args::<&str>(&[])
}
