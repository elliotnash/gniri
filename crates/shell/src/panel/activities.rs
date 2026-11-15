// SPDX-License-Identifier: GPL-3.0-or-later

//! The activities button with its workspace dots. Port of `WorkspaceDot`,
//! `WorkspaceIndicators` and `ActivitiesButton` in `js/ui/panel.js`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use gniri_core::Subscription;

use super::{button, set_checked};
use crate::services::Services;

/// `.workspace-dot` min-width and min-height (`$scalable_icon_size * 0.5`).
const DOT_SIZE: f64 = 8.0;
/// panel.js `INACTIVE_WORKSPACE_DOT_SCALE`.
const INACTIVE_SCALE: f64 = 0.75;
/// `_updateVisuals`: an inactive dot is half transparent.
const INACTIVE_OPACITY: f64 = 0.5;
/// GNOME moves the dots with the workspace switch animation
/// (`WINDOW_ANIMATION_TIME` in workspaceAnimation.js).
const SWITCH_DURATION_MS: u32 = 250;
/// windowManager.js `SCROLL_TIMEOUT_TIME`: at most one switch per this long.
const SCROLL_INTERVAL: Duration = Duration::from_millis(150);

/// How much wider than a dot the active workspace's pill is
/// (`WorkspaceIndicators._updateExpansion`).
fn width_multiplier(dots: usize) -> f64 {
    match dots {
        0..=2 => 3.625,
        3..=5 => 3.25,
        _ => 2.75,
    }
}

fn lerp(from: f64, to: f64, amount: f64) -> f64 {
    from + (to - from) * amount
}

/// One workspace's dot. GNOME gives each dot a slot whose width grows as the
/// workspace becomes active and draws the dot scaled inside it; here the slot
/// is the widget's margins.
struct Dot {
    workspace: u64,
    widget: gtk::Box,
    /// 0 for an inactive workspace, 1 for the active one, between while
    /// switching.
    expansion: Cell<f64>,
    multiplier: Cell<f64>,
    animation: RefCell<Option<adw::TimedAnimation>>,
}

impl Dot {
    fn new(workspace: u64, active: bool, multiplier: f64) -> Rc<Self> {
        let dot = Rc::new(Self {
            workspace,
            widget: gtk::Box::builder()
                .css_classes(["workspace-dot"])
                .valign(gtk::Align::Center)
                .build(),
            expansion: Cell::new(if active { 1.0 } else { 0.0 }),
            multiplier: Cell::new(multiplier),
            animation: RefCell::new(None),
        });
        dot.apply();
        dot
    }

    /// Sizes the dot and its slot for the current expansion
    /// (`WorkspaceDot.vfunc_get_preferred_width` and `_updateVisuals`).
    fn apply(&self) {
        let expansion = self.expansion.get();
        let slot = (DOT_SIZE * lerp(1.0, self.multiplier.get(), expansion)).round() as i32;
        let scale = lerp(INACTIVE_SCALE, 1.0, expansion);
        let width = (f64::from(slot) * scale).round() as i32;
        let height = (DOT_SIZE * scale).round() as i32;
        let before = (slot - width) / 2;
        self.widget.set_size_request(width, height);
        self.widget.set_margin_start(before);
        self.widget.set_margin_end(slot - width - before);
        self.widget
            .set_opacity(lerp(INACTIVE_OPACITY, 1.0, expansion));
    }

    fn set_multiplier(&self, multiplier: f64) {
        if self.multiplier.replace(multiplier) != multiplier {
            self.apply();
        }
    }

    /// Animates towards active or inactive.
    fn set_active(self: &Rc<Self>, active: bool) {
        let target = if active { 1.0 } else { 0.0 };
        let previous = self.animation.borrow_mut().take();
        let already_heading_there = previous
            .as_ref()
            .is_some_and(|animation| animation.value_to() == target);
        if let Some(animation) = previous {
            if already_heading_there {
                *self.animation.borrow_mut() = Some(animation);
                return;
            }
            animation.pause();
        }
        if self.expansion.get() == target {
            return;
        }

        let dot = Rc::downgrade(self);
        let callback = adw::CallbackAnimationTarget::new(move |value| {
            if let Some(dot) = dot.upgrade() {
                dot.expansion.set(value);
                dot.apply();
            }
        });
        let animation = adw::TimedAnimation::builder()
            .widget(&self.widget)
            .value_from(self.expansion.get())
            .value_to(target)
            .duration(SWITCH_DURATION_MS)
            .easing(adw::Easing::EaseOutCubic)
            .target(&callback)
            .build();
        animation.play();
        *self.animation.borrow_mut() = Some(animation);
    }
}

pub fn build(
    services: &Rc<Services>,
    output: Option<&str>,
    subscriptions: &mut Vec<Subscription>,
) -> gtk::Button {
    let indicators = gtk::Box::builder()
        .css_classes(["workspace-indicators"])
        .build();
    let button = button(&indicators);
    button.set_widget_name("panelActivities");
    button.set_tooltip_text(None);
    button.update_property(&[gtk::accessible::Property::Label("Activities")]);

    button.connect_clicked({
        let services = Rc::clone(services);
        move |_| services.niri_action(niri_ipc::Action::ToggleOverview {})
    });

    // Scrolling switches workspace on this panel's output
    // (`ActivitiesButton.vfunc_scroll_event`).
    let scroll = gtk::EventControllerScroll::new(
        gtk::EventControllerScrollFlags::VERTICAL | gtk::EventControllerScrollFlags::DISCRETE,
    );
    scroll.connect_scroll({
        let services = Rc::clone(services);
        let output = output.map(str::to_owned);
        let last = Cell::new(None::<Instant>);
        move |_, _, dy| {
            if dy == 0.0
                || last
                    .get()
                    .is_some_and(|last| last.elapsed() < SCROLL_INTERVAL)
            {
                return gtk::glib::Propagation::Stop;
            }
            last.set(Some(Instant::now()));
            let target = services.niri.with(|niri| {
                let workspaces = workspaces_on(niri, output.as_deref());
                let active = workspaces.iter().position(|(_, active)| *active)?;
                let next = if dy > 0.0 {
                    active + 1
                } else {
                    active.checked_sub(1)?
                };
                workspaces.get(next).map(|(id, _)| *id)
            });
            if let (Some(id), Some(client)) = (target, &services.niri_client)
                && let Err(error) = client.focus_workspace(id)
            {
                eprintln!("gniri: cannot switch workspace: {error}");
            }
            gtk::glib::Propagation::Stop
        }
    });
    button.add_controller(scroll);

    // Checked while the overview is open.
    subscriptions.push(services.niri.select(|niri| niri.overview_open, {
        let button = button.clone();
        move |open| set_checked(&button, *open)
    }));

    // One dot per workspace on this output.
    let dots: RefCell<Vec<Rc<Dot>>> = RefCell::new(Vec::new());
    subscriptions.push(services.niri.select(
        {
            let output = output.map(str::to_owned);
            move |niri| workspaces_on(niri, output.as_deref())
        },
        move |workspaces| sync_dots(&indicators, &mut dots.borrow_mut(), workspaces),
    ));

    button
}

/// `(id, active)` for each workspace of `output`, top to bottom. Without an
/// output name (a compositor that does not name its outputs) every workspace
/// counts.
fn workspaces_on(niri: &gniri_services::niri::State, output: Option<&str>) -> Vec<(u64, bool)> {
    match output {
        Some(output) => niri
            .workspaces_on(output)
            .map(|workspace| (workspace.id, workspace.active))
            .collect(),
        None => niri
            .workspaces
            .iter()
            .map(|workspace| (workspace.id, workspace.active))
            .collect(),
    }
}

/// Makes the row of dots match `workspaces`, reusing the dot of a workspace
/// that is still there so its animation carries on.
fn sync_dots(indicators: &gtk::Box, dots: &mut Vec<Rc<Dot>>, workspaces: &[(u64, bool)]) {
    let multiplier = width_multiplier(workspaces.len());
    let unchanged_order = dots.len() == workspaces.len()
        && dots
            .iter()
            .zip(workspaces)
            .all(|(dot, (id, _))| dot.workspace == *id);

    if !unchanged_order {
        let mut old: Vec<Rc<Dot>> = std::mem::take(dots);
        for dot in &old {
            indicators.remove(&dot.widget);
        }
        for (id, active) in workspaces {
            let dot = match old.iter().position(|dot| dot.workspace == *id) {
                Some(index) => old.swap_remove(index),
                None => Dot::new(*id, *active, multiplier),
            };
            indicators.append(&dot.widget);
            dots.push(dot);
        }
    }

    for (dot, (_, active)) in dots.iter().zip(workspaces) {
        dot.set_multiplier(multiplier);
        dot.set_active(*active);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Widths GNOME Shell 51.1 gave the active dot
    /// (tests/visual/reference/gnome-51.1/metrics/panel.json has the first).
    #[test]
    fn active_pill_width_follows_the_dot_count() {
        let width = |dots| (DOT_SIZE * width_multiplier(dots)).round();
        assert_eq!(width(1), 29.0);
        assert_eq!(width(2), 29.0);
        assert_eq!(width(3), 26.0);
        assert_eq!(width(5), 26.0);
        assert_eq!(width(6), 22.0);
    }
}
