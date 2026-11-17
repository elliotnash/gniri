// SPDX-License-Identifier: GPL-3.0-or-later

//! A level bar: a rounded trough filled up to a value. Port of
//! `js/ui/barLevel.js`.
//!
//! St draws these from `-barlevel-*` style properties, which GTK CSS does
//! not have, so the colours come from the theme's token table instead.

use std::cell::Cell;
use std::f64::consts::TAU;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{cairo, gdk};

/// A colour from the token table of the theme in use
/// (crates/theme/scss/_tokens.scss). Looked up when something is drawn:
/// the theme can change under a widget.
pub fn token_colour(name: &str) -> gdk::RGBA {
    crate::style::token(name)
        .and_then(|value| gdk::RGBA::parse(value).ok())
        .unwrap_or_else(|| {
            eprintln!("gniri: the theme has no colour token {name:?}");
            gdk::RGBA::WHITE
        })
}

#[derive(Clone, Copy)]
pub struct Colours {
    /// `-barlevel-background-color`
    pub trough: gdk::RGBA,
    /// `-barlevel-active-background-color`
    pub active: gdk::RGBA,
    /// `-barlevel-overdrive-color`
    pub overdrive: gdk::RGBA,
}

/// What `draw` needs to know.
#[derive(Clone, Copy)]
pub struct Level {
    pub value: f64,
    /// The value at the right end.
    pub maximum: f64,
    /// Values above this are drawn in the overdrive colour; equal to
    /// `maximum` for none.
    pub overdrive_start: f64,
    /// `-barlevel-height`
    pub bar_height: f64,
    /// `-barlevel-overdrive-separator-width`
    pub separator_width: f64,
}

fn set_source(cr: &cairo::Context, colour: &gdk::RGBA) {
    cr.set_source_rgba(
        f64::from(colour.red()),
        f64::from(colour.green()),
        f64::from(colour.blue()),
        f64::from(colour.alpha()),
    );
}

/// Draws the bar across `width`, centred in `height`. Port of
/// `BarLevel.vfunc_repaint` (left-to-right only).
pub fn draw(
    cr: &cairo::Context,
    width: f64,
    height: f64,
    level: &Level,
    colours: &Colours,
    foreground: &gdk::RGBA,
) {
    let bar = level.bar_height;
    let radius = width.min(bar) / 2.0;
    let (top, bottom) = ((height - bar) / 2.0, (height + bar) / 2.0);
    let span = width - 2.0 * radius;

    let end_x = if level.maximum > 0.0 {
        radius + span * (level.value / level.maximum).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let overdrive_active = level.overdrive_start < level.maximum;
    let separator_x = radius + span * (level.overdrive_start / level.maximum.max(f64::EPSILON));
    let separator = if overdrive_active {
        level.separator_width
    } else {
        0.0
    };
    let (arc_start, arc_end) = (radius, width - radius);

    // The trough, from the value to the right end.
    cr.arc(arc_end, height / 2.0, radius, TAU * 0.75, TAU * 0.25);
    cr.line_to(end_x, bottom);
    cr.line_to(end_x, top);
    cr.line_to(arc_end, top);
    set_source(cr, &colours.trough);
    let _ = cr.fill();

    // The filled part, from the left end to the value.
    let x = end_x.min(separator_x - separator / 2.0);
    cr.arc(arc_start, height / 2.0, radius, TAU * 0.25, TAU * 0.75);
    cr.line_to(x, top);
    cr.line_to(x, bottom);
    cr.line_to(arc_start, bottom);
    if level.value > 0.0 {
        set_source(cr, &colours.active);
    }
    let _ = cr.fill();

    // The part past the overdrive mark.
    if level.value > level.overdrive_start {
        let x = end_x.min(separator_x) + separator / 2.0;
        cr.rectangle(x, top, end_x - x, bar);
        set_source(cr, &colours.overdrive);
        let _ = cr.fill();
    }

    // The rounded end of the filled part.
    if level.value > 0.0 {
        set_source(
            cr,
            if level.value <= level.overdrive_start {
                &colours.active
            } else {
                &colours.overdrive
            },
        );
        cr.arc(end_x, height / 2.0, radius, TAU * 0.75, TAU * 0.25);
        cr.line_to(end_x.floor(), bottom);
        cr.line_to(end_x.floor(), top);
        cr.line_to(end_x, top);
        let _ = cr.fill();
    }

    if overdrive_active {
        cr.rectangle(separator_x - separator / 2.0, top, separator, bar);
        set_source(
            cr,
            if level.value <= level.overdrive_start {
                foreground
            } else {
                &colours.trough
            },
        );
        let _ = cr.fill();
    }
}

/// osdWindow.js `LEVEL_ANIMATION_TIME`
const LEVEL_ANIMATION_TIME: u32 = 100;

/// A level bar that only shows a value: the OSD's `.level`.
#[derive(Clone)]
pub struct LevelBar {
    pub widget: gtk::DrawingArea,
    level: Rc<Cell<Level>>,
    value: Rc<super::motion::Eased>,
}

impl LevelBar {
    /// `colours` is asked each time the bar is drawn.
    pub fn new(
        bar_height: f64,
        separator_width: f64,
        colours: impl Fn() -> Colours + 'static,
    ) -> Self {
        let level = Rc::new(Cell::new(Level {
            value: 0.0,
            maximum: 1.0,
            overdrive_start: 1.0,
            bar_height,
            separator_width,
        }));
        let widget = gtk::DrawingArea::builder().css_classes(["level"]).build();
        widget.set_draw_func({
            let level = Rc::clone(&level);
            move |area, cr, width, height| {
                draw(
                    cr,
                    f64::from(width),
                    f64::from(height),
                    &level.get(),
                    &colours(),
                    &area.color(),
                );
            }
        });
        let value = Rc::new(super::motion::Eased::new(&widget, 0.0, {
            let (level, widget) = (Rc::clone(&level), widget.downgrade());
            move |value| {
                let mut now = level.get();
                now.value = value;
                level.set(now);
                if let Some(widget) = widget.upgrade() {
                    widget.queue_draw();
                }
            }
        }));
        Self {
            widget,
            level,
            value,
        }
    }

    fn set_maximum(&self, maximum: f64) {
        let mut level = self.level.get();
        level.maximum = maximum;
        level.overdrive_start = maximum.min(1.0);
        self.level.set(level);
    }

    /// Shows `value` out of `maximum` at once; anything above 1.0 is
    /// overdrive.
    pub fn set(&self, value: f64, maximum: f64) {
        self.set_maximum(maximum);
        self.value.jump(value);
    }

    /// The same, the bar gliding there (`OsdWindow.setLevel` while it
    /// shows).
    pub fn glide(&self, value: f64, maximum: f64) {
        self.set_maximum(maximum);
        self.value
            .ease(value, LEVEL_ANIMATION_TIME, adw::Easing::EaseOutQuad, || {});
    }
}
