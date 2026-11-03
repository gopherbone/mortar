// SPDX-License-Identifier: GPL-2.0-or-later
//! Talking to the shell over its control socket (the same line protocol as
//! `mortar-shell ctl`, $XDG_RUNTIME_DIR/mortar-shell.sock): start a game, open
//! the on-screen keyboard, ask for attention. From short-lived threads, so
//! the window never waits. Outside the shell these do nothing.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone)]
pub enum Shell {
    /// Over the socket.
    Socket(PathBuf),
    /// No shell here.
    None,
    /// Collects the lines: tests.
    Record(Arc<Mutex<Vec<String>>>),
}

impl Shell {
    pub fn detect() -> Shell {
        match std::env::var_os("XDG_RUNTIME_DIR") {
            Some(d) => {
                let p = PathBuf::from(d).join("mortar-shell.sock");
                if p.exists() { Shell::Socket(p) } else { Shell::None }
            }
            None => Shell::None,
        }
    }

    pub fn present(&self) -> bool {
        !matches!(self, Shell::None)
    }

    /// Send one line; `done` hears the error, if there is one.
    pub fn send(&self, line: String, done: impl FnOnce(Result<(), String>) + Send + 'static) {
        match self {
            Shell::None => done(Err("Not running in the shell.".into())),
            Shell::Record(v) => {
                v.lock().unwrap().push(line);
                done(Ok(()));
            }
            Shell::Socket(p) => {
                let p = p.clone();
                std::thread::spawn(move || done(send(&p, &line)));
            }
        }
    }

    /// The on-screen keyboard is a START press away.
    pub fn toggle_keyboard(&self) {
        self.send("press start".into(), |_| {});
    }

    /// Needs you: something waits for the person while this app is behind.
    pub fn attention(&self, text: Option<&str>) {
        let Ok(app) = std::env::var("MORTAR_APP") else { return };
        let j = match text {
            Some(t) => serde_json::json!({ "op": "set", "app": app, "text": t }),
            None => serde_json::json!({ "op": "clear", "app": app }),
        };
        self.send(format!("attention {j}"), |_| {});
    }
}

fn send(path: &PathBuf, line: &str) -> Result<(), String> {
    let mut s = std::os::unix::net::UnixStream::connect(path).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(3))).ok();
    s.write_all(line.as_bytes()).and_then(|_| s.write_all(b"\n")).map_err(|e| e.to_string())?;
    let mut out = String::new();
    BufReader::new(&s).read_line(&mut out).map_err(|e| e.to_string())?;
    match out.trim().strip_prefix("error: ") {
        Some(e) => Err(e.to_string()),
        None => Ok(()),
    }
}
