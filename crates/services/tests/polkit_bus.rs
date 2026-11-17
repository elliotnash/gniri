// SPDX-License-Identifier: GPL-3.0-or-later

//! The polkit agent against a stand-in for polkit's authority (a bare
//! python-dbusmock object) and a stand-in for its helper (a shell script).

mod common;

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::{Follower, MockBus, TIMEOUT};
use futures_lite::future::block_on;
use gniri_services::polkit::backend::{self, Options, PATH};
use gniri_services::polkit::helper::Helper;
use gniri_services::polkit::{self, Command, Notice, try_again};
use gniri_services::user::User;
use zbus::Variant;

const AUTHORITY_NAME: &str = "org.freedesktop.PolicyKit1";
const AUTHORITY_PATH: &str = "/org/freedesktop/PolicyKit1/Authority";
const AUTHORITY: &str = "org.freedesktop.PolicyKit1.Authority";
const AGENT: &str = "org.freedesktop.PolicyKit1.AuthenticationAgent";
const MOCK: &str = "org.freedesktop.DBus.Mock";

/// What polkit's helper does, as far as the agent can tell: it reads the
/// cookie, asks for a password and says whether it was "sesame". A password
/// of "slow" is never answered.
const HELPER: &str = r#"#!/bin/sh
read cookie
echo "$1 $cookie" >> "$0.log"
echo "PAM_PROMPT_ECHO_OFF Password: "
read answer
case "$answer" in
    sesame) echo SUCCESS ;;
    slow) sleep 60 ;;
    *) echo FAILURE ;;
esac
"#;

struct Setup {
    bus: MockBus,
    agent: Follower<polkit::State>,
    /// The bus name of the agent's connection.
    agent_name: String,
    helper: PathBuf,
    user: String,
}

fn setup(test: &str) -> Option<Setup> {
    let mut bus = MockBus::start()?;
    bus.object(AUTHORITY_NAME, AUTHORITY_PATH, AUTHORITY);
    bus.call(
        AUTHORITY_NAME,
        AUTHORITY_PATH,
        MOCK,
        "AddMethod",
        &(
            AUTHORITY,
            "RegisterAuthenticationAgent",
            "(sa{sv})ss",
            "",
            "",
        ),
    );

    let helper = std::env::temp_dir().join(format!("gniri-polkit-{test}-{}", std::process::id()));
    std::fs::write(&helper, HELPER).unwrap();
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
    let _ = std::fs::remove_file(log_of(&helper));

    let user = User::current().name;
    let options = Options {
        session_id: Some("c1".to_owned()),
        user: user.clone(),
        locale: "en_US.UTF-8".to_owned(),
        helper: Helper {
            socket: None,
            program: helper.clone(),
        },
    };
    // The agent's own connection, so the test knows where to call it.
    let connection = bus.connection();
    let agent_name = connection.unique_name().expect("a unique name").to_string();
    let (link, events, commands) = gniri_services::Link::new();
    std::thread::spawn(move || {
        if let Err(error) = block_on(backend::run(connection, events, commands, options)) {
            eprintln!("the agent stopped: {error}");
        }
    });

    let setup = Setup {
        bus,
        agent: Follower::new(link),
        agent_name,
        helper,
        user,
    };
    setup.wait_for_registration();
    Some(setup)
}

fn log_of(helper: &std::path::Path) -> PathBuf {
    PathBuf::from(format!("{}.log", helper.display()))
}

type Call = std::thread::JoinHandle<zbus::Result<zbus::Message>>;

impl Setup {
    fn wait_for_registration(&self) {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let calls: Vec<(u64, Vec<zbus::OwnedVariant>)> = self
                .bus
                .call(
                    AUTHORITY_NAME,
                    AUTHORITY_PATH,
                    MOCK,
                    "GetMethodCalls",
                    &("RegisterAuthenticationAgent"),
                )
                .body()
                .deserialize()
                .expect("the calls so far");
            if let Some((_, arguments)) = calls.first() {
                let subject: (String, HashMap<String, zbus::OwnedVariant>) = arguments[0]
                    .try_clone()
                    .unwrap()
                    .try_into()
                    .expect("a subject");
                assert_eq!(subject.0, "unix-session");
                let session: String = subject.1["session-id"]
                    .try_clone()
                    .unwrap()
                    .try_into()
                    .unwrap();
                assert_eq!(session, "c1");
                let path: String = arguments[2].try_clone().unwrap().try_into().unwrap();
                assert_eq!(path, PATH);
                return;
            }
            assert!(Instant::now() < deadline, "the agent never registered");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Calls `BeginAuthentication` as polkit would, on a thread: the call
    /// only returns when the request is over.
    fn begin(&self, cookie: &str, users: &[u32]) -> Call {
        let connection = self.bus.connection();
        let agent = self.agent_name.clone();
        let cookie = cookie.to_owned();
        let identities: Vec<(String, HashMap<String, zbus::OwnedVariant>)> = users
            .iter()
            .map(|uid| {
                let details =
                    HashMap::from([("uid".to_owned(), Variant::U32(*uid).try_to_owned().unwrap())]);
                ("unix-user".to_owned(), details)
            })
            .collect();
        std::thread::spawn(move || {
            block_on(connection.call_method(
                Some(agent.as_str()),
                PATH,
                Some(AGENT),
                "BeginAuthentication",
                &(
                    "org.example.act",
                    "Authentication is needed to act",
                    "",
                    HashMap::<String, String>::new(),
                    cookie,
                    identities,
                ),
            ))
        })
    }

    fn cancel(&self, cookie: &str) {
        self.bus.call(
            &self.agent_name,
            PATH,
            AGENT,
            "CancelAuthentication",
            &(cookie),
        );
    }

    fn wait_for_prompt(&mut self) {
        self.agent.wait("the prompt", |state| {
            state
                .request
                .as_ref()
                .is_some_and(|request| request.prompt.is_some())
        });
    }
}

impl Drop for Setup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.helper);
        let _ = std::fs::remove_file(log_of(&self.helper));
    }
}

fn own_uid() -> u32 {
    std::os::unix::fs::MetadataExt::uid(&std::fs::metadata("/proc/self").unwrap())
}

fn error_name(result: zbus::Result<zbus::Message>) -> String {
    match result {
        Err(zbus::Error::MethodError(name, _, _)) => name.to_string(),
        other => panic!("expected an error reply, got {other:?}"),
    }
}

#[test]
fn a_wrong_answer_asks_again_and_the_right_one_ends_the_call() {
    let Some(mut setup) = setup("retry") else {
        return;
    };
    let call = setup.begin("cookie-1", &[0, own_uid()]);
    setup.wait_for_prompt();
    let request = setup.agent.state.request.clone().unwrap();
    assert_eq!(request.message, "Authentication is needed to act");
    assert_eq!(request.user, setup.user, "the user before root");
    assert_eq!(request.prompt.unwrap().hint(), "Password");

    setup.agent.send(Command::Respond("wrong".to_owned()));
    setup.agent.wait("the refusal", |state| {
        state
            .request
            .as_ref()
            .is_some_and(|request| request.failures == 1)
    });
    assert_eq!(
        setup.agent.state.request.as_ref().unwrap().notice,
        Notice::Error(try_again())
    );
    assert!(!call.is_finished(), "polkit is still waiting");

    // A new helper has started and asks again.
    setup.wait_for_prompt();
    setup.agent.send(Command::Respond("sesame".to_owned()));
    setup.agent.wait("the end", |state| state.request.is_none());
    call.join()
        .unwrap()
        .expect("the call returns without error");

    // Each helper was given the user and the cookie.
    let log = std::fs::read_to_string(log_of(&setup.helper)).unwrap();
    let expected = format!("{} cookie-1\n", setup.user);
    assert_eq!(log, expected.repeat(2));
}

#[test]
fn dismissing_and_withdrawing_end_the_call_with_an_error() {
    let Some(mut setup) = setup("cancel") else {
        return;
    };
    // The user gives up.
    let call = setup.begin("cookie-1", &[own_uid()]);
    setup.wait_for_prompt();
    setup.agent.send(Command::Cancel);
    assert_eq!(
        error_name(call.join().unwrap()),
        "org.freedesktop.PolicyKit1.Error.Cancelled"
    );
    setup.agent.wait("the end", |state| state.request.is_none());

    // polkit gives up, while the helper is busy with an answer.
    let call = setup.begin("cookie-2", &[own_uid()]);
    setup.wait_for_prompt();
    setup.agent.send(Command::Respond("slow".to_owned()));
    setup.cancel("cookie-2");
    assert_eq!(
        error_name(call.join().unwrap()),
        "org.freedesktop.PolicyKit1.Error.Cancelled"
    );
    setup.agent.wait("the end", |state| state.request.is_none());
}

#[test]
fn requests_wait_their_turn() {
    let Some(mut setup) = setup("queue") else {
        return;
    };
    let first = setup.begin("cookie-1", &[own_uid()]);
    setup.wait_for_prompt();
    let second = setup.begin("cookie-2", &[own_uid()]);
    let third = setup.begin("cookie-3", &[own_uid()]);
    std::thread::sleep(Duration::from_millis(300));

    // The third is withdrawn before its turn.
    setup.cancel("cookie-3");
    assert_eq!(
        error_name(third.join().unwrap()),
        "org.freedesktop.PolicyKit1.Error.Cancelled"
    );

    setup.agent.send(Command::Respond("sesame".to_owned()));
    first.join().unwrap().expect("the first is granted");
    // The dialog closes and opens again for the second.
    setup
        .agent
        .wait("the first to end", |state| state.request.is_none());
    setup.wait_for_prompt();
    setup.agent.send(Command::Respond("sesame".to_owned()));
    second.join().unwrap().expect("the second is granted");
    setup.agent.wait("the end", |state| state.request.is_none());

    let log = std::fs::read_to_string(log_of(&setup.helper)).unwrap();
    assert!(
        log.contains("cookie-1") && log.contains("cookie-2"),
        "{log}"
    );
    assert!(!log.contains("cookie-3"), "{log}");
}

#[test]
fn a_request_nobody_can_answer_fails_at_once() {
    let Some(setup) = setup("nobody") else {
        return;
    };
    // A user id that has no account.
    let call = setup.begin("cookie-1", &[4_000_000_000]);
    assert_eq!(
        error_name(call.join().unwrap()),
        "org.freedesktop.PolicyKit1.Error.Failed"
    );
}
