//! gniri: a GNOME Shell lookalike for the niri compositor.

mod app;
mod debug;
mod ipc;
mod outputs;
mod panel;
mod ready;
mod services;
mod style;
mod surfaces;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use gtk::glib;

/// One binary, several processes: with no subcommand it is the shell itself.
#[derive(Parser, Debug, Clone)]
#[command(name = "gniri", version, about)]
pub struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Config file to read instead of ~/.config/gniri/config.toml.
    #[arg(long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Wallpaper: an image file, or a colour such as "#808080". Overrides
    /// the config file.
    #[arg(long, value_name = "FILE|COLOUR")]
    wallpaper: Option<String>,

    /// Freeze the clock and take service state from a scene file, for
    /// reproducible screenshots.
    #[arg(long, value_name = "FILE")]
    fixture: Option<PathBuf>,
}

#[derive(Subcommand, Debug, Clone)]
enum Command {
    /// Send a request to the running shell.
    Msg {
        #[command(subcommand)]
        message: Message,
    },
    /// Open a plain flat-coloured window (for tests).
    DebugWindow {
        #[arg(long, default_value = "#f6f5f4")]
        color: String,
    },
}

#[derive(Subcommand, Debug, Clone)]
enum Message {
    /// Inspect the running shell.
    Debug {
        #[command(subcommand)]
        request: DebugRequest,
    },
}

#[derive(Subcommand, Debug, Clone)]
enum DebugRequest {
    /// Print every mapped widget of every surface as JSON.
    DumpTree,
}

fn main() -> glib::ExitCode {
    let cli = Cli::parse();
    match &cli.command {
        None => app::run(cli),
        Some(Command::DebugWindow { color }) => debug::window::run(color),
        Some(Command::Msg { message }) => {
            let request = match message {
                Message::Debug {
                    request: DebugRequest::DumpTree,
                } => ipc::Request::DumpTree,
            };
            match ipc::send(&request) {
                Ok(value) => {
                    println!("{value:#}");
                    glib::ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("gniri: {error}");
                    glib::ExitCode::FAILURE
                }
            }
        }
    }
}
