// SPDX-License-Identifier: GPL-2.0-or-later
//! The OSK suggestion socket, $XDG_RUNTIME_DIR/mortar-osk.sock (mode 0600):
//! zsh in the shell's terminals connects once at startup
//! (zsh/mortar-osk.zsh); the protocol and the strip are in osk_suggest.rs.
//!
//! - Only processes of our own uid are accepted (SO_PEERCRED), and each
//!   connection belongs to the app whose launched process is an ancestor
//!   of the peer pid (Apps::for_pid). Only the connections of the app on
//!   screen are asked, and only their answers to the newest question are
//!   shown, so another terminal (or anything else) cannot put chips over
//!   this one.
//! - Questions ("q SEQ") go out only while the OSK is up, SETTLE after the
//!   last key the OSK typed, so zsh does nothing while the OSK is closed
//!   and nothing while keys are still coming.

use super::state::Shell;
use crate::osk::{self, Tap};
use crate::osk_suggest::{Nav, Reply};
use mortar_ui::input::Button;
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction, RegistrationToken};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Quiet time after the last typed key before zsh is asked.
const SETTLE: Duration = Duration::from_millis(140);
/// A line longer than this ends the connection.
const MAX_LINE: usize = 64 * 1024;

pub fn default_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    dir.join("mortar-osk.sock")
}

struct Conn {
    stream: UnixStream,
    pid: u32,
    /// The app it belongs to (looked up again while unknown).
    app: Option<u32>,
    buf: Vec<u8>,
}

#[derive(Default)]
pub struct Suggest {
    pub strip: crate::osk_suggest::Strip,
    conns: HashMap<u64, Conn>,
    next_id: u64,
    /// The newest question; older answers are dropped.
    seq: u64,
    /// The app the strip's chips are for.
    app: Option<u32>,
    timer: Option<RegistrationToken>,
}

fn peer(s: &UnixStream) -> Option<(u32, u32)> {
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: cred/len are valid for the call
    let r = unsafe {
        libc::getsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut cred as *mut libc::ucred).cast(), &mut len)
    };
    (r == 0 && cred.pid > 0).then_some((cred.pid as u32, cred.uid))
}

pub fn listen(handle: &LoopHandle<'static, Shell>, path: &Path) -> Result<(), String> {
    let _ = std::fs::remove_file(path);
    let l = UnixListener::bind(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    l.set_nonblocking(true).map_err(|e| e.to_string())?;
    handle
        .insert_source(Generic::new(l, Interest::READ, Mode::Level), |_, l, shell: &mut Shell| {
            while let Ok((s, _)) = l.accept() {
                shell.suggest_accept(s);
            }
            Ok(PostAction::Continue)
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

impl Shell {
    fn suggest_accept(&mut self, s: UnixStream) {
        // SAFETY: plain getter
        let me = unsafe { libc::getuid() };
        let Some((pid, uid)) = peer(&s) else { return };
        if uid != me || s.set_nonblocking(true).is_err() {
            return;
        }
        let Ok(read_half) = s.try_clone() else { return };
        let id = self.suggest.next_id;
        self.suggest.next_id += 1;
        let app = self.apps.for_pid(pid, crate::apps::proc_parent);
        tracing::debug!(pid, ?app, "osk suggestions: zsh connected");
        let src = Generic::new(read_half, Interest::READ, Mode::Level);
        let ok = self.loop_handle.insert_source(src, move |_, s, shell: &mut Shell| {
            let s: &UnixStream = s;
            Ok(if shell.suggest_read(id, s) { PostAction::Continue } else { PostAction::Remove })
        });
        if ok.is_ok() {
            self.suggest.conns.insert(id, Conn { stream: s, pid, app, buf: Vec::new() });
            if self.osk.visible {
                self.suggest_request(Duration::ZERO);
            }
        }
    }

    /// Read what zsh sent; false when the connection is done.
    fn suggest_read(&mut self, id: u64, mut s: &UnixStream) -> bool {
        let mut chunk = [0u8; 4096];
        let mut lines = Vec::new();
        let alive = loop {
            match s.read(&mut chunk) {
                Ok(0) => break false,
                Ok(n) => {
                    let Some(c) = self.suggest.conns.get_mut(&id) else { break false };
                    c.buf.extend_from_slice(&chunk[..n]);
                    while let Some(i) = c.buf.iter().position(|b| *b == b'\n') {
                        let line: Vec<u8> = c.buf.drain(..=i).collect();
                        lines.push(String::from_utf8_lossy(&line).into_owned());
                    }
                    if c.buf.len() > MAX_LINE {
                        break false;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break true,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break false,
            }
        };
        for l in lines {
            self.suggest_line(id, l.trim());
        }
        if !alive {
            self.suggest.conns.remove(&id);
        }
        alive
    }

    /// The app a connection belongs to.
    fn conn_app(&mut self, id: u64) -> Option<u32> {
        let c = self.suggest.conns.get(&id)?;
        if c.app.is_some_and(|a| self.apps.get(a).is_some()) {
            return c.app;
        }
        let app = self.apps.for_pid(c.pid, crate::apps::proc_parent);
        if let Some(c) = self.suggest.conns.get_mut(&id) {
            c.app = app;
        }
        app
    }

    fn suggest_line(&mut self, id: u64, line: &str) {
        let Some(reply) = crate::osk_suggest::parse(line) else { return };
        let app = self.conn_app(id);
        if !self.osk.visible || app.is_none() || app != self.visible_app() {
            return;
        }
        match reply {
            Reply::Retry(seq) if seq == self.suggest.seq => self.suggest_request(SETTLE),
            Reply::Chips(seq, chips) if seq == self.suggest.seq => {
                self.suggest.app = app;
                self.suggest.strip.set(chips);
                self.osk_dirty = true;
                self.needs_redraw = true;
            }
            _ => {}
        }
    }

    /// The connections of the app on screen.
    fn suggest_ids(&mut self) -> Vec<u64> {
        let Some(vis) = self.visible_app() else { return Vec::new() };
        let ids: Vec<u64> = self.suggest.conns.keys().copied().collect();
        ids.into_iter().filter(|id| self.conn_app(*id) == Some(vis)).collect()
    }

    /// Ask the app's zsh for chips after `after` (restarts the wait).
    pub fn suggest_request(&mut self, after: Duration) {
        if let Some(t) = self.suggest.timer.take() {
            self.loop_handle.remove(t);
        }
        // answers to older questions are stale from now on
        self.suggest.seq += 1;
        let tok = self.loop_handle.insert_source(Timer::from_duration(after), |_, _, shell: &mut Shell| {
            shell.suggest.timer = None;
            shell.suggest_ask();
            TimeoutAction::Drop
        });
        self.suggest.timer = tok.ok();
    }

    fn suggest_ask(&mut self) {
        if !self.osk.visible {
            return;
        }
        let ids = self.suggest_ids();
        let q = format!("q {}\n", self.suggest.seq);
        for id in ids {
            if let Some(c) = self.suggest.conns.get_mut(&id) {
                // a full socket (zsh busy) just misses this question
                let _ = c.stream.write(q.as_bytes());
            }
        }
    }

    /// The OSK opened: drop chips from before, ask at once.
    pub fn suggest_opened(&mut self) {
        self.suggest.strip.clear();
        self.suggest.app = self.visible_app();
        self.suggest_request(Duration::ZERO);
    }

    /// Before the OSK handles a button: the strip's own moves and
    /// accepts. True when the button was used.
    pub fn suggest_button(&mut self, b: Button) -> bool {
        if self.suggest.app != self.visible_app() {
            // another app came to the front under the keyboard
            self.suggest_opened();
        }
        match self.suggest.strip.button(b, self.osk.at_top()) {
            Nav::Pass => false,
            Nav::Moved => true,
            Nav::Accept(chip) => {
                self.suggest.strip.clear();
                for t in chip.taps() {
                    self.tap(t);
                }
                self.suggest_request(SETTLE);
                true
            }
        }
    }

    /// The OSK typed `t`: chips are stale. Enter (a command runs) and
    /// Ctrl/Alt chords clear them at once.
    pub fn suggest_typed(&mut self, t: &Tap) {
        if t.code == osk::k::ENTER || t.ctrl || t.alt {
            self.suggest.strip.clear();
        }
        self.suggest_request(SETTLE);
    }

    /// The OSK closed.
    pub fn suggest_closed(&mut self) {
        if let Some(t) = self.suggest.timer.take() {
            self.loop_handle.remove(t);
        }
        self.suggest.seq += 1;
        self.suggest.strip.clear();
    }

    /// Chip labels, for `ctl state`.
    pub fn suggest_labels(&self) -> Vec<String> {
        self.suggest.strip.chips.iter().map(|c| c.label.clone()).collect()
    }
}
