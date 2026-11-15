//! The wallpaper: two surfaces on the background layer.
//!
//! - `gniri-wallpaper` is the sharp image. niri zooms background-layer
//!   surfaces together with each workspace in the overview, and its default
//!   ("xray") blur samples them, so this is what the panel's blur shows.
//! - `gniri-backdrop` is a blurred, dimmed copy. `data/niri/gniri.kdl` places
//!   it behind the zoomed-out workspaces in the overview, where blur-my-shell
//!   shows the same thing.
//!
//! Both are rendered once at the output's pixel size and the full-size image
//! is dropped: wallpapers are often far larger than the screen (Fedora's is
//! 5120x5120, 105 MB uncompressed).
//!
//! The lock screen shows the blurred copy too ([`backdrop`]): blur-my-shell
//! blurs the lock screen's background the way it does the overview's. So
//! does the overview's own surface, in front of niri's workspaces, while
//! the app grid or a search shows ([`Wallpaper::cover`]).

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, graphene, gsk};
use gtk4_layer_shell::Layer;

use super::layer::{self, Anchor, Zone};
use crate::style;

/// blur-my-shell's default blur as a Gaussian sigma, in logical pixels
/// (`unscaled_radius` 30 / 4; docs/DECISIONS.md, S5).
const BACKDROP_SIGMA: f64 = 7.5;
/// blur-my-shell's default brightness.
const BACKDROP_BRIGHTNESS: f32 = 0.6;

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

/// The style class of a cover that is one flat colour.
const FLAT_COVER_CLASS: &str = "overview-cover-flat";
/// The name of the style rule that paints a flat-coloured wallpaper.
const COLOUR_RULE: &str = "wallpaper-colour";

/// Takes the style of a flat-coloured wallpaper away again: the wallpaper
/// is now a picture, or none.
pub fn forget_colour() {
    style::set_rule(COLOUR_RULE, "");
}

/// One output's wallpaper.
pub struct Wallpaper {
    /// The wallpaper and backdrop surfaces.
    pub windows: [gtk::ApplicationWindow; 2],
    /// The backdrop's picture, for an image wallpaper.
    blurred: Option<gtk::Picture>,
}

impl Wallpaper {
    /// A widget that shows what the backdrop surface shows, to be laid out
    /// at the output's size: the same pixels in the same place, so that
    /// something drawn over the compositor's workspaces can look as if
    /// they were not there.
    pub fn cover(&self) -> gtk::Widget {
        match &self.blurred {
            // The same texture, not another copy of it.
            Some(blurred) => {
                let cover = picture();
                blurred
                    .bind_property("paintable", &cover, "paintable")
                    .sync_create()
                    .build();
                cover.upcast()
            }
            None => gtk::Box::builder()
                .css_classes([FLAT_COVER_CLASS])
                .build()
                .upcast(),
        }
    }
}

/// The cover where gniri draws no wallpaper: the flat colour GNOME's own
/// overview has without blur-my-shell, from the theme.
pub fn cover_without_wallpaper() -> gtk::Widget {
    gtk::Box::builder()
        .css_classes(["overview-cover-plain"])
        .build()
        .upcast()
}

/// Builds and maps the wallpaper and backdrop surfaces for `monitor`.
pub fn build(
    app: &adw::Application,
    monitor: &gdk::Monitor,
    source: &WallpaperSource,
) -> Wallpaper {
    let fill = |name| {
        layer::window(
            app,
            monitor,
            name,
            Layer::Background,
            Anchor::Fill,
            Zone::Ignore,
        )
    };
    let wallpaper = fill("wallpaper");
    let backdrop = fill("backdrop");

    let blurred = match source {
        WallpaperSource::Colour(colour) => {
            // A blurred flat colour is the same colour, so only dim it.
            let dimmed = gdk::RGBA::new(
                colour.red() * BACKDROP_BRIGHTNESS,
                colour.green() * BACKDROP_BRIGHTNESS,
                colour.blue() * BACKDROP_BRIGHTNESS,
                1.0,
            );
            // One rule for every output, replaced when the colour changes.
            style::set_rule(
                COLOUR_RULE,
                &format!(
                    "window.gniri-wallpaper-window {{ background-color: {colour}; }}
                     window.gniri-backdrop-window {{ background-color: {dimmed}; }}
                     .{FLAT_COVER_CLASS} {{ background-color: {dimmed}; }}"
                ),
            );
            None
        }
        WallpaperSource::Image(path) => {
            let sharp = picture();
            let image = Rc::new(Image {
                path: path.clone(),
                sharp: Some(sharp.clone()),
                blurred: picture(),
                rendered_for: Cell::new(None),
            });
            wallpaper.set_child(Some(&sharp));
            backdrop.set_child(Some(&image.blurred));
            image.follow(&wallpaper);
            Some(image.blurred.clone())
        }
    };

    wallpaper.present();
    backdrop.present();
    Wallpaper {
        windows: [wallpaper, backdrop],
        blurred,
    }
}

/// A widget that shows the blurred, dimmed wallpaper at the size of
/// `window`, which it is to be put in: what the lock screen has behind its
/// clock. `class` styles it.
pub fn backdrop(
    window: &gtk::ApplicationWindow,
    source: &WallpaperSource,
    class: &str,
) -> gtk::Widget {
    match source {
        WallpaperSource::Colour(colour) => {
            let dimmed = gdk::RGBA::new(
                colour.red() * BACKDROP_BRIGHTNESS,
                colour.green() * BACKDROP_BRIGHTNESS,
                colour.blue() * BACKDROP_BRIGHTNESS,
                1.0,
            );
            style::add_rule(&format!(".{class} {{ background-color: {dimmed}; }}"));
            gtk::Box::builder().css_classes([class]).build().upcast()
        }
        WallpaperSource::Image(path) => {
            let image = Rc::new(Image {
                path: path.clone(),
                sharp: None,
                blurred: picture(),
                rendered_for: Cell::new(None),
            });
            image.blurred.add_css_class(class);
            image.follow(window);
            image.blurred.clone().upcast()
        }
    }
}

/// A picture showing a texture that already has the output's pixel size.
fn picture() -> gtk::Picture {
    gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Fill)
        .can_shrink(true)
        .build()
}

/// An image wallpaper and the pictures showing it.
struct Image {
    path: PathBuf,
    /// Absent where only the blurred copy is shown.
    sharp: Option<gtk::Picture>,
    blurred: gtk::Picture,
    /// Physical pixel size the pictures were last rendered for.
    rendered_for: Cell<Option<(u32, u32)>>,
}

impl Image {
    /// Renders the pictures when `window`'s size and scale are known, and
    /// again whenever either changes.
    fn follow(self: &Rc<Self>, window: &gtk::ApplicationWindow) {
        let image = Rc::clone(self);
        window.connect_realize(move |window| {
            let surface = window.surface().expect("a realized window has a surface");
            let on_layout = {
                let (image, window) = (Rc::clone(&image), window.clone());
                move |_: &gdk::Surface, _: i32, _: i32| image.update(&window)
            };
            let on_scale = {
                let (image, window) = (Rc::clone(&image), window.clone());
                move |_: &gdk::Surface| image.update(&window)
            };
            surface.connect_layout(on_layout);
            surface.connect_scale_notify(on_scale);
        });
    }

    /// Re-renders both pictures if the output's pixel size changed.
    fn update(&self, window: &gtk::ApplicationWindow) {
        let Some(surface) = window.surface() else {
            return;
        };
        let scale = surface.scale();
        let to_physical = |logical: i32| (f64::from(logical) * scale).ceil() as u32;
        let size = (to_physical(surface.width()), to_physical(surface.height()));
        if size.0 == 0 || size.1 == 0 || self.rendered_for.get() == Some(size) {
            return;
        }
        let Some(renderer) = window.renderer() else {
            return;
        };
        // Whatever happens below, do not retry for this size on every frame.
        self.rendered_for.set(Some(size));

        // Synchronous: about 160 ms for the Fedora wallpaper.
        let original = match gdk::Texture::from_filename(&self.path) {
            Ok(texture) => texture,
            Err(error) => {
                report_load_error(&self.path, &error);
                return;
            }
        };
        let sharp = render_sharp(&renderer, &original, size);
        let blurred = render_backdrop(&renderer, &sharp, size, BACKDROP_SIGMA * scale);
        if let Some(picture) = &self.sharp {
            picture.set_paintable(Some(&sharp));
        }
        self.blurred.set_paintable(Some(&blurred));
    }
}

fn report_load_error(path: &Path, error: &gtk::glib::Error) {
    eprintln!("gniri: cannot load wallpaper {}: {error}", path.display());
}

fn bounds((width, height): (u32, u32)) -> graphene::Rect {
    graphene::Rect::new(0.0, 0.0, width as f32, height as f32)
}

fn render(renderer: &gsk::Renderer, snapshot: gtk::Snapshot, size: (u32, u32)) -> gdk::Texture {
    let node = snapshot
        .to_node()
        .expect("the wallpaper snapshot always draws something");
    renderer.render_texture(node, Some(&bounds(size)))
}

/// Scales `original` to cover `size`, cropping the excess equally on both
/// sides. This is GNOME's "zoom" picture option.
fn render_sharp(
    renderer: &gsk::Renderer,
    original: &gdk::Texture,
    size: (u32, u32),
) -> gdk::Texture {
    let (width, height) = (size.0 as f32, size.1 as f32);
    let (image_width, image_height) = (original.width() as f32, original.height() as f32);
    let scale = (width / image_width).max(height / image_height);
    let (scaled_width, scaled_height) = (image_width * scale, image_height * scale);
    let cover = graphene::Rect::new(
        (width - scaled_width) / 2.0,
        (height - scaled_height) / 2.0,
        scaled_width,
        scaled_height,
    );

    let snapshot = gtk::Snapshot::new();
    snapshot.push_clip(&bounds(size));
    // Trilinear: the image is usually being shrunk by more than half.
    snapshot.append_scaled_texture(original, gsk::ScalingFilter::Trilinear, &cover);
    snapshot.pop();
    render(renderer, snapshot, size)
}

/// Blurs `sharp` with a Gaussian of `sigma` physical pixels and dims it.
fn render_backdrop(
    renderer: &gsk::Renderer,
    sharp: &gdk::Texture,
    size: (u32, u32),
    sigma: f64,
) -> gdk::Texture {
    let area = bounds(size);
    let (width, height) = (area.width(), area.height());
    // The blur must see image, not transparency, beyond the edges, or the
    // screen edges come out dark. Mirrored copies around the image stand in
    // for the clamp-to-edge sampling blur-my-shell uses.
    let reach = (sigma * 4.0).ceil() as f32;

    let snapshot = gtk::Snapshot::new();
    snapshot.push_clip(&area);
    // GSK's blur radius is twice the Gaussian sigma.
    snapshot.push_blur(sigma * 2.0);
    snapshot.push_clip(&area.inset_r(-reach, -reach));
    for row in -1..=1 {
        for column in -1..=1 {
            snapshot.save();
            snapshot.translate(&graphene::Point::new(
                column as f32 * width,
                row as f32 * height,
            ));
            if column != 0 {
                snapshot.translate(&graphene::Point::new(width, 0.0));
                snapshot.scale(-1.0, 1.0);
            }
            if row != 0 {
                snapshot.translate(&graphene::Point::new(0.0, height));
                snapshot.scale(1.0, -1.0);
            }
            snapshot.append_texture(sharp, &area);
            snapshot.restore();
        }
    }
    snapshot.pop();
    snapshot.pop();
    let shade = gdk::RGBA::new(0.0, 0.0, 0.0, 1.0 - BACKDROP_BRIGHTNESS);
    snapshot.append_color(&shade, &area);
    snapshot.pop();
    render(renderer, snapshot, size)
}
