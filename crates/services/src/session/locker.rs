// SPDX-License-Identifier: GPL-3.0-or-later

//! The screen locker as the session backend sees it: something that can be
//! started and then says when it has locked the screen and when it is gone.
//!
//! The real one is `gniri lock`, a process of its own, so that nothing that
//! goes wrong in the shell can take the lock screen down with it. It prints
//! `locked` on its standard output once the compositor has confirmed the
//! lock, and its exit status says how it ended.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use async_channel::Sender;

/// The line the locker prints once the screen is covered.
pub const LOCKED_LINE: &str = "locked";
/// Exit status of a locker the compositor would not give the lock: another
/// locker has it, or the compositor has no session lock.
pub const REFUSED_STATUS: i32 = 3;

/// How a locker ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    /// The user unlocked.
    Unlocked,
    /// The compositor would not let it lock.
    Refused,
    /// It crashed or was killed. If it had locked, the compositor keeps
    /// the screen locked and blank until a new locker takes over.
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Report {
    /// The compositor confirmed the lock: nothing of the session shows.
    Locked,
    Gone(Exit),
}

pub trait Locker: Send + 'static {
    /// Starts a locker, which reports on `reports` until it is gone.
    fn start(&self, reports: Sender<Report>) -> std::io::Result<()>;
}

/// A locker that is a program.
pub struct Process {
    pub program: PathBuf,
    pub arguments: Vec<String>,
}

impl Process {
    /// This program again, as `gniri lock`, with `arguments` before the
    /// subcommand.
    pub fn this_program(mut arguments: Vec<String>) -> Self {
        arguments.push("lock".to_owned());
        Self {
            // If the kernel will not say, whatever `gniri` is on PATH.
            program: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("gniri")),
            arguments,
        }
    }
}

/// What an exit status says ([`REFUSED_STATUS`]).
pub fn exit(status: Option<i32>) -> Exit {
    match status {
        Some(0) => Exit::Unlocked,
        Some(REFUSED_STATUS) => Exit::Refused,
        _ => Exit::Failed,
    }
}

impl Locker for Process {
    fn start(&self, reports: Sender<Report>) -> std::io::Result<()> {
        let mut child = Command::new(&self.program)
            .args(&self.arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()?;
        let output = child.stdout.take().expect("stdout is piped");
        std::thread::Builder::new()
            .name("locker".to_owned())
            .spawn(move || {
                for line in BufReader::new(output).lines() {
                    match line {
                        Ok(line) if line.trim() == LOCKED_LINE => {
                            let _ = reports.send_blocking(Report::Locked);
                        }
                        Ok(_) => {}
                        Err(_) => break,
                    }
                }
                // Its output closed: it is ending or has ended.
                let status = child.wait().ok().and_then(|status| status.code());
                let _ = reports.send_blocking(Report::Gone(exit(status)));
            })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(script: &str) -> Vec<Report> {
        let (sender, receiver) = async_channel::unbounded();
        Process {
            program: "sh".into(),
            arguments: vec!["-c".to_owned(), script.to_owned()],
        }
        .start(sender)
        .unwrap();
        let mut reports = Vec::new();
        while let Ok(report) = receiver.recv_blocking() {
            reports.push(report);
        }
        reports
    }

    #[test]
    fn a_locker_reports_the_lock_and_how_it_ended() {
        assert_eq!(
            run("echo starting; echo locked; exit 0"),
            [Report::Locked, Report::Gone(Exit::Unlocked)]
        );
        assert_eq!(run("exit 3"), [Report::Gone(Exit::Refused)]);
        assert_eq!(
            run("echo locked; kill -KILL $$"),
            [Report::Locked, Report::Gone(Exit::Failed)]
        );
        assert_eq!(run("exit 1"), [Report::Gone(Exit::Failed)]);
    }
}
