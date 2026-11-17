// SPDX-License-Identifier: GPL-3.0-or-later

//! `gniri polkit`: the polkit authentication agent, a process of its own,
//! so that nothing that happens to the shell can interrupt a password
//! being asked for.
//!
//! It registers with polkit for this session, waits, and shows the
//! authentication dialog for each request in turn
//! (crates/services/src/polkit.rs).

use std::cell::RefCell;
use std::rc::Rc;

use gniri_core::Subscription;
use gniri_services::polkit::{self, Command, Event, Prompt, State};
use gniri_services::user::User;
use gniri_services::{Bus, Fixture, Link, niri};
use gtk::glib;
use gtk::prelude::*;

use crate::Cli;
use crate::app;
use crate::dialogs::{self, auth};
use crate::ipc::Role;
use crate::services::{self, CommandLog, Service};

/// Everything that must live as long as the agent.
struct Agent {
    _service: Rc<Service<State>>,
    _subscription: Subscription,
}

/// The agent of a scene: no polkit, no helper. It shows the scene's
/// request and accepts the scene's password, so that a test can go through
/// the dialog's motions.
fn stand_in(fixture: &Fixture) -> Link<State> {
    let (link, events, commands) = Link::<State>::new();
    let Some(request) = fixture.polkit.request.clone() else {
        return link;
    };
    let password = fixture.lock.password.clone();
    let prompt = request.prompt.clone().unwrap_or(Prompt {
        text: "Password: ".to_owned(),
        echo: false,
    });
    glib::spawn_future_local(async move {
        let _ = events.send(Event::Begin(request)).await;
        while let Ok(command) = commands.recv().await {
            let answer = match command {
                Command::Respond(answer) if answer == password => vec![Event::End],
                Command::Respond(_) => vec![Event::Denied, Event::Prompt(prompt.clone())],
                Command::Cancel => vec![Event::End],
            };
            for event in answer {
                let _ = events.send(event).await;
            }
        }
    });
    link
}

/// The output the user is looking at, asked of niri once per dialog.
fn focused_output() -> Option<String> {
    let client = niri::Client::new(niri::socket_path()?);
    client.focused_output()
}

/// Quits once standard input closes. The shell gives its agent a pipe
/// there and never writes to it (`PolkitAgent` in app.rs).
fn end_with_shell(app: &adw::Application) {
    let (closed, on_close) = async_channel::bounded::<()>(1);
    let watch = std::thread::Builder::new()
        .name("shell".to_owned())
        .spawn(move || {
            // Returns once the pipe is closed, or cannot be read.
            let _ = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink());
            let _ = closed.send_blocking(());
        });
    if let Err(error) = watch {
        eprintln!("gniri: polkit: cannot watch for the shell to end: {error}");
        return;
    }
    let app = app.clone();
    glib::spawn_future_local(async move {
        let _ = on_close.recv().await;
        app.quit();
    });
}

pub fn run(cli: Cli, with_shell: bool) -> glib::ExitCode {
    app::run_helper(cli, Role::Polkit, move |app, _config, fixture| {
        if with_shell {
            end_with_shell(app);
        }
        let scheduler = services::scheduler();
        let log = CommandLog::default();
        let link = match fixture {
            Some(fixture) => stand_in(fixture),
            None => {
                let mut bus = Bus::system();
                let link = bus.add("polkit", |connection, events, commands| {
                    polkit::backend::run(
                        connection,
                        events,
                        commands,
                        polkit::backend::Options::system(),
                    )
                });
                bus.start();
                link
            }
        };
        // The backend ends when polkit will not have this agent (the
        // session has one already) or the bus goes away: nothing to wait
        // for then.
        let service = Rc::new(Service::live_then("polkit", link, &scheduler, &log, {
            let app = app.clone();
            move || app.quit()
        }));

        let shown_user = fixture.map(|fixture| fixture.user.clone());
        let open: RefCell<Option<auth::Dialog>> = RefCell::new(None);
        let subscription = service.state.select(
            // A request is the same request while what it asks stays the
            // same, whatever its prompt and messages do.
            |state| {
                state.request.as_ref().map(|request| {
                    (
                        request.action_id.clone(),
                        request.message.clone(),
                        request.user.clone(),
                    )
                })
            },
            {
                let (app, service) = (app.clone(), Rc::downgrade(&service));
                move |request: &Option<(String, String, String)>| {
                    if let Some(dialog) = open.take() {
                        dialog.close();
                    }
                    let Some(service) = service.upgrade() else {
                        return;
                    };
                    if request.is_none() {
                        return;
                    }
                    let Some(request) = service.state.with(|state| state.request.clone()) else {
                        return;
                    };
                    let Some(monitor) = dialogs::monitor(focused_output().as_deref()) else {
                        // No screen to ask on.
                        service.send(Command::Cancel);
                        return;
                    };
                    let user = match &shown_user {
                        Some(user) if user.name == request.user => user.clone(),
                        _ => User::named(&request.user),
                    };
                    open.replace(Some(auth::open(&app, &monitor, &service, &request, &user)));
                }
            },
        );
        Some(Box::new(Agent {
            _service: service,
            _subscription: subscription,
        }))
    })
}
