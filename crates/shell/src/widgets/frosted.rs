// SPDX-License-Identifier: GPL-3.0-or-later

//! A box the compositor blurs behind (`backdrop-filter` in the theme) that
//! can fade away with its blur: a menu, a banner, the OSD, a dialog's
//! shade.
//!
//! The compositor draws the blur at full strength wherever the surface
//! says, whatever the opacity of what is drawn there, and GTK says where
//! for as long as the widget is drawn at all. A blurred widget that only
//! fades keeps its blur to the last frame, where it goes at once
//! (docs/DECISIONS.md, "Closing what is blurred").
//!
//! So as it starts to close, the box takes a picture of the screen where
//! it is ([`Frosted::shoot`]), and draws that in place of itself and
//! without the blur ([`Frosted::close`]). The picture has the blur in it,
//! and the theme's transition fades and moves the box as before.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use gniri_services::capture::{self, Capturer, Format, Picture, Rect};
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene};

/// How long a picture may take to come before the box closes without one.
/// One of a menu takes about 5 ms and one of a whole output about 30 ms.
const PATIENCE: Duration = Duration::from_millis(100);

thread_local! {
    static CAPTURER: std::cell::OnceCell<Capturer> = const { std::cell::OnceCell::new() };
}

fn animated() -> bool {
    gtk::Settings::default().is_some_and(|settings| settings.is_gtk_enable_animations())
}

/// The process's connection for pictures, made when first asked for.
fn capturer() -> Capturer {
    CAPTURER.with(|capturer| capturer.get_or_init(Capturer::spawn).clone())
}

/// Makes the connection for pictures ahead of the first close, which
/// would come too soon after it for the compositor to have named its
/// outputs. Nothing is made where nothing animates, as under a test's
/// fixture.
pub fn prepare() {
    if animated() {
        capturer();
    }
}

/// Where the compositor has put a box's surface on its output.
#[derive(Clone, Copy, Debug)]
pub enum Place {
    /// Over all of it.
    Fill,
    /// At the top, in the middle, under whatever keeps `below` logical
    /// pixels to itself there (the panel).
    Top { below: i32 },
    /// At the bottom, in the middle.
    Bottom,
}

/// A picture of the screen where a box is, placed in the box's own
/// coordinates.
pub struct Shot {
    texture: gdk::Texture,
    bounds: graphene::Rect,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Frosted {
        /// Drawn instead of the box's own content while it closes.
        pub(super) shot: RefCell<Option<Shot>>,
        /// When the box last began to open.
        pub(super) opened: Cell<Option<Instant>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Frosted {
        const NAME: &'static str = "GniriFrosted";
        type Type = super::Frosted;
        type ParentType = gtk::Box;
    }

    impl ObjectImpl for Frosted {}

    impl WidgetImpl for Frosted {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            match &*self.shot.borrow() {
                // GTK has drawn the background under this, and cuts this
                // to the box's rounded corners (its overflow is hidden
                // meanwhile).
                Some(shot) => snapshot.append_texture(&shot.texture, &shot.bounds),
                None => self.parent_snapshot(snapshot),
            }
        }
    }

    impl BoxImpl for Frosted {}
}

glib::wrapper! {
    pub struct Frosted(ObjectSubclass<imp::Frosted>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl Frosted {
    /// A horizontal box with the CSS `classes`, one of which the theme
    /// gives a `backdrop-filter` (`bms-blur`).
    pub fn new(classes: &[&str]) -> Self {
        let frosted: Self = glib::Object::new();
        for class in classes {
            frosted.add_css_class(class);
        }
        frosted
    }

    /// Draws the box itself again, blurred behind, if a picture stood in
    /// for it.
    pub fn show_live(&self) {
        if self.imp().shot.take().is_some() {
            self.remove_css_class("ghost");
            self.set_overflow(gtk::Overflow::Visible);
            self.queue_draw();
        }
    }

    /// Starts the theme's transition to the open look (the `open` class).
    pub fn open(&self) {
        self.show_live();
        if !self.has_css_class("open") {
            self.imp().opened.set(Some(Instant::now()));
            self.add_css_class("open");
        }
    }

    /// Starts the theme's transition to the closed look, with `shot`, if
    /// there is one, standing in for the box and its blur from this frame
    /// on.
    pub fn close(&self, shot: Option<Shot>) {
        if let Some(shot) = shot {
            self.imp().shot.replace(Some(shot));
            // `.ghost` takes the `backdrop-filter` away.
            self.add_css_class("ghost");
            self.set_overflow(gtk::Overflow::Hidden);
            self.queue_draw();
        }
        self.remove_css_class("open");
    }

    /// What to ask for a picture of, where its corner goes in the box, and
    /// how many of its pixels make one of the box's.
    fn target(&self, place: Place) -> Option<(String, Rect, graphene::Point, f64)> {
        let native = self.native()?;
        let surface = native.surface()?;
        let monitor = surface.display().monitor_at_surface(&surface)?;
        let output = monitor.connector()?;
        // The border box on the surface and in the box's own coordinates.
        let on_surface = self.compute_bounds(&native)?;
        let own = self.compute_bounds(self)?;

        let whole = Rect {
            x: on_surface.x().floor() as i32,
            y: on_surface.y().floor() as i32,
            width: 0,
            height: 0,
        };
        let whole = Rect {
            width: (on_surface.x() + on_surface.width()).ceil() as i32 - whole.x,
            height: (on_surface.y() + on_surface.height()).ceil() as i32 - whole.y,
            ..whole
        };
        if whole.width <= 0 || whole.height <= 0 {
            return None;
        }
        // On whole device pixels of the surface, so that the picture is
        // drawn back onto the very pixels it was taken from.
        let scale = surface.scale();
        let aligned = capture::outward(whole, capture::whole_step(scale));

        let screen = monitor.geometry();
        let (x, y) = match place {
            Place::Fill => (0, 0),
            Place::Top { below } => (capture::centred(screen.width(), surface.width()), below),
            Place::Bottom => (
                capture::centred(screen.width(), surface.width()),
                screen.height() - surface.height(),
            ),
        };
        let on_output = Rect {
            x: x + aligned.x,
            y: y + aligned.y,
            ..aligned
        };
        // The compositor cuts a picture that reaches past the output's
        // edges to what there is. Past the right or the bottom that is
        // only a smaller picture; past the left or the top its corner
        // would no longer be where it was asked for.
        if on_output.x < 0 || on_output.y < 0 {
            return None;
        }
        let corner = graphene::Point::new(
            own.x() + (aligned.x as f32 - on_surface.x()),
            own.y() + (aligned.y as f32 - on_surface.y()),
        );
        Some((output.into(), on_output, corner, scale))
    }

    /// Takes a picture of the screen where the box is, for
    /// [`Frosted::close`], and gives it to `done`.
    ///
    /// `done` gets nothing, and at once, where the box closes without a
    /// fade (GTK's animations are off) or is not yet as open as it gets
    /// (it opened less than `fade` ago, and a picture of it half faded in
    /// would be faded once more). It gets nothing a little later where
    /// there is no picture to be had. Whoever asks goes on to close the
    /// box either way.
    pub fn shoot(&self, place: Place, fade: Duration, done: impl FnOnce(Option<Shot>) + 'static) {
        let settled = self
            .imp()
            .opened
            .get()
            .is_some_and(|opened| opened.elapsed() >= fade);
        if !animated() || !settled {
            done(None);
            return;
        }
        let Some((output, rect, corner, scale)) = self.target(place) else {
            done(None);
            return;
        };
        let answer = capturer().capture(&output, rect);
        glib::spawn_future_local(async move {
            let picture = glib::future_with_timeout(PATIENCE, answer.recv())
                .await
                .ok()
                .and_then(Result::ok)
                .flatten();
            done(picture.map(|picture| Shot {
                bounds: graphene::Rect::new(
                    corner.x(),
                    corner.y(),
                    (f64::from(picture.width) / scale) as f32,
                    (f64::from(picture.height) / scale) as f32,
                ),
                texture: texture(picture),
            }));
        });
    }
}

fn texture(picture: Picture) -> gdk::Texture {
    let format = match picture.format {
        Format::Bgrx => gdk::MemoryFormat::B8g8r8x8,
        Format::Rgbx => gdk::MemoryFormat::R8g8b8x8,
    };
    gdk::MemoryTexture::new(
        picture.width as i32,
        picture.height as i32,
        format,
        &glib::Bytes::from_owned(picture.pixels),
        picture.stride as usize,
    )
    .upcast()
}
