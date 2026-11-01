// SPDX-License-Identifier: GPL-2.0-or-later
//! The game's control socket, for mortar-shell's game panel (MENU > Running
//! > Y on a game), its hibernation and its sleep safety:
//! `$XDG_RUNTIME_DIR/mortar-play-<pid>.sock` (0600), one JSON request line
//! per connection, one JSON reply line.
//!
//!   {"cmd":"list_states"}           slots, ages, thumbnails, discs, fast-forward
//!   {"cmd":"save_state","slot":1}   1-4, or "auto"
//!   {"cmd":"load_state","slot":1}
//!   {"cmd":"reset"}
//!   {"cmd":"fast_forward","on":true|false|"toggle"}
//!   {"cmd":"change_disc","to":"next"}
//!   {"cmd":"quit","quiet":false}    writes `auto`, replies, then exits
//!
//! Replies: {"ok":true,...} or {"ok":false,"error":"..."}. The socket is
//! polled once per loop iteration (a frame while playing, ~16 ms while
//! paused), never blocking the game for longer than a request takes.

use serde::Deserialize;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum SlotArg {
    N(u8),
    Name(String),
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum OnArg {
    Bool(bool),
    Word(String),
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    ListStates,
    SaveState { slot: SlotArg },
    LoadState { slot: SlotArg },
    Reset,
    FastForward { on: OnArg },
    ChangeDisc {
        #[serde(default)]
        to: Option<String>,
    },
    Quit {
        #[serde(default)]
        quiet: bool,
    },
}

/// A protocol slot: 1-4 are the menu's slots (0-3 inside), "auto" the resume state.
pub fn parse_slot(s: &SlotArg) -> Result<Option<u8>, String> {
    match s {
        SlotArg::N(n) if (1..=crate::session::SLOTS).contains(n) => Ok(Some(n - 1)),
        SlotArg::Name(w) if w == "auto" => Ok(None),
        SlotArg::Name(w) if w.parse::<u8>().is_ok_and(|n| (1..=crate::session::SLOTS).contains(&n)) => Ok(Some(w.parse::<u8>().unwrap() - 1)),
        _ => Err(format!("slot: 1-{} or \"auto\"", crate::session::SLOTS)),
    }
}

/// on/off/toggle against the current state.
pub fn parse_on(a: &OnArg, cur: bool) -> Result<bool, String> {
    match a {
        OnArg::Bool(b) => Ok(*b),
        OnArg::Word(w) => match w.as_str() {
            "on" => Ok(true),
            "off" => Ok(false),
            "toggle" => Ok(!cur),
            _ => Err("on: true | false | \"toggle\"".into()),
        },
    }
}

pub struct Control {
    listener: UnixListener,
    pub path: PathBuf,
}

/// One request waiting for its answer.
pub struct Pending {
    pub req: Result<Request, String>,
    stream: UnixStream,
}

impl Pending {
    pub fn reply(self, v: serde_json::Value) {
        let mut s = self.stream;
        let _ = s.set_write_timeout(Some(Duration::from_millis(500)));
        let _ = s.write_all(v.to_string().as_bytes());
        let _ = s.write_all(b"\n");
    }
}

pub fn socket_path() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")?;
    Some(PathBuf::from(dir).join(format!("mortar-play-{}.sock", std::process::id())))
}

impl Control {
    pub fn open() -> Result<Control, String> {
        Control::open_at(socket_path().ok_or("XDG_RUNTIME_DIR is not set")?)
    }

    pub fn open_at(path: PathBuf) -> Result<Control, String> {
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        Ok(Control { listener, path })
    }

    /// Requests that came in since the last call (non-blocking accept; a
    /// client gets 200 ms to send its line).
    pub fn poll(&self) -> Vec<Pending> {
        let mut v = Vec::new();
        while let Ok((s, _)) = self.listener.accept() {
            let _ = s.set_nonblocking(false);
            let _ = s.set_read_timeout(Some(Duration::from_millis(200)));
            let mut line = String::new();
            let req = match BufReader::new(&s).read_line(&mut line) {
                Ok(_) => serde_json::from_str::<Request>(line.trim()).map_err(|e| format!("bad request: {e}")),
                Err(e) => Err(e.to_string()),
            };
            v.push(Pending { req, stream: s });
        }
        v
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Tell mortar-shell (its control socket, `toast {json}`) that the game was
/// saved as it quit. Best effort, at most ~300 ms.
pub fn shell_toast(title: &str, sub: &str) {
    let path = std::env::var_os("MORTAR_SHELL_SOCKET")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_RUNTIME_DIR").map(|d| PathBuf::from(d).join("mortar-shell.sock")));
    let Some(path) = path else { return };
    let Ok(mut s) = UnixStream::connect(&path) else { return };
    let _ = s.set_write_timeout(Some(Duration::from_millis(300)));
    let _ = s.set_read_timeout(Some(Duration::from_millis(300)));
    let msg = serde_json::json!({ "title": title, "sub": sub, "sound": "none" });
    if s.write_all(format!("toast {msg}\n").as_bytes()).is_ok() {
        let mut r = String::new();
        let _ = BufReader::new(&s).read_line(&mut r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests() {
        let p = |s: &str| serde_json::from_str::<Request>(s);
        assert_eq!(p(r#"{"cmd":"list_states"}"#).unwrap(), Request::ListStates);
        assert_eq!(p(r#"{"cmd":"save_state","slot":2}"#).unwrap(), Request::SaveState { slot: SlotArg::N(2) });
        assert_eq!(p(r#"{"cmd":"quit"}"#).unwrap(), Request::Quit { quiet: false });
        assert_eq!(p(r#"{"cmd":"quit","quiet":true}"#).unwrap(), Request::Quit { quiet: true });
        assert_eq!(p(r#"{"cmd":"change_disc","to":"next"}"#).unwrap(), Request::ChangeDisc { to: Some("next".into()) });
        assert!(p(r#"{"cmd":"format_disk"}"#).is_err());
        assert_eq!(parse_slot(&SlotArg::N(1)), Ok(Some(0)));
        assert_eq!(parse_slot(&SlotArg::N(4)), Ok(Some(3)));
        assert!(parse_slot(&SlotArg::N(5)).is_err());
        assert_eq!(parse_slot(&SlotArg::Name("auto".into())), Ok(None));
        assert_eq!(parse_on(&OnArg::Word("toggle".into()), true), Ok(false));
        assert_eq!(parse_on(&OnArg::Bool(true), false), Ok(true));
    }

    #[test]
    fn socket_round_trip() {
        let dir = std::env::temp_dir().join(format!("mp-ctl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let c = Control::open_at(dir.join("t.sock")).unwrap();
        assert!(c.path.exists());
        let path = c.path.clone();
        let t = std::thread::spawn(move || {
            let mut s = UnixStream::connect(&path).unwrap();
            s.write_all(b"{\"cmd\":\"reset\"}\n").unwrap();
            let mut r = String::new();
            BufReader::new(&s).read_line(&mut r).unwrap();
            r
        });
        let mut got = Vec::new();
        for _ in 0..200 {
            got = c.poll();
            if !got.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let p = got.pop().unwrap();
        assert_eq!(p.req.as_ref().unwrap(), &Request::Reset);
        p.reply(serde_json::json!({"ok": true}));
        assert_eq!(t.join().unwrap().trim(), "{\"ok\":true}");
        let path = c.path.clone();
        drop(c);
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
