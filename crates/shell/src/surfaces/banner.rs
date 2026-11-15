//! Notification banners: a small overlay surface under the panel of each
//! output, mapped only while it shows a notification, and the tray that
//! decides which notification that is and for how long. Port of
//! `MessageTray` in `js/ui/messageTray.js`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gniri_core::Subscription;
use gniri_services::notifications::{self, Command, Urgency};
use gtk::{cairo, gdk, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use super::layer;
use crate::menus::Menu;
use crate::messages::{self, Message};
use crate::services::Services;

/// messageTray.js `ANIMATION_TIME`. The theme's transition on
/// `.notification-banner` lasts as long.
const ANIMATION_TIME: Duration = Duration::from_millis(200);
/// messageTray.js `NOTIFICATION_TIMEOUT`.
const NOTIFICATION_TIMEOUT: Duration = Duration::from_millis(4000);
/// messageTray.js `HIDE_TIMEOUT`: how long after the pointer left a banner
/// it goes away.
const HIDE_TIMEOUT: Duration = Duration::from_millis(200);

fn animated() -> bool {
    gtk::Settings::default().is_some_and(|settings| settings.is_gtk_enable_animations())
}

struct Surface {
    window: gtk::ApplicationWindow,
    /// `.gniri-banner-area`: the banner with room around it for its shadow.
    area: gtk::Box,
    /// Counts shows and hides, so a fade-out overtaken by a new banner does
    /// not unmap.
    generation: Cell<u64>,
}

/// One output's banner surface.
#[derive(Clone)]
pub struct Banner {
    pub window: gtk::ApplicationWindow,
    surface: Rc<Surface>,
}

/// Builds the banner surface for `monitor`, unmapped.
pub fn build(app: &adw::Application, monitor: &gdk::Monitor) -> Banner {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .css_classes(["gniri-layer", "gniri-banner-window"])
        .build();
    window.init_layer_shell();
    window.set_namespace(Some("gniri-banner"));
    window.set_layer(Layer::Overlay);
    window.set_monitor(Some(monitor));
    // Top centre, below the panel: the panel's exclusive zone pushes it
    // down, as GNOME's tray is laid out in the work area.
    window.set_anchor(Edge::Top, true);
    window.set_keyboard_mode(KeyboardMode::None);

    let area = gtk::Box::builder()
        .css_classes(["gniri-banner-area"])
        .build();
    window.set_child(Some(&area));

    let surface = Rc::new(Surface {
        window: window.clone(),
        area,
        generation: Cell::new(0),
    });
    // Only the banner itself takes input, not the room for its shadow.
    // Worked out from the surface's size, which is known the moment the
    // compositor has placed it; the banner's own box is not until GTK has
    // laid the new content out, which is later.
    let token = |name: &str| {
        gniri_theme::token(name)
            .and_then(|value| value.strip_suffix("px"))
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(0)
    };
    let margin = token("notification-banner-margin");
    let room = token("notification-banner-shadow-room");
    window.connect_realize(move |window| {
        let Some(surface) = window.surface() else {
            return;
        };
        surface.connect_layout(move |surface, width, height| {
            let region = cairo::Region::create();
            if !layer::pointer_ignored() {
                let _ = region.union_rectangle(&cairo::RectangleInt::new(
                    room + margin,
                    margin,
                    (width - 2 * (room + margin)).max(0),
                    (height - room - 2 * margin).max(0),
                ));
            }
            surface.set_input_region(Some(&region));
        });
    });
    Banner { window, surface }
}

impl Banner {
    /// Maps the surface with `banner` on it.
    /// `MessageTray._showNotification`
    fn show(&self, banner: &gtk::Box) {
        let surface = &self.surface;
        let generation = surface.generation.get() + 1;
        surface.generation.set(generation);

        while let Some(old) = surface.area.first_child() {
            surface.area.remove(&old);
        }
        surface.area.append(banner);
        surface.window.present();
        // The surface must be on screen with the banner in its hidden look
        // before the class that starts the transition is added.
        glib::idle_add_local_once({
            let (surface, banner) = (Rc::clone(surface), banner.clone());
            move || {
                if surface.generation.get() == generation {
                    banner.add_css_class("open");
                }
            }
        });
    }

    /// Fades the banner out (or drops it at once) and unmaps the surface,
    /// then calls `done`. `MessageTray._hideNotification`
    fn hide(&self, animate: bool, done: impl FnOnce() + 'static) {
        let surface = &self.surface;
        let generation = surface.generation.get() + 1;
        surface.generation.set(generation);
        if let Some(banner) = surface.area.first_child() {
            banner.remove_css_class("open");
        }
        let finish = {
            let surface = Rc::clone(surface);
            move || {
                // A mapped surface with nothing to draw is never repainted
                // (docs/DECISIONS.md, S3), so it goes away with its banner.
                if surface.generation.get() == generation {
                    surface.window.set_visible(false);
                    while let Some(old) = surface.area.first_child() {
                        surface.area.remove(&old);
                    }
                }
                done();
            }
        };
        if animate && animated() {
            glib::timeout_add_local_once(ANIMATION_TIME, finish);
        } else {
            finish();
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Hidden,
    Shown,
    Hiding,
}

/// The banner that is up.
struct Showing {
    banner: Banner,
    message: Message,
    shown: notifications::Banner,
    critical: bool,
}

struct Inner {
    services: Rc<Services>,
    /// The surface of the output a new banner goes to.
    surface: Box<dyn Fn() -> Option<Banner>>,
    phase: Cell<Phase>,
    showing: RefCell<Option<Showing>>,
    /// Runs while the banner has time left on screen.
    timeout: RefCell<Option<glib::SourceId>>,
    /// Runs after the pointer left the banner.
    left_timeout: RefCell<Option<glib::SourceId>>,
    /// The banner's time is up.
    timed_out: Cell<bool>,
    pointer_in: Cell<bool>,
}

/// Shows the notifications that ask for a banner, one at a time. Dropping
/// it stops that.
pub struct Tray {
    _inner: Rc<Inner>,
    _subscriptions: Vec<Subscription>,
}

impl Inner {
    fn clear_timeout(&self) {
        if let Some(timeout) = self.timeout.take() {
            timeout.remove();
        }
    }

    fn clear_left_timeout(&self) {
        if let Some(timeout) = self.left_timeout.take() {
            timeout.remove();
        }
    }

    /// `MessageTray._showNotificationCompleted`: a banner stays four
    /// seconds after it has slid in; a critical one until it is dealt with.
    fn start_timeout(self: &Rc<Self>, critical: bool) {
        self.clear_timeout();
        self.timed_out.set(false);
        if critical {
            return;
        }
        let slide = if animated() {
            ANIMATION_TIME
        } else {
            Duration::ZERO
        };
        let timeout = glib::timeout_add_local_once(slide + NOTIFICATION_TIMEOUT, {
            let inner = Rc::downgrade(self);
            move || {
                if let Some(inner) = inner.upgrade() {
                    inner.timeout.take();
                    inner.timed_out.set(true);
                    inner.update();
                }
            }
        });
        self.timeout.replace(Some(timeout));
    }

    /// `MessageTray._onNotificationHoverChanged`
    fn hover_changed(self: &Rc<Self>, hovered: bool) {
        // A banner that is on its way out is left by the pointer as its
        // surface goes; that must not count against the next one.
        if self.phase.get() != Phase::Shown {
            return;
        }
        self.clear_left_timeout();
        if hovered {
            self.pointer_in.set(true);
            self.update();
            return;
        }
        // Wait a little before hiding, in case the pointer comes back.
        let timeout = glib::timeout_add_local_once(HIDE_TIMEOUT, {
            let inner = Rc::downgrade(self);
            move || {
                if let Some(inner) = inner.upgrade() {
                    inner.left_timeout.take();
                    // `_onNotificationLeftTimeout`: having been looked at,
                    // the banner has no more time to wait out.
                    inner.pointer_in.set(false);
                    inner.clear_timeout();
                    inner.timed_out.set(true);
                    inner.update();
                }
            }
        });
        self.left_timeout.replace(Some(timeout));
    }

    fn show(self: &Rc<Self>, shown: notifications::Banner) {
        let Some(banner) = (self.surface)() else {
            return;
        };
        let found = self
            .services
            .notifications
            .state
            .with(|state| messages::find(state, shown.id));
        let Some((source, notification)) = found else {
            return;
        };

        let message = messages::notification_message(&self.services, shown.id, || false, || {});
        message.root().add_css_class("notification-banner");
        message.set_expand_button_visible(false);
        let now = self.services.time.get();
        messages::show_notification(&message, &source, &notification, &now);

        let motion = gtk::EventControllerMotion::new();
        motion.connect_enter({
            let inner = Rc::downgrade(self);
            move |_, _, _| {
                if let Some(inner) = inner.upgrade() {
                    inner.hover_changed(true);
                }
            }
        });
        motion.connect_leave({
            let inner = Rc::downgrade(self);
            move |_| {
                if let Some(inner) = inner.upgrade() {
                    inner.hover_changed(false);
                }
            }
        });
        message.root().add_controller(motion);

        let critical = notification.urgency == Urgency::Critical;
        // `_updateShowingNotification`: critical notifications show in
        // full from the start.
        if critical {
            message.expand();
        }
        banner.show(message.root());
        self.pointer_in.set(false);
        self.clear_left_timeout();
        self.start_timeout(critical);
        self.showing.replace(Some(Showing {
            banner,
            message,
            shown,
            critical,
        }));
        self.phase.set(Phase::Shown);
    }

    /// Takes the banner down. `tell` says whether the notification is still
    /// there to be told its banner is over.
    fn hide(self: &Rc<Self>, animate: bool, tell: bool) {
        let Some(showing) = self.showing.take() else {
            return;
        };
        self.clear_timeout();
        self.clear_left_timeout();
        self.phase.set(Phase::Hiding);
        showing.banner.hide(animate, {
            let inner = Rc::clone(self);
            // Kept until the banner is gone: its widgets are fading.
            let message = showing.message;
            move || {
                drop(message);
                inner.phase.set(Phase::Hidden);
                inner.pointer_in.set(false);
                if tell {
                    inner.services.notifications.send(Command::BannerHidden);
                }
                inner.update();
            }
        });
    }

    /// `MessageTray._updateState`: all of what happens when is decided
    /// here; everything else only records what changed and calls this.
    fn update(self: &Rc<Self>) {
        let (wanted, waiting) = self
            .services
            .notifications
            .state
            .with(|state| (state.banner, !state.queue.is_empty()));
        // `Main.messageTray.bannerBlocked`: no banners over the list they
        // would end up in.
        let blocked = self.services.menus.with(|menus| {
            menus
                .open
                .as_ref()
                .is_some_and(|open| open.menu == Menu::DateMenu)
        });

        match self.phase.get() {
            Phase::Hidden => match wanted {
                Some(shown) if blocked => {
                    // Its turn came just as the list opened over it.
                    let _ = shown;
                    self.services.notifications.send(Command::BannerHidden);
                }
                Some(shown) => self.show(shown),
                None if waiting && !blocked => {
                    self.services.notifications.send(Command::ShowNextBanner);
                }
                None => {}
            },
            Phase::Shown => {
                let (shown, critical, expanded) = match &*self.showing.borrow() {
                    Some(showing) => (
                        showing.shown,
                        showing.critical,
                        showing.message.is_expanded(),
                    ),
                    None => return,
                };
                match wanted {
                    // The notification is gone: so is its banner, at once.
                    None => self.hide(false, false),
                    Some(now) if now.id != shown.id => self.hide(false, false),
                    Some(_) if blocked => self.hide(false, true),
                    Some(now) if now.updates != shown.updates => {
                        // It changed: show the new text and start its time
                        // over.
                        let found = self
                            .services
                            .notifications
                            .state
                            .with(|state| messages::find(state, now.id));
                        if let (Some((source, notification)), Some(showing)) =
                            (found, &mut *self.showing.borrow_mut())
                        {
                            let time = self.services.time.get();
                            messages::show_notification(
                                &showing.message,
                                &source,
                                &notification,
                                &time,
                            );
                            showing.shown = now;
                            showing.critical = notification.urgency == Urgency::Critical;
                            let critical = showing.critical;
                            self.start_timeout(critical);
                        }
                    }
                    Some(_) => {
                        let expired = self.timed_out.get() && !critical && !self.pointer_in.get();
                        if expired {
                            self.hide(true, true);
                        } else if self.pointer_in.get() && !expanded {
                            // `_expandBanner`
                            if let Some(showing) = &*self.showing.borrow() {
                                showing.message.expand();
                            }
                        }
                    }
                }
            }
            // `hide` calls back when it is done.
            Phase::Hiding => {}
        }
    }
}

impl Tray {
    /// Starts showing banners. `surface` is asked for the output's surface
    /// each time a banner is about to appear.
    pub fn start(services: &Rc<Services>, surface: impl Fn() -> Option<Banner> + 'static) -> Self {
        let inner = Rc::new(Inner {
            services: Rc::clone(services),
            surface: Box::new(surface),
            phase: Cell::new(Phase::Hidden),
            showing: RefCell::new(None),
            timeout: RefCell::new(None),
            left_timeout: RefCell::new(None),
            timed_out: Cell::new(false),
            pointer_in: Cell::new(false),
        });
        let update = |inner: &Rc<Inner>| {
            let inner = Rc::downgrade(inner);
            move || {
                if let Some(inner) = inner.upgrade() {
                    inner.update();
                }
            }
        };
        let subscriptions = vec![
            services
                .notifications
                .state
                .select(|state| (state.banner, !state.queue.is_empty()), {
                    let update = update(&inner);
                    move |_| update()
                }),
            services
                .menus
                .select(|menus| menus.open.as_ref().map(|open| open.menu), {
                    let update = update(&inner);
                    move |_| update()
                }),
        ];
        Self {
            _inner: inner,
            _subscriptions: subscriptions,
        }
    }
}
