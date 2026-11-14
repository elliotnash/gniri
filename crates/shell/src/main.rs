//! gniri: a GNOME Shell lookalike for the niri compositor.
//!
//! This is the M0 skeleton: one blurred panel surface, enough to prove the
//! toolchain, the layer-shell link order (spike S1) and CSS-driven blur from
//! a Rust binary.

use adw::prelude::*;
use gtk::{gdk, glib};
use gtk4_layer_shell::{Edge, Layer, LayerShell};

/// Reverse-DNS application ID. Not final; change here before publishing.
const APP_ID: &str = "dev.gniri.Shell";

const CSS: &str = "
window.gniri-layer { background: transparent; }

.panel {
    min-height: 32px;
    backdrop-filter: blur(10px);
    background-color: rgba(30, 30, 33, 0.5);
    color: white;
    font-weight: bold;
}
";

fn main() -> glib::ExitCode {
    // A suffix lets a nested test instance run beside a real one.
    let app_id = match std::env::var("GNIRI_INSTANCE") {
        Ok(instance) if !instance.is_empty() => format!("{APP_ID}.{instance}"),
        _ => APP_ID.to_owned(),
    };

    let app = adw::Application::builder().application_id(app_id).build();
    app.connect_startup(|_| load_css());
    app.connect_activate(build_panel);
    app.run()
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(CSS);
    gtk::style_context_add_provider_for_display(
        &gdk::Display::default().expect("no display"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

fn build_panel(app: &adw::Application) {
    if !gtk4_layer_shell::is_supported() {
        eprintln!("gniri: the compositor does not support layer-shell, or the shim did not load");
        app.quit();
        return;
    }

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .css_classes(["gniri-layer"])
        .build();
    window.init_layer_shell();
    window.set_namespace(Some("gniri-panel"));
    window.set_layer(Layer::Top);
    for edge in [Edge::Top, Edge::Left, Edge::Right] {
        window.set_anchor(edge, true);
    }
    window.auto_exclusive_zone_enable();

    let bar = gtk::CenterBox::builder().css_classes(["panel"]).build();
    bar.set_center_widget(Some(&gtk::Label::new(Some("gniri"))));
    window.set_child(Some(&bar));
    window.present();

    println!("GNIRI_READY");
}
