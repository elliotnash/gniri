//! Modal dialogs: a shade over the whole output with a dialog in its
//! middle. Port of `js/ui/modalDialog.js` and `js/ui/dialog.js`.
//!
//! A dialog has a surface of its own, on the overlay layer, mapped only
//! while the dialog is open. The shade and the dialog are both blurred
//! (docs/DECISIONS.md, M4), and the surface takes the keyboard, as GNOME's
//! modal grab does.

pub mod auth;
pub mod end_session;
pub mod network;
pub mod pairing;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gniri_core::Subscription;
use gniri_services::bluetooth::agent as bluetooth_agent;
use gniri_services::network::agent as network_agent;
use gtk::{gdk, gio, glib};
use gtk4_layer_shell::{KeyboardMode, Layer, LayerShell};

use crate::services::Services;
use crate::surfaces::layer::{self, Anchor, Zone};

/// modalDialog.js `OPEN_AND_CLOSE_TIME`. The theme's transition on
/// `.modal-dialog-lightbox` lasts as long.
const OPEN_AND_CLOSE_TIME: Duration = Duration::from_millis(100);

/// The monitor a dialog opens on: the one called `output` (niri's focused
/// output), or else the first.
pub fn monitor(output: Option<&str>) -> Option<gdk::Monitor> {
    let monitors: Vec<gdk::Monitor> = gdk::Display::default()?
        .monitors()
        .iter::<gdk::Monitor>()
        .filter_map(Result::ok)
        .collect();
    monitors
        .iter()
        .find(|monitor| output.is_some() && monitor.connector().as_deref() == output)
        .or(monitors.first())
        .cloned()
}

/// One open dialog: its surface, and the parts a dialog fills in.
#[derive(Clone)]
pub struct Modal {
    pub window: gtk::ApplicationWindow,
    /// The shade, which fades in and out with the dialog on it.
    lightbox: gtk::Box,
    /// `Dialog.contentLayout`
    pub content: gtk::Box,
    /// `Dialog.buttonLayout`
    buttons: gtk::Box,
    closed: Rc<Cell<bool>>,
}

impl Modal {
    /// Builds a dialog on `monitor`, unmapped. `class` is the kind of
    /// dialog, such as `prompt-dialog`.
    pub fn new(app: &adw::Application, monitor: &gdk::Monitor, class: &str) -> Self {
        let window = layer::window(
            app,
            monitor,
            "dialog",
            Layer::Overlay,
            Anchor::Fill,
            Zone::Ignore,
        );
        window.set_keyboard_mode(KeyboardMode::Exclusive);
        // St shows where the keyboard focus is from the start; GTK would
        // wait for the first key.
        window.set_focus_visible(true);

        // `Dialog._createDialog`
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["modal-dialog-content-box"])
            .vexpand(true)
            .build();
        let buttons = gtk::Box::builder()
            .css_classes(["modal-dialog-button-box"])
            .homogeneous(true)
            .build();
        let dialog = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["modal-dialog", class])
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .hexpand(true)
            .vexpand(true)
            .build();
        dialog.append(&content);
        dialog.append(&buttons);

        let lightbox = gtk::Box::builder()
            .css_classes(["modal-dialog-lightbox"])
            .build();
        lightbox.append(&dialog);
        window.set_child(Some(&lightbox));

        Self {
            window,
            lightbox,
            content,
            buttons,
            closed: Rc::default(),
        }
    }

    /// `Dialog.addButton`
    pub fn add_button(&self, label: &str, action: impl Fn() + 'static) -> gtk::Button {
        let button = gtk::Button::builder()
            .css_classes(["modal-dialog-button"])
            .label(label)
            .hexpand(true)
            .build();
        button.connect_clicked(move |_| action());
        self.buttons.append(&button);
        button
    }

    /// Runs `action` when Escape is pressed, before any widget sees it.
    pub fn on_escape(&self, action: impl Fn() + 'static) {
        let keys = gtk::EventControllerKey::builder()
            .propagation_phase(gtk::PropagationPhase::Capture)
            .build();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                action();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        self.window.add_controller(keys);
    }

    /// Maps the surface and fades the dialog in. `ModalDialog.open`.
    pub fn open(&self) {
        self.window.present();
        // The surface must be on screen in its closed look before the
        // class that starts the transition is added.
        glib::idle_add_local_once({
            let (lightbox, closed) = (self.lightbox.clone(), Rc::clone(&self.closed));
            move || {
                if !closed.get() {
                    lightbox.add_css_class("open");
                }
            }
        });
    }

    /// Fades the dialog out and takes its surface away. `ModalDialog.close`.
    pub fn close(&self) {
        if self.closed.replace(true) {
            return;
        }
        self.lightbox.remove_css_class("open");
        let window = self.window.clone();
        let animated =
            gtk::Settings::default().is_some_and(|settings| settings.is_gtk_enable_animations());
        if animated {
            glib::timeout_add_local_once(OPEN_AND_CLOSE_TIME, move || window.destroy());
        } else {
            window.destroy();
        }
    }
}

/// `Dialog.MessageDialogContent`: a title over a description, both
/// centred and wrapped to the dialog's width.
pub struct MessageContent {
    pub root: gtk::Box,
    pub title: gtk::Label,
    pub description: gtk::Label,
}

/// A label that takes the width it is given and wraps in it, centred.
fn wrapped_label(class: &str) -> gtk::Label {
    gtk::Label::builder()
        .css_classes([class])
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .justify(gtk::Justification::Center)
        .xalign(0.5)
        // Its container's width decides, not its text's.
        .max_width_chars(1)
        .hexpand(true)
        .build()
}

pub fn message_content() -> MessageContent {
    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .css_classes(["message-dialog-content"])
        .hexpand(true)
        .build();
    let title = wrapped_label("message-dialog-title");
    let description = wrapped_label("message-dialog-description");
    root.append(&title);
    root.append(&description);
    MessageContent {
        root,
        title,
        description,
    }
}

/// A dialog that is open, and where.
struct Shown<D> {
    monitor: gdk::Monitor,
    dialog: D,
}

struct Dialogs {
    app: adw::Application,
    services: Rc<Services>,
    end_session: RefCell<Option<Shown<end_session::Dialog>>>,
    network: RefCell<Option<Shown<network::Dialog>>>,
    pairing: RefCell<Option<Shown<pairing::Dialog>>>,
}

impl Dialogs {
    /// The monitor of the output the user is looking at, unless the
    /// session is locked: nothing asks from under the lock screen.
    fn monitor(&self) -> Option<gdk::Monitor> {
        if self.services.session.state.with(|session| session.locked) {
            return None;
        }
        let output = self.services.niri.with(|niri| niri.focused_output.clone());
        monitor(output.as_deref())
    }

    /// Opens the end-session dialog, or with `None` closes it.
    fn show_end_session(&self, kind: Option<end_session::Kind>) {
        if let Some(shown) = self.end_session.take() {
            shown.dialog.close();
        }
        let Some(kind) = kind else { return };
        match self.monitor() {
            Some(monitor) => {
                let dialog = end_session::open(&self.app, &monitor, &self.services, kind);
                self.end_session.replace(Some(Shown { monitor, dialog }));
            }
            None => self.services.end_session.update(|open| *open = None),
        }
    }

    /// Opens the dialog for what NetworkManager's agent asks, or with
    /// `None` closes it.
    fn show_network(&self, request: Option<&network_agent::Request>) {
        if let Some(shown) = self.network.take() {
            shown.dialog.close();
        }
        let Some(request) = request else { return };
        match self.monitor() {
            Some(monitor) => {
                let dialog = network::open(&self.app, &monitor, &self.services, request);
                self.network.replace(Some(Shown { monitor, dialog }));
            }
            // No screen to ask on: the answer is that nobody answered.
            None => self
                .services
                .network_agent
                .send(network_agent::Command::Cancel),
        }
    }

    /// The same for the Bluetooth pairing agent.
    fn show_pairing(&self, request: Option<&bluetooth_agent::Request>) {
        if let Some(shown) = self.pairing.take() {
            shown.dialog.close();
        }
        let Some(request) = request else { return };
        match self.monitor() {
            Some(monitor) => {
                let dialog = pairing::open(&self.app, &monitor, &self.services, request);
                self.pairing.replace(Some(Shown { monitor, dialog }));
            }
            None => self
                .services
                .bluetooth_agent
                .send(bluetooth_agent::Command::Reject),
        }
    }

    /// A dialog whose output was unplugged opens again where the user is
    /// now. (The end-session dialog's minute starts over.)
    fn monitors_changed(&self, monitors: &gio::ListModel) {
        let present: Vec<gdk::Monitor> = monitors
            .iter::<gdk::Monitor>()
            .filter_map(Result::ok)
            .collect();
        let gone = |monitor: &gdk::Monitor| !present.contains(monitor);
        if self
            .end_session
            .borrow()
            .as_ref()
            .is_some_and(|shown| gone(&shown.monitor))
        {
            self.show_end_session(self.services.end_session.get());
        }
        if self
            .network
            .borrow()
            .as_ref()
            .is_some_and(|shown| gone(&shown.monitor))
        {
            let request = self.services.network_agent.state.get().request;
            self.show_network(request.as_ref());
        }
        if self
            .pairing
            .borrow()
            .as_ref()
            .is_some_and(|shown| gone(&shown.monitor))
        {
            let request = self.services.bluetooth_agent.state.get().request;
            self.show_pairing(request.as_ref());
        }
    }
}

/// Shows the shell's own dialogs when their state says so. Dropping it
/// stops that.
pub struct Host {
    _subscriptions: Vec<Subscription>,
    monitors: Option<(gio::ListModel, glib::SignalHandlerId)>,
}

impl Drop for Host {
    fn drop(&mut self) {
        if let Some((monitors, handler)) = self.monitors.take() {
            monitors.disconnect(handler);
        }
    }
}

/// Opens each dialog while its state asks for it, on the output the user
/// is looking at: the end-session dialog while `Services::end_session` is
/// set, and the two agents' dialogs while they have a request.
pub fn host(app: &adw::Application, services: &Rc<Services>) -> Host {
    let dialogs = Rc::new(Dialogs {
        app: app.clone(),
        services: Rc::clone(services),
        end_session: RefCell::new(None),
        network: RefCell::new(None),
        pairing: RefCell::new(None),
    });
    let subscriptions = vec![
        services.end_session.select(|kind| *kind, {
            let dialogs = Rc::clone(&dialogs);
            move |kind| dialogs.show_end_session(*kind)
        }),
        services
            .network_agent
            .state
            .select(|state| state.request.clone(), {
                let dialogs = Rc::clone(&dialogs);
                move |request: &Option<network_agent::Request>| {
                    dialogs.show_network(request.as_ref())
                }
            }),
        services
            .bluetooth_agent
            .state
            .select(|state| state.request.clone(), {
                let dialogs = Rc::clone(&dialogs);
                move |request: &Option<bluetooth_agent::Request>| {
                    dialogs.show_pairing(request.as_ref())
                }
            }),
    ];
    let monitors = gdk::Display::default().map(|display| {
        let monitors = display.monitors();
        let handler = monitors.connect_items_changed({
            let dialogs = Rc::downgrade(&dialogs);
            move |monitors, _, _, _| {
                if let Some(dialogs) = dialogs.upgrade() {
                    dialogs.monitors_changed(monitors);
                }
            }
        });
        (monitors, handler)
    });
    Host {
        _subscriptions: subscriptions,
        monitors,
    }
}
