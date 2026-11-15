// SPDX-License-Identifier: GPL-3.0-or-later

//! Application setup: one `adw::Application` owning every surface.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gniri_config::Config;
use gniri_services::Fixture;
use gtk::{gdk, gio, glib};

use crate::Cli;
use crate::debug;
use crate::dialogs;
use crate::idle;
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
fn application_id(role: ipc::Role) -> String {
    let id = match role {
        ipc::Role::Shell => APP_ID.to_owned(),
        ipc::Role::Lock => format!("{APP_ID}.Lock"),
        ipc::Role::Polkit => format!("{APP_ID}.Polkit"),
    };
    match std::env::var("GNIRI_INSTANCE") {
        Ok(instance) if !instance.is_empty() => format!("{id}.{instance}"),
        _ => id,
    }
}

/// The polkit agent, a process the shell starts and stops with itself.
struct PolkitAgent(std::process::Child);

impl PolkitAgent {
    fn start() -> Option<Self> {
        let program = std::env::current_exe().ok()?;
        match std::process::Command::new(program).arg("polkit").spawn() {
            Ok(child) => Some(Self(child)),
            Err(error) => {
                eprintln!("gniri: cannot start the polkit agent: {error}");
                None
            }
        }
    }
}

impl Drop for PolkitAgent {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Everything that must live as long as the application.
struct Running {
    _outputs: Rc<Outputs>,
    _dialogs: Rc<dialogs::Host>,
    _ipc: Option<gio::SocketService>,
    _subscriptions: Vec<gniri_core::Subscription>,
    _polkit: Option<PolkitAgent>,
    /// Watches the config file.
    _config_watch: Option<Vec<gio::FileMonitor>>,
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

    let app = application(ipc::Role::Shell);
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
            ipc::cleanup(ipc::Role::Shell);
        }
    });
    // Pass no arguments: clap already consumed them.
    app.run_with_args::<&str>(&[])
}

/// An application for one of gniri's processes, themed as the shell is.
fn application(role: ipc::Role) -> adw::Application {
    let app = adw::Application::builder()
        .application_id(application_id(role))
        // We parsed the command line ourselves; GApplication must not.
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_startup(|_| {
        // GNOME Shell's own chrome is dark whatever the app colour scheme is.
        adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
        style::load();
    });
    app
}

/// What a helper process keeps alive while it runs.
pub type Kept = Box<dyn std::any::Any>;

/// Runs `gniri lock` or `gniri polkit`: an application of its own that
/// reads the same configuration, looks the same, and in a test scene
/// answers the requests that inspect it.
///
/// `activate` builds the process's surfaces once. Returning `None` ends
/// the process. The exit status is whatever [`quit_with`] was told, or
/// success.
pub fn run_helper(
    cli: Cli,
    role: ipc::Role,
    activate: impl Fn(&adw::Application, &Config, Option<&Fixture>) -> Option<Kept> + 'static,
) -> glib::ExitCode {
    let config = load_config(&cli);
    let fixture = match cli.fixture.as_deref().map(Fixture::load).transpose() {
        Ok(fixture) => fixture,
        Err(error) => {
            eprintln!("gniri: {error}");
            return glib::ExitCode::FAILURE;
        }
    };

    let app = application(role);
    let kept: Rc<RefCell<Vec<Kept>>> = Rc::default();
    let activated = std::cell::Cell::new(false);
    app.connect_activate({
        let kept = Rc::clone(&kept);
        move |app| {
            if activated.replace(true) {
                return;
            }
            if fixture.is_some() {
                if let Some(settings) = gtk::Settings::default() {
                    settings.set_gtk_enable_animations(false);
                }
                // Only a scene's process can be looked into: the real lock
                // screen and the real agent take no requests.
                let served = ipc::serve(role, {
                    let app = app.clone();
                    move |request| -> ipc::Pending {
                        match request {
                            ipc::Request::Sync => Box::pin(sync(app.clone())),
                            ipc::Request::DumpTree => {
                                let reply = serde_json::to_value(debug::dump::dump(&app))
                                    .map_err(|error| error.to_string());
                                Box::pin(async move { reply })
                            }
                            _ => Box::pin(async {
                                Err("this process only answers dump-tree and sync".to_owned())
                            }),
                        }
                    }
                });
                if let Some(served) = served {
                    kept.borrow_mut().push(Box::new(served));
                }
            }
            if cli.no_pointer {
                crate::surfaces::layer::ignore_pointer();
            }
            match activate(app, &config, fixture.as_ref()) {
                Some(state) => {
                    kept.borrow_mut().push(state);
                    std::mem::forget(app.hold());
                }
                None => app.quit(),
            }
        }
    });
    app.connect_shutdown(move |_| {
        if !kept.borrow().is_empty() {
            kept.borrow_mut().clear();
            ipc::cleanup(role);
        }
    });
    app.run_with_args::<&str>(&[]);
    EXIT_STATUS.with(|status| glib::ExitCode::from(status.get()))
}

thread_local! {
    /// What [`run_helper`] exits with.
    static EXIT_STATUS: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

/// Ends a helper process with `status`.
pub fn quit_with(app: &adw::Application, status: u8) {
    EXIT_STATUS.with(|current| current.set(status));
    app.quit();
}

/// The config file this process reads: `--config`, else the user's.
fn config_path(cli: &Cli) -> Option<std::path::PathBuf> {
    cli.config.clone().or_else(gniri_config::default_path)
}

pub fn load_config(cli: &Cli) -> Config {
    let Some(path) = config_path(cli) else {
        return Config::default();
    };
    Config::load(&path).unwrap_or_else(|error| {
        eprintln!("gniri: {error}");
        eprintln!("gniri: using the default configuration");
        Config::default()
    })
}

/// How long the config file has to stay as it is before it is read again:
/// an editor may write it in several steps.
const CONFIG_SETTLE: std::time::Duration = std::time::Duration::from_millis(150);

/// Reads the config file again whenever it changes, and takes its
/// settings into use while the shell runs.
///
/// A file with a mistake in it changes nothing: the settings stay as they
/// were, and a notification says what is wrong.
fn watch_config(
    app: &adw::Application,
    cli: &Cli,
    services: &Rc<Services>,
) -> Option<Vec<gio::FileMonitor>> {
    let path = config_path(cli)?;
    let pending: Rc<std::cell::Cell<u64>> = Rc::default();
    let changed = {
        let (app, services, path) = (app.clone(), Rc::clone(services), path.clone());
        move || {
            // Every change puts the reading off, so that it happens once,
            // after the last.
            let number = pending.get() + 1;
            pending.set(number);
            glib::timeout_add_local_once(CONFIG_SETTLE, {
                let (app, services, path) = (app.clone(), Rc::clone(&services), path.clone());
                let pending = Rc::clone(&pending);
                move || {
                    if pending.get() == number {
                        reload_config(&app, &services, &path);
                    }
                }
            });
        }
    };
    let watch = |file: &std::path::Path| {
        // A file that is replaced, or only created later, is still this
        // file.
        let monitor = gio::File::for_path(file)
            .monitor_file(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
            .inspect_err(|error| eprintln!("gniri: cannot watch {}: {error}", file.display()))
            .ok()?;
        monitor.connect_changed({
            let changed = changed.clone();
            move |_, _, _, _| changed()
        });
        Some(monitor)
    };
    let mut monitors: Vec<gio::FileMonitor> = watch(&path).into_iter().collect();
    // A config file kept elsewhere and linked into place (by a dotfile
    // manager, say) changes where the link points to, not where it is.
    if let Ok(target) = std::fs::canonicalize(&path)
        && target != path
    {
        monitors.extend(watch(&target));
    }
    Some(monitors)
}

/// A config error on one line, for a notification: where it is and what it
/// is, without the excerpt of the file a parse error comes with.
fn error_summary(error: &str) -> String {
    let mut lines = error.lines().map(str::trim).filter(|line| !line.is_empty());
    let first = lines.next().unwrap_or_default();
    match lines.next_back() {
        Some(last) => format!("{first}: {last}"),
        None => first.to_owned(),
    }
}

/// The id of the notification about a mistake in the config file: a later
/// one replaces it, and a file that is right again takes it away.
const CONFIG_NOTIFICATION: &str = "config-error";

fn reload_config(app: &adw::Application, services: &Rc<Services>, path: &std::path::Path) {
    match Config::load(path) {
        Ok(config) => {
            app.withdraw_notification(CONFIG_NOTIFICATION);
            if services.config.with(|current| *current != config) {
                eprintln!("gniri: {} changed; settings reloaded", path.display());
                services.reload(config);
            }
        }
        Err(error) => {
            eprintln!("gniri: {error}");
            eprintln!("gniri: keeping the settings as they were");
            let notification = gio::Notification::new("gniri’s settings were not changed");
            notification.set_body(Some(&error_summary(&error.to_string())));
            notification.set_icon(&gio::ThemedIcon::new("dialog-warning-symbolic"));
            app.send_notification(Some(CONFIG_NOTIFICATION), &notification);
        }
    }
}

/// The wallpaper to draw: `--wallpaper` if given, else the config file's.
pub fn wallpaper_source(cli: &Cli, config: &Config) -> Option<WallpaperSource> {
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
    // Not under a fixture: a scene starts the agent it wants to look at.
    let polkit = fixture.is_none().then(PolkitAgent::start).flatten();
    let services = Services::start(config, fixture, cli.locker_arguments());

    let ready = ReadyTracker::new();
    let wallpaper_for = {
        let cli = cli.clone();
        move |config: &Config| wallpaper_source(&cli, config)
    };
    let outputs = Outputs::start(app, &services, wallpaper_for, &ready);
    ready.arm();
    let config_watch = watch_config(app, cli, &services);

    let dialogs = Rc::new(dialogs::host(app, &services));
    let ipc = ipc::serve(ipc::Role::Shell, {
        let (app, services, outputs) = (app.clone(), Rc::clone(&services), Rc::clone(&outputs));
        let dialogs = Rc::clone(&dialogs);
        move |request| -> ipc::Pending {
            match request {
                ipc::Request::Sync => Box::pin(sync(app.clone())),
                ipc::Request::Outputs(plugged) => {
                    outputs.set_unplugged(!plugged);
                    dialogs.set_unplugged(!plugged);
                    Box::pin(async { Ok(serde_json::Value::Null) })
                }
                request => {
                    let reply = handle(&app, &services, &outputs, request);
                    Box::pin(async move { reply })
                }
            }
        }
    });

    // Layer surfaces do not count as application windows that keep the
    // process alive on every platform; hold the application explicitly.
    std::mem::forget(app.hold());

    // Opening the overview closes any menu, as in GNOME. It is also when
    // an application installed since is first wanted; and a closed
    // overview forgets what it showed.
    let close_menus = services.niri.select(|niri| niri.overview_open, {
        let services = Rc::clone(&services);
        move |open| {
            if *open {
                services.menus.update(|menus| menus.close());
                services.apps.send(gniri_services::apps::Command::Refresh);
            } else {
                services
                    .overview
                    .update(|overview| *overview = crate::overview::Overview::default());
            }
        }
    });
    // What is typed is asked of the search providers.
    let search = services.overview.select(crate::overview::Overview::terms, {
        let services = Rc::clone(&services);
        move |terms| {
            // Nothing was being searched for and nothing is: say nothing.
            if !terms.is_empty() || services.search.state.with(|state| !state.terms.is_empty()) {
                services
                    .search
                    .send(gniri_services::search::Command::Search(terms.clone()));
            }
        }
    });
    // So does a dialog, and the lock screen: neither leaves a menu open
    // behind it.
    let close_menus_for_dialog = services.end_session.select(|open| open.is_some(), {
        let services = Rc::clone(&services);
        move |open| {
            if *open {
                services.menus.update(|menus| menus.close());
            }
        }
    });
    let close_for_lock = services.session.state.select(|session| session.locked, {
        let services = Rc::clone(&services);
        move |locked| {
            if *locked {
                services.menus.update(|menus| menus.close());
                services.end_session.update(|open| *open = None);
                // Nor does anything ask from under the lock screen: GNOME
                // has no agent while it is locked.
                if services
                    .network_agent
                    .state
                    .with(|state| state.request.is_some())
                {
                    services
                        .network_agent
                        .send(gniri_services::network::agent::Command::Cancel);
                }
                if services
                    .bluetooth_agent
                    .state
                    .with(|state| state.request.is_some())
                {
                    services
                        .bluetooth_agent
                        .send(gniri_services::bluetooth::agent::Command::Reject);
                }
            }
        }
    });
    let mut subscriptions = vec![close_menus, close_menus_for_dialog, close_for_lock, search];
    subscriptions.extend(idle::start(&services));

    Some(Running {
        _outputs: outputs,
        _dialogs: dialogs,
        _ipc: ipc,
        _subscriptions: subscriptions,
        _polkit: polkit,
        _config_watch: config_watch,
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
        // Answered by `sync`, which has to wait, and by the caller.
        ipc::Request::Sync | ipc::Request::Outputs(_) => Ok(Value::Null),
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
        ipc::Request::Lock => {
            services
                .session
                .send(gniri_services::session::Command::Lock);
            Ok(Value::Null)
        }
        ipc::Request::EndSession(kind) => {
            services.end_session.update(|open| *open = Some(kind));
            Ok(Value::Null)
        }
        ipc::Request::CloseDialog => {
            services.end_session.update(|open| *open = None);
            // The agents' dialogs go as when their request is withdrawn.
            services
                .network_agent
                .inject(gniri_services::network::agent::Event::End);
            services
                .bluetooth_agent
                .inject(gniri_services::bluetooth::agent::Event::End);
            Ok(Value::Null)
        }
        ipc::Request::Idle { timer, resumed } => {
            use gniri_services::idle::Event;
            let timer = timer.timer();
            services.idle.inject(if resumed {
                Event::Resumed(timer)
            } else {
                Event::Idle(timer)
            });
            Ok(Value::Null)
        }
        ipc::Request::Apps => {
            let showing = services.niri.with(|niri| niri.overview_open)
                && services
                    .overview
                    .with(|overview| overview.apps && !overview.searching());
            if showing {
                services.close_overview();
            } else {
                services.overview.update(|overview| overview.set_apps(true));
                services.niri_action(niri_ipc::Action::OpenOverview {});
            }
            Ok(Value::Null)
        }
        ipc::Request::ShowApps(apps) => {
            services.overview.update(|overview| overview.set_apps(apps));
            Ok(Value::Null)
        }
        ipc::Request::Search { text, quiet } => {
            services.overview.update(|overview| {
                overview.set_query(&text);
                overview.quiet = quiet;
            });
            Ok(Value::Null)
        }
        ipc::Request::Page(page) => {
            services.overview.update(|overview| overview.page = page);
            Ok(Value::Null)
        }
        ipc::Request::Pairing(pairing) => {
            use gniri_services::bluetooth::agent::Event;
            services
                .bluetooth_agent
                .inject(Event::Begin(pairing.request()));
            Ok(Value::Null)
        }
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
pub async fn sync(app: adw::Application) -> ipc::Reply {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_config_error_fits_a_notification() {
        let parse = "/home/someone/.config/gniri/config.toml: TOML parse error at line 2, column 1\n  |\n2 | format24 = true\n  | ^^^^^^^^\nunknown field `format24`, expected one of `format`, `show_date`\n";
        assert_eq!(
            error_summary(parse),
            "/home/someone/.config/gniri/config.toml: TOML parse error at line 2, column 1: \
             unknown field `format24`, expected one of `format`, `show_date`"
        );
        let invalid = "config.toml: night_light.temperature: 9000 is not between 1700 and 4700";
        assert_eq!(error_summary(invalid), invalid);
        assert_eq!(error_summary(""), "");
    }
}
