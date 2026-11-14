//! `cargo xtask`: development and visual-test driver for gniri.

mod session;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

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
}

#[derive(clap::Args)]
struct SessionArgs {
    /// Wallpaper for gniri: an image file or a colour such as "#808080".
    #[arg(long)]
    wallpaper: Option<String>,
    /// niri config to use instead of tests/visual/niri/test.kdl.
    #[arg(long)]
    niri_config: Option<PathBuf>,
    /// GTK renderer (GSK_RENDERER).
    #[arg(long, default_value = "vulkan")]
    renderer: String,
}

impl SessionArgs {
    fn options(&self, name: &str) -> SessionOptions {
        let mut gniri_args = Vec::new();
        if let Some(wallpaper) = &self.wallpaper {
            gniri_args.extend(["--wallpaper".to_owned(), wallpaper.clone()]);
        }
        SessionOptions {
            name: name.to_owned(),
            niri_config: self.niri_config.clone(),
            renderer: self.renderer.clone(),
            gniri_args,
        }
    }
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Nest { session } => {
            let session = Session::start(&session.options("nest"))?;
            println!("NIRI_SOCKET={}", session.niri_socket.display());
            println!("WAYLAND_DISPLAY={}", session.wayland_display);
            println!("Running. Press Ctrl-C to stop.");
            session.wait()
        }
        Command::Shot {
            name,
            overview,
            session,
        } => {
            let session = Session::start(&session.options(&name))?;
            if overview {
                session.action(niri_ipc::Action::OpenOverview {})?;
            }
            // Readiness means each surface committed a frame; give niri a
            // moment to composite it (and its blur) before capturing.
            std::thread::sleep(Duration::from_millis(250));
            let path = session::visual_dir()?.join(format!("{name}.png"));
            session.screenshot(&path).context("taking the screenshot")?;
            session.quit()?;
            println!("{}", path.display());
            Ok(())
        }
    }
}
