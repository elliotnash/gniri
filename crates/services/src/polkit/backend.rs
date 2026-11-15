//! The agent on the system bus: registered with polkit for this session,
//! and serving `org.freedesktop.PolicyKit1.AuthenticationAgent`.
//!
//! polkit calls `BeginAuthentication` and waits for the call to return:
//! normally once the helper has reported success, with an error if the user
//! gave up. Requests are answered one at a time, in the order they came.

use std::collections::{HashMap, VecDeque};

use async_channel::{Receiver, Sender};
use futures_lite::StreamExt;
use zbus::{OwnedVariant, Variant};

use super::helper::{Helper, Line, Session};
use super::{Command, Event, Request, choose_identity};
use crate::bus::{Wakes, merge};
use crate::logind;
use crate::user;

/// Where GNOME Shell serves its agent too.
pub const PATH: &str = "/org/freedesktop/PolicyKit1/AuthenticationAgent";

pub struct Options {
    /// This session's id (`XDG_SESSION_ID`); asked of logind if unset.
    pub session_id: Option<String>,
    /// Login name of the user gniri runs as.
    pub user: String,
    /// The language polkit words its messages in, such as `en_US.UTF-8`.
    pub locale: String,
    pub helper: Helper,
}

impl Options {
    pub fn system() -> Self {
        let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
            .unwrap_or_else(|| "C".to_owned());
        Self {
            session_id: logind::session_id_from_env(),
            user: user::User::current().name,
            locale,
            helper: Helper::system(),
        }
    }
}

#[zbus::proxy(
    interface = "org.freedesktop.PolicyKit1.Authority",
    default_service = "org.freedesktop.PolicyKit1",
    default_path = "/org/freedesktop/PolicyKit1/Authority"
)]
trait Authority {
    /// `subject` is a kind and its details: `("unix-session",
    /// {"session-id": id})`.
    fn register_authentication_agent(
        &self,
        subject: &(&str, HashMap<&str, Variant<'_>>),
        locale: &str,
        object_path: &str,
    ) -> zbus::Result<()>;
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.PolicyKit1.Error")]
enum AgentError {
    #[zbus(error)]
    ZBus(zbus::Error),
    /// The user dismissed the dialog.
    Cancelled(String),
    Failed(String),
}

/// One `BeginAuthentication` call.
struct Begin {
    action_id: String,
    message: String,
    cookie: String,
    /// Login names of the users polkit would accept.
    identities: Vec<String>,
    /// Answered when the request is over.
    reply: Sender<Result<(), AgentError>>,
}

enum Call {
    Begin(Begin),
    /// polkit withdrew the request with this cookie.
    Cancel(String),
}

/// The object polkit talks to. It only passes calls on to [`run`].
struct Agent {
    calls: Sender<Call>,
}

/// The login names among polkit's identities: `("unix-user", {"uid": n})`.
/// Groups are not something a password can be asked for.
fn user_names(identities: &[(String, HashMap<String, OwnedVariant>)]) -> Vec<String> {
    identities
        .iter()
        .filter(|(kind, _)| kind == "unix-user")
        .filter_map(|(_, details)| u32::try_from(details.get("uid")?).ok())
        .filter_map(user::login_name)
        .collect()
}

#[zbus::interface(name = "org.freedesktop.PolicyKit1.AuthenticationAgent")]
impl Agent {
    async fn begin_authentication(
        &self,
        action_id: String,
        message: String,
        _icon_name: String,
        _details: HashMap<String, String>,
        cookie: String,
        identities: Vec<(String, HashMap<String, OwnedVariant>)>,
    ) -> Result<(), AgentError> {
        let gone = || AgentError::Failed("the agent is shutting down".to_owned());
        let (reply, result) = async_channel::bounded(1);
        let begin = Begin {
            action_id,
            message,
            cookie,
            identities: user_names(&identities),
            reply,
        };
        self.calls
            .send(Call::Begin(begin))
            .await
            .map_err(|_| gone())?;
        result.recv().await.map_err(|_| gone())?
    }

    async fn cancel_authentication(&self, cookie: String) {
        let _ = self.calls.send(Call::Cancel(cookie)).await;
    }
}

enum Wake {
    Call(Call),
    Command(Command),
    /// From the helper with this number.
    Helper(u64, Line),
}

/// The request being answered.
struct Active {
    begin: Begin,
    user: String,
    /// `None` between a helper ending and the next starting.
    session: Option<Session>,
}

struct Runner {
    options: Options,
    events: Sender<Event>,
    lines: Sender<(u64, Line)>,
    active: Option<Active>,
    waiting: VecDeque<Begin>,
    /// Numbers the helpers, so that a line from one that was replaced is
    /// not taken for its successor's.
    helper: u64,
}

impl Runner {
    async fn tell(&self, event: Event) {
        let _ = self.events.send(event).await;
    }

    /// Starts a helper for the active request. False if none would start.
    fn start_helper(&mut self) -> bool {
        let Some(active) = &mut self.active else {
            return false;
        };
        self.helper += 1;
        // The old one first: two helpers must never hold the same cookie.
        active.session = None;
        match self.options.helper.start(
            &active.user,
            &active.begin.cookie,
            self.lines.clone(),
            self.helper,
        ) {
            Ok(session) => {
                active.session = Some(session);
                true
            }
            Err(error) => {
                eprintln!("gniri: polkit: cannot start polkit's helper: {error}");
                false
            }
        }
    }

    /// Makes `begin` the active request.
    async fn begin(&mut self, begin: Begin) {
        let Some(user) = choose_identity(&self.options.user, &begin.identities) else {
            let _ = begin
                .reply
                .send(Err(AgentError::Failed(
                    "no user to authenticate as".to_owned(),
                )))
                .await;
            return;
        };
        let user = user.to_owned();
        self.tell(Event::Begin(Request {
            action_id: begin.action_id.clone(),
            message: begin.message.clone(),
            user: user.clone(),
            ..Request::default()
        }))
        .await;
        self.active = Some(Active {
            begin,
            user,
            session: None,
        });
        if !self.start_helper() {
            self.end(Err(AgentError::Failed(
                "polkit's helper did not start".to_owned(),
            )))
            .await;
        }
    }

    /// Ends the active request with `result` and takes up the next.
    async fn end(&mut self, result: Result<(), AgentError>) {
        if let Some(active) = self.active.take() {
            // Dropping the session stops a helper that is still waiting.
            let _ = active.begin.reply.send(result).await;
            self.tell(Event::End).await;
        }
        while self.active.is_none() {
            match self.waiting.pop_front() {
                Some(next) => Box::pin(self.begin(next)).await,
                None => break,
            }
        }
    }

    async fn handle(&mut self, wake: Wake) {
        match wake {
            Wake::Call(Call::Begin(begin)) => {
                if self.active.is_some() {
                    self.waiting.push_back(begin);
                } else {
                    self.begin(begin).await;
                }
            }
            Wake::Call(Call::Cancel(cookie)) => {
                let withdrawn = || AgentError::Cancelled("polkit withdrew the request".to_owned());
                if self
                    .active
                    .as_ref()
                    .is_some_and(|active| active.begin.cookie == cookie)
                {
                    self.end(Err(withdrawn())).await;
                } else if let Some(index) =
                    self.waiting.iter().position(|begin| begin.cookie == cookie)
                    && let Some(begin) = self.waiting.remove(index)
                {
                    let _ = begin.reply.send(Err(withdrawn())).await;
                }
            }
            Wake::Command(Command::Cancel) => {
                self.end(Err(AgentError::Cancelled(
                    "the user dismissed the dialog".to_owned(),
                )))
                .await;
            }
            Wake::Command(Command::Respond(answer)) => {
                let sent = self
                    .active
                    .as_mut()
                    .and_then(|active| active.session.as_mut())
                    .map(|session| session.respond(&answer));
                if let Some(Err(error)) = sent {
                    eprintln!("gniri: polkit: cannot answer polkit's helper: {error}");
                }
            }
            Wake::Helper(number, line) => {
                if number != self.helper || self.active.is_none() {
                    return;
                }
                match line {
                    Line::Prompt(prompt) => self.tell(Event::Prompt(prompt)).await,
                    Line::Info(text) => self.tell(Event::Info(text)).await,
                    Line::Error(text) => self.tell(Event::Error(text)).await,
                    // The helper has told polkit itself.
                    Line::Success => self.end(Ok(())).await,
                    // `_onSessionCompleted`: say so and ask again.
                    Line::Failure => {
                        self.tell(Event::Denied).await;
                        if !self.start_helper() {
                            self.end(Err(AgentError::Failed(
                                "polkit's helper did not start".to_owned(),
                            )))
                            .await;
                        }
                    }
                    // It went away without a verdict; another would too.
                    Line::Closed => {
                        self.end(Err(AgentError::Failed(
                            "polkit's helper stopped without an answer".to_owned(),
                        )))
                        .await;
                    }
                }
            }
        }
    }
}

pub async fn run(
    connection: zbus::Connection,
    events: Sender<Event>,
    commands: Receiver<Command>,
    options: Options,
) -> zbus::Result<()> {
    let (calls, incoming) = async_channel::unbounded();
    connection.object_server().at(PATH, Agent { calls }).await?;

    let session_id = match &options.session_id {
        Some(id) => id.clone(),
        None => logind::session(&connection, None).await?.id().await?,
    };
    let subject = (
        "unix-session",
        HashMap::from([("session-id", Variant::from(session_id.as_str()))]),
    );
    AuthorityProxy::new(&connection)
        .await?
        .register_authentication_agent(&subject, &options.locale, PATH)
        .await?;

    let (lines, helper_lines) = async_channel::unbounded();
    let mut wakes = merge(vec![
        Box::pin(incoming.map(Wake::Call)) as Wakes<Wake>,
        Box::pin(commands.map(Wake::Command)),
        Box::pin(helper_lines.map(|(number, line)| Wake::Helper(number, line))),
    ]);
    let mut runner = Runner {
        options,
        events,
        lines,
        active: None,
        waiting: VecDeque::new(),
        helper: 0,
    };
    while let Some(wake) = wakes.next().await {
        // The dialog closing its end means the agent is shutting down.
        if runner.events.is_closed() {
            break;
        }
        runner.handle(wake).await;
    }
    Ok(())
}
