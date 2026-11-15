//! Checking that the person at the keyboard is the user: PAM.
//!
//! PAM blocks while it works (a wrong password costs two seconds on a
//! stock Fedora), so everything here is called from a thread of its own.
//!
//! The lock screen runs two checks side by side, as GDM does: the password
//! through the `gniri` service, and, where it is set up, a fingerprint
//! through `gniri-fingerprint`. Whichever succeeds first unlocks.
//! `data/pam/` holds both service files.
//!
//! Side by side, and so through two services: a system's ordinary login
//! stack may ask for a finger first and only look at the password once the
//! reader has given up waiting. Until gniri's own files are installed, the
//! services the distribution has for exactly this split are used where
//! they exist.

use std::ffi::{OsStr, OsString};
use std::path::Path;

use nonstick::{AuthnFlags, ConversationAdapter, ErrorCode, Transaction, TransactionBuilder};

/// The PAM services of the password check, best first: gniri's own, the
/// password-only stack of Fedora and its relatives (what GDM's password
/// prompt uses there), and the one every system has.
const PASSWORD_SERVICES: [&str; 3] = ["gniri", "password-auth", "login"];
/// The same for the fingerprint check. There is no general fallback:
/// without one of these no finger is asked for.
const FINGERPRINT_SERVICES: [&str; 2] = ["gniri-fingerprint", "fingerprint-auth"];

/// Something a PAM module wants the user to read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Info(String),
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Granted,
    /// Wrong password, unknown finger, or too many tries.
    Denied,
    /// The check could not be made at all: no such service, no fingerprint
    /// reader, no enrolled finger. Trying again will not help.
    Unavailable(String),
}

/// Checks a password. The lock screen's only way to unlock by typing.
pub trait Authenticator: Send + Sync + 'static {
    /// Blocks until PAM has decided. `tell` receives what the modules
    /// say meanwhile.
    fn check(&self, password: &str, tell: &dyn Fn(Message)) -> Outcome;
}

/// True if `service` has a file in one of the directories PAM reads.
fn service_exists(service: &str) -> bool {
    ["/etc/pam.d", "/usr/lib/pam.d", "/usr/share/pam.d"]
        .iter()
        .any(|directory| Path::new(directory).join(service).is_file())
}

/// The first of `services` that is installed. (A service PAM does not
/// know falls through to `other`, which denies everyone.)
fn installed(services: &[&'static str]) -> Option<&'static str> {
    services
        .iter()
        .copied()
        .find(|service| service_exists(service))
}

/// The real thing.
pub struct Pam {
    service: &'static str,
    user: String,
}

impl Pam {
    /// The password check for `user`.
    pub fn password(user: &str) -> Self {
        let service = installed(&PASSWORD_SERVICES).unwrap_or(PASSWORD_SERVICES[0]);
        if service != PASSWORD_SERVICES[0] {
            eprintln!(
                "gniri: /etc/pam.d/{} is not installed; checking passwords as the \
                 \"{service}\" service does",
                PASSWORD_SERVICES[0]
            );
        }
        Self {
            service,
            user: user.to_owned(),
        }
    }
}

struct Conversation<'a> {
    /// What a hidden prompt is answered with. `None` answers none.
    password: Option<&'a str>,
    tell: &'a dyn Fn(Message),
}

impl ConversationAdapter for Conversation<'_> {
    /// A visible prompt would be for a user name or a one-time code;
    /// neither is asked for on the lock screen.
    fn prompt(&self, _request: impl AsRef<OsStr>) -> nonstick::Result<OsString> {
        Err(ErrorCode::ConversationError)
    }

    fn masked_prompt(&self, _request: impl AsRef<OsStr>) -> nonstick::Result<OsString> {
        self.password
            .map(OsString::from)
            .ok_or(ErrorCode::ConversationError)
    }

    fn error_msg(&self, message: impl AsRef<OsStr>) {
        (self.tell)(Message::Error(
            message.as_ref().to_string_lossy().into_owned(),
        ));
    }

    fn info_msg(&self, message: impl AsRef<OsStr>) {
        (self.tell)(Message::Info(
            message.as_ref().to_string_lossy().into_owned(),
        ));
    }
}

fn run(service: &str, user: &str, password: Option<&str>, tell: &dyn Fn(Message)) -> Outcome {
    let conversation = Conversation { password, tell };
    let transaction = TransactionBuilder::new_with_service(service)
        .username(user)
        .build(conversation.into_conversation());
    let mut transaction = match transaction {
        Ok(transaction) => transaction,
        Err(error) => return Outcome::Unavailable(format!("PAM would not start: {error}")),
    };
    match transaction.authenticate(AuthnFlags::empty()) {
        Ok(()) => Outcome::Granted,
        Err(
            error @ (ErrorCode::AuthInfoUnavailable
            | ErrorCode::ServiceError
            | ErrorCode::SystemError
            | ErrorCode::OpenError
            | ErrorCode::SymbolError),
        ) => Outcome::Unavailable(error.to_string()),
        Err(_) => Outcome::Denied,
    }
}

impl Authenticator for Pam {
    fn check(&self, password: &str, tell: &dyn Fn(Message)) -> Outcome {
        run(self.service, &self.user, Some(password), tell)
    }
}

/// Whether a fingerprint check is set up: a PAM service for it is
/// installed. Whether there is a reader and an enrolled finger only shows
/// when [`fingerprint`] is tried.
pub fn fingerprint_configured() -> bool {
    installed(&FINGERPRINT_SERVICES).is_some()
}

/// Waits for a finger on the reader. Blocks until one was read, the
/// module gave up waiting, or it found nothing to read with.
pub fn fingerprint(user: &str, tell: &dyn Fn(Message)) -> Outcome {
    match installed(&FINGERPRINT_SERVICES) {
        Some(service) => run(service, user, None, tell),
        None => Outcome::Unavailable("no PAM service for fingerprints".to_owned()),
    }
}

/// A stand-in that knows one password, for tests of the lock screen. It
/// never touches PAM.
pub struct Fake {
    pub password: String,
}

impl Authenticator for Fake {
    fn check(&self, password: &str, _tell: &dyn Fn(Message)) -> Outcome {
        if password == self.password {
            Outcome::Granted
        } else {
            Outcome::Denied
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stand_in_knows_one_password() {
        let fake = Fake {
            password: "open sesame".to_owned(),
        };
        assert_eq!(fake.check("open sesame", &|_| {}), Outcome::Granted);
        assert_eq!(fake.check("", &|_| {}), Outcome::Denied);
        assert_eq!(fake.check("open sesame ", &|_| {}), Outcome::Denied);
    }

    #[test]
    fn the_first_installed_service_is_used() {
        assert!(!service_exists("gniri-no-such-service"));
        assert_eq!(installed(&["gniri-no-such-service"]), None);
        // Every system with PAM has this one.
        if service_exists("login") {
            assert_eq!(
                installed(&["gniri-no-such-service", "login", "other"]),
                Some("login")
            );
        }
    }
}
