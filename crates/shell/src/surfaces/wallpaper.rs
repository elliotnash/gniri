//! The wallpaper surface on the background layer.
//!
//! niri zooms background-layer surfaces together with each workspace in the
//! overview, and its default ("xray") blur samples them, so this surface is
//! what the panel's blur shows.

use std::path::PathBuf;

use adw::prelude::*;
use gtk::gdk;
use gtk4_layer_shell::Layer;

use super::layer::{self, Anchor, Zone};
use crate::style;

#[derive(Clone, Debug, PartialEq)]
pub enum WallpaperSource {
    Colour(gdk::RGBA),
    Image(PathBuf),
}

impl WallpaperSource {
    /// Parses `"#rrggbb"` (or any CSS colour) or an image path.
    pub fn parse(spec: &str) -> Result<Self, String> {
        if let Ok(colour) = gdk::RGBA::parse(spec) {
            return Ok(Self::Colour(colour));
        }
        let path = PathBuf::from(spec);
        if path.is_file() {
            Ok(Self::Image(path))
        } else {
            Err(format!("{spec:?} is neither a colour nor an existing file"))
        }
    }
}

pub fn build(app: &adw::Application, source: &WallpaperSource) -> gtk::ApplicationWindow {
    let window = layer::window(
        app,
        "wallpaper",
        Layer::Background,
        Anchor::Fill,
        Zone::Ignore,
    );

    match source {
        WallpaperSource::Colour(colour) => {
            style::add_rule(&format!(
                "window.gniri-wallpaper-window {{ background-color: {colour}; }}"
            ));
        }
        WallpaperSource::Image(path) => match gdk::Texture::from_filename(path) {
            Ok(texture) => {
                // Cover is GNOME's "zoom": fill the output, crop the excess.
                let picture = gtk::Picture::builder()
                    .paintable(&texture)
                    .content_fit(gtk::ContentFit::Cover)
                    .can_shrink(true)
                    .build();
                window.set_child(Some(&picture));
            }
            Err(error) => {
                eprintln!("gniri: cannot load wallpaper {}: {error}", path.display());
            }
        },
    }

    window.present();
    window
}
