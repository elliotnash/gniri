// SPDX-License-Identifier: GPL-3.0-or-later

//! The pairing agent: when a Bluetooth device is paired, or one that is not
//! trusted connects, BlueZ asks the session's agent what the user says: to
//! type a PIN, to compare two numbers, to allow it at all.
//!
//! GNOME Shell has no such agent. GNOME Settings brings one while its
//! Bluetooth panel is open (the gnome-bluetooth library), and nothing
//! answers BlueZ the rest of the time. gniri's is there all the time, and
//! asks with gnome-bluetooth's words (`lib/bluetooth-pairing-dialog.c`).
//! [`backend`] is the object on the bus.

pub mod backend;

use serde::Deserialize;

use crate::model::Model;

/// What BlueZ wants to know, by the method of `org.bluez.Agent1` it calls.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// `RequestPinCode`: the PIN the device has, or shows.
    EnterPin,
    /// `RequestPasskey`: the same, as a number of up to six digits.
    EnterPasskey,
    /// `DisplayPinCode` and `DisplayPasskey`: a PIN to type on the device.
    ShowPin(String),
    /// `RequestConfirmation`: the number the device shows too, if all is
    /// well.
    Confirm(String),
    /// `RequestAuthorization`: a device wants to pair, with nothing to
    /// compare.
    Pair,
    /// `AuthorizeService`: a device that is not paired wants to connect.
    Connect,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// The name the user knows the device by.
    pub device: String,
    /// The device is a keyboard: a PIN typed on it ends with Return.
    #[serde(default)]
    pub keyboard: bool,
    pub kind: Kind,
}

/// A passkey as BlueZ's own tools show it: six digits.
pub fn passkey_text(passkey: u32) -> String {
    format!("{passkey:06}")
}

impl Request {
    /// The dialog's title. The texts are gnome-bluetooth's
    /// (`bluetooth_pairing_dialog_set_mode`).
    pub fn title(&self) -> String {
        match self.kind {
            Kind::EnterPin | Kind::EnterPasskey | Kind::Confirm(_) => {
                "Confirm Bluetooth PIN".to_owned()
            }
            Kind::ShowPin(_) => format!("Pairing “{}”", self.device),
            Kind::Pair => "Bluetooth Pairing Request".to_owned(),
            Kind::Connect => "Confirm Bluetooth Connection".to_owned(),
        }
    }

    pub fn description(&self) -> String {
        let device = &self.device;
        match self.kind {
            Kind::EnterPin | Kind::EnterPasskey => {
                format!("Please confirm the PIN that was entered on “{device}”.")
            }
            Kind::ShowPin(_) if self.keyboard => format!(
                "Please enter the following PIN on “{device}”. \
                 Then press “Return” on the keyboard."
            ),
            Kind::ShowPin(_) => format!("Please enter the following PIN on “{device}”."),
            Kind::Confirm(_) => format!(
                "Please confirm that the following PIN matches the one displayed on “{device}”."
            ),
            Kind::Pair => {
                format!("“{device}” wants to pair with this device. Do you want to allow pairing?")
            }
            Kind::Connect => {
                format!("“{device}” wants to connect with this device. Do you want to allow it?")
            }
        }
    }

    /// The PIN the dialog shows, large.
    pub fn pin(&self) -> Option<&str> {
        match &self.kind {
            Kind::ShowPin(pin) | Kind::Confirm(pin) => Some(pin),
            _ => None,
        }
    }

    /// The user types the answer.
    pub fn asks_for_text(&self) -> bool {
        matches!(self.kind, Kind::EnterPin | Kind::EnterPasskey)
    }

    /// The button that turns the request down, which Escape presses too.
    pub fn reject_label(&self) -> &'static str {
        match self.kind {
            Kind::Pair | Kind::Connect => "Dismiss",
            _ => "Cancel",
        }
    }

    /// The button that goes ahead. A PIN that is only shown has none: the
    /// dialog goes when the device is paired.
    pub fn accept_label(&self) -> Option<&'static str> {
        match self.kind {
            Kind::EnterPin | Kind::EnterPasskey | Kind::Confirm(_) => Some("Confirm"),
            Kind::Pair | Kind::Connect => Some("Allow"),
            Kind::ShowPin(_) => None,
        }
    }

    /// Whether `text` is something BlueZ takes for an answer: a PIN is 1
    /// to 16 characters, a passkey a number below a million.
    pub fn accepts(&self, text: &str) -> bool {
        match self.kind {
            Kind::EnterPin => {
                (1..=16).contains(&text.len()) && text.chars().all(|c| c.is_ascii_alphanumeric())
            }
            Kind::EnterPasskey => {
                (1..=6).contains(&text.len()) && text.chars().all(|c| c.is_ascii_digit())
            }
            _ => false,
        }
    }

    /// What a command comes to for this request: what BlueZ is answered.
    /// `None` for a command that does not fit the request.
    pub fn answer(&self, command: &Command) -> Option<Answer> {
        match (command, &self.kind) {
            (Command::Reject, _) => Some(Answer::Rejected),
            (Command::Respond(text), Kind::EnterPin) if self.accepts(text) => {
                Some(Answer::Pin(text.clone()))
            }
            (Command::Respond(text), Kind::EnterPasskey) if self.accepts(text) => {
                text.parse().ok().map(Answer::Passkey)
            }
            (Command::Accept, Kind::Confirm(_) | Kind::Pair | Kind::Connect) => {
                Some(Answer::Accepted)
            }
            _ => None,
        }
    }
}

/// What BlueZ hears back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    Accepted,
    Pin(String),
    Passkey(u32),
    Rejected,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct State {
    /// The question being asked. BlueZ asks one at a time.
    pub request: Option<Request>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Begin(Request),
    /// The question is over: answered, withdrawn, or the pairing it was
    /// part of has ended.
    End,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// "Confirm" or "Allow".
    Accept,
    /// The PIN or passkey the user typed.
    Respond(String),
    /// "Cancel" or "Dismiss".
    Reject,
}

impl Model for State {
    type Event = Event;
    type Command = Command;

    fn apply(&mut self, event: Event) {
        match event {
            Event::Begin(request) => self.request = Some(request),
            Event::End => self.request = None,
        }
    }

    /// An answer that fits closes the dialog at once.
    fn predict(&mut self, command: &Command) {
        if self
            .request
            .as_ref()
            .is_some_and(|request| request.answer(command).is_some())
        {
            self.request = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(kind: Kind) -> Request {
        Request {
            device: "Keys".to_owned(),
            keyboard: false,
            kind,
        }
    }

    #[test]
    fn the_texts_are_gnome_bluetooth_s() {
        let confirm = request(Kind::Confirm("012345".to_owned()));
        assert_eq!(confirm.title(), "Confirm Bluetooth PIN");
        assert_eq!(
            confirm.description(),
            "Please confirm that the following PIN matches the one displayed on “Keys”."
        );
        assert_eq!(confirm.pin(), Some("012345"));
        assert_eq!(
            (confirm.reject_label(), confirm.accept_label()),
            ("Cancel", Some("Confirm"))
        );

        let enter = request(Kind::EnterPin);
        assert_eq!(enter.title(), "Confirm Bluetooth PIN");
        assert_eq!(
            enter.description(),
            "Please confirm the PIN that was entered on “Keys”."
        );
        assert!(enter.asks_for_text() && enter.pin().is_none());

        let mut show = request(Kind::ShowPin("4821".to_owned()));
        assert_eq!(show.title(), "Pairing “Keys”");
        assert_eq!(
            show.description(),
            "Please enter the following PIN on “Keys”."
        );
        show.keyboard = true;
        assert_eq!(
            show.description(),
            "Please enter the following PIN on “Keys”. Then press “Return” on the keyboard."
        );
        assert_eq!((show.reject_label(), show.accept_label()), ("Cancel", None));

        let pair = request(Kind::Pair);
        assert_eq!(pair.title(), "Bluetooth Pairing Request");
        assert_eq!(
            pair.description(),
            "“Keys” wants to pair with this device. Do you want to allow pairing?"
        );
        assert_eq!(
            (pair.reject_label(), pair.accept_label()),
            ("Dismiss", Some("Allow"))
        );

        let connect = request(Kind::Connect);
        assert_eq!(connect.title(), "Confirm Bluetooth Connection");
        assert_eq!(
            connect.description(),
            "“Keys” wants to connect with this device. Do you want to allow it?"
        );
    }

    #[test]
    fn a_passkey_is_shown_with_six_digits() {
        assert_eq!(passkey_text(42), "000042");
        assert_eq!(passkey_text(999_999), "999999");
    }

    #[test]
    fn only_what_bluez_takes_is_an_answer() {
        let respond = |text: &str| Command::Respond(text.to_owned());
        let pin = request(Kind::EnterPin);
        assert_eq!(
            pin.answer(&respond("0000")),
            Some(Answer::Pin("0000".to_owned()))
        );
        assert_eq!(
            pin.answer(&respond("abc123")),
            Some(Answer::Pin("abc123".to_owned()))
        );
        assert_eq!(pin.answer(&respond("")), None);
        assert_eq!(pin.answer(&respond("12 34")), None);
        assert_eq!(pin.answer(&respond(&"1".repeat(17))), None);
        // A question that wants a text is not answered with a nod.
        assert_eq!(pin.answer(&Command::Accept), None);
        assert_eq!(pin.answer(&Command::Reject), Some(Answer::Rejected));

        let passkey = request(Kind::EnterPasskey);
        assert_eq!(
            passkey.answer(&respond("004821")),
            Some(Answer::Passkey(4821))
        );
        assert_eq!(passkey.answer(&respond("1234567")), None);
        assert_eq!(passkey.answer(&respond("12ab")), None);

        for kind in [Kind::Confirm("1".to_owned()), Kind::Pair, Kind::Connect] {
            let request = request(kind);
            assert_eq!(request.answer(&Command::Accept), Some(Answer::Accepted));
            assert_eq!(request.answer(&respond("1")), None);
        }
        // A PIN that is only shown can only be given up on.
        let show = request(Kind::ShowPin("1".to_owned()));
        assert_eq!(show.answer(&Command::Accept), None);
        assert_eq!(show.answer(&Command::Reject), Some(Answer::Rejected));
    }

    #[test]
    fn a_fitting_answer_closes_the_request() {
        let mut state = State::default();
        state.predict(&Command::Accept);
        assert_eq!(state, State::default());

        state.apply(Event::Begin(request(Kind::EnterPin)));
        state.predict(&Command::Accept);
        assert!(state.request.is_some());
        state.predict(&Command::Respond("1234".to_owned()));
        assert_eq!(state, State::default());

        state.apply(Event::Begin(request(Kind::Pair)));
        state.apply(Event::End);
        assert_eq!(state, State::default());
    }

    #[test]
    fn a_scene_can_describe_a_request() {
        let state: State = toml::from_str(
            r#"
            [request]
            device = "Keys"
            keyboard = true
            kind = { show-pin = "482193" }
            "#,
        )
        .unwrap();
        let request = state.request.unwrap();
        assert_eq!(request.pin(), Some("482193"));
        let state: State =
            toml::from_str("request = { device = \"Phone\", kind = \"pair\" }").unwrap();
        assert_eq!(state.request.unwrap().kind, Kind::Pair);
    }
}
