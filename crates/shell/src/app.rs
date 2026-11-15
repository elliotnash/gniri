//! Application setup: one `adw::Application` owning every surface.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gniri_config::Config;
use gniri_services::Fixture;
use gtk::{gdk, gio, glib};

use crate::Cli;
use crate::debug;
use crate::ipc;
use crate::keys;
use crate::menus::Menu;
use crate::outputs::Outputs;
use crate::ready::ReadyTracker;
use crate::services::Services;
use crate::style;
use crate::surfaces::wallpaper::WallpaperSource;

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

/// Everything that must live as long as the application.
struct Running {
    _outputs: Rc<Outputs>,
    _ipc: Option<gio::SocketService>,
    _subscriptions: Vec<gniri_core::Subscription>,
}

pub fn run(cli: Cli) -> glib::ExitCode {
    // Read both files before anything is shown, so a mistake in them is the
    // first thing on stderr. A broken config falls back to the defaults; a
    // broken fixture is fatal, since the screenshot would be meaningless.
    let config = load_config(&cli);
    let fixture = match cli.fixture.as_deref().map(Fixture::load).transpose() {
        Ok(fixture) => fixture,
        Err(error) => {
            eprintln!("gniri: {error}");
            return glib::ExitCode::FAILURE;
        }
    };

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

    let running: Rc<RefCell<Option<Running>>> = Rc::default();
    let setup = RefCell::new(Some((config, fixture)));
    app.connect_activate({
        let running = Rc::clone(&running);
        move |app| {
            // `activate` can fire again; build everything only once.
            if let Some((config, fixture)) = setup.borrow_mut().take() {
                *running.borrow_mut() = activate(app, &cli, config, fixture);
            }
        }
    });
    app.connect_shutdown(move |_| {
        if running.borrow_mut().take().is_some() {
            ipc::cleanup();
        }
    });
    // Pass no arguments: clap already consumed them.
    app.run_with_args::<&str>(&[])
}

fn load_config(cli: &Cli) -> Config {
    let path = cli.config.clone().or_else(gniri_config::default_path);
    let Some(path) = path else {
        return Config::default();
    };
    Config::load(&path).unwrap_or_else(|error| {
        eprintln!("gniri: {error}");
        eprintln!("gniri: using the default configuration");
        Config::default()
    })
}

/// The wallpaper to draw: `--wallpaper` if given, else the config file's.
fn wallpaper_source(cli: &Cli, config: &Config) -> Option<WallpaperSource> {
    if let Some(spec) = &cli.wallpaper {
        return match WallpaperSource::parse(spec) {
            Ok(source) => Some(source),
            Err(error) => {
                eprintln!("gniri: ignoring --wallpaper: {error}");
                None
            }
        };
    }
    if let Some(image) = &config.wallpaper.image {
        if image.is_file() {
            return Some(WallpaperSource::Image(image.clone()));
        }
        eprintln!("gniri: wallpaper image {} does not exist", image.display());
    }
    let colour = config.wallpaper.color.as_deref()?;
    gdk::RGBA::parse(colour).ok().map(WallpaperSource::Colour)
}

fn activate(
    app: &adw::Application,
    cli: &Cli,
    config: Config,
    fixture: Option<Fixture>,
) -> Option<Running> {
    if !gtk4_layer_shell::is_supported() {
        eprintln!("gniri: the compositor does not support layer-shell");
        app.quit();
        return None;
    }

    // A fixture is for reproducible screenshots: nothing may be half way
    // through an animation when one is taken.
    if fixture.is_some()
        && let Some(settings) = gtk::Settings::default()
    {
        settings.set_gtk_enable_animations(false);
    }

    if cli.no_pointer {
        crate::surfaces::layer::ignore_pointer();
    }
    let wallpaper = wallpaper_source(cli, &config);
    let services = Services::start(config, fixture);

    let ready = ReadyTracker::new();
    let outputs = Outputs::start(app, &services, wallpaper, &ready);
    ready.arm();

    let ipc = ipc::serve({
        let (app, services, outputs) = (app.clone(), Rc::clone(&services), Rc::clone(&outputs));
        move |request| -> ipc::Pending {
            if request == ipc::Request::Sync {
                return Box::pin(sync(app.clone()));
            }
            let reply = handle(&app, &services, &outputs, request);
            Box::pin(async move { reply })
        }
    });

    // Layer surfaces do not count as application windows that keep the
    // process alive on every platform; hold the application explicitly.
    std::mem::forget(app.hold());

    // Opening the overview closes any menu, as in GNOME.
    let close_menus = services.niri.select(|niri| niri.overview_open, {
        let services = Rc::clone(&services);
        move |open| {
            if *open {
                services.menus.update(|menus| menus.close());
            }
        }
    });

    Some(Running {
        _outputs: outputs,
        _ipc: ipc,
        _subscriptions: vec![close_menus],
    })
}

/// Answers one `gniri msg` request.
fn handle(
    app: &adw::Application,
    services: &Rc<Services>,
    outputs: &Rc<Outputs>,
    request: ipc::Request,
) -> ipc::Reply {
    use serde_json::Value;
    match request {
        ipc::Request::DumpTree => {
            serde_json::to_value(debug::dump::dump(app)).map_err(|error| error.to_string())
        }
        // Answered by `sync`, which has to wait.
        ipc::Request::Sync => Ok(Value::Null),
        ipc::Request::Commands => Ok(Value::from(services.commands.entries())),
        ipc::Request::OpenMenu { menu, output } => {
            let output = output.or_else(|| outputs.default_output());
            services
                .menus
                .update(|menus| menus.open(menu, output.as_deref()));
            Ok(Value::Null)
        }
        ipc::Request::OpenSubmenu(submenu) => {
            if services.menus.with(|menus| menus.open.is_none()) {
                return Err("no menu is open".to_owned());
            }
            services
                .menus
                .update(|menus| menus.set_submenu(Some(submenu)));
            Ok(Value::Null)
        }
        ipc::Request::OpenGroup(source) => {
            if !services.menus.with(|menus| {
                menus
                    .open
                    .as_ref()
                    .is_some_and(|open| open.menu == Menu::DateMenu)
            }) {
                return Err("the date menu is not open".to_owned());
            }
            services.menus.update(|menus| menus.set_group(Some(source)));
            Ok(Value::Null)
        }
        ipc::Request::CloseMenu => {
            services.menus.update(|menus| menus.close());
            Ok(Value::Null)
        }
        ipc::Request::Volume(key) => {
            let command = services.audio.state.with(|state| match key {
                ipc::VolumeKey::Up => state.step_output(1),
                ipc::VolumeKey::Down => state.step_output(-1),
                ipc::VolumeKey::Mute => state.toggle_output_mute(),
            });
            let command = command.ok_or("there is no sound output")?;
            services.audio.send(command);
            // The state now holds the predicted result; show that.
            if let Some(content) = services.audio.state.with(keys::volume_osd) {
                outputs.show_osd(&content);
            }
            Ok(Value::Null)
        }
        ipc::Request::Osd { icon, label, level } => {
            outputs.show_osd(&crate::surfaces::osd::Content {
                icon,
                label,
                level: level.map(|level| (level, level.max(1.0))),
            });
            Ok(Value::Null)
        }
        ipc::Request::Notify(notify) => Ok(Value::from(debug::notify(services, notify))),
        ipc::Request::Brightness(step) => {
            let steps = match step {
                ipc::Step::Up => 1,
                ipc::Step::Down => -1,
            };
            let command = services
                .brightness
                .state
                .with(|state| state.step(steps))
                .ok_or("this screen's brightness cannot be set")?;
            services.brightness.send(command);
            if let Some(content) = services.brightness.state.with(keys::brightness_osd) {
                outputs.show_osd(&content);
            }
            Ok(Value::Null)
        }
    }
}

/// Waits until what was asked for before now is on screen.
///
/// State changes reach widgets from an idle callback, and a surface that
/// was just presented takes a round trip to the compositor before it can
/// draw. So: let the main loop go idle, then wait for every mapped surface
/// to finish painting one more frame, twice over to cover a surface that
/// only became mapped during the first wait.
async fn sync(app: adw::Application) -> ipc::Reply {
    for _ in 0..2 {
        let (idle, went_idle) = async_channel::bounded::<()>(1);
        glib::idle_add_local_full(glib::Priority::LOW, move || {
            let _ = idle.try_send(());
            glib::ControlFlow::Break
        });
        let _ = went_idle.recv().await;

        let (painted, all_painted) = async_channel::unbounded::<()>();
        let mut waiting = 0;
        for window in app.windows() {
            if !window.is_mapped() {
                continue;
            }
            let Some(clock) = window.frame_clock() else {
                continue;
            };
            waiting += 1;
            let painted = painted.clone();
            let handler = Rc::new(std::cell::Cell::new(None));
            let id = clock.connect_after_paint({
                let handler = Rc::clone(&handler);
                move |clock| {
                    if let Some(id) = handler.take() {
                        clock.disconnect(id);
                        let _ = painted.try_send(());
                    }
                }
            });
            handler.set(Some(id));
            window.queue_draw();
        }
        for _ in 0..waiting {
            // A surface the compositor does not show gets no frames; do not
            // wait for it for ever.
            let timeout = glib::timeout_future(std::time::Duration::from_millis(500));
            either(all_painted.recv(), timeout).await;
        }
    }
    Ok(serde_json::Value::Null)
}

/// Waits for whichever of the two finishes first.
async fn either<A, B>(first: A, second: B)
where
    A: std::future::Future,
    B: std::future::Future,
{
    use std::pin::pin;
    use std::task::Poll;
    let (mut first, mut second) = (pin!(first), pin!(second));
    std::future::poll_fn(|context| {
        if first.as_mut().poll(context).is_ready() || second.as_mut().poll(context).is_ready() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await;
}
