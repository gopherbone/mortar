// SPDX-License-Identifier: GPL-2.0-or-later
//! A made-up service: every package, size and date here is invented. The preview draws
//! from it, the tests drive the app against it, and `mortar-packages fake-service` puts
//! it on a real socket for the headless test, where it also logs what it was asked.

use crate::backend::Backend;
use crate::protocol::*;
use mortar_index::Kind;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

pub const NOW: u64 = 1_793_750_400;
const MIB: u64 = 1048576;

fn base(id: &str, name: &str, category: &str, icon: &str, version: &str, days: u64, description: &str) -> Row {
    Row {
        repo: "mortar".into(),
        repo_name: "Mortar".into(),
        official: true,
        id: id.into(),
        name: name.into(),
        description: description.into(),
        version: version.into(),
        category: category.into(),
        icon: icon.into(),
        licence: "MIT, LGPL-2.1, others".into(),
        source: format!("https://example.org/{id}"),
        kind: Kind::Nix,
        published: NOW - days * 86400,
        ..Row::default()
    }
}

fn tool(id: &str, program: &str, version: &str, description: &str) -> Row {
    let mut r = base(id, program, "tools", "term", version, 3, description);
    r.kind = Kind::Tool;
    r.program = Some(program.into());
    r.licence = String::new();
    r.source = String::new();
    r
}

pub fn rows() -> Vec<Row> {
    let mut v = vec![
        {
            let mut r = base("pond-radio", "Pond Radio", "music", "wifi", "4.1", 1, "Stations from far away, by the dial");
            r.state = RowState::Update;
            r.installed_version = Some("4.0".into());
            r.can_roll_back = true;
            r
        },
        base("harbour-browser", "Harbour Browser", "apps", "link", "2.54.1", 3, "A web browser, for the pad and the screen"),
        {
            let mut r = base("cave-runner", "Cave Runner II", "games", "pad", "1.8", 12, "The engine for a game you already own");
            r.requires = vec!["~/.local/share/cave-runner/CAVES.DAT".into(), "~/.local/share/cave-runner/SOUND.DAT".into()];
            r
        },
        {
            let mut r = base("tracker-nine", "Tracker Nine", "music", "pad", "0.9.2", 30, "Nine channels and a very small screen");
            r.state = RowState::Installed;
            r.installed_version = Some("0.9.2".into());
            r.can_roll_back = true;
            r
        },
        {
            let mut r = base("sky-forge", "Sky Forge", "games", "pad", "1.1", 5, "A bundle: a folder and a manifest");
            r.kind = Kind::Bundle;
            r.state = RowState::NotYet;
            r
        },
        {
            let mut r = base("moon-miner", "Moon Miner Deluxe", "games", "pad", "3.0", 90, "Put here from a computer");
            r.state = RowState::Elsewhere;
            r.installed_version = Some("2.9".into());
            r
        },
    ];
    let mut ledger = tool("ledger-line", "ledger-line", "2.2", "Accounts in plain text, one line each");
    ledger.state = RowState::Installed;
    ledger.installed_version = Some("2.2".into());
    v.push(ledger);
    v
}

/// What a search of the big list finds.
pub fn tools() -> Vec<Row> {
    vec![
        tool("ripline", "rl", "14.1", "Search through files, faster than you can read"),
        tool("hopper", "hop", "3.4", "Look at what's running"),
        tool("tubeget", "tg", "2025.1", "Fetch a video for later"),
        tool("ledger-line", "ledger-line", "2.2", "Accounts in plain text, one line each"),
        tool("catwings", "cw", "0.25", "Read a file, with colour"),
        tool("treeline", "tl", "2.1", "A folder and what's in it, as a tree"),
    ]
}

fn sizes(id: &str) -> Size {
    match id {
        "harbour-browser" => Size { download: 52 * MIB, unpacked: 209 * MIB, paths: 16 },
        "pond-radio" => Size { download: 3 * MIB, unpacked: 9 * MIB, paths: 2 },
        "cave-runner" => Size { download: 11 * MIB, unpacked: 38 * MIB, paths: 4 },
        _ => Size { download: MIB, unpacked: 3 * MIB, paths: 1 },
    }
}

fn display_name(s: &DemoState, id: &str) -> String {
    s.rows.iter().chain(s.tools.iter()).find(|r| r.id == id).map(|r| r.name.clone()).unwrap_or_else(|| id.to_string())
}

pub struct DemoState {
    pub repos: Vec<RepoView>,
    pub rows: Vec<Row>,
    pub tools: Vec<Row>,
    pub job: Option<JobView>,
    pub free: u64,
    pub battery: Option<u8>,
    pub charging: bool,
    /// What the next Details says instead of a size.
    pub details_problem: Option<Problem>,
    /// What the next install ends with, instead of working.
    pub fails: Option<Problem>,
    /// How long a fake download takes.
    pub job_ms: u64,
    /// A timeline runs jobs on its own; in the preview they hold still.
    pub timeline: bool,
}

pub struct Demo {
    pub state: Mutex<DemoState>,
    changed: Condvar,
    /// What it was asked, in order: `op repo id`.
    pub log: Mutex<Vec<String>>,
}

impl Demo {
    pub fn new() -> Arc<Demo> {
        Arc::new(Demo {
            state: Mutex::new(DemoState {
                repos: vec![RepoView { id: "mortar".into(), name: "Mortar".into(), official: true, published: NOW - 3 * 86400, checked: NOW - 3600, problem: None, tools: 93_663 }],
                rows: rows(),
                tools: tools(),
                job: None,
                free: 218 * 1024 * MIB,
                battery: Some(84),
                charging: false,
                details_problem: None,
                fails: None,
                job_ms: 6000,
                timeline: false,
            }),
            changed: Condvar::new(),
            log: Mutex::new(Vec::new()),
        })
    }

    pub fn with(&self, f: impl FnOnce(&mut DemoState)) {
        f(&mut self.state.lock().unwrap());
        self.changed.notify_all();
    }

    fn set_row(s: &mut DemoState, id: &str, f: impl FnOnce(&mut Row)) {
        if let Some(r) = s.rows.iter_mut().chain(s.tools.iter_mut()).find(|r| r.id == id) {
            f(r);
        }
    }

    fn start(self: &Arc<Demo>, action: Action, repo: &str, id: &str) -> Reply {
        let mut s = self.state.lock().unwrap();
        let name = s.rows.iter().chain(s.tools.iter()).find(|r| r.id == id).map(|r| r.name.clone()).unwrap_or_else(|| id.into());
        if s.job.as_ref().is_some_and(|j| !j.phase.ended()) {
            return Reply::Refused(Problem::new("busy", "One thing at a time", "Something else is being installed."));
        }
        let view = JobView { repo: repo.into(), id: id.into(), name, action, phase: Phase::Checking, total: sizes(id).unpacked, ..Default::default() };
        s.job = Some(view.clone());
        if s.timeline {
            let d = self.clone();
            let ms = s.job_ms;
            std::thread::spawn(move || d.run_job(ms));
        }
        Reply::Job(view)
    }

    /// A download on a timeline, a tenth at a time.
    fn run_job(self: Arc<Demo>, ms: u64) {
        for n in 1..=10u64 {
            std::thread::sleep(Duration::from_millis(ms / 10));
            let mut s = self.state.lock().unwrap();
            let fails = s.fails.take();
            let Some(j) = s.job.as_mut() else { return };
            if j.cancelling {
                j.phase = Phase::Cancelled;
                j.cancelling = false;
                self.changed.notify_all();
                return;
            }
            if let Some(p) = fails {
                j.phase = Phase::Failed;
                j.problem = Some(p);
                self.changed.notify_all();
                return;
            }
            j.phase = Phase::Downloading;
            j.done = j.total * n / 10;
            self.changed.notify_all();
        }
        let mut s = self.state.lock().unwrap();
        let (id, action) = {
            let j = s.job.as_mut().unwrap();
            j.phase = Phase::Done;
            (j.id.clone(), j.action)
        };
        Self::set_row(&mut s, &id, |r| {
            if action == Action::Update {
                r.can_roll_back = true;
            }
            r.state = RowState::Installed;
            r.installed_version = Some(r.version.clone());
        });
        self.changed.notify_all();
    }
}

impl Demo {
    fn answer(self: &Arc<Demo>, req: &Request) -> Reply {
        let repo = req.repo.as_deref().unwrap_or("");
        let id = req.id.as_deref().unwrap_or("");
        if req.op != Op::Watch {
            self.log.lock().unwrap().push(format!("{:?} {repo} {id}{}", req.op, req.query.as_deref().map(|q| format!(" {q}")).unwrap_or_default()).trim().to_string());
        }
        match req.op {
            Op::Overview => {
                let s = self.state.lock().unwrap();
                let job = s.job.clone().filter(|j| !j.phase.ended());
                Reply::Overview(Overview { repos: s.repos.clone(), rows: s.rows.clone(), job })
            }
            Op::Details => {
                let s = self.state.lock().unwrap();
                Reply::Details(Details {
                    repo: repo.into(),
                    id: id.into(),
                    size: if s.details_problem.is_some() { None } else { Some(sizes(id)) },
                    free: s.free,
                    need: sizes(id).unpacked + 1024 * MIB,
                    battery: s.battery,
                    charging: s.charging,
                    power_ok: s.charging || s.battery.is_none_or(|b| b >= 30),
                    problem: s.details_problem.clone(),
                })
            }
            Op::Search => {
                let s = self.state.lock().unwrap();
                let q = req.query.as_deref().unwrap_or("").to_lowercase();
                let rows: Vec<Row> = s.tools.iter().filter(|r| q.split_whitespace().all(|w| r.id.contains(w) || r.name.contains(w) || r.description.to_lowercase().contains(w))).cloned().collect();
                Reply::Found(Found { total: rows.len() as u64, rows, searched: 93_663 })
            }
            Op::Install => self.start(Action::Install, repo, id),
            Op::Update => self.start(Action::Update, repo, id),
            Op::Remove => {
                let mut s = self.state.lock().unwrap();
                Self::set_row(&mut s, id, |r| {
                    r.state = RowState::Available;
                    r.installed_version = None;
                    r.can_roll_back = false;
                });
                Reply::Ok { message: format!("{} is gone", display_name(&s, id)) }
            }
            Op::Rollback => {
                let mut s = self.state.lock().unwrap();
                Self::set_row(&mut s, id, |r| {
                    r.state = RowState::Update;
                    r.installed_version = Some("the old one".into());
                    r.can_roll_back = false;
                });
                Reply::Ok { message: format!("{} is back to the old one", display_name(&s, id)) }
            }
            Op::Cancel => {
                let mut s = self.state.lock().unwrap();
                match s.job.as_mut().filter(|j| !j.phase.ended()) {
                    Some(j) => {
                        j.cancelling = true;
                        self.changed.notify_all();
                        Reply::Ok { message: "Stopping".into() }
                    }
                    None => Reply::Refused(Problem::new("no-job", "Nothing to cancel", "")),
                }
            }
            Op::Status | Op::Watch => self.state.lock().unwrap().job.clone().map(Reply::Job).unwrap_or(Reply::NoJob),
            Op::Auto => Reply::Ok { message: "Nothing to do".into() },
        }
    }
}

/// In-process: the preview and the app's tests. `Arc<Demo>` from the caller, so they can
/// look at it.
pub struct DemoBackend(pub Arc<Demo>);

impl Backend for DemoBackend {
    fn ask(&self, req: &Request) -> Result<Reply, String> {
        Ok(self.0.answer(req))
    }

    fn watch(&self, on: &mut dyn FnMut(JobView)) -> Result<(), String> {
        let mut last = None;
        let mut s = self.0.state.lock().unwrap();
        loop {
            if s.job != last {
                last = s.job.clone();
                if let Some(j) = &last {
                    let j = j.clone();
                    let ended = j.phase.ended();
                    drop(s);
                    on(j);
                    if ended {
                        return Ok(());
                    }
                    s = self.0.state.lock().unwrap();
                    continue;
                }
            }
            s = self.0.changed.wait(s).unwrap();
        }
    }
}

/// `mortar-packages fake-service --socket PATH --log FILE`: the same, on a socket, for the
/// headless test. Every request also lands in the log, a line each.
pub fn serve(socket: &Path, log: &Path) -> Result<(), String> {
    let _ = std::fs::remove_file(socket);
    let listener = std::os::unix::net::UnixListener::bind(socket).map_err(|e| format!("{}: {e}", socket.display()))?;
    let demo = Demo::new();
    demo.with(|s| {
        s.timeline = true;
        s.job_ms = std::env::var("MORTAR_PACKAGES_JOB_MS").ok().and_then(|m| m.parse().ok()).unwrap_or(4000);
    });
    let log = log.to_path_buf();
    for conn in listener.incoming() {
        let Ok(conn) = conn else { continue };
        let (demo, log) = (demo.clone(), log.clone());
        std::thread::spawn(move || {
            let mut w = conn.try_clone().expect("clone");
            let mut line = String::new();
            if BufReader::new(conn).read_line(&mut line).is_err() {
                return;
            }
            let Ok(req) = serde_json::from_str::<Request>(&line) else {
                let _ = writeln!(w, "{}", serde_json::to_string(&Reply::Refused(Problem::new("bad-request", "No", ""))).unwrap());
                return;
            };
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log) {
                let _ = f.write_all(line.as_bytes());
            }
            if req.op == Op::Watch {
                let backend = DemoBackend(demo.clone());
                let _ = backend.watch(&mut |j| {
                    let _ = writeln!(w, "{}", serde_json::to_string(&Reply::Job(j)).unwrap());
                });
                return;
            }
            let r = demo.answer(&req);
            let _ = writeln!(w, "{}", serde_json::to_string(&r).unwrap());
        });
    }
    Ok(())
}
