// SPDX-License-Identifier: GPL-3.0-or-later

//! What idleness does: the screen dims, the screens turn off, the session
//! locks. The idle service says when; this acts on it.
//!
//! In GNOME the first two are gnome-settings-daemon's power plugin and the
//! third is the screen shield (`js/ui/screenShield.js`).

use std::cell::Cell;
use std::rc::Rc;

use gniri_core::Subscription;
use gniri_services::idle::{DIM_BRIGHTNESS, Timer};
use gniri_services::{brightness, session};

use crate::services::Services;

pub fn start(services: &Rc<Services>) -> Vec<Subscription> {
    // The brightness to go back to while the screen is dimmed.
    let undimmed: Rc<Cell<Option<f64>>> = Rc::default();
    let dim = services
        .idle
        .state
        .select(|idle| idle.has_passed(Timer::Dim), {
            let services = Rc::clone(services);
            move |passed| {
                if *passed {
                    // A screen that is darker already is left alone.
                    let level = services.brightness.state.with(|state| state.level);
                    if let Some(level) = level.filter(|level| *level > DIM_BRIGHTNESS) {
                        undimmed.set(Some(level));
                        services
                            .brightness
                            .send(brightness::Command::Set(DIM_BRIGHTNESS));
                    }
                } else if let Some(level) = undimmed.take() {
                    services.brightness.send(brightness::Command::Set(level));
                }
            }
        });

    // The screens turn off after the full delay, and much sooner while
    // the lock screen shows. niri turns them on again at the next input.
    let blank = |services: &Services| {
        let locked = services.session.state.with(|session| session.locked);
        services.idle.state.with(|idle| {
            idle.has_passed(Timer::Blank) || (locked && idle.has_passed(Timer::BlankLocked))
        })
    };
    let blank_when_idle = services.idle.state.select(
        {
            let services = Rc::downgrade(services);
            move |_| services.upgrade().is_some_and(|services| blank(&services))
        },
        {
            let services = Rc::downgrade(services);
            move |blank| {
                if *blank && let Some(services) = services.upgrade() {
                    services.power_off_monitors();
                }
            }
        },
    );
    // Locked by something other than idleness (a command from elsewhere)
    // while nobody is there.
    let blank_when_locked = services.session.state.select(|session| session.locked, {
        let services = Rc::downgrade(services);
        move |locked| {
            if *locked
                && let Some(services) = services.upgrade()
                && blank(&services)
            {
                services.power_off_monitors();
            }
        }
    });

    let lock = services
        .idle
        .state
        .select(|idle| idle.has_passed(Timer::Lock), {
            let services = Rc::downgrade(services);
            move |passed| {
                if *passed
                    && let Some(services) = services.upgrade()
                    && !services.session.state.with(|session| session.locked)
                {
                    services.session.send(session::Command::Lock);
                }
            }
        });

    vec![dim, blank_when_idle, blank_when_locked, lock]
}
