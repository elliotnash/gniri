//! gniri: a GNOME Shell lookalike for the niri compositor.

mod app;
mod ready;
mod style;
mod surfaces;

use clap::Parser;
use gtk::glib;

/// Command line of the shell daemon.
///
/// Later milestones add the `lock`, `polkit` and `msg` subcommands, which run
/// as separate processes from this same binary.
#[derive(Parser, Debug, Clone)]
#[command(name = "gniri", version, about)]
pub struct Cli {
    /// Wallpaper to draw: an image file, or a colour such as "#808080".
    ///
    /// Without it gniri draws no wallpaper surface. Temporary: this moves to
    /// the config file in M1.
    #[arg(long, value_name = "FILE|COLOUR")]
    wallpaper: Option<String>,
}

fn main() -> glib::ExitCode {
    let cli = Cli::parse();
    app::run(cli)
}
