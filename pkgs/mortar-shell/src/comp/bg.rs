// SPDX-License-Identifier: GPL-2.0-or-later
//! Background workers for the MENU's glass pages, so no socket round trip
//! or process start ever runs on the compositor thread:
//!
//! - `NetWorker`: mortar-setupd's control socket (mortar_setup::control,
//!   JSON lines; the root daemon talks to iwd and BlueZ over D-Bus). Polls
//!   `status` every second while Settings or a Wi-Fi/Bluetooth page shows,
//!   and sends the pages' requests (scan, connect, forget, pair, ...).
//! - `MixerWorker`: PipeWire through `pw-dump` (the apps' output streams)
//!   and `wpctl` (their volume and mute, the default sink's mute), polled
//!   every second while the Mixer shows.
//!
//! The compositor reads the newest snapshot when its generation changes.

use crate::mixer::{parse_streams, Stream};
use crate::shellui::NetState;
use mortar_setup::control::{Client, Request};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A snapshot shared with a worker, and its generation.
struct Shared<T> {
    val: Mutex<T>,
    seq: AtomicU64,
    active: AtomicBool,
}

impl<T> Shared<T> {
    fn put(&self, v: T) {
        if let Ok(mut g) = self.val.lock() {
            *g = v;
        }
        self.seq.fetch_add(1, Ordering::Release);
    }
}

pub struct NetWorker {
    tx: Sender<Request>,
    shared: Arc<Shared<Option<NetState>>>,
    seen: u64,
}

impl NetWorker {
    pub fn new(socket: PathBuf) -> NetWorker {
        let (tx, rx) = channel::<Request>();
        let shared = Arc::new(Shared { val: Mutex::new(None), seq: AtomicU64::new(0), active: AtomicBool::new(false) });
        let sh = shared.clone();
        let spawned = std::thread::Builder::new().name("setup-net".into()).spawn(move || {
            let client = Client::new(&socket);
            let poll = |sh: &Shared<Option<NetState>>| {
                let st = match client.status() {
                    Ok(s) => NetState { wifi: s.wifi, bt: s.bluetooth, error: None },
                    Err(e) => NetState { error: Some(e), ..Default::default() },
                };
                sh.put(Some(st));
            };
            loop {
                match rx.recv_timeout(Duration::from_secs(1)) {
                    Ok(req) => {
                        if let Err(e) = client.act(req) {
                            tracing::warn!("setup service: {e}");
                        }
                        poll(&sh);
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        if sh.active.load(Ordering::Acquire) {
                            poll(&sh);
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        });
        if let Err(e) = spawned {
            tracing::warn!("setup-net thread: {e}");
        }
        NetWorker { tx, shared, seen: 0 }
    }

    /// Poll while the pages show (the first poll right away).
    pub fn set_active(&mut self, on: bool) {
        let was = self.shared.active.swap(on, Ordering::AcqRel);
        if on && !was {
            let _ = self.tx.send(Request::Status);
        }
    }

    pub fn send(&self, r: Request) {
        let _ = self.tx.send(r);
    }

    /// A newer snapshot, if there is one.
    pub fn take(&mut self) -> Option<Option<NetState>> {
        let g = self.shared.seq.load(Ordering::Acquire);
        if g == self.seen {
            return None;
        }
        self.seen = g;
        self.shared.val.lock().ok().map(|v| v.clone())
    }
}

enum MixJob {
    Run(Vec<String>),
    Refresh,
}

pub struct MixerWorker {
    tx: Sender<MixJob>,
    shared: Arc<Shared<Vec<Stream>>>,
    seen: u64,
    /// Home's Now playing: look every 4 s (the Mixer: every second).
    slow: Arc<AtomicBool>,
}

impl MixerWorker {
    pub fn new(pw_dump: Vec<String>) -> MixerWorker {
        let (tx, rx) = channel::<MixJob>();
        let shared = Arc::new(Shared { val: Mutex::new(Vec::new()), seq: AtomicU64::new(0), active: AtomicBool::new(false) });
        let sh = shared.clone();
        let slow = Arc::new(AtomicBool::new(false));
        let sl = slow.clone();
        let spawned = std::thread::Builder::new().name("mixer".into()).spawn(move || {
            let mut ticks = 0u32;
            let dump = |sh: &Shared<Vec<Stream>>| {
                let Some((cmd, args)) = pw_dump.split_first() else { return };
                match run_to_memory(cmd, args) {
                    Ok(out) => sh.put(parse_streams(&out)),
                    Err(e) => {
                        tracing::debug!("{cmd}: {e}");
                        sh.put(Vec::new());
                    }
                }
            };
            loop {
                match rx.recv_timeout(Duration::from_secs(1)) {
                    Ok(MixJob::Run(argv)) => {
                        // run every queued change, then look once
                        super::state::run_quiet(&argv);
                        while let Ok(j) = rx.try_recv() {
                            if let MixJob::Run(a) = j {
                                super::state::run_quiet(&a);
                            }
                        }
                        dump(&sh);
                    }
                    Ok(MixJob::Refresh) => dump(&sh),
                    Err(RecvTimeoutError::Timeout) => {
                        ticks = ticks.wrapping_add(1);
                        if sh.active.load(Ordering::Acquire) || (sl.load(Ordering::Acquire) && ticks % 4 == 0) {
                            dump(&sh);
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        });
        if let Err(e) = spawned {
            tracing::warn!("mixer thread: {e}");
        }
        MixerWorker { tx, shared, seen: 0, slow }
    }

    pub fn set_slow(&self, on: bool) {
        self.slow.store(on, Ordering::Release);
    }

    pub fn set_active(&mut self, on: bool) {
        let was = self.shared.active.swap(on, Ordering::AcqRel);
        if on && !was {
            let _ = self.tx.send(MixJob::Refresh);
        }
    }

    /// Run a wpctl (or other) command off the loop, then look again.
    pub fn run(&self, argv: Vec<String>) {
        let _ = self.tx.send(MixJob::Run(argv));
    }

    pub fn take(&mut self) -> Option<Vec<Stream>> {
        let g = self.shared.seq.load(Ordering::Acquire);
        if g == self.seen {
            return None;
        }
        self.seen = g;
        self.shared.val.lock().ok().map(|v| v.clone())
    }
}

/// Run a command and return its stdout, collected in a memfd rather than
/// a pipe: pw-dump writes its ~100 KB of JSON in ~3,500 small writes, and
/// through a pipe each one woke this thread (measured: ~500 wakeups/s
/// with a dump every 4 s).
fn run_to_memory(cmd: &str, args: &[String]) -> std::io::Result<String> {
    use std::io::{Read, Seek};
    use std::os::fd::FromRawFd;
    // SAFETY: memfd_create with a NUL-terminated name; the File owns the
    // new descriptor from here on.
    let fd = unsafe { libc::memfd_create(c"mortar-shell-out".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: fd is a fresh descriptor nothing else owns
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    let status = std::process::Command::new(cmd)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(file.try_clone()?)
        .stderr(std::process::Stdio::null())
        .status()?;
    if !status.success() {
        return Err(std::io::Error::other(format!("exit status {status}")));
    }
    file.rewind()?;
    let mut out = String::new();
    file.take(16 * 1024 * 1024).read_to_string(&mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn collects_stdout_in_memory() {
        let script = "for i in 1 2 3; do printf 'line %s\\n' $i; done";
        let out = super::run_to_memory("sh", &["-c".into(), script.into()]).unwrap();
        assert_eq!(out, "line 1\nline 2\nline 3\n");
        assert!(super::run_to_memory("sh", &["-c".into(), "exit 3".into()]).is_err());
    }
}
