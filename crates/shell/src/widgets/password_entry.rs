//! An entry for a password, with the eye that shows what was typed. Port of
//! `St.PasswordEntry` and of `CapsLockWarning` in `js/ui/shellEntry.js`.

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};

/// What St hides each character behind.
const PASSWORD_CHAR: char = '\u{25cf}';

#[derive(Clone)]
pub struct PasswordEntry {
    /// `.st-entry`: the frame, with the text and the eye in it.
    pub widget: gtk::Box,
    pub text: gtk::Text,
    peek: gtk::Image,
    /// The text is shown as typed.
    shown: Rc<Cell<bool>>,
}

impl PasswordEntry {
    /// `class` is the style class of this entry's place, such as
    /// `prompt-dialog-password-entry`.
    pub fn new(class: &str) -> Self {
        let text = gtk::Text::builder()
            .visibility(false)
            .invisible_char(u32::from(PASSWORD_CHAR))
            .input_purpose(gtk::InputPurpose::Password)
            // No emoji picker, no remembering: this is a password, and on
            // the lock screen nothing may open beside the prompt.
            .input_hints(gtk::InputHints::NO_EMOJI | gtk::InputHints::PRIVATE)
            .enable_emoji_completion(false)
            .hexpand(true)
            .valign(gtk::Align::Center)
            // The frame's width is the theme's alone to decide.
            .width_chars(1)
            .max_width_chars(1)
            .build();
        let peek = gtk::Image::builder()
            .icon_name("view-reveal-symbolic")
            .css_classes(["peek-password"])
            .valign(gtk::Align::Center)
            .build();
        let widget = gtk::Box::builder()
            .css_classes(["st-entry", class])
            .halign(gtk::Align::Center)
            .build();
        widget.append(&text);
        widget.append(&peek);

        let entry = Self {
            widget,
            text,
            peek,
            shown: Rc::default(),
        };

        // A click on the eye shows or hides the text.
        let click = gtk::GestureClick::new();
        click.connect_released({
            let entry = entry.clone();
            move |_, _, _, _| entry.set_shown(!entry.shown.get())
        });
        entry.peek.add_controller(click);

        // A click anywhere in the frame puts the cursor in the text.
        let focus = gtk::GestureClick::new();
        focus.connect_pressed({
            let text = entry.text.clone();
            move |_, _, _, _| {
                text.grab_focus_without_selecting();
            }
        });
        entry.widget.add_controller(focus);
        entry
    }

    /// The same frame for a text that is no secret: shown as typed, and
    /// without the eye. Port of `St.Entry`, beside the password entries of
    /// a dialog.
    pub fn plain(class: &str) -> Self {
        let entry = Self::new(class);
        entry.set_shown(true);
        entry.peek.set_visible(false);
        entry.text.set_input_purpose(gtk::InputPurpose::FreeForm);
        entry
    }

    /// Shows a text that cannot be changed or focused: St's entry that is
    /// not `reactive`.
    pub fn set_fixed(&self, text: &str) {
        self.text.set_text(text);
        self.text.set_editable(false);
        self.text.set_can_focus(false);
        self.widget.set_can_target(false);
    }

    /// Shows the text as typed, or as dots. `StPasswordEntry`'s
    /// `password-visible`.
    pub fn set_shown(&self, shown: bool) {
        self.shown.set(shown);
        self.text.set_visibility(shown);
        self.peek.set_icon_name(Some(if shown {
            "view-conceal-symbolic"
        } else {
            "view-reveal-symbolic"
        }));
    }

    pub fn set_hint(&self, hint: &str) {
        self.text.set_placeholder_text(Some(hint));
    }

    pub fn password(&self) -> String {
        self.text.text().to_string()
    }

    pub fn clear(&self) {
        self.text.set_text("");
    }

    pub fn grab_focus(&self) {
        self.text.grab_focus_without_selecting();
    }

    /// Shakes the entry: the answer was wrong. `wiggle` in
    /// `js/misc/animationUtils.js`.
    pub fn wiggle(&self) {
        wiggle(&self.widget);
    }
}

/// Shakes `widget` from side to side once.
pub fn wiggle(widget: &impl IsA<gtk::Widget>) {
    // The theme's animation, which lasts 455ms.
    const WIGGLE: std::time::Duration = std::time::Duration::from_millis(455);
    let widget = widget.as_ref().clone();
    widget.remove_css_class("wiggle");
    widget.add_css_class("wiggle");
    glib::timeout_add_local_once(WIGGLE, move || widget.remove_css_class("wiggle"));
}

/// The line that says Caps Lock is on, shown only while it is.
pub fn caps_lock_warning() -> gtk::Label {
    let label = gtk::Label::builder()
        .label("Caps lock is on")
        .css_classes(["caps-lock-warning-label"])
        .visible(false)
        .build();
    let keyboard = gdk::Display::default()
        .and_then(|display| display.default_seat())
        .and_then(|seat| seat.keyboard());
    if let Some(keyboard) = keyboard {
        label.set_visible(keyboard.is_caps_locked());
        let handler = keyboard.connect_caps_lock_state_notify({
            let label = label.downgrade();
            move |keyboard| {
                if let Some(label) = label.upgrade() {
                    label.set_visible(keyboard.is_caps_locked());
                }
            }
        });
        // The keyboard outlives the label; stop telling it.
        let handler = Cell::new(Some(handler));
        label.connect_destroy(move |_| {
            if let Some(handler) = handler.take() {
                keyboard.disconnect(handler);
            }
        });
    }
    label
}
