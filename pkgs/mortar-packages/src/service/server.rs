// SPDX-License-Identifier: GPL-2.0-or-later
//! The socket. systemd holds it (`mortar-packages.socket`, mode 0660, the session
//! user's group) and starts this on the first connection, handing it over as fd 3.
//! A thread per connection, a line in and a line out, and an exit once nothing has
//! happened for a while. A connection that arrives during the exit waits on the socket
//! for the next start.

use super::daemon::Daemon;
use crate::protocol::*;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// A connection that says nothing shouldn't keep the service up.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// How often a watcher hears of progress.
const WATCH_EVERY: Duration = Duration::from_millis(200);
/// More than any real use, less than running out of threads.
const MAX_CONNECTIONS: usize = 16;

/// What systemd passed, if it did.
fn activated() -> Option<UnixListener> {
    let pid: u32 = std::env::var("LISTEN_PID").ok()?.parse().ok()?;
    let n: u32 = std::env::var("LISTEN_FDS").ok()?.parse().ok()?;
    if pid != std::process::id() || n != 1 {
        return None;
    }
    // SAFETY: systemd's contract: fd 3 is ours, and nothing else in here opened it
    Some(unsafe { UnixListener::from_raw_fd(3) })
}

fn bound(path: &Path) -> Result<UnixListener, String> {
    let _ = std::fs::remove_file(path);
    UnixListener::bind(path).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn run(d: Arc<Daemon>, socket: Option<&Path>) -> Result<(), String> {
    let listener = match activated() {
        Some(l) => l,
        None => bound(socket.unwrap_or(Path::new(SOCKET)))?,
    };
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let idle_after = Duration::from_secs(d.cfg.idle_seconds);
    let mut last = Instant::now();
    loop {
        match listener.accept() {
            Ok((conn, _)) => {
                last = Instant::now();
                let d = d.clone();
                if d.busy.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
                    continue; // dropping it closes it
                }
                d.busy.fetch_add(1, Ordering::SeqCst);
                std::thread::spawn(move || {
                    if let Err(e) = serve(&d, conn) {
                        eprintln!("mortar-packages: {e}");
                    }
                    d.busy.fetch_sub(1, Ordering::SeqCst);
                });
                continue;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.to_string()),
        }
        if d.job_running() || d.busy.load(Ordering::SeqCst) > 0 {
            last = Instant::now();
        } else if last.elapsed() >= idle_after {
            return Ok(());
        }
        // asleep until a connection comes, or it's time to look at the clock
        let mut p = libc::pollfd { fd: listener.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // SAFETY: one valid pollfd, for the duration of the call
        unsafe {
            libc::poll(&mut p, 1, 1000);
        }
    }
}

fn send(conn: &mut UnixStream, r: &Reply) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(r).expect("reply serializes");
    line.push(b'\n');
    conn.write_all(&line)
}

fn serve(d: &Arc<Daemon>, conn: UnixStream) -> Result<(), String> {
    conn.set_nonblocking(false).map_err(|e| e.to_string())?;
    conn.set_read_timeout(Some(REQUEST_TIMEOUT)).map_err(|e| e.to_string())?;
    let mut w = conn.try_clone().map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(conn.take(MAX_REQUEST)).read_line(&mut line).map_err(|e| format!("request: {e}"))?;
    let req: Request = match serde_json::from_str(&line) {
        Ok(r) => r,
        Err(e) => {
            let _ = send(&mut w, &Reply::Refused(Problem::new("bad-request", "That isn't a request", e.to_string())));
            return Ok(());
        }
    };
    let reply = handle(d, &req);
    if req.op == Op::Watch {
        return watch(d, &mut w);
    }
    send(&mut w, &reply).map_err(|e| e.to_string())
}

fn handle(d: &Arc<Daemon>, req: &Request) -> Reply {
    let (repo, id) = (req.repo.as_deref(), req.id.as_deref());
    let done = |r: Result<String, Problem>| match r {
        Ok(message) => Reply::Ok { message },
        Err(p) => Reply::Refused(p),
    };
    let job = |r: Result<JobView, Problem>| match r {
        Ok(j) => Reply::Job(j),
        Err(p) => Reply::Refused(p),
    };
    match req.op {
        Op::Overview => Reply::Overview(d.overview(req.refresh)),
        Op::Details => match d.details(repo, id) {
            Ok(x) => Reply::Details(x),
            Err(p) => Reply::Refused(p),
        },
        Op::Search => match d.search(req.query.as_deref().unwrap_or("")) {
            Ok(f) => Reply::Found(f),
            Err(p) => Reply::Refused(p),
        },
        Op::Install => job(d.start(Action::Install, repo, id)),
        Op::Update => job(d.start(Action::Update, repo, id)),
        Op::Remove => done(d.remove(repo, id)),
        Op::Rollback => done(d.rollback(repo, id)),
        Op::Cancel => done(d.cancel()),
        Op::Auto => done(d.auto()),
        Op::Status | Op::Watch => d.job().map(Reply::Job).unwrap_or(Reply::NoJob),
    }
}

/// Each change to the job until it ends, at most five a second. The watcher going away
/// doesn't stop the job: it carries on, and can be watched again.
fn watch(d: &Arc<Daemon>, w: &mut UnixStream) -> Result<(), String> {
    let mut last = d.job();
    let Some(first) = last.clone() else {
        let _ = send(w, &Reply::NoJob);
        return Ok(());
    };
    send(w, &Reply::Job(first)).map_err(|e| e.to_string())?;
    while !last.as_ref().is_none_or(|j| j.phase.ended()) {
        std::thread::sleep(WATCH_EVERY);
        let now = d.job_after(&last);
        if let Some(j) = &now {
            send(w, &Reply::Job(j.clone())).map_err(|e| e.to_string())?;
        }
        last = now;
    }
    Ok(())
}
