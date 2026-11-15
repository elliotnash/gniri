//! `cargo xtask`: development and visual-test driver for gniri.

mod check;
mod diff;
mod report;
mod scene;
mod session;
mod tree;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use scene::Scene;
use session::{Session, SessionOptions};

#[derive(Parser)]
#[command(name = "xtask", about = "Development and visual-test driver for gniri")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run gniri in a nested niri window until Ctrl-C.
    Nest {
        #[command(flatten)]
        session: SessionArgs,
    },
    /// Run gniri nested, screenshot the output, and exit.
    Shot {
        /// Name of the screenshot: written to target/visual/<NAME>.png.
        name: String,
        /// Open niri's overview before the screenshot.
        #[arg(long)]
        overview: bool,
        #[command(flatten)]
        session: SessionArgs,
    },
    /// Compare a component with the GNOME reference
    /// (tests/visual/map/<COMPONENT>.toml).
    Diff { component: String },
    /// Diff every component and write target/visual/report/index.html.
    Report,
    /// Check how the shell behaves in a live nested session.
    Check,
}

#[derive(clap::Args)]
struct SessionArgs {
    /// Wallpaper for gniri: an image file or a colour such as "#808080".
    #[arg(long)]
    wallpaper: Option<String>,
    /// Scene to set up first (tests/visual/scenes/<SCENE>.toml): its fixture
    /// is given to gniri and its steps are played.
    #[arg(long)]
    scene: Option<String>,
    /// niri config to use instead of tests/visual/niri/test.kdl.
    #[arg(long)]
    niri_config: Option<PathBuf>,
    /// GTK renderer (GSK_RENDERER).
    #[arg(long, default_value = "vulkan")]
    renderer: String,
}

impl SessionArgs {
    /// Starts the session, with the scene (if any) played.
    fn start(&self, name: &str) -> Result<Session> {
        let scene = self.scene.as_deref().map(Scene::load).transpose()?;
        let fixture = match &scene {
            Some(scene) => Some(scene.write_fixture(name, None)?),
            None => None,
        };
        let session = Session::start(&SessionOptions {
            name: name.to_owned(),
            niri_config: self.niri_config.clone(),
            renderer: self.renderer.clone(),
            gniri_args: scene::gniri_args(fixture.as_deref(), self.wallpaper.as_deref()),
        })?;
        if let Some(scene) = &scene {
            scene.play(&session)?;
        }
        Ok(session)
    }
}

fn run() -> Result<bool> {
    match Cli::parse().command {
        Command::Nest { session } => {
            let session = session.start("nest")?;
            println!("NIRI_SOCKET={}", session.niri_socket.display());
            println!("WAYLAND_DISPLAY={}", session.wayland_display);
            println!("Running. Press Ctrl-C to stop.");
            session.wait()?;
            Ok(true)
        }
        Command::Shot {
            name,
            overview,
            session,
        } => {
            let session = session.start(&name)?;
            if overview {
                session.action(niri_ipc::Action::OpenOverview {})?;
            }
            session.settle()?;
            let path = session::visual_dir()?.join(format!("{name}.png"));
            session.screenshot(&path).context("taking the screenshot")?;
            session.quit()?;
            println!("{}", path.display());
            Ok(true)
        }
        Command::Diff { component } => {
            let checks = diff::run(&component)?;
            Ok(diff::print(&component, &checks))
        }
        Command::Report => report::run(),
        Command::Check => check::run(),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    /// gniri's own code runs on the GLib main loop and a few plain
    /// threads; zbus is used with its default runtime. A crate that pulls
    /// tokio in would start a second runtime beside them.
    #[test]
    fn nothing_depends_on_tokio() {
        let output = Command::new(env!("CARGO"))
            .args(["tree", "--workspace", "--invert", "tokio"])
            .current_dir(crate::session::workspace_root())
            .output()
            .expect("cargo runs");
        let listing = String::from_utf8_lossy(&output.stdout);
        assert!(
            listing.trim().is_empty(),
            "tokio is in the dependency tree:\n{listing}"
        );
    }
}
