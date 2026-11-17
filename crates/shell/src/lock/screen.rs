// SPDX-License-Identifier: GPL-3.0-or-later

//! One output's lock surface: the blurred wallpaper, and on it either the
//! clock or the password prompt. Port of `UnlockDialog`, `UnlockDialogClock`
//! and `UnlockDialogLayout` in `js/ui/unlockDialog.js`.

use gniri_core::tr;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gniri_core::{Subscription, lock_clock_date, lock_clock_time};
use gniri_services::mpris;
use gtk::{gdk, glib};
use gtk4_layer_shell::{KeyboardMode, Layer, LayerShell};

use super::Lock;
use super::prompt::{self, Prompt};
use crate::surfaces::layer::{self, Anchor, Zone};
use crate::surfaces::wallpaper;
use crate::widgets::motion::Eased;
use crate::widgets::scale_bin::ScaleBin;

/// unlockDialog.js `CROSSFADE_TIME`: the clock and the prompt change
/// places this fast.
const CROSSFADE_TIME: u32 = 300;
/// unlockDialog.js `HINT_TIMEOUT`: how long the clock shows before it
/// says what to do.
const HINT_TIMEOUT: Duration = Duration::from_secs(4);
/// unlockDialog.js `IDLE_TIMEOUT`: a prompt nobody types into goes back
/// to the clock after this long.
const IDLE_TIMEOUT: Duration = Duration::from_secs(2 * 60);
/// unlockDialog.js `AUTH_PROMPT_VERTICAL_CENTER_OFFSET`: the prompt sits
/// this far below the middle of the screen.
const AUTH_PROMPT_VERTICAL_CENTER_OFFSET: i32 = 50;

/// unlockDialog.js `FADE_OUT_TRANSLATION` and `FADE_OUT_SCALE`: the clock
/// rises and shrinks away as the prompt comes up from below and grows.
const FADE_OUT_TRANSLATION: f64 = 200.0;
const FADE_OUT_SCALE: f64 = 0.3;

mod imp {
    use std::cell::Cell;

    use gtk::glib;
    use gtk::prelude::*;
    use gtk::subclass::prelude::*;

    /// `UnlockStack`: the clock and the prompt over each other, as tall
    /// as the one that shows, and between the two while they change.
    #[derive(Default)]
    pub struct UnlockStack {
        pub(super) progress: Cell<f64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for UnlockStack {
        const NAME: &'static str = "GniriUnlockStack";
        type Type = super::UnlockStack;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for UnlockStack {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for UnlockStack {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let obj = self.obj();
            let (Some(clock), Some(prompt)) = (obj.first_child(), obj.last_child()) else {
                return (0, 0, -1, -1);
            };
            let (clock_min, clock_natural, _, _) = clock.measure(orientation, for_size);
            let (prompt_min, prompt_natural, _, _) = prompt.measure(orientation, for_size);
            if orientation == gtk::Orientation::Horizontal {
                return (
                    clock_min.max(prompt_min),
                    clock_natural.max(prompt_natural),
                    -1,
                    -1,
                );
            }
            let progress = self.progress.get();
            let between = |clock: i32, prompt: i32| {
                (f64::from(clock) + f64::from(prompt - clock) * progress) as i32
            };
            (
                between(clock_min, prompt_min),
                between(clock_natural, prompt_natural),
                -1,
                -1,
            )
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            // Each over the whole stack, centred where it needs more.
            let mut child = self.obj().first_child();
            while let Some(widget) = child {
                child = widget.next_sibling();
                let (min, _, _, _) = widget.measure(gtk::Orientation::Vertical, width);
                let tall = min.max(height);
                widget.size_allocate(
                    &gtk::Allocation::new(0, (height - tall) / 2, width, tall),
                    -1,
                );
            }
        }
    }
}

glib::wrapper! {
    pub struct UnlockStack(ObjectSubclass<imp::UnlockStack>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl UnlockStack {
    fn new(clock: &impl IsA<gtk::Widget>, prompt: &impl IsA<gtk::Widget>) -> Self {
        let stack: Self = glib::Object::new();
        clock.set_parent(&stack);
        prompt.set_parent(&stack);
        stack
    }

    fn set_progress(&self, progress: f64) {
        use gtk::subclass::prelude::ObjectSubclassIsExt;
        if self.imp().progress.replace(progress) != progress {
            self.queue_resize();
        }
    }
}

struct Inner {
    lock: std::rc::Weak<Lock>,
    /// 0 on the clock, 1 on the prompt (`_adjustment`).
    progress: Eased,
    hint: gtk::Label,
    prompt: Prompt,
    /// Shows the hint after a while on the clock.
    hint_timer: RefCell<Option<glib::SourceId>>,
    /// Goes back to the clock after a while on the prompt.
    idle_timer: RefCell<Option<glib::SourceId>>,
    /// True under a fixture: no timers.
    still: bool,
    on_prompt: Cell<bool>,
    /// What `progress` shows the prompt for.
    wants_prompt: Rc<Cell<bool>>,
}

pub struct Screen {
    pub window: gtk::ApplicationWindow,
    inner: Rc<Inner>,
    _subscriptions: Vec<Subscription>,
}

impl Screen {
    pub fn shows_prompt(&self) -> bool {
        self.window.is_realized() && self.inner.on_prompt.get()
    }
}

fn cancel(timer: &RefCell<Option<glib::SourceId>>) {
    if let Some(timer) = timer.take() {
        timer.remove();
    }
}

impl Inner {
    /// `_showClock`
    fn show_clock(self: &Rc<Self>) {
        self.on_prompt.set(false);
        self.wants_prompt.set(false);
        self.progress
            .ease(0.0, CROSSFADE_TIME, adw::Easing::EaseOutQuad, || {});
        self.prompt.entry.clear();
        cancel(&self.idle_timer);
        if let Some(lock) = self.lock.upgrade() {
            lock.prompt_shown(false);
        }

        // `UnlockDialogClock`: the hint appears once the user has looked
        // at the clock for a while without doing anything.
        self.hint.remove_css_class("shown");
        cancel(&self.hint_timer);
        if !self.still {
            let inner = Rc::downgrade(self);
            let timer = glib::timeout_add_local_once(HINT_TIMEOUT, move || {
                if let Some(inner) = inner.upgrade() {
                    inner.hint_timer.take();
                    inner.hint.add_css_class("shown");
                }
            });
            self.hint_timer.replace(Some(timer));
        }
    }

    /// `_showPrompt`
    fn show_prompt(self: &Rc<Self>) {
        self.on_prompt.set(true);
        self.wants_prompt.set(true);
        // There to take the keyboard from the start.
        self.progress.jump(self.progress.value());
        self.progress
            .ease(1.0, CROSSFADE_TIME, adw::Easing::EaseOutQuad, || {});
        self.prompt.entry.grab_focus();
        cancel(&self.hint_timer);
        self.touch();
        if let Some(lock) = self.lock.upgrade() {
            lock.prompt_shown(true);
        }
    }

    /// Someone is typing: the prompt stays.
    fn touch(self: &Rc<Self>) {
        cancel(&self.idle_timer);
        if self.still || !self.on_prompt.get() {
            return;
        }
        let inner = Rc::downgrade(self);
        let timer = glib::timeout_add_local_once(IDLE_TIMEOUT, move || {
            if let Some(inner) = inner.upgrade() {
                inner.idle_timer.take();
                inner.show_clock();
            }
        });
        self.idle_timer.replace(Some(timer));
    }
}

fn clock(lock: &Rc<Lock>, subscriptions: &mut Vec<Subscription>) -> (gtk::Box, gtk::Label) {
    let time = gtk::Label::builder()
        .css_classes(["unlock-dialog-clock-time"])
        .halign(gtk::Align::Center)
        .build();
    let date = gtk::Label::builder()
        .css_classes(["unlock-dialog-clock-date"])
        .halign(gtk::Align::Center)
        .build();
    let hint = gtk::Label::builder()
        .label(tr("Click or press a key"))
        .css_classes(["unlock-dialog-clock-hint"])
        .halign(gtk::Align::Center)
        .build();
    let clock = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .css_classes(["unlock-dialog-clock"])
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    clock.append(&time);
    clock.append(&date);
    clock.append(&hint);

    let hour12 = lock.hour12;
    subscriptions.push(lock.time.select(
        move |now| (lock_clock_time(now, hour12), lock_clock_date(now)),
        move |(time_text, date_text)| {
            time.set_label(time_text);
            date.set_label(date_text);
        },
    ));
    (clock, hint)
}

/// Builds the lock surface for `monitor`, unrealized: the session lock
/// maps it. A preview's is an ordinary surface over everything else.
pub fn build(lock: &Rc<Lock>, monitor: &gdk::Monitor) -> Screen {
    let window = if lock.is_preview() {
        let window = layer::window(
            lock.application(),
            monitor,
            "lock",
            Layer::Overlay,
            Anchor::Fill,
            Zone::Ignore,
        );
        window.set_keyboard_mode(KeyboardMode::Exclusive);
        window
    } else {
        let window = gtk::ApplicationWindow::builder()
            .application(lock.application())
            .css_classes(["gniri-layer", crate::debug::dump::SESSION_LOCK_CLASS])
            .build();
        if layer::pointer_ignored() {
            layer::take_no_input(&window);
        }
        window
    };
    window.set_widget_name("gniri-lock");
    window.add_css_class("gniri-lock-window");

    let mut subscriptions = Vec::new();
    let (clock, hint) = clock(lock, &mut subscriptions);
    let prompt = prompt::build(lock);
    // `UnlockDialogLayout.vfunc_allocate` centres both in one column and
    // then moves the prompt down; a top margin of twice the offset does
    // the same.
    let column = gtk::Box::builder()
        .css_classes(["unlock-dialog-column"])
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .margin_top(2 * AUTH_PROMPT_VERTICAL_CENTER_OFFSET)
        .build();
    prompt.root.set_hexpand(true);
    column.append(&prompt.root);

    // Each scaled about its middle, without changing what it takes up
    // (`set_pivot_point(0.5, 0.5)`).
    let clock_page = ScaleBin::new(&clock);
    clock_page.keep_size();
    let prompt_page = ScaleBin::new(&column);
    prompt_page.keep_size();
    let stack = UnlockStack::new(&clock_page, &prompt_page);
    stack.add_css_class("unlock-dialog");
    // Whether the prompt is wanted: it shows, and has the keyboard, from
    // the start of its coming up to the end of its going.
    let wants_prompt = Rc::new(Cell::new(false));
    let progress = Eased::new(&stack, 0.0, {
        let (clock_page, prompt_page) = (clock_page.downgrade(), prompt_page.downgrade());
        let stack = stack.downgrade();
        let wants_prompt = Rc::clone(&wants_prompt);
        // `_setTransitionProgress`
        move |progress| {
            let (Some(clock), Some(prompt), Some(stack)) =
                (clock_page.upgrade(), prompt_page.upgrade(), stack.upgrade())
            else {
                return;
            };
            stack.set_progress(progress);
            let wanted = wants_prompt.get();
            prompt.set_child_visible(progress > 0.0 || wanted);
            clock.set_child_visible(progress < 1.0 || !wanted);
            prompt.set_opacity(progress);
            prompt.set_scale(FADE_OUT_SCALE + (1.0 - FADE_OUT_SCALE) * progress);
            prompt.set_shift_y(FADE_OUT_TRANSLATION * (1.0 - progress));
            clock.set_opacity(1.0 - progress);
            clock.set_scale(FADE_OUT_SCALE + (1.0 - FADE_OUT_SCALE) * (1.0 - progress));
            clock.set_shift_y(-FADE_OUT_TRANSLATION * progress);
        }
    });
    progress.jump(0.0);

    // `NotificationsBox`, at the foot of the screen; the clock moves up
    // rather than be covered by it (`UnlockDialogLayout`).
    let notifications = Rc::new(super::notifications::NotificationsBox::new({
        let lock = Rc::downgrade(lock);
        move |command| {
            if let Some(lock) = lock.upgrade() {
                lock.mpris.send(command);
            }
        }
    }));
    subscriptions.push(lock.notifications.observe({
        let notifications = Rc::clone(&notifications);
        move |listed| notifications.show(listed)
    }));
    subscriptions.push(lock.mpris.state.observe({
        let notifications = Rc::clone(&notifications);
        move |state: &mpris::State| {
            let players: Vec<mpris::Player> = state.shown().cloned().collect();
            notifications.show_players(&players);
        }
    }));
    let overlay = gtk::Overlay::new();
    if let Some(source) = &lock.wallpaper {
        overlay.set_child(Some(&wallpaper::backdrop(
            &window,
            source,
            "screen-shield-background",
        )));
    }
    overlay.add_overlay(&stack);
    overlay.add_overlay(&notifications.widget);
    // `UnlockDialogLayout.vfunc_allocate`: the notifications at the foot,
    // no taller than leaves a tenth of the screen and the clock above
    // them, and the clock centred unless that would put it over them.
    overlay.connect_get_child_position({
        let (stack, notifications) = (stack.clone(), notifications.widget.clone());
        move |overlay, child| {
            let (width, height) = (overlay.width(), overlay.height());
            let (_, stack_height, _, _) = stack.measure(gtk::Orientation::Vertical, width);
            let (notifications_width, notifications_height) = if notifications.is_visible() {
                let (_, natural, _, _) = notifications.measure(gtk::Orientation::Horizontal, -1);
                let natural = natural.min(width);
                let (_, tall, _, _) = notifications.measure(gtk::Orientation::Vertical, natural);
                (
                    natural,
                    tall.min(height - height / 10 - stack_height).max(0),
                )
            } else {
                (0, 0)
            };
            if child == notifications.upcast_ref::<gtk::Widget>() {
                return Some(gdk::Rectangle::new(
                    (width - notifications_width) / 2,
                    height - notifications_height,
                    notifications_width,
                    notifications_height,
                ));
            }
            if child == stack.upcast_ref::<gtk::Widget>() {
                // The stack centres what it shows: making it shorter at the
                // foot moves that up by half as much.
                let centred = (height - stack_height) / 2;
                let highest = height - stack_height - notifications_height;
                let up = (centred - highest).max(0);
                return Some(gdk::Rectangle::new(0, 0, width, height - 2 * up));
            }
            None
        }
    });
    window.set_child(Some(&overlay));

    let inner = Rc::new(Inner {
        lock: Rc::downgrade(lock),
        progress,
        wants_prompt,
        hint,
        prompt,
        hint_timer: RefCell::new(None),
        idle_timer: RefCell::new(None),
        still: lock.still,
        on_prompt: Cell::new(false),
    });

    // `_onKeyPress`: any key brings the prompt up, and a key that types
    // something types it there. Escape goes back to the clock.
    let keys = gtk::EventControllerKey::builder()
        .propagation_phase(gtk::PropagationPhase::Capture)
        .build();
    keys.connect_key_pressed({
        let inner = Rc::clone(&inner);
        move |_, key, _, modifiers| {
            if inner.on_prompt.get() {
                if key == gdk::Key::Escape {
                    inner.show_clock();
                    return glib::Propagation::Stop;
                }
                inner.touch();
                return glib::Propagation::Proceed;
            }
            // Shift on its own is the start of a capital, not a key press
            // to answer.
            if matches!(
                key,
                gdk::Key::Shift_L
                    | gdk::Key::Shift_R
                    | gdk::Key::Control_L
                    | gdk::Key::Control_R
                    | gdk::Key::Alt_L
                    | gdk::Key::Alt_R
                    | gdk::Key::Super_L
                    | gdk::Key::Super_R
                    | gdk::Key::Caps_Lock
                    | gdk::Key::ISO_Level3_Shift
            ) {
                return glib::Propagation::Proceed;
            }
            inner.show_prompt();
            let typed = key.to_unicode().filter(|character| {
                !character.is_control()
                    && !character.is_whitespace()
                    && !modifiers.intersects(
                        gdk::ModifierType::CONTROL_MASK
                            | gdk::ModifierType::ALT_MASK
                            | gdk::ModifierType::SUPER_MASK,
                    )
            });
            if let Some(character) = typed {
                let text = &inner.prompt.entry.text;
                let mut position = -1;
                text.insert_text(&character.to_string(), &mut position);
                text.set_position(-1);
            }
            glib::Propagation::Stop
        }
    });
    window.add_controller(keys);

    // A click anywhere on the clock does the same.
    let click = gtk::GestureClick::new();
    click.connect_released({
        let inner = Rc::clone(&inner);
        move |_, _, _, _| {
            if !inner.on_prompt.get() {
                inner.show_prompt();
            }
        }
    });
    window.add_controller(click);

    inner.show_clock();
    Screen {
        window,
        inner,
        _subscriptions: subscriptions,
    }
}

impl Screen {
    /// Puts the prompt up at once, as a key press would.
    pub fn show_prompt(&self) {
        self.inner.show_prompt();
    }
}
