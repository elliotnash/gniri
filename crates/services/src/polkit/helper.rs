//! polkit's helper: the privileged program that runs PAM on the agent's
//! behalf and reports the result to polkit itself.
//!
//! It is reached in one of two ways, tried in the order libpolkit-agent
//! tries them: through a socket systemd listens on (polkit 126 and later,
//! where the distribution enables it), or by running the setuid program.
//! Either way the conversation is lines of text: the helper writes
//! `PAM_PROMPT_ECHO_OFF Password: ` and the like, the agent writes one line
//! per answer, and the helper ends with `SUCCESS` or `FAILURE`.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use async_channel::Sender;

use super::Prompt;

/// One line from the helper.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    Prompt(Prompt),
    Info(String),
    Error(String),
    Success,
    Failure,
    /// The helper's output ended.
    Closed,
}

/// Undoes GLib's `g_strescape`, which the helper passes PAM's text
/// through: `\n`, `\t`, `\"`, `\\` and octal `\303`.
fn unescape(text: &str) -> String {
    let mut bytes = Vec::with_capacity(text.len());
    let mut input = text.bytes().peekable();
    while let Some(byte) = input.next() {
        if byte != b'\\' {
            bytes.push(byte);
            continue;
        }
        match input.next() {
            Some(b'n') => bytes.push(b'\n'),
            Some(b't') => bytes.push(b'\t'),
            Some(b'r') => bytes.push(b'\r'),
            Some(b'b') => bytes.push(8),
            Some(b'f') => bytes.push(12),
            Some(b'v') => bytes.push(11),
            Some(digit @ b'0'..=b'7') => {
                let mut value = u32::from(digit - b'0');
                for _ in 0..2 {
                    match input.peek() {
                        Some(next @ b'0'..=b'7') => {
                            value = value * 8 + u32::from(next - b'0');
                            input.next();
                        }
                        _ => break,
                    }
                }
                bytes.push(value as u8);
            }
            Some(other) => bytes.push(other),
            None => {}
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Reads one line of the helper's output. `None` for a line that means
/// nothing to the agent.
pub fn parse(line: &str) -> Option<Line> {
    let line = line.trim_end_matches(['\n', '\r']);
    let prompt = |text: &str, echo| {
        Line::Prompt(Prompt {
            text: unescape(text),
            echo,
        })
    };
    if let Some(text) = line.strip_prefix("PAM_PROMPT_ECHO_OFF ") {
        Some(prompt(text, false))
    } else if let Some(text) = line.strip_prefix("PAM_PROMPT_ECHO_ON ") {
        Some(prompt(text, true))
    } else if let Some(text) = line.strip_prefix("PAM_ERROR_MSG ") {
        Some(Line::Error(unescape(text)))
    } else if let Some(text) = line.strip_prefix("PAM_TEXT_INFO ") {
        Some(Line::Info(unescape(text)))
    } else {
        match line {
            "SUCCESS" => Some(Line::Success),
            "FAILURE" => Some(Line::Failure),
            _ => None,
        }
    }
}

/// Where the helper is.
#[derive(Clone, Debug)]
pub struct Helper {
    /// The socket of the systemd-activated helper, tried first.
    pub socket: Option<PathBuf>,
    /// The setuid program.
    pub program: PathBuf,
}

impl Helper {
    /// The helper polkit installed: the socket at its fixed place, and
    /// the program wherever this distribution puts it.
    pub fn system() -> Self {
        let program = [
            "/usr/lib/polkit-1/polkit-agent-helper-1",
            "/usr/libexec/polkit-agent-helper-1",
            "/usr/lib/policykit-1/polkit-agent-helper-1",
            "/usr/libexec/polkit-1/polkit-agent-helper-1",
        ]
        .iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("/usr/lib/polkit-1/polkit-agent-helper-1"));
        Self {
            socket: Some(PathBuf::from("/run/polkit/agent-helper.socket")),
            program,
        }
    }

    /// Starts a helper that authenticates `user` for the request polkit
    /// calls `cookie`. Every line it writes arrives on `lines`, tagged
    /// with `tag`, and [`Line::Closed`] after the last.
    pub fn start<T: Clone + Send + 'static>(
        &self,
        user: &str,
        cookie: &str,
        lines: Sender<(T, Line)>,
        tag: T,
    ) -> std::io::Result<Session> {
        let connected = self
            .socket
            .as_ref()
            .and_then(|socket| UnixStream::connect(socket).ok());
        let (input, output, process): (Box<dyn Write + Send>, Box<dyn Read + Send>, _) =
            match connected {
                Some(stream) => {
                    // The socket's helper cannot be told the user on a
                    // command line; it reads the name first.
                    let mut input = stream.try_clone()?;
                    writeln!(input, "{user}")?;
                    let closer = stream.try_clone()?;
                    (Box::new(input), Box::new(stream), Process::Socket(closer))
                }
                None => {
                    let mut child = Command::new(&self.program)
                        .arg(user)
                        .stdin(Stdio::piped())
                        .stdout(Stdio::piped())
                        .spawn()?;
                    let input = child.stdin.take().expect("stdin is piped");
                    let output = child.stdout.take().expect("stdout is piped");
                    (Box::new(input), Box::new(output), Process::Child(child))
                }
            };
        let mut session = Session { input, process };
        // On its input, not its command line, where other users' programs
        // could read it.
        session.respond(cookie)?;

        std::thread::Builder::new()
            .name("polkit-helper".to_owned())
            .spawn(move || {
                for line in BufReader::new(output).lines() {
                    let Ok(line) = line else { break };
                    if let Some(line) = parse(&line)
                        && lines.send_blocking((tag.clone(), line)).is_err()
                    {
                        return;
                    }
                }
                let _ = lines.send_blocking((tag, Line::Closed));
            })?;
        Ok(session)
    }
}

enum Process {
    Child(Child),
    Socket(UnixStream),
}

/// A running helper.
pub struct Session {
    input: Box<dyn Write + Send>,
    process: Process,
}

impl Session {
    /// Answers the helper's prompt.
    pub fn respond(&mut self, answer: &str) -> std::io::Result<()> {
        self.input.write_all(answer.as_bytes())?;
        self.input.write_all(b"\n")?;
        self.input.flush()
    }
}

/// Dropping a session stops its helper, whatever it was waiting for.
impl Drop for Session {
    fn drop(&mut self) {
        match &mut self.process {
            Process::Child(child) => {
                let _ = child.kill();
                let _ = child.wait();
            }
            Process::Socket(stream) => {
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_helper_s_lines() {
        assert_eq!(
            parse("PAM_PROMPT_ECHO_OFF Password: \n"),
            Some(Line::Prompt(Prompt {
                text: "Password: ".to_owned(),
                echo: false
            }))
        );
        assert_eq!(
            parse("PAM_PROMPT_ECHO_ON Code:"),
            Some(Line::Prompt(Prompt {
                text: "Code:".to_owned(),
                echo: true
            }))
        );
        assert_eq!(
            parse("PAM_TEXT_INFO Place your finger on the reader"),
            Some(Line::Info("Place your finger on the reader".to_owned()))
        );
        assert_eq!(
            parse("PAM_ERROR_MSG Two\\nlines, a \\\"quote\\\" and \\303\\251"),
            Some(Line::Error("Two\nlines, a \"quote\" and é".to_owned()))
        );
        assert_eq!(parse("SUCCESS"), Some(Line::Success));
        assert_eq!(parse("FAILURE\n"), Some(Line::Failure));
        assert_eq!(parse("SUCCESSFUL"), None);
        assert_eq!(parse(""), None);
    }

    /// A stand-in helper: checks its arguments and the cookie, asks for a
    /// password and judges it.
    const SCRIPT: &str = r#"
        [ "$1" = ada ] || exit 9
        read -r cookie
        [ "$cookie" = c00kie ] || exit 9
        echo "PAM_PROMPT_ECHO_OFF Password: "
        read -r answer
        if [ "$answer" = 'pass word\' ]; then echo SUCCESS; else echo FAILURE; fi
    "#;

    fn script() -> (PathBuf, Helper) {
        let path = std::env::temp_dir().join(format!(
            "gniri-helper-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&path, format!("#!/bin/sh\n{SCRIPT}")).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o700);
        std::fs::set_permissions(&path, permissions).unwrap();
        let helper = Helper {
            socket: None,
            program: path.clone(),
        };
        (path, helper)
    }

    #[test]
    fn talks_to_a_helper_program() {
        let (path, helper) = script();
        let (sender, lines) = async_channel::unbounded();

        let mut session = helper.start("ada", "c00kie", sender.clone(), 1).unwrap();
        let (_, first) = lines.recv_blocking().unwrap();
        assert!(matches!(first, Line::Prompt(_)), "{first:?}");
        // Sent as typed: a backslash is not an escape on the way in.
        session.respond("pass word\\").unwrap();
        assert_eq!(lines.recv_blocking().unwrap(), (1, Line::Success));
        assert_eq!(lines.recv_blocking().unwrap(), (1, Line::Closed));

        let mut session = helper.start("ada", "c00kie", sender.clone(), 2).unwrap();
        lines.recv_blocking().unwrap();
        session.respond("wrong").unwrap();
        assert_eq!(lines.recv_blocking().unwrap(), (2, Line::Failure));
        assert_eq!(lines.recv_blocking().unwrap(), (2, Line::Closed));

        // A helper that is waiting goes away with its session.
        let session = helper.start("ada", "c00kie", sender, 3).unwrap();
        lines.recv_blocking().unwrap();
        drop(session);
        assert_eq!(lines.recv_blocking().unwrap(), (3, Line::Closed));

        std::fs::remove_file(path).unwrap();
    }
}
