//! The polkit authentication agent: when a program asks polkit for a
//! privilege the user must authenticate for (`pkexec`, mounting a disk,
//! installing software), polkit asks the session's agent to do the asking.
//! Port of `js/ui/components/polkitAgent.js` and, for the part GNOME Shell
//! has in C, of libpolkit-agent's session.
//!
//! The agent never sees whether a password is right. It starts polkit's own
//! helper, a privileged program that runs PAM for the chosen user, passes
//! the helper's questions to the dialog and the answers back, and the helper
//! tells polkit the result itself.

pub mod backend;
pub mod helper;

use serde::Deserialize;

use crate::model::Model;

/// What `AuthenticationDialog._onSessionCompleted` says after a failed try,
/// unless the helper said something more specific.
pub const TRY_AGAIN: &str = "Sorry, that didn’t work. Please try again.";

/// What the helper wants answered.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prompt {
    /// As PAM words it, usually "Password: ".
    pub text: String,
    /// Show what is typed.
    #[serde(default)]
    pub echo: bool,
}

impl Prompt {
    /// The entry's hint. Port of the "hack" in `_onSessionRequest`: PAM's
    /// "Password:" becomes the dialog's own word, anything else loses its
    /// colon.
    pub fn hint(&self) -> &str {
        match self.text.as_str() {
            "Password:" | "Password: " => "Password",
            other => other.trim_end_matches(' ').trim_end_matches(':').trim(),
        }
    }
}

/// The line under the entry.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Notice {
    /// Nothing to say; the line keeps its room.
    #[default]
    None,
    Info(String),
    Error(String),
}

/// One request to authenticate, as the dialog shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Request {
    /// The privilege asked for, such as `org.freedesktop.policykit.exec`.
    pub action_id: String,
    /// polkit's sentence about it, in the user's language.
    pub message: String,
    /// Login name of the user whose password is asked for.
    pub user: String,
    /// `None` until the helper asks, and again while an answer is checked.
    pub prompt: Option<Prompt>,
    pub notice: Notice,
    /// How many answers were wrong. The dialog shakes the entry when this
    /// rises.
    pub failures: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct State {
    /// The request being answered. Others wait their turn.
    pub request: Option<Request>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Begin(Request),
    Prompt(Prompt),
    Info(String),
    Error(String),
    /// The helper turned the answer down; another helper is starting.
    Denied,
    /// The request is over: granted, cancelled or withdrawn.
    End,
}

#[derive(Clone, PartialEq, Eq)]
pub enum Command {
    /// The answer to the prompt.
    Respond(String),
    Cancel,
}

/// Never prints the answer: commands are logged.
impl std::fmt::Debug for Command {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Respond(_) => formatter.write_str("Respond(…)"),
            Self::Cancel => formatter.write_str("Cancel"),
        }
    }
}

impl Model for State {
    type Event = Event;
    type Command = Command;

    fn apply(&mut self, event: Event) {
        match (event, &mut self.request) {
            (Event::Begin(request), _) => self.request = Some(request),
            (Event::End, _) => self.request = None,
            (Event::Prompt(prompt), Some(request)) => request.prompt = Some(prompt),
            (Event::Info(text), Some(request)) => request.notice = Notice::Info(text),
            (Event::Error(text), Some(request)) => request.notice = Notice::Error(text),
            (Event::Denied, Some(request)) => {
                request.failures += 1;
                request.prompt = None;
                // The helper's own words, if it had any, say more.
                if !matches!(request.notice, Notice::Error(_)) {
                    request.notice = Notice::Error(TRY_AGAIN.to_owned());
                }
            }
            (_, None) => {}
        }
    }

    fn predict(&mut self, command: &Command) {
        match command {
            // `_onEntryActivate`: the entry waits, the messages go.
            Command::Respond(_) => {
                if let Some(request) = &mut self.request {
                    request.prompt = None;
                    request.notice = Notice::None;
                }
            }
            Command::Cancel => self.request = None,
        }
    }
}

/// Whose password to ask for. Port of the choice in
/// `AuthenticationDialog._init`: the user themselves if polkit allows it,
/// else root, else whoever comes first.
pub fn choose_identity<'a>(current_user: &str, allowed: &'a [String]) -> Option<&'a str> {
    allowed
        .iter()
        .find(|name| *name == current_user)
        .or_else(|| allowed.iter().find(|name| *name == "root"))
        .or(allowed.first())
        .map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn the_user_is_asked_before_root_before_anyone() {
        assert_eq!(
            choose_identity("ada", &names(&["root", "ada"])),
            Some("ada")
        );
        assert_eq!(
            choose_identity("ada", &names(&["bob", "root"])),
            Some("root")
        );
        assert_eq!(choose_identity("ada", &names(&["bob", "eve"])), Some("bob"));
        assert_eq!(choose_identity("ada", &[]), None);
    }

    #[test]
    fn the_hint_is_the_prompt_without_its_colon() {
        let hint = |text: &str| {
            Prompt {
                text: text.to_owned(),
                echo: false,
            }
            .hint()
            .to_owned()
        };
        assert_eq!(hint("Password: "), "Password");
        assert_eq!(hint("Password:"), "Password");
        assert_eq!(hint("Verification code:  "), "Verification code");
        assert_eq!(hint(" PIN "), "PIN");
    }

    #[test]
    fn a_request_runs_its_course() {
        let mut state = State::default();
        // Nothing to apply these to yet.
        state.apply(Event::Denied);
        assert_eq!(state, State::default());

        state.apply(Event::Begin(Request {
            action_id: "org.example.act".to_owned(),
            message: "Authentication is needed".to_owned(),
            user: "ada".to_owned(),
            ..Request::default()
        }));
        let prompt = Prompt {
            text: "Password: ".to_owned(),
            echo: false,
        };
        state.apply(Event::Prompt(prompt.clone()));
        assert_eq!(state.request.as_ref().unwrap().prompt, Some(prompt.clone()));

        state.predict(&Command::Respond("wrong".to_owned()));
        assert_eq!(state.request.as_ref().unwrap().prompt, None);
        state.apply(Event::Denied);
        let request = state.request.as_ref().unwrap();
        assert_eq!(request.failures, 1);
        assert_eq!(request.notice, Notice::Error(TRY_AGAIN.to_owned()));

        // The next answer clears the message; a message of the helper's
        // own is kept over the stock one.
        state.apply(Event::Prompt(prompt));
        state.predict(&Command::Respond("wrong again".to_owned()));
        assert_eq!(state.request.as_ref().unwrap().notice, Notice::None);
        state.apply(Event::Error("Account locked".to_owned()));
        state.apply(Event::Denied);
        let request = state.request.as_ref().unwrap();
        assert_eq!(request.failures, 2);
        assert_eq!(request.notice, Notice::Error("Account locked".to_owned()));

        state.apply(Event::End);
        assert_eq!(state, State::default());
    }

    #[test]
    fn an_answer_is_never_printed() {
        let printed = format!("{:?}", Command::Respond("hunter2".to_owned()));
        assert!(!printed.contains("hunter2"), "{printed}");
    }
}
