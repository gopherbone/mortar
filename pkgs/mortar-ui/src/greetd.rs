// SPDX-License-Identifier: GPL-2.0-or-later
//! greetd IPC (JSON over $GREETD_SOCK via the greetd_ipc crate) behind a
//! small trait, plus a fake for dev mode on a Mac.
//!
//! Login flow: create_session(user) -> zero or more auth prompts answered with
//! post_auth_message_response -> success -> start_session(cmd, env). Mortar
//! accounts are passwordless (the module adds a pam_succeed_if rule for the
//! configured profiles), so normally create_session succeeds straight away.
//! If PAM asks something (a future PIN), the greeter shows the PIN pad.

use crate::config::{Auth, Config};
use greetd_ipc::codec::SyncCodec;
use greetd_ipc::{AuthMessageType, ErrorType, Request, Response};
use std::os::unix::net::UnixStream;

#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    Success,
    /// PAM wants input: `secret` hides what's typed.
    Prompt { text: String, secret: bool },
    /// PAM info/error text; acknowledge with `respond(None)`.
    Info(String),
    /// Login failed; `auth` = wrong credentials vs. other error.
    Error { text: String, auth: bool },
}

pub trait Greetd {
    fn create_session(&mut self, user: &str) -> Result<Reply, String>;
    fn respond(&mut self, answer: Option<String>) -> Result<Reply, String>;
    fn start_session(&mut self, cmd: Vec<String>, env: Vec<String>) -> Result<Reply, String>;
    fn cancel(&mut self) -> Result<(), String>;
}

pub struct Real {
    sock: UnixStream,
}

impl Real {
    pub fn connect() -> Result<Real, String> {
        let path = std::env::var("GREETD_SOCK").map_err(|_| "GREETD_SOCK is not set (not started by greetd?)")?;
        Ok(Real { sock: UnixStream::connect(&path).map_err(|e| format!("{path}: {e}"))? })
    }
    fn call(&mut self, req: Request) -> Result<Reply, String> {
        req.write_to(&mut self.sock).map_err(|e| e.to_string())?;
        let resp = Response::read_from(&mut self.sock).map_err(|e| e.to_string())?;
        Ok(match resp {
            Response::Success => Reply::Success,
            Response::Error { error_type, description } => {
                Reply::Error { text: description, auth: matches!(error_type, ErrorType::AuthError) }
            }
            Response::AuthMessage { auth_message_type, auth_message } => match auth_message_type {
                AuthMessageType::Visible => Reply::Prompt { text: auth_message, secret: false },
                AuthMessageType::Secret => Reply::Prompt { text: auth_message, secret: true },
                AuthMessageType::Info | AuthMessageType::Error => Reply::Info(auth_message),
            },
        })
    }
}

impl Greetd for Real {
    fn create_session(&mut self, user: &str) -> Result<Reply, String> {
        self.call(Request::CreateSession { username: user.to_string() })
    }
    fn respond(&mut self, answer: Option<String>) -> Result<Reply, String> {
        self.call(Request::PostAuthMessageResponse { response: answer })
    }
    fn start_session(&mut self, cmd: Vec<String>, env: Vec<String>) -> Result<Reply, String> {
        self.call(Request::StartSession { cmd, env })
    }
    fn cancel(&mut self) -> Result<(), String> {
        self.call(Request::CancelSession).map(|_| ())
    }
}

/// Dev-mode stand-in: profiles with `"auth": "pin"` ask for PIN 1234, others
/// log straight in; start_session just prints the command.
pub struct Fake {
    pin_users: Vec<String>,
    pending: Option<String>,
    authed: bool,
}

impl Fake {
    pub fn new(cfg: &Config) -> Fake {
        Fake {
            pin_users: cfg.profiles.iter().filter(|p| p.auth == Auth::Pin).map(|p| p.user.clone()).collect(),
            pending: None,
            authed: false,
        }
    }
}

impl Greetd for Fake {
    fn create_session(&mut self, user: &str) -> Result<Reply, String> {
        if self.pending.is_some() {
            return Ok(Reply::Error { text: "a session is already being configured".into(), auth: false });
        }
        self.pending = Some(user.to_string());
        self.authed = !self.pin_users.iter().any(|u| u == user);
        Ok(if self.authed { Reply::Success } else { Reply::Prompt { text: "PIN:".into(), secret: true } })
    }
    fn respond(&mut self, answer: Option<String>) -> Result<Reply, String> {
        if self.pending.is_none() {
            return Ok(Reply::Error { text: "no session".into(), auth: false });
        }
        if answer.as_deref() == Some("1234") {
            self.authed = true;
            Ok(Reply::Success)
        } else {
            self.pending = None;
            Ok(Reply::Error { text: "Wrong PIN".into(), auth: true })
        }
    }
    fn start_session(&mut self, cmd: Vec<String>, env: Vec<String>) -> Result<Reply, String> {
        if !self.authed {
            return Ok(Reply::Error { text: "not authenticated".into(), auth: false });
        }
        eprintln!("fake greetd: start_session user={:?} cmd={cmd:?} env={env:?}", self.pending);
        self.pending = None;
        self.authed = false;
        Ok(Reply::Success)
    }
    fn cancel(&mut self) -> Result<(), String> {
        self.pending = None;
        self.authed = false;
        Ok(())
    }
}
