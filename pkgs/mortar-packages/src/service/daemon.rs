// SPDX-License-Identifier: GPL-2.0-or-later
//! What the service does for a request. Installing, in order, stopping at the first
//! refusal with nothing changed:
//!
//! 1. Read the index and check it: signature, channel, device, date.
//! 2. Check the battery and the room, as a system update does.
//! 3. `nix-store --realise`, with that source's URLs and keys on the command line.
//! 4. Check it's what it should be, then `nix-env -p <profile> --set`: the last step,
//!    and atomic. The app appears whole or not at all, and the shell notices.
//!
//! One job at a time. Everything else answers at once.

use super::config::Config;
use super::nix::{self, DryRun};
use super::state::{Installed, Previous, State};
use crate::protocol::*;
use mortar_index::catalog::Catalog;
use mortar_index::compact::{self, Tool};
use mortar_index::repo::{self, read_index};
use mortar_index::{Kind, Package, RepoKind, Repository, fsutil, power, time};
use mortar_update::FetchError;
use std::collections::BTreeMap;
use std::fs;
use std::io::BufReader;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

const APP_MANIFEST: &str = "share/mortar/app.json";

/// What a package comes down to, once its index has been read.
#[derive(Clone, Debug)]
struct Target {
    repo: Repository,
    id: String,
    name: String,
    version: String,
    kind: Kind,
    path: String,
    /// A tool from a large index: only the upstream's signature will do.
    from_compact: bool,
}

impl Target {
    fn sources(&self) -> (Vec<String>, Vec<String>) {
        if self.from_compact { self.repo.tool_sources() } else { self.repo.package_sources() }
    }
}

pub struct Daemon {
    pub cfg: Config,
    state: Mutex<State>,
    /// The job now, or the last one to end.
    job: Mutex<Option<JobView>>,
    changed: Condvar,
    running: AtomicBool,
    cancel: AtomicBool,
    child: AtomicI32,
    /// Connections, and collections in the background: not idle while any is going.
    pub busy: Arc<AtomicUsize>,
    /// Why each repository's newest index wasn't taken, since this service started.
    problems: Mutex<BTreeMap<String, Problem>>,
    /// One realise of a large index at a time.
    tools_lock: Mutex<()>,
}

fn problem(code: &str, headline: &str, detail: impl Into<String>) -> Problem {
    Problem::new(code, headline, detail)
}

fn offline(detail: impl Into<String>) -> Problem {
    problem("offline", "Can't reach it right now", detail)
}

fn human(b: u64) -> String {
    if b >= 1 << 30 {
        format!("{:.1} GB", b as f64 / (1u64 << 30) as f64)
    } else {
        format!("{} MB", b.div_ceil(1 << 20))
    }
}

impl Daemon {
    pub fn new(cfg: Config) -> Arc<Daemon> {
        let mut state = State::load(&cfg.state_file());
        // a profile removed by hand isn't installed any more
        state.installed.retain(|id, i| {
            let dir = if i.kind == Kind::Tool { &cfg.tool_profiles } else { &cfg.app_profiles };
            dir.join(id).symlink_metadata().is_ok()
        });
        Arc::new(Daemon {
            cfg,
            state: Mutex::new(state),
            job: Mutex::new(None),
            changed: Condvar::new(),
            running: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            child: AtomicI32::new(0),
            busy: Arc::new(AtomicUsize::new(0)),
            problems: Mutex::new(BTreeMap::new()),
            tools_lock: Mutex::new(()),
        })
    }

    pub fn job_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    fn save(&self, st: &State) {
        if let Err(e) = st.save(&self.cfg.state_file()) {
            eprintln!("mortar-packages: {}: {e}", self.cfg.state_file().display());
        }
    }

    fn app_profile(&self, id: &str) -> PathBuf {
        self.cfg.app_profiles.join(id)
    }

    fn tool_profile(&self, id: &str) -> PathBuf {
        self.cfg.tool_profiles.join(id)
    }

    fn profile(&self, kind: Kind, id: &str) -> PathBuf {
        if kind == Kind::Tool { self.tool_profile(id) } else { self.app_profile(id) }
    }

    fn device(&self) -> &str {
        self.cfg.device.as_deref().unwrap_or("")
    }

    // ---------------------------------------------------------------- indexes ----

    fn cached(&self, r: &Repository) -> Option<Catalog> {
        let b = fs::read(self.cfg.cached_index(&r.id)).ok()?;
        read_index(&b, r, 0).ok()
    }

    /// Read a repository's index, keep it if it's good, and remember the date. The last
    /// good one stays on disk, whatever comes back.
    fn fetch(&self, r: &Repository) -> Result<Option<Catalog>, Problem> {
        let url = r.index_url();
        let bytes = match mortar_update::get_public(&url, &self.cfg.curl) {
            Ok(b) => b,
            Err(FetchError::NotFound) => {
                let mut st = self.state.lock().unwrap();
                st.checked.insert(r.id.clone(), time::now());
                self.save(&st);
                return Ok(None);
            }
            Err(FetchError::Offline(e)) => return Err(offline(e)),
            Err(e) => return Err(problem("fetch", "Couldn't read the list", e.to_string())),
        };
        let mut st = self.state.lock().unwrap();
        st.checked.insert(r.id.clone(), time::now());
        let seen = st.seen.get(&r.id).max(self.cached(r).map(|c| c.published).unwrap_or(0));
        let c = match read_index(&bytes, r, seen) {
            Ok(c) => c,
            Err(e) => {
                self.save(&st);
                return Err(e.into());
            }
        };
        st.seen.raise(&r.id, c.published);
        self.save(&st);
        drop(st);
        if let Err(e) = fsutil::write_atomic_in(&self.cfg.cached_index(&r.id), &bytes, 0o644, 0o755) {
            eprintln!("mortar-packages: {}: {e}", self.cfg.cached_index(&r.id).display());
        }
        Ok(Some(c))
    }

    fn refresh(&self, r: &Repository) {
        let got = self.fetch(r);
        let mut problems = self.problems.lock().unwrap();
        match got {
            Ok(_) => {
                problems.remove(&r.id);
            }
            Err(p) => {
                eprintln!("mortar-packages: {}: {}: {}", r.id, p.headline, p.detail);
                problems.insert(r.id.clone(), p);
            }
        }
    }

    // --------------------------------------------------------------- overview ----

    /// Who holds a profile, if anyone: this service knows what it installed.
    fn row_state(&self, st: &State, r: &Repository, p: &Package, path: Option<&str>) -> (RowState, Option<String>, bool) {
        if p.kind == Kind::Bundle {
            return (RowState::NotYet, None, false);
        }
        if let Some(i) = st.installed.get(&p.id) {
            if i.repo != r.id {
                return (RowState::Elsewhere, Some(i.version.clone()), false);
            }
            let update = path.is_some_and(|b| b != i.path);
            return (if update { RowState::Update } else { RowState::Installed }, Some(i.version.clone()), i.previous.is_some());
        }
        if self.profile(p.kind, &p.id).symlink_metadata().is_ok() {
            return (RowState::Elsewhere, None, false);
        }
        (RowState::Available, None, false)
    }

    fn row(&self, st: &State, r: &Repository, published: u64, p: &Package) -> Row {
        let build = p.build_for(self.device(), &self.cfg.system);
        let (state, installed_version, can_roll_back) = self.row_state(st, r, p, build.map(|b| b.path.as_str()));
        Row {
            repo: r.id.clone(),
            repo_name: r.name.clone(),
            official: r.official,
            id: p.id.clone(),
            name: p.name.clone(),
            description: p.description.clone(),
            version: p.version.clone(),
            category: p.category.clone(),
            icon: p.icon.clone(),
            licence: p.licence.clone(),
            source: p.source.clone(),
            kind: p.kind,
            requires: p.requires.clone(),
            program: p.program.clone(),
            published,
            state,
            installed_version,
            can_roll_back,
        }
    }

    fn tool_row(&self, st: &State, r: &Repository, published: u64, t: &Tool) -> Row {
        let path = t.path();
        let (state, installed_version, can_roll_back) = match st.installed.get(&t.attr) {
            Some(i) if i.repo != r.id => (RowState::Elsewhere, Some(i.version.clone()), false),
            Some(i) => (if i.path != path { RowState::Update } else { RowState::Installed }, Some(i.version.clone()), i.previous.is_some()),
            None if self.tool_profile(&t.attr).symlink_metadata().is_ok() => (RowState::Elsewhere, None, false),
            None => (RowState::Available, None, false),
        };
        Row {
            repo: r.id.clone(),
            repo_name: r.name.clone(),
            official: r.official,
            id: t.attr.clone(),
            name: t.title().to_string(),
            description: t.description.clone(),
            version: t.version().to_string(),
            category: "tools".into(),
            icon: "term".into(),
            kind: Kind::Tool,
            program: (!t.program.is_empty()).then(|| t.program.clone()),
            published,
            state,
            installed_version,
            can_roll_back,
            ..Row::default()
        }
    }

    pub fn overview(&self, refresh: bool) -> Overview {
        if refresh {
            for r in self.nix_repos() {
                self.refresh(r);
            }
        }
        let problems = self.problems.lock().unwrap().clone();
        let st = self.state.lock().unwrap();
        let mut repos = Vec::new();
        let mut rows = Vec::new();
        for r in self.nix_repos() {
            let c = self.cached(r);
            let mut view = RepoView { id: r.id.clone(), name: r.name.clone(), official: r.official, checked: st.checked.get(&r.id).copied().unwrap_or(0), problem: problems.get(&r.id).cloned(), ..Default::default() };
            if let Some(c) = &c {
                view.published = c.published;
                view.tools = c.tools_for(&self.cfg.system).map(|t| t.count).unwrap_or(0);
                for p in &c.packages {
                    // a build for another handheld is never listed; a bundle has no builds yet
                    if p.kind == Kind::Bundle || p.build_for(self.device(), &self.cfg.system).is_some() {
                        rows.push(self.row(&st, r, c.published, p));
                    }
                }
            }
            repos.push(view);
        }
        // Installed, and not (any longer) in what the index says: still removable.
        for (id, i) in &st.installed {
            if !rows.iter().any(|x: &Row| x.id == *id && x.repo == i.repo) {
                rows.push(Row {
                    repo: i.repo.clone(),
                    repo_name: self.cfg.repo(&i.repo).map(|r| r.name.clone()).unwrap_or_else(|| i.repo.clone()),
                    official: self.cfg.repo(&i.repo).is_some_and(|r| r.official),
                    id: id.clone(),
                    name: i.name.clone(),
                    version: i.version.clone(),
                    category: if i.kind == Kind::Tool { "tools".into() } else { "apps".into() },
                    icon: if i.kind == Kind::Tool { "term".into() } else { String::new() },
                    kind: i.kind,
                    state: RowState::Installed,
                    installed_version: Some(i.version.clone()),
                    can_roll_back: i.previous.is_some(),
                    ..Row::default()
                });
            }
        }
        rows.sort_by_cached_key(|r| (r.state != RowState::Update, r.name.to_lowercase(), r.id.clone()));
        Overview { repos, rows, job: self.job.lock().unwrap().clone() }
    }

    // ------------------------------------------------------------- large index ----

    /// The large index's file, fetched and kept if it's a newer one. If the new one can't
    /// be had, the one here will do.
    fn tools_file(&self, r: &Repository, c: &Catalog) -> Result<Option<PathBuf>, Problem> {
        let Some(want) = c.tools_for(&self.cfg.system) else { return Ok(None) };
        let _one = self.tools_lock.lock().unwrap();
        let root = self.cfg.tools_root(&r.id);
        let have = nix::resolve(&root);
        if have.as_deref() != Some(want.path.as_str()) {
            let (subs, keys) = r.own_sources();
            // A list and nothing else. With references it would bring every package it
            // names, thousands of them, and no one asked for that.
            if let Ok(DryRun::Fetch(s)) = nix::dry_run(&self.cfg.nix_store, &want.path, &subs, &keys)
                && s.paths > 1
            {
                return Err(problem("bad-index", "This list of tools can't be used", format!("It would bring {} other packages with it.", s.paths - 1)));
            }
            let _ = fs::create_dir_all(&self.cfg.gc_roots);
            let idle = AtomicI32::new(0);
            if let Err((off, e)) = nix::realise(&self.cfg.nix_store, &want.path, &root, &subs, &keys, &idle, |_| {}) {
                if have.is_none() {
                    return Err(if off { offline(e) } else { problem("fetch", "Couldn't get the list of tools", e) });
                }
                eprintln!("mortar-packages: keeping the tools index here: {e}");
            }
        }
        let dir = nix::resolve(&root).ok_or_else(|| problem("fetch", "Couldn't get the list of tools", "It didn't arrive."))?;
        Ok(Some(PathBuf::from(dir).join(compact::FILE)))
    }

    fn open_tools(file: &Path) -> Result<BufReader<fs::File>, Problem> {
        use std::io::BufRead;
        let f = fs::File::open(file).map_err(|e| problem("fetch", "Couldn't read the list of tools", e.to_string()))?;
        let mut r = BufReader::with_capacity(64 * 1024, f);
        let mut first = String::new();
        r.read_line(&mut first).map_err(|e| problem("fetch", "Couldn't read the list of tools", e.to_string()))?;
        match compact::check_header(&first) {
            Ok(()) => Ok(r),
            Err(n) => Err(mortar_index::Refusal::NewerFormat(n).into()),
        }
    }

    /// Search each repository's large index, and its curated tools.
    pub fn search(&self, query: &str) -> Result<Found, Problem> {
        const LIMIT: usize = 50;
        let query = query.chars().take(64).collect::<String>();
        let low = query.to_lowercase();
        let mut found = Found::default();
        for r in self.nix_repos() {
            let Some(c) = self.cached(r) else { continue };
            {
                let st = self.state.lock().unwrap();
                for p in c.packages.iter().filter(|p| p.kind == Kind::Tool && p.build_for(self.device(), &self.cfg.system).is_some()) {
                    if low.split_whitespace().all(|w| p.id.to_lowercase().contains(w) || p.name.to_lowercase().contains(w) || p.description.to_lowercase().contains(w)) {
                        found.rows.push(self.row(&st, r, c.published, p));
                        found.total += 1;
                    }
                }
            }
            let Some(file) = self.tools_file(r, &c)? else { continue };
            let hits = compact::search(Self::open_tools(&file)?, &query, LIMIT).map_err(|e| problem("fetch", "Couldn't read the list of tools", e.to_string()))?;
            found.searched += c.tools_for(&self.cfg.system).map(|t| t.count).unwrap_or(0);
            found.total += hits.total;
            let st = self.state.lock().unwrap();
            found.rows.extend(hits.hits.iter().map(|t| self.tool_row(&st, r, c.published, t)));
        }
        found.rows.truncate(LIMIT);
        Ok(found)
    }

    // ---------------------------------------------------------------- a target ----

    fn repo_of(&self, repo: Option<&str>) -> Result<&Repository, Problem> {
        let repo = repo.ok_or_else(|| problem("bad-request", "Which repository?", ""))?;
        self.nix_repos().find(|r| r.id == repo).ok_or_else(|| problem("no-repository", "No such repository", format!("'{repo}' isn't configured here.")))
    }

    /// What's read here. A bundle repository has a runtime to wait for.
    fn nix_repos(&self) -> impl Iterator<Item = &Repository> {
        self.cfg.repositories.iter().filter(|r| r.kind == RepoKind::Nix)
    }

    fn target(&self, r: &Repository, c: &Catalog, id: &str) -> Result<Target, Problem> {
        if let Some(p) = c.package(id) {
            if p.kind == Kind::Bundle {
                return Err(problem("not-yet", "Not installable yet", "This version of Mortar can't install bundles."));
            }
            let b = repo::pick_build(p, self.device(), &self.cfg.system)?;
            return Ok(Target { repo: r.clone(), id: p.id.clone(), name: p.name.clone(), version: p.version.clone(), kind: p.kind, path: b.path.clone(), from_compact: false });
        }
        if let Some(file) = self.tools_file(r, c)? {
            let t = compact::find(Self::open_tools(&file)?, id).map_err(|e| problem("fetch", "Couldn't read the list of tools", e.to_string()))?;
            if let Some(t) = t {
                return Ok(Target { repo: r.clone(), id: t.attr.clone(), name: t.title().to_string(), version: t.version().to_string(), kind: Kind::Tool, path: t.path(), from_compact: true });
            }
        }
        Err(problem("no-package", "It's not listed", format!("{} doesn't list '{id}'.", r.name)))
    }

    /// What a dry run says this device is missing, and what's left to go on.
    pub fn details(&self, repo: Option<&str>, id: Option<&str>) -> Result<Details, Problem> {
        let r = self.repo_of(repo)?;
        let id = id.ok_or_else(|| problem("bad-request", "Which package?", ""))?;
        let c = self.cached(r).ok_or_else(|| problem("no-index", "Nothing read from it yet", ""))?;
        let t = self.target(r, &c, id)?;
        let pw = power::read_power(&self.cfg.power_supply_dir);
        let mut d = Details {
            repo: r.id.clone(),
            id: id.to_string(),
            free: fsutil::free_bytes(Path::new("/nix/store")).unwrap_or(0),
            battery: pw.capacity,
            charging: pw.charging,
            power_ok: pw.ok_for_update(self.cfg.min_battery),
            ..Default::default()
        };
        let (subs, keys) = t.sources();
        match nix::dry_run(&self.cfg.nix_store, &t.path, &subs, &keys) {
            Ok(DryRun::Fetch(s)) => {
                d.need = s.unpacked + self.cfg.min_free_mib * 1048576;
                d.size = Some(s);
            }
            Ok(DryRun::Unavailable(e)) => d.problem = Some(problem("unavailable", "Not available", e)),
            Err((true, e)) => d.problem = Some(offline(e)),
            Err((false, e)) => d.problem = Some(problem("unavailable", "Not available", e)),
        }
        Ok(d)
    }

    // -------------------------------------------------------------------- jobs ----

    fn set_job(&self, f: impl FnOnce(&mut JobView)) {
        let mut j = self.job.lock().unwrap();
        if let Some(v) = j.as_mut() {
            f(v);
        }
        self.changed.notify_all();
    }

    /// The job now.
    pub fn job(&self) -> Option<JobView> {
        self.job.lock().unwrap().clone()
    }

    /// Wait until the job isn't `last` any more.
    pub fn job_after(&self, last: &Option<JobView>) -> Option<JobView> {
        let mut j = self.job.lock().unwrap();
        while *j == *last && !j.as_ref().is_none_or(|v| v.phase.ended()) {
            j = self.changed.wait(j).unwrap();
        }
        j.clone()
    }

    /// Start an install or an update on the worker. Refusals that don't need the index
    /// answer here; the rest show up in the job.
    pub fn start(self: &Arc<Daemon>, action: Action, repo: Option<&str>, id: Option<&str>) -> Result<JobView, Problem> {
        let r = self.repo_of(repo)?.clone();
        let id = id.ok_or_else(|| problem("bad-request", "Which package?", ""))?.to_string();
        if self.running.swap(true, Ordering::SeqCst) {
            return Err(problem("busy", "One thing at a time", "Something else is being installed. Try again when it's done."));
        }
        self.cancel.store(false, Ordering::SeqCst);
        let name = self.cached(&r).and_then(|c| c.package(&id).map(|p| p.name.clone())).unwrap_or_else(|| id.clone());
        let view = JobView { repo: r.id.clone(), id: id.clone(), name, action, ..Default::default() };
        *self.job.lock().unwrap() = Some(view.clone());
        let d = self.clone();
        std::thread::spawn(move || {
            let result = d.install(action, &r, &id);
            d.finish(result);
            d.running.store(false, Ordering::SeqCst);
        });
        Ok(view)
    }

    fn finish(&self, result: Result<(), Problem>) {
        let cancelled = self.cancel.load(Ordering::SeqCst);
        self.set_job(|v| {
            v.cancelling = false;
            match result {
                Ok(()) => v.phase = Phase::Done,
                Err(_) if cancelled => v.phase = Phase::Cancelled,
                Err(p) => {
                    eprintln!("mortar-packages: {}: {}: {}", v.id, p.headline, p.detail);
                    v.phase = Phase::Failed;
                    v.problem = Some(p);
                }
            }
        });
    }

    pub fn cancel(&self) -> Result<String, Problem> {
        if !self.job_running() {
            return Err(problem("no-job", "Nothing to cancel", ""));
        }
        if self.job().is_some_and(|j| j.phase == Phase::Installing) {
            return Err(problem("too-late", "It's nearly done", "It's being put in place, and that can't be half-done."));
        }
        self.cancel.store(true, Ordering::SeqCst);
        let pid = self.child.load(Ordering::SeqCst);
        if pid > 0 {
            // SAFETY: only signals a process this service started, while it runs
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
        }
        self.set_job(|v| v.cancelling = true);
        Ok("Stopping".into())
    }

    fn check_cancelled(&self) -> Result<(), Problem> {
        if self.cancel.load(Ordering::SeqCst) { Err(problem("cancelled", "Stopped", "")) } else { Ok(()) }
    }

    fn install(&self, action: Action, r: &Repository, id: &str) -> Result<(), Problem> {
        // 1. The index, read now, for this device
        let c = match self.fetch(r) {
            Ok(Some(c)) => {
                self.problems.lock().unwrap().remove(&r.id);
                c
            }
            Ok(None) => return Err(problem("no-index", "Nothing published", format!("{} has no list here yet.", r.name))),
            Err(p) => {
                if p.code != "offline" {
                    self.problems.lock().unwrap().insert(r.id.clone(), p.clone());
                }
                return Err(p);
            }
        };
        let t = self.target(r, &c, id)?;
        self.set_job(|v| v.name = t.name.clone());
        self.install_target(action, &t)
    }

    fn install_target(&self, action: Action, t: &Target) -> Result<(), Problem> {
        let id = t.id.as_str();
        let profile = self.profile(t.kind, id);
        let was = self.state.lock().unwrap().installed.get(id).cloned();
        match &was {
            Some(i) if i.repo != t.repo.id => {
                return Err(problem("elsewhere", "Already here from elsewhere", format!("{} came from {}. Remove it first.", i.name, i.repo)));
            }
            None if profile.symlink_metadata().is_ok() => {
                return Err(problem("elsewhere", "Already here from elsewhere", format!("{} was put here from a computer, and stays as it is.", t.name)));
            }
            None if action == Action::Update => return Err(problem("not-installed", "It isn't installed", "")),
            _ => {}
        }
        if nix::resolve(&profile).as_deref() == Some(t.path.as_str()) {
            return Ok(());
        }
        self.check_cancelled()?;

        // 2. Charge and room, as a system update
        let pw = power::read_power(&self.cfg.power_supply_dir);
        if !pw.ok_for_update(self.cfg.min_battery) {
            return Err(problem("low-battery", &format!("Charge to {}% first", self.cfg.min_battery), format!("The battery is at {}% and not charging.", pw.capacity.unwrap_or(0))));
        }
        let (subs, keys) = t.sources();
        if subs.is_empty() {
            return Err(problem("unavailable", "Not available", "This repository names no cache to get its tools from."));
        }
        let size = match nix::dry_run(&self.cfg.nix_store, &t.path, &subs, &keys) {
            Ok(DryRun::Fetch(s)) => s,
            Ok(DryRun::Unavailable(e)) => return Err(problem("unsigned", "Not available", e)),
            Err((true, e)) => return Err(offline(e)),
            Err((false, e)) => return Err(problem("unavailable", "Not available", e)),
        };
        self.ensure_room(size.unpacked)?;
        self.check_cancelled()?;

        // 3. Download
        self.set_job(|v| {
            v.phase = Phase::Downloading;
            v.total = size.unpacked;
        });
        fs::create_dir_all(&self.cfg.gc_roots).map_err(|e| problem("io", "Couldn't get ready", e.to_string()))?;
        let root = self.cfg.install_root(id);
        let got = nix::realise(&self.cfg.nix_store, &t.path, &root, &subs, &keys, &self.child, |n| {
            self.set_job(|v| v.done = n.min(v.total.max(n)));
        });
        // whatever happened, the profile will hold on to it from here
        let drop_root = || {
            let _ = fs::remove_file(&root);
        };
        if let Err((off, e)) = got {
            drop_root();
            self.check_cancelled()?;
            if e.starts_with("not signed") {
                return Err(problem("unsigned", "It isn't signed as it should be", format!("{} refused it: {e}.", t.repo.name)));
            }
            return Err(if off { offline(e) } else { problem("download", "The download failed", e) });
        }
        if let Err(e) = self.check_cancelled() {
            drop_root();
            return Err(e);
        }

        // 4. Put it in place, the one step that shows
        self.set_job(|v| {
            v.phase = Phase::Installing;
            v.done = v.total;
        });
        if let Err(e) = self.check_contents(t) {
            drop_root();
            return Err(e);
        }
        let parent = profile.parent().expect("profiles are in a folder");
        fs::create_dir_all(parent).map_err(|e| problem("io", "Couldn't get ready", e.to_string()))?;
        let _ = fs::set_permissions(parent, std::os::unix::fs::PermissionsExt::from_mode(0o755));
        let previous = was.as_ref().map(|i| Previous { version: i.version.clone(), path: i.path.clone() });
        let old_gens = nix::generations(&profile).len();
        let set = nix::set_profile(&self.cfg.nix_env, &profile, &t.path);
        drop_root();
        set.map_err(|e| problem("install", "Couldn't put it in place", e))?;
        // a profile keeps this version and the last one, for one step back
        if let Err(e) = nix::prune(&self.cfg.nix_env, &profile) {
            eprintln!("mortar-packages: {e}");
        }
        let links = if t.kind == Kind::Tool { self.link_tool(t, was.as_ref()) } else { Vec::new() };
        let mut st = self.state.lock().unwrap();
        st.installed.insert(
            id.to_string(),
            Installed { repo: t.repo.id.clone(), kind: t.kind, name: t.name.clone(), version: t.version.clone(), path: t.path.clone(), previous, rolled_back_from: None, links },
        );
        self.save(&st);
        drop(st);
        if old_gens >= 2 {
            self.collect_in_background();
        }
        Ok(())
    }

    /// Battery aside, what a system update asks: enough room after the download, with
    /// unreferenced paths collected first if that's what it takes.
    fn ensure_room(&self, unpacked: u64) -> Result<(), Problem> {
        let want = unpacked + self.cfg.min_free_mib * 1048576;
        let store = Path::new("/nix/store");
        let free = || fsutil::free_bytes(store).unwrap_or(u64::MAX);
        if free() >= want {
            return Ok(());
        }
        eprintln!("mortar-packages: {} free, collecting garbage", human(free()));
        let _ = std::process::Command::new(&self.cfg.nix_collect_garbage).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).status();
        if free() >= want {
            return Ok(());
        }
        Err(problem("no-room", "Not enough room", format!("It needs {} and {} is free. Removing things you don't use makes room.", human(want), human(free()))))
    }

    /// A catalog can say anything of a path: what arrived has to be what the kind needs,
    /// or nothing is touched.
    fn check_contents(&self, t: &Target) -> Result<(), Problem> {
        let p = Path::new(&t.path);
        match t.kind {
            Kind::Tool => {
                if !p.join("bin").is_dir() {
                    return Err(problem("bad-package", "Not a terminal tool", format!("{} has no commands.", t.name)));
                }
            }
            _ => {
                let bad = |d: String| Err(problem("bad-package", "Not an app", d));
                let text = match fs::read_to_string(p.join(APP_MANIFEST)) {
                    Ok(t) => t,
                    Err(_) => return bad(format!("{} has no manifest.", t.name)),
                };
                let v: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(v) => v,
                    Err(e) => return bad(format!("Its manifest doesn't read: {e}")),
                };
                if v.get("id").and_then(|i| i.as_str()) != Some(t.id.as_str()) {
                    return bad(format!("Its manifest doesn't say it's {}.", t.id));
                }
            }
        }
        Ok(())
    }

    /// Each of a tool's commands on Terminal's path, through its profile so an update
    /// needs no change here. A command another tool already has stays that tool's.
    fn link_tool(&self, t: &Target, was: Option<&Installed>) -> Vec<String> {
        let bin = &self.cfg.tools_bin;
        if let Err(e) = fs::create_dir_all(bin) {
            eprintln!("mortar-packages: {}: {e}", bin.display());
            return vec![];
        }
        let _ = fs::set_permissions(bin, std::os::unix::fs::PermissionsExt::from_mode(0o755));
        let mut links = Vec::new();
        let Ok(rd) = fs::read_dir(Path::new(&t.path).join("bin")) else { return vec![] };
        let profile = self.tool_profile(&t.id);
        for e in rd.flatten() {
            let Ok(name) = e.file_name().into_string() else { continue };
            let link = bin.join(&name);
            let mine = was.is_some_and(|i| i.links.contains(&name));
            if link.symlink_metadata().is_ok() {
                if !mine {
                    eprintln!("mortar-packages: {name} already belongs to another tool");
                    continue;
                }
            } else if let Err(e) = symlink(profile.join("bin").join(&name), &link) {
                eprintln!("mortar-packages: {}: {e}", link.display());
                continue;
            }
            links.push(name);
        }
        // a command the new version dropped
        if let Some(i) = was {
            for gone in i.links.iter().filter(|l| !links.contains(l)) {
                let _ = fs::remove_file(bin.join(gone));
            }
        }
        links.sort();
        links
    }

    fn unlink_tool(&self, i: &Installed) {
        for l in &i.links {
            let _ = fs::remove_file(self.cfg.tools_bin.join(l));
        }
    }

    /// The profile goes at once. A running app keeps running; its data was never in here.
    pub fn remove(self: &Arc<Daemon>, repo: Option<&str>, id: Option<&str>) -> Result<String, Problem> {
        let r = self.repo_of(repo)?;
        let id = id.ok_or_else(|| problem("bad-request", "Which package?", ""))?;
        self.refuse_while_running(id)?;
        let mut st = self.state.lock().unwrap();
        let i = st.installed.get(id).cloned().ok_or_else(|| problem("not-installed", "It isn't installed", "Or it was put here from a computer, and stays as it is."))?;
        if i.repo != r.id {
            return Err(problem("elsewhere", "From elsewhere", format!("{} came from {}.", i.name, i.repo)));
        }
        nix::remove_profile(&self.profile(i.kind, id)).map_err(|e| problem("io", "Couldn't remove it", e.to_string()))?;
        self.unlink_tool(&i);
        st.installed.remove(id);
        self.save(&st);
        drop(st);
        self.collect_in_background();
        Ok(format!("{} is gone", i.name))
    }

    /// One step back: the previous generation, and the one left is forgotten. The daily
    /// check won't bring it back; asking still will.
    pub fn rollback(self: &Arc<Daemon>, repo: Option<&str>, id: Option<&str>) -> Result<String, Problem> {
        let r = self.repo_of(repo)?;
        let id = id.ok_or_else(|| problem("bad-request", "Which package?", ""))?;
        self.refuse_while_running(id)?;
        let mut st = self.state.lock().unwrap();
        let Some(i) = st.installed.get_mut(id) else { return Err(problem("not-installed", "It isn't installed", "")) };
        if i.repo != r.id {
            return Err(problem("elsewhere", "From elsewhere", format!("{} came from {}.", i.name, i.repo)));
        }
        let Some(prev) = i.previous.clone() else { return Err(problem("no-previous", "Nothing to go back to", "There's only this version here.")) };
        let profile = if i.kind == Kind::Tool { self.cfg.tool_profiles.join(id) } else { self.cfg.app_profiles.join(id) };
        nix::rollback(&self.cfg.nix_env, &profile).map_err(|e| problem("install", "Couldn't go back", e))?;
        i.rolled_back_from = Some(std::mem::replace(&mut i.path, prev.path));
        i.version = prev.version.clone();
        i.previous = None;
        let name = i.name.clone();
        self.save(&st);
        drop(st);
        self.collect_in_background();
        Ok(format!("{name} is back to {}", prev.version))
    }

    fn refuse_while_running(&self, id: &str) -> Result<(), Problem> {
        if self.job_running() && self.job().is_some_and(|j| j.id == id) {
            return Err(problem("busy", "One thing at a time", "It's still being installed."));
        }
        Ok(())
    }

    /// `nix-collect-garbage`, while the service stays up for it. It takes a while on a
    /// card and nothing waits for it: a running app's paths are live, and a download in
    /// progress holds a root.
    fn collect_in_background(&self) {
        let gc = self.cfg.nix_collect_garbage.clone();
        self.busy.fetch_add(1, Ordering::SeqCst);
        let busy = self.busy.clone();
        std::thread::spawn(move || {
            let _ = std::process::Command::new(gc)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            busy.fetch_sub(1, Ordering::SeqCst);
        });
    }

    // ------------------------------------------------------------- daily check ----

    /// The daily check, when Auto-check is on: official repositories' packages that have
    /// a newer build, unless they were rolled back from it. Others are only offered. It
    /// never interrupts anything: a running app keeps what it started with.
    pub fn auto(self: &Arc<Daemon>) -> Result<String, Problem> {
        if !self.cfg.auto_check_now() {
            return Ok("Auto-check is off".into());
        }
        if self.running.swap(true, Ordering::SeqCst) {
            // tomorrow will do
            return Ok("Busy".into());
        }
        let mut updated = 0;
        for r in self.nix_repos().filter(|r| r.official) {
            self.refresh(r);
            let Some(c) = self.cached(r) else { continue };
            let installed: Vec<(String, Installed)> = self.state.lock().unwrap().installed.iter().filter(|(_, i)| i.repo == r.id).map(|(k, v)| (k.clone(), v.clone())).collect();
            for (id, i) in installed {
                let t = match self.target(r, &c, &id) {
                    Ok(t) => t,
                    Err(p) => {
                        eprintln!("mortar-packages: {id}: {}", p.headline);
                        continue;
                    }
                };
                if t.path == i.path || i.rolled_back_from.as_deref() == Some(t.path.as_str()) {
                    continue;
                }
                *self.job.lock().unwrap() = Some(JobView { repo: r.id.clone(), id: id.clone(), name: t.name.clone(), action: Action::Update, ..Default::default() });
                self.cancel.store(false, Ordering::SeqCst);
                let result = self.install_target(Action::Update, &t);
                if result.is_ok() {
                    updated += 1;
                }
                self.finish(result);
            }
        }
        self.running.store(false, Ordering::SeqCst);
        Ok(format!("{updated} updated"))
    }
}
