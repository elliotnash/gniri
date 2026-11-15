// SPDX-License-Identifier: GPL-3.0-or-later

//! Showing the pictures notifications and media players come with: an icon
//! name, an image file, or raw pixels.

use gniri_services::notifications::{Image, Pixels};
use gtk::prelude::*;
use gtk::{gdk, glib};

fn texture(pixels: &Pixels) -> gdk::Texture {
    let format = if pixels.has_alpha {
        gdk::MemoryFormat::R8g8b8a8
    } else {
        gdk::MemoryFormat::R8g8b8
    };
    gdk::MemoryTexture::new(
        pixels.width,
        pixels.height,
        format,
        &glib::Bytes::from(&pixels.data[..]),
        pixels.rowstride as usize,
    )
    .upcast()
}

/// Shows `picture` in `image`, or the icon called `fallback` if there is
/// none or it cannot be loaded. Returns whether what is shown is a symbolic
/// icon, which GNOME styles differently from a picture.
pub fn show(image: &gtk::Image, picture: Option<&Image>, fallback: &str) -> bool {
    match picture {
        Some(Image::Name(name)) => {
            let known = gtk::IconTheme::for_display(&image.display()).has_icon(name);
            let name = if known { name.as_str() } else { fallback };
            image.set_icon_name(Some(name));
            name.ends_with("-symbolic")
        }
        Some(Image::File(path)) => match gdk::Texture::from_filename(path) {
            Ok(texture) => {
                image.set_paintable(Some(&texture));
                false
            }
            Err(_) => {
                image.set_icon_name(Some(fallback));
                fallback.ends_with("-symbolic")
            }
        },
        Some(Image::Pixels(pixels)) => {
            image.set_paintable(Some(&texture(pixels)));
            false
        }
        None => {
            image.set_icon_name(Some(fallback));
            fallback.ends_with("-symbolic")
        }
    }
}
