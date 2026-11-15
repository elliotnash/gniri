//! One message: a notification or a media player's card. Port of
//! `MessageHeader` and `Message` in `js/ui/messageList.js`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gniri_services::notifications::{Action, Image, markup};
use gtk::prelude::*;
use gtk::{gdk, glib, pango};

use crate::widgets::picture;

/// messageList.js `DEFAULT_EXPAND_LINES`.
const EXPAND_LINES: i32 = 6;
/// messageList.js `MAX_NOTIFICATION_BUTTONS`.
const MAX_NOTIFICATION_BUTTONS: usize = 3;

type Callback = RefCell<Option<Rc<dyn Fn()>>>;
type ActionCallback = Rc<dyn Fn(&str)>;

#[derive(Default)]
struct Handlers {
    clicked: Callback,
    close: Callback,
    /// Called before the message expands; returning false keeps it closed
    /// (a closed group opens instead).
    may_expand: RefCell<Option<Rc<dyn Fn() -> bool>>>,
    action: RefCell<Option<ActionCallback>>,
}

fn call(callback: &Callback) {
    // Cloned out first: the callback may replace itself.
    let callback = callback.borrow().clone();
    if let Some(callback) = callback {
        callback();
    }
}

pub struct Inner {
    root: gtk::Box,
    source_icon: gtk::Image,
    source_title: gtk::Label,
    time: gtk::Label,
    expand_button: gtk::Button,
    close_button: gtk::Button,
    icon: gtk::Image,
    title: gtk::Label,
    body: gtk::Label,
    media_controls: gtk::Box,
    action_bin: gtk::Box,
    expanded: Cell<bool>,
    actions: RefCell<Vec<Action>>,
    handlers: Rc<Handlers>,
}

/// The widgets of one message. Dropping it lets them go; nothing a widget
/// holds refers back to it.
pub struct Message(Rc<Inner>);

impl std::ops::Deref for Message {
    type Target = Inner;

    fn deref(&self) -> &Inner {
        &self.0
    }
}

/// A label that takes the width it is given instead of asking for the width
/// of its text: a long title must not widen the menu.
fn text_label(class: &str) -> gtk::Label {
    gtk::Label::builder()
        .css_classes([class])
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(pango::EllipsizeMode::End)
        .max_width_chars(1)
        .build()
}

fn header_button(class: &str, icon_name: &str) -> gtk::Button {
    gtk::Button::builder()
        .css_classes([class, "icon-button"])
        .valign(gtk::Align::Center)
        .child(&gtk::Image::from_icon_name(icon_name))
        .build()
}

impl Message {
    pub fn new() -> Self {
        // `MessageHeader`
        let source_icon = gtk::Image::builder()
            .css_classes(["message-source-icon"])
            .valign(gtk::Align::Center)
            .build();
        let source_title = gtk::Label::builder()
            .css_classes(["message-source-title"])
            .valign(gtk::Align::End)
            .ellipsize(pango::EllipsizeMode::End)
            .build();
        let time = gtk::Label::builder()
            .css_classes(["event-time"])
            .valign(gtk::Align::End)
            .halign(gtk::Align::Start)
            .hexpand(true)
            .visible(false)
            .build();
        let header_content = gtk::Box::builder()
            .css_classes(["message-header-content"])
            .valign(gtk::Align::Center)
            .hexpand(true)
            .build();
        header_content.append(&source_title);
        header_content.append(&time);
        let expand_button = header_button("message-expand-button", "notification-expand-symbolic");
        let close_button = header_button("message-close-button", "window-close-symbolic");
        let header = gtk::Box::builder()
            .css_classes(["message-header"])
            .hexpand(true)
            .build();
        header.append(&source_icon);
        header.append(&header_content);
        header.append(&expand_button);
        header.append(&close_button);

        // `Message`
        // The image is its own frame: it centres what it shows in the
        // size the theme gives it, and cuts a picture to its corners.
        let icon = gtk::Image::builder()
            .css_classes(["message-icon"])
            .valign(gtk::Align::Start)
            .overflow(gtk::Overflow::Hidden)
            .visible(false)
            .build();

        let title = text_label("message-title");
        title.set_valign(gtk::Align::End);
        let body = text_label("message-body");
        body.set_wrap(true);
        body.set_wrap_mode(pango::WrapMode::WordChar);
        body.set_lines(1);
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["message-content"])
            .hexpand(true)
            .build();
        content.append(&title);
        content.append(&body);

        let media_controls = gtk::Box::builder()
            .css_classes(["message-media-controls"])
            .build();
        let message_box = gtk::Box::builder().css_classes(["message-box"]).build();
        message_box.append(&icon);
        message_box.append(&content);
        message_box.append(&media_controls);

        let action_bin = gtk::Box::builder()
            .css_classes(["message-action-bin"])
            .homogeneous(true)
            .visible(false)
            .build();

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["message"])
            .hexpand(true)
            .focusable(true)
            .build();
        root.append(&header);
        root.append(&message_box);
        root.append(&action_bin);

        let message = Self(Rc::new(Inner {
            root,
            source_icon,
            source_title,
            time,
            expand_button,
            close_button,
            icon,
            title,
            body,
            media_controls,
            action_bin,
            expanded: Cell::default(),
            actions: RefCell::default(),
            handlers: Rc::default(),
        }));
        message.connect_input();
        message.update_expand_button();
        message
    }

    fn connect_input(&self) {
        // The message is a button, but one with buttons inside, which a
        // `GtkButton` cannot hold. A click that one of them takes never
        // gets here.
        let click = gtk::GestureClick::builder().button(1).build();
        click.connect_pressed(|click, _, _, _| {
            if let Some(root) = click.widget() {
                root.set_state_flags(gtk::StateFlags::ACTIVE, false);
            }
        });
        click.connect_released({
            let handlers = Rc::clone(&self.handlers);
            move |click, _, x, y| {
                let Some(root) = click.widget() else {
                    return;
                };
                root.unset_state_flags(gtk::StateFlags::ACTIVE);
                if root.contains(x, y) {
                    call(&handlers.clicked);
                }
            }
        });
        click.connect_cancel(|click, _| {
            if let Some(root) = click.widget() {
                root.unset_state_flags(gtk::StateFlags::ACTIVE);
            }
        });
        self.root.add_controller(click);

        // Return activates; BackSpace and Delete close, as in GNOME.
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed({
            let handlers = Rc::clone(&self.handlers);
            move |keys, key, _, _| {
                // Only for the message itself, not for a button in it.
                if !keys.widget().is_some_and(|root| root.has_focus()) {
                    return glib::Propagation::Proceed;
                }
                match key {
                    gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::space => {
                        call(&handlers.clicked);
                    }
                    gdk::Key::BackSpace | gdk::Key::Delete => call(&handlers.close),
                    _ => return glib::Propagation::Proceed,
                }
                glib::Propagation::Stop
            }
        });
        self.root.add_controller(keys);

        self.close_button.connect_clicked({
            let handlers = Rc::clone(&self.handlers);
            move |_| call(&handlers.close)
        });
        self.expand_button.connect_clicked({
            let inner = Rc::downgrade(&self.0);
            move |_| {
                let Some(message) = inner.upgrade().map(Message) else {
                    return;
                };
                if message.expanded.get() {
                    message.unexpand();
                } else {
                    let may_expand = message.handlers.may_expand.borrow().clone();
                    if may_expand.is_none_or(|may_expand| may_expand()) {
                        message.expand();
                    }
                }
            }
        });
        // Whether the body is cut off is only known once it is laid out.
        self.body.connect_map({
            let inner = Rc::downgrade(&self.0);
            move |_| {
                if let Some(message) = inner.upgrade().map(Message) {
                    message.update_expand_button_later();
                }
            }
        });
    }

    /// `.message`
    pub fn root(&self) -> &gtk::Box {
        &self.root
    }

    /// What a click on the message does.
    pub fn connect_clicked(&self, clicked: impl Fn() + 'static) {
        self.handlers.clicked.replace(Some(Rc::new(clicked)));
    }

    /// What its close button does.
    pub fn connect_close(&self, close: impl Fn() + 'static) {
        self.handlers.close.replace(Some(Rc::new(close)));
    }

    /// Asked before the expand button opens the message.
    pub fn connect_may_expand(&self, may_expand: impl Fn() -> bool + 'static) {
        self.handlers.may_expand.replace(Some(Rc::new(may_expand)));
    }

    /// What one of its action buttons does, given the action's key.
    pub fn connect_action(&self, action: impl Fn(&str) + 'static) {
        self.handlers.action.replace(Some(Rc::new(action)));
    }

    /// The application's name and icon in the header.
    pub fn set_source(&self, title: &str, icon: Option<&Image>) {
        self.source_title.set_label(title);
        picture::show(&self.source_icon, icon, "application-x-executable-symbolic");
    }

    /// The age beside the application's name; `None` for a message without
    /// one.
    pub fn set_time(&self, text: Option<&str>) {
        self.time.set_visible(text.is_some());
        self.time.set_label(text.unwrap_or_default());
    }

    pub fn set_title(&self, text: &str) {
        self.title.set_markup(&markup::title(text));
    }

    pub fn set_body(&self, text: &str) {
        self.body.set_markup(&markup::body(text));
        self.body.set_visible(!text.is_empty());
        self.update_expand_button_later();
    }

    /// The picture beside the text, if any.
    pub fn set_icon(&self, picture: Option<&Image>) {
        self.icon.set_visible(picture.is_some());
        if picture.is_some() {
            let symbolic = picture::show(&self.icon, picture, "image-missing-symbolic");
            if symbolic {
                self.icon.add_css_class("message-themed-icon");
            } else {
                self.icon.remove_css_class("message-themed-icon");
            }
        }
    }

    /// The buttons under the text, shown while the message is expanded
    /// (`NotificationMessage._addAction`).
    pub fn set_actions(&self, actions: &[Action]) {
        if *self.actions.borrow() == actions {
            return;
        }
        self.actions.replace(actions.to_vec());
        while let Some(child) = self.action_bin.first_child() {
            self.action_bin.remove(&child);
        }
        for action in actions.iter().take(MAX_NOTIFICATION_BUTTONS) {
            let button = gtk::Button::builder()
                .css_classes(["notification-button"])
                .hexpand(true)
                .child(
                    &gtk::Label::builder()
                        .label(&action.label)
                        .ellipsize(pango::EllipsizeMode::End)
                        .build(),
                )
                .build();
            button.connect_clicked({
                let (handlers, key) = (Rc::clone(&self.handlers), action.key.clone());
                move |_| {
                    let action = handlers.action.borrow().clone();
                    if let Some(action) = action {
                        action(&key);
                    }
                }
            });
            self.action_bin.append(&button);
        }
        self.action_bin
            .set_visible(self.expanded.get() && !actions.is_empty());
        self.update_expand_button();
    }

    /// `Message.addMediaControl`
    pub fn add_media_control(&self, icon_name: &str, clicked: impl Fn() + 'static) -> gtk::Button {
        let button = gtk::Button::builder()
            .css_classes(["message-media-control"])
            .child(&gtk::Image::from_icon_name(icon_name))
            .build();
        button.connect_clicked(move |_| clicked());
        self.media_controls.append(&button);
        button
    }

    pub fn set_close_visible(&self, visible: bool) {
        self.close_button.set_visible(visible);
    }

    /// A banner has no expand button: it opens under the pointer.
    pub fn set_expand_button_visible(&self, visible: bool) {
        self.expand_button.set_visible(visible);
    }

    pub fn is_expanded(&self) -> bool {
        self.expanded.get()
    }

    /// Shows the whole body, up to six lines, and the action buttons.
    /// `Message.expand`, without the animation.
    pub fn expand(&self) {
        if self.expanded.replace(true) {
            return;
        }
        self.body.set_lines(EXPAND_LINES);
        self.action_bin
            .set_visible(!self.actions.borrow().is_empty());
        self.expand_button.add_css_class("expanded");
        self.update_expand_button();
    }

    pub fn unexpand(&self) {
        if !self.expanded.replace(false) {
            return;
        }
        self.body.set_lines(1);
        self.action_bin.set_visible(false);
        self.expand_button.remove_css_class("expanded");
        self.update_expand_button_later();
    }

    /// `Message._updateExpandButton`: the button shows only when there is
    /// more to see. It keeps its place either way.
    fn update_expand_button(&self) {
        let can_expand = self.expanded.get()
            || !self.actions.borrow().is_empty()
            || self.body.layout().is_ellipsized();
        self.expand_button
            .set_opacity(if can_expand { 1.0 } else { 0.0 });
        self.expand_button.set_can_target(can_expand);
    }

    fn update_expand_button_later(&self) {
        let inner = Rc::downgrade(&self.0);
        // After the layout pass that follows the change.
        glib::idle_add_local_full(glib::Priority::LOW, move || {
            if let Some(message) = inner.upgrade().map(Message) {
                message.update_expand_button();
            }
            glib::ControlFlow::Break
        });
    }
}
