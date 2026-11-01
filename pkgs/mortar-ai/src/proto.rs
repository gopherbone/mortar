// SPDX-License-Identifier: GPL-2.0-or-later
//! The Claude Code side: run `claude -p` with stream-json in and out, and
//! answer its tool-permission requests over the same pipe.
//!
//! This is the control protocol the Agent SDKs use (`--permission-prompt-tool
//! stdio`): the CLI writes `{"type":"control_request","request":{"subtype":
//! "can_use_tool",...}}` lines and waits for a matching `control_response`.
//! Checked against Claude Code 2.1.31x (see DESIGN.md for transcripts).

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

/// What to run. `program` is `claude` unless `$MORTAR_AI_CLAUDE` says
/// otherwise (tests use a fake).
#[derive(Clone, Debug)]
pub struct Spawn {
    pub program: String,
    pub cwd: Option<PathBuf>,
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    pub extra: Vec<String>,
    pub resume: Option<String>,
}

impl Default for Spawn {
    fn default() -> Self {
        Spawn {
            program: std::env::var("MORTAR_AI_CLAUDE").unwrap_or_else(|_| "claude".into()),
            cwd: None,
            model: None,
            permission_mode: None,
            extra: Vec::new(),
            resume: None,
        }
    }
}

impl Spawn {
    pub fn args(&self) -> Vec<String> {
        let mut a: Vec<String> = [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--permission-prompt-tool",
            "stdio",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        if let Some(m) = &self.model {
            a.extend(["--model".into(), m.clone()]);
        }
        if let Some(m) = &self.permission_mode {
            a.extend(["--permission-mode".into(), m.clone()]);
        }
        if let Some(r) = &self.resume {
            a.extend(["--resume".into(), r.clone()]);
        }
        a.extend(self.extra.iter().cloned());
        a
    }
}

#[derive(Debug)]
pub enum Msg {
    /// One JSON line from stdout.
    Json(Value),
    /// A stdout line that is not JSON (shown as a notice).
    Text(String),
    /// A stderr line.
    Stderr(String),
    /// The process exited (after stdout closed).
    Exit,
}

/// A running `claude -p`. Lines arrive on `rx`; every message also writes
/// one byte to the wake pipe so a poll() loop can sleep on `wake_fd()`.
pub struct Claude {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    pub rx: Receiver<Msg>,
    wake_r: OwnedFd,
    next_id: u64,
}

fn pipe() -> std::io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    for fd in fds {
        unsafe {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK);
        }
    }
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

#[derive(Clone)]
struct Waker {
    tx: Sender<Msg>,
    fd: std::sync::Arc<OwnedFd>,
}

impl Waker {
    fn send(&self, m: Msg) {
        let _ = self.tx.send(m);
        unsafe {
            libc::write(self.fd.as_raw_fd(), b"x".as_ptr() as *const libc::c_void, 1);
        }
    }
}

impl Claude {
    pub fn spawn(s: &Spawn) -> std::io::Result<Claude> {
        let (wake_r, wake_w) = pipe()?;
        let (tx, rx) = channel();
        let waker = Waker { tx, fd: std::sync::Arc::new(wake_w) };
        let mut cmd = Command::new(&s.program);
        cmd.args(s.args()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        if let Some(d) = &s.cwd {
            cmd.current_dir(d);
        }
        let mut child = cmd.spawn()?;
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let stdin = child.stdin.take();

        let w = waker.clone();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<Value>(&line) {
                    Ok(v) => w.send(Msg::Json(v)),
                    Err(_) => w.send(Msg::Text(line)),
                }
            }
            w.send(Msg::Exit); // the exit code comes from reap()
        });
        let w = waker;
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                w.send(Msg::Stderr(line));
            }
        });
        let mut c = Claude { child: Some(child), stdin, rx, wake_r, next_id: 0 };
        let id = c.id("init");
        c.send(&json!({"type":"control_request","request_id": id,"request":{"subtype":"initialize"}}));
        Ok(c)
    }

    pub fn wake_fd(&self) -> RawFd {
        self.wake_r.as_raw_fd()
    }

    /// Drain the wake pipe (call before draining `rx`).
    pub fn ack_wake(&self) {
        let mut buf = [0u8; 256];
        while unsafe { libc::read(self.wake_r.as_raw_fd(), buf.as_mut_ptr() as *mut libc::c_void, buf.len()) } > 0 {}
    }

    fn id(&mut self, what: &str) -> String {
        self.next_id += 1;
        format!("mortar_{what}_{}", self.next_id)
    }

    pub fn send(&mut self, v: &Value) -> bool {
        let Some(stdin) = self.stdin.as_mut() else { return false };
        let mut line = v.to_string();
        line.push('\n');
        stdin.write_all(line.as_bytes()).and_then(|_| stdin.flush()).is_ok()
    }

    pub fn interrupt(&mut self) -> bool {
        let id = self.id("interrupt");
        self.send(&json!({"type":"control_request","request_id": id,"request":{"subtype":"interrupt"}}))
    }

    /// Reap the child once stdout closed. Returns its exit code.
    pub fn reap(&mut self) -> Option<i32> {
        self.stdin = None;
        let mut c = self.child.take()?;
        c.wait().ok().and_then(|s| s.code())
    }
}

impl Drop for Claude {
    fn drop(&mut self) {
        self.stdin = None; // EOF: claude -p ends after the current turn
        if let Some(mut c) = self.child.take() {
            unsafe {
                libc::kill(c.id() as i32, libc::SIGTERM);
            }
            let _ = c.wait();
        }
    }
}

// ------------------------------------------------------------ messages ----

pub fn user_message(text: &str) -> Value {
    json!({"type":"user","message":{"role":"user","content": text}})
}

/// The answer to a `can_use_tool` control request.
pub fn permission_response(request_id: &str, d: &Decision, input: &Value, suggestions: &Value) -> Value {
    let inner = match d {
        Decision::Allow => json!({"behavior":"allow","updatedInput": input}),
        Decision::AllowAlways => json!({
            "behavior":"allow",
            "updatedInput": input,
            "updatedPermissions": if suggestions.is_array() { suggestions.clone() } else { json!([]) },
        }),
        Decision::Deny(msg) => json!({"behavior":"deny","message": msg}),
    };
    json!({"type":"control_response","response":{"subtype":"success","request_id": request_id,"response": inner}})
}

/// An error answer for control requests we don't implement (hooks, MCP).
pub fn control_error(request_id: &str, msg: &str) -> Value {
    json!({"type":"control_response","response":{"subtype":"error","request_id": request_id,"error": msg}})
}

#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    Allow,
    /// Allow and apply Claude Code's suggested rule (e.g. `Bash(git status:*)`
    /// into .claude/settings.local.json), so it doesn't ask again.
    AllowAlways,
    Deny(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn responses() {
        let r = permission_response("r1", &Decision::Allow, &json!({"command":"ls"}), &Value::Null);
        assert_eq!(r["response"]["request_id"], "r1");
        assert_eq!(r["response"]["response"]["behavior"], "allow");
        assert_eq!(r["response"]["response"]["updatedInput"]["command"], "ls");
        let s = json!([{"type":"addRules"}]);
        let r = permission_response("r2", &Decision::AllowAlways, &json!({}), &s);
        assert_eq!(r["response"]["response"]["updatedPermissions"], s);
        let r = permission_response("r3", &Decision::Deny("no".into()), &json!({}), &s);
        assert_eq!(r["response"]["response"]["behavior"], "deny");
        assert_eq!(r["response"]["response"]["message"], "no");
    }
    #[test]
    fn args_have_stdio_prompts() {
        let a = Spawn { model: Some("haiku".into()), ..Default::default() }.args();
        assert!(a.windows(2).any(|w| w[0] == "--permission-prompt-tool" && w[1] == "stdio"));
        assert!(a.windows(2).any(|w| w[0] == "--model" && w[1] == "haiku"));
    }
}
