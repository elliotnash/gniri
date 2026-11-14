//! gniri: a GNOME Shell lookalike for the niri compositor.

mod app;
mod debug;
mod ipc;
mod keys;
mod menus;
mod outputs;
mod panel;
mod quick_settings;
mod ready;
mod services;
mod settings;
mod style;
mod surfaces;
mod widgets;

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
    /// Change the output volume by one step, or toggle mute, and show the
    /// on-screen display. Bind this to the volume keys.
    Volume { key: ipc::VolumeKey },
    /// Change the screen brightness by one step and show the on-screen
    /// display. Bind this to the brightness keys.
    Brightness { step: ipc::Step },
    /// Inspect or drive the running shell (for tests).
    Debug {
        #[command(subcommand)]
        request: DebugRequest,
    },
}

#[derive(Subcommand, Debug, Clone)]
enum DebugRequest {
    /// Print every mapped widget of every surface as JSON.
    DumpTree,
    /// Print every command the shell has sent to a service.
    Commands,
    /// Open a panel menu.
    OpenMenu {
        menu: menus::Menu,
        /// Connector name of the output; the first with a panel if unset.
        #[arg(long)]
        output: Option<String>,
    },
    /// Open a menu inside the quick settings menu.
    OpenSubmenu { submenu: menus::Submenu },
    /// Close whatever menu is open.
    CloseMenu,
}

impl Message {
    fn request(&self) -> ipc::Request {
        match self.clone() {
            Self::Volume { key } => ipc::Request::Volume(key),
            Self::Brightness { step } => ipc::Request::Brightness(step),
            Self::Debug { request } => match request {
                DebugRequest::DumpTree => ipc::Request::DumpTree,
                DebugRequest::Commands => ipc::Request::Commands,
                DebugRequest::OpenMenu { menu, output } => ipc::Request::OpenMenu { menu, output },
                DebugRequest::OpenSubmenu { submenu } => ipc::Request::OpenSubmenu(submenu),
                DebugRequest::CloseMenu => ipc::Request::CloseMenu,
            },
        }
    }
}

fn main() -> glib::ExitCode {
    let cli = Cli::parse();
    match &cli.command {
        None => app::run(cli),
        Some(Command::DebugWindow { color }) => debug::window::run(color),
        Some(Command::Msg { message }) => match ipc::send(&message.request()) {
            Ok(value) => {
                if !value.is_null() {
                    println!("{value:#}");
                }
                glib::ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("gniri: {error}");
                glib::ExitCode::FAILURE
            }
        },
    }
}
