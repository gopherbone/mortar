// SPDX-License-Identifier: GPL-2.0-or-later
//! The app's state and what the buttons do. No drawing (view.rs), no window
//! (mortar_files::wl): as in Files, `App` takes key events and a clock, and says when
//! it wants a frame. It changes nothing itself: everything goes to the service as a
//! request naming a repository and an ID.

use crate::backend::Backend;
use crate::protocol::*;
use mortar_files::app::{Field, Slide, SlideDir};
use mortar_files::bg::Wake;
use mortar_files::keys::{self, Decoder, Input};
use mortar_files::shell::Shell;
use mortar_index::Kind;
use mortar_ui::input::Button;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

pub const SCROLL_MS: u64 = 110;
pub const SLIDE_MS: u64 = 190;
pub const TOAST_MS: u64 = 2800;
/// Wait this long after a letter before asking the service to search.
const SEARCH_DEBOUNCE_MS: u64 = 250;
/// Rows a page moves.
const PAGE: usize = 6;
/// The on-screen keyboard is up when the window is shorter than this.
const SHORT: i32 = 640;
/// An index read longer ago than this is read again when the app opens.
const STALE_SECS: u64 = 6 * 3600;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Home,
    Tools,
    Details,
    Progress,
}

/// A line of the Home list.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    Package(Row),
    /// The way into Terminal tools, after the apps.
    Tools { count: u64, installed: usize },
}

/// A list that rolls under the band.
#[derive(Clone, Debug, Default)]
pub struct List {
    pub sel: usize,
    pub scroll_from: f32,
    pub scroll_start: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmKind {
    Remove,
    Rollback,
}

pub struct Confirm {
    pub kind: ConfirmKind,
    pub row: Row,
    /// A or some other button got pressed: say which one's needed.
    pub nudge: bool,
}

pub enum Overlay {
    Confirm(Confirm),
    Report(Problem),
    Find(Field),
}

/// What a worker sends.
pub enum Bg {
    Overview(Result<Overview, String>),
    Details { key: (String, String), result: Result<Details, Problem> },
    Found { generation: u64, result: Result<Found, Problem> },
    Job(JobView),
    /// A request that came back: what it said. Then the list is read again.
    Answer { message: Result<String, Problem>, quiet: bool },
    Started(Result<JobView, Problem>),
}

pub struct Setup {
    pub backend: Arc<dyn Backend>,
    pub shell: Shell,
    /// Unix time, for "3 days ago".
    pub now_secs: u64,
    pub home: PathBuf,
    pub reduce_motion: bool,
    pub wake: Wake,
}

pub struct App {
    backend: Arc<dyn Backend>,
    pub shell: Shell,
    wake: Wake,
    tx: Sender<Bg>,
    rx: Receiver<Bg>,
    pub now: u64,
    pub now_secs: u64,
    pub home: PathBuf,
    pub reduce_motion: bool,
    pub dirty: bool,
    pub done: bool,
    decoder: Decoder,
    focused: bool,
    pub short: bool,
    keyboard_ours: bool,

    pub screen: Screen,
    /// Where Details and Progress go back to.
    pub back_to: Screen,
    pub overlay: Option<Overlay>,
    pub toast: Option<(String, u64)>,
    pub slide_req: Option<SlideDir>,
    pub slide: Option<Slide>,

    pub overview: Overview,
    /// The service answered at least once.
    pub loaded: bool,
    /// Couldn't reach the service: why.
    pub unreachable: Option<String>,
    /// Indexes are being read again.
    pub checking: bool,
    pub entries: Vec<Entry>,
    pub home_list: List,

    pub tool_rows: Vec<Row>,
    pub tools_list: List,
    /// What was searched for; empty: the tools here and the curated ones.
    pub query: String,
    pub searching: bool,
    pub found_total: u64,
    pub searched: u64,
    search_due: Option<u64>,
    search_generation: u64,

    /// The package on the Details and Progress screens.
    pub row: Option<Row>,
    pub details: Option<Details>,
    /// A dry run is on its way.
    pub sizing: bool,

    pub job: Option<JobView>,
    watching: bool,
    /// Asked for, and the service hasn't answered yet.
    starting: bool,
    /// Its end has been said, so an overview saying so again says nothing.
    job_reported: bool,
}

impl App {
    pub fn new(s: Setup) -> App {
        let (tx, rx) = channel();
        let mut app = App {
            backend: s.backend,
            shell: s.shell,
            wake: s.wake,
            tx,
            rx,
            now: 0,
            now_secs: s.now_secs,
            home: s.home,
            reduce_motion: s.reduce_motion,
            dirty: true,
            done: false,
            decoder: Decoder::default(),
            focused: true,
            short: false,
            keyboard_ours: false,
            screen: Screen::Home,
            back_to: Screen::Home,
            overlay: None,
            toast: None,
            slide_req: None,
            slide: None,
            overview: Overview::default(),
            loaded: false,
            unreachable: None,
            checking: false,
            entries: Vec::new(),
            home_list: List::default(),
            tool_rows: Vec::new(),
            tools_list: List::default(),
            query: String::new(),
            searching: false,
            found_total: 0,
            searched: 0,
            search_due: None,
            search_generation: 0,
            row: None,
            details: None,
            sizing: false,
            job: None,
            watching: false,
            starting: false,
            job_reported: true,
        };
        app.read_overview(false);
        app
    }

    // ------------------------------------------------------------- the service ----

    /// In a thread, so the window never waits. The answer wakes the loop.
    fn in_background(&self, f: impl FnOnce(&dyn Backend) -> Bg + Send + 'static) {
        let (b, tx, wake) = (self.backend.clone(), self.tx.clone(), self.wake.clone());
        std::thread::spawn(move || {
            let _ = tx.send(f(&*b));
            wake();
        });
    }

    fn read_overview(&mut self, refresh: bool) {
        if refresh {
            self.checking = true;
            self.dirty = true;
        }
        self.in_background(move |b| {
            let req = Request { refresh, ..Request::new(Op::Overview) };
            Bg::Overview(match b.ask(&req) {
                Ok(Reply::Overview(o)) => Ok(o),
                Ok(other) => Err(format!("{other:?}")),
                Err(e) => Err(e),
            })
        });
    }

    fn read_details(&mut self, row: &Row) {
        self.sizing = true;
        let key = (row.repo.clone(), row.id.clone());
        self.in_background(move |b| {
            let result = match b.ask(&Request::package(Op::Details, &key.0, &key.1)) {
                Ok(Reply::Details(d)) => Ok(d),
                Ok(Reply::Refused(p)) => Err(p),
                Ok(o) => Err(Problem::new("service", "That didn't work", format!("{o:?}"))),
                Err(e) => Err(unreachable_problem(e)),
            };
            Bg::Details { key, result }
        });
    }

    fn start_job(&mut self, op: Op, row: &Row) {
        let (repo, id) = (row.repo.clone(), row.id.clone());
        self.in_background(move |b| {
            Bg::Started(match b.ask(&Request::package(op, &repo, &id)) {
                Ok(Reply::Job(j)) => Ok(j),
                Ok(Reply::Refused(p)) => Err(p),
                Ok(o) => Err(Problem::new("service", "That didn't work", format!("{o:?}"))),
                Err(e) => Err(unreachable_problem(e)),
            })
        });
    }

    fn send_simple(&mut self, op: Op, row: Option<&Row>) {
        let req = match row {
            Some(r) => Request::package(op, &r.repo, &r.id),
            None => Request::new(op),
        };
        // the screen already says "Stopping"
        let quiet = op == Op::Cancel;
        self.in_background(move |b| {
            Bg::Answer {
                quiet,
                message: match b.ask(&req) {
                    Ok(Reply::Ok { message }) => Ok(message),
                    Ok(Reply::Refused(p)) => Err(p),
                    Ok(o) => Err(Problem::new("service", "That didn't work", format!("{o:?}"))),
                    Err(e) => Err(unreachable_problem(e)),
                },
            }
        });
    }

    /// One connection for as long as a job runs: progress wakes the app, at most five
    /// times a second, only then.
    fn watch_job(&mut self) {
        if self.watching {
            return;
        }
        self.watching = true;
        let (b, tx, wake) = (self.backend.clone(), self.tx.clone(), self.wake.clone());
        std::thread::spawn(move || {
            let w = wake.clone();
            let t = tx.clone();
            let r = b.watch(&mut |j| {
                let _ = t.send(Bg::Job(j));
                w();
            });
            if let Err(e) = r {
                eprintln!("mortar-packages: {e}");
            }
            // whether it ended or the connection went: look again
            let _ = tx.send(Bg::Job(JobView { phase: Phase::Done, id: String::new(), ..JobView::default() }));
            wake();
        });
    }

    /// What workers have sent. True if anything arrived.
    pub fn poll(&mut self) -> bool {
        let mut any = false;
        while let Ok(m) = self.rx.try_recv() {
            any = true;
            self.dirty = true;
            match m {
                Bg::Overview(Ok(o)) => self.take_overview(o),
                Bg::Overview(Err(e)) => {
                    self.checking = false;
                    self.loaded = true;
                    self.unreachable = Some(e);
                }
                Bg::Details { key, result } => {
                    if self.row.as_ref().is_some_and(|r| (r.repo.as_str(), r.id.as_str()) == (key.0.as_str(), key.1.as_str())) {
                        self.sizing = false;
                        match result {
                            Ok(d) => self.details = Some(d),
                            Err(p) => {
                                self.details = Some(Details { repo: key.0, id: key.1, problem: Some(p), ..Details::default() });
                            }
                        }
                    }
                }
                Bg::Found { generation, result } => {
                    if generation == self.search_generation {
                        self.searching = false;
                        match result {
                            Ok(f) => self.take_found(f),
                            Err(p) => self.report(p),
                        }
                    }
                }
                Bg::Job(j) => self.take_job(j),
                Bg::Started(Ok(j)) => {
                    self.starting = false;
                    self.job = Some(j);
                    self.job_reported = false;
                    self.watch_job();
                }
                Bg::Started(Err(p)) => {
                    self.starting = false;
                    self.job = None;
                    if self.screen == Screen::Progress {
                        self.leave_progress(true);
                    }
                    self.report(p);
                }
                Bg::Answer { message, quiet } => {
                    match message {
                        Ok(_) if quiet => {}
                        Ok(m) => self.say(m),
                        Err(p) => self.report(p),
                    }
                    self.read_overview(false);
                }
            }
        }
        any
    }

    fn take_overview(&mut self, o: Overview) {
        self.unreachable = None;
        self.checking = false;
        let first = !self.loaded;
        self.loaded = true;
        match &o.job {
            Some(j) if !j.phase.ended() => {
                self.job = Some(j.clone());
                self.job_reported = false;
                self.watch_job();
            }
            // one it was following, and the connection went before it heard the end
            Some(j) if self.job.as_ref().is_some_and(|m| m.id == j.id) && !self.watching && !self.starting => self.take_job(j.clone()),
            // ended before this app opened: nothing to say about it
            Some(_) if self.job.is_none() => self.job_reported = true,
            // the service went away and forgot it
            None if !self.watching && !self.starting && self.job.is_some() => {
                self.job = None;
                if self.screen == Screen::Progress {
                    self.leave_progress(true);
                }
            }
            _ => {}
        }
        // read the indexes again if it's been a while, once, when opening
        if first && o.repos.iter().any(|r| self.now_secs.saturating_sub(r.checked) > STALE_SECS) {
            self.read_overview(true);
        }
        self.overview = o;
        self.rebuild();
    }

    /// The lists, from the last overview, keeping what's selected.
    fn rebuild(&mut self) {
        let keep = self.selected_key();
        let tool_count: u64 = self.overview.repos.iter().map(|r| r.tools).sum();
        let tools: Vec<Row> = self.overview.rows.iter().filter(|r| r.kind == Kind::Tool).cloned().collect();
        let installed = tools.iter().filter(|r| is_here(r)).count();
        self.entries = self.overview.rows.iter().filter(|r| r.kind != Kind::Tool).cloned().map(Entry::Package).collect();
        if tool_count > 0 || !tools.is_empty() {
            self.entries.push(Entry::Tools { count: tool_count.max(tools.len() as u64), installed });
        }
        if let Some(k) = keep
            && let Some(i) = self.entries.iter().position(|e| entry_key(e) == k)
        {
            self.home_list.sel = i;
            self.home_list.scroll_from = i as f32;
        }
        self.home_list.sel = self.home_list.sel.min(self.entries.len().saturating_sub(1));
        if self.query.is_empty() {
            let id = self.tool_rows.get(self.tools_list.sel).map(|r| r.id.clone());
            self.tool_rows = tools;
            if let Some(i) = id.and_then(|id| self.tool_rows.iter().position(|r| r.id == id)) {
                self.tools_list.sel = i;
            }
            self.tools_list.sel = self.tools_list.sel.min(self.tool_rows.len().saturating_sub(1));
        }
        // the open package, as it's now
        if let Some(cur) = &self.row
            && let Some(n) = self.overview.rows.iter().find(|r| r.id == cur.id && r.repo == cur.repo)
        {
            self.row = Some(n.clone());
        }
    }

    fn selected_key(&self) -> Option<String> {
        self.entries.get(self.home_list.sel).map(entry_key)
    }

    fn take_found(&mut self, f: Found) {
        // apps match here, from what's already read; tools come from the service
        let q = self.query.to_lowercase();
        let mut rows: Vec<Row> = self
            .overview
            .rows
            .iter()
            .filter(|r| r.kind != Kind::Tool && q.split_whitespace().all(|w| r.name.to_lowercase().contains(w) || r.id.contains(w) || r.description.to_lowercase().contains(w)))
            .cloned()
            .collect();
        self.found_total = f.total + rows.len() as u64;
        self.searched = f.searched;
        rows.extend(f.rows);
        self.tool_rows = rows;
        self.tools_list = List::default();
    }

    fn take_job(&mut self, j: JobView) {
        // the sentinel: the connection is over
        if j.id.is_empty() {
            self.watching = false;
            self.read_overview(false);
            if self.search_active() {
                self.search_now();
            }
            return;
        }
        let ended = j.phase.ended();
        if ended && !self.job_reported {
            self.job_reported = true;
            match j.phase {
                Phase::Done => {
                    let verb = if j.action == Action::Update { "is up to date" } else { "is ready" };
                    self.say(format!("{} {verb}", j.name));
                    if self.screen == Screen::Progress && self.row.as_ref().is_some_and(|r| r.id == j.id) {
                        self.leave_progress(false);
                    }
                }
                Phase::Cancelled => {
                    self.say(format!("{} wasn't installed", j.name));
                    if self.screen == Screen::Progress {
                        self.leave_progress(true);
                    }
                }
                Phase::Failed => {
                    let p = j.problem.clone().unwrap_or_else(|| Problem::new("failed", "That didn't work", ""));
                    if self.focused {
                        if self.screen == Screen::Progress {
                            self.leave_progress(true);
                        }
                        self.report(p);
                    } else {
                        self.shell.attention(Some(&format!("{} didn't install", j.name)));
                        self.report(p);
                    }
                }
                _ => {}
            }
        }
        self.job = if ended { None } else { Some(j) };
    }

    fn search_active(&self) -> bool {
        !self.query.is_empty()
    }

    /// Back to Details, asking for the size again if it's still to be installed.
    fn leave_progress(&mut self, ask: bool) {
        self.screen = Screen::Details;
        self.details = None;
        if ask && let Some(r) = self.row.clone() {
            self.read_details(&r);
        }
    }

    fn say(&mut self, s: String) {
        self.toast = Some((s, self.now + TOAST_MS));
        self.dirty = true;
    }

    fn report(&mut self, p: Problem) {
        self.overlay = Some(Overlay::Report(p));
        self.dirty = true;
    }

    // ------------------------------------------------------------------ search ----

    fn search_later(&mut self) {
        self.search_due = Some(self.now + SEARCH_DEBOUNCE_MS);
        self.searching = true;
    }

    fn search_now(&mut self) {
        self.search_due = None;
        self.search_generation += 1;
        if self.query.trim().is_empty() {
            self.searching = false;
            self.query.clear();
            self.rebuild();
            self.tools_list = List::default();
            self.found_total = 0;
            return;
        }
        self.searching = true;
        let (generation, query) = (self.search_generation, self.query.clone());
        self.in_background(move |b| {
            let req = Request { query: Some(query), ..Request::new(Op::Search) };
            Bg::Found {
                generation,
                result: match b.ask(&req) {
                    Ok(Reply::Found(f)) => Ok(f),
                    Ok(Reply::Refused(p)) => Err(p),
                    Ok(o) => Err(Problem::new("service", "That didn't work", format!("{o:?}"))),
                    Err(e) => Err(unreachable_problem(e)),
                },
            }
        });
    }

    // ------------------------------------------------------------------- clock ----

    pub fn tick(&mut self, now: u64) {
        self.now = now;
        if self.toast.as_ref().is_some_and(|t| now >= t.1) {
            self.toast = None;
            self.dirty = true;
        }
        if self.search_due.is_some_and(|t| now >= t) {
            self.search_now();
        }
    }

    /// When the app next has something to do with no input. None: it can sleep until
    /// something wakes it.
    pub fn next_deadline(&self) -> Option<u64> {
        let mut d: Option<u64> = None;
        let mut add = |t: u64| d = Some(d.map_or(t, |x: u64| x.min(t)));
        if let Some(t) = &self.toast {
            add(t.1);
        }
        if let Some(t) = self.search_due {
            add(t);
        }
        d
    }

    /// Something is moving. False at rest, so an idle app draws nothing.
    pub fn animating(&self) -> bool {
        if self.slide.is_some() || self.slide_req.is_some() {
            return true;
        }
        let l = match self.screen {
            Screen::Home => &self.home_list,
            Screen::Tools => &self.tools_list,
            _ => return false,
        };
        !self.reduce_motion && self.now < l.scroll_start + SCROLL_MS
    }

    pub fn idle(&self) -> bool {
        !self.checking && !self.sizing && !self.searching && self.loaded
    }

    pub fn scroll_pos(&self, l: &List) -> f32 {
        if self.reduce_motion || self.now >= l.scroll_start + SCROLL_MS {
            return l.sel as f32;
        }
        let t = (self.now - l.scroll_start) as f32 / SCROLL_MS as f32;
        let e = 1.0 - (1.0 - t).powi(3);
        l.scroll_from + (l.sel as f32 - l.scroll_from) * e
    }

    pub fn slide_progress(&self) -> Option<f32> {
        let s = self.slide.as_ref()?;
        let t = (self.now.saturating_sub(s.start)) as f32 / SLIDE_MS as f32;
        Some(1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3))
    }

    pub fn slide_done(&self) -> bool {
        self.slide.as_ref().is_some_and(|s| self.now.saturating_sub(s.start) >= SLIDE_MS)
    }

    // ------------------------------------------------------------------ window ----

    pub fn repeats(&self, code: u16) -> bool {
        let moving = matches!(code, keys::UP | keys::DOWN | keys::PAGEUP | keys::PAGEDOWN);
        match &self.overlay {
            Some(Overlay::Find(_)) => moving || code == keys::BACKSPACE,
            Some(_) => false,
            None => moving && matches!(self.screen, Screen::Home | Screen::Tools),
        }
    }

    pub fn focus(&mut self, on: bool) {
        self.focused = on;
        self.dirty = true;
        if !on {
            self.decoder.reset();
        } else {
            self.shell.attention(None);
            // what's installed may have changed while it was behind
            if self.loaded {
                self.read_overview(false);
            }
        }
    }

    pub fn resized(&mut self, h: i32) {
        let short = h < SHORT;
        if short == self.short {
            return;
        }
        self.short = short;
        self.slide = None;
        self.slide_req = None;
        self.dirty = true;
        if short {
            if self.keyboard_ours && !matches!(self.overlay, Some(Overlay::Find(_))) {
                self.shell.toggle_keyboard();
                return;
            }
            // START from a list: search
            if matches!(self.screen, Screen::Home | Screen::Tools) && self.overlay.is_none() {
                self.screen = Screen::Tools;
                self.overlay = Some(Overlay::Find(Field::new(&self.query, None)));
            }
        } else {
            self.keyboard_ours = false;
            if matches!(self.overlay, Some(Overlay::Find(_))) {
                self.overlay = None;
                if self.search_due.is_some() {
                    self.search_now();
                }
            }
        }
    }

    pub fn key(&mut self, code: u16, down: bool, repeat: bool) {
        for i in self.decoder.key(code, down, repeat) {
            self.input(i);
        }
    }

    // ------------------------------------------------------------------- input ----

    pub fn input(&mut self, i: Input) {
        self.dirty = true;
        match &self.overlay {
            Some(Overlay::Find(_)) => return self.find_input(i),
            Some(Overlay::Confirm(_)) => return self.confirm_input(i),
            Some(Overlay::Report(_)) => {
                if matches!(i, Input::Press(Button::A | Button::B | Button::L2, false)) {
                    self.overlay = None;
                    if self.screen == Screen::Details {
                        self.refresh_details();
                    }
                }
                return;
            }
            None => {}
        }
        // a real keyboard just types
        if let Input::Char(c) = i
            && matches!(self.screen, Screen::Home | Screen::Tools)
        {
            self.screen = Screen::Tools;
            let mut f = Field::new(&self.query, None);
            f.insert(c);
            self.query = f.string();
            self.overlay = Some(Overlay::Find(f));
            self.search_later();
            return;
        }
        let Input::Press(b, repeat) = i else { return };
        match self.screen {
            Screen::Home => self.home_input(b, repeat),
            Screen::Tools => self.tools_input(b, repeat),
            Screen::Details => {
                if !repeat {
                    self.details_input(b);
                }
            }
            Screen::Progress => {
                if !repeat {
                    self.progress_input(b);
                }
            }
        }
    }

    fn move_list(&mut self, tools: bool, to: isize) {
        let (n, now) = if tools { (self.tool_rows.len(), self.now) } else { (self.entries.len(), self.now) };
        if n == 0 {
            return;
        }
        let pos = {
            let l = if tools { &self.tools_list } else { &self.home_list };
            self.scroll_pos(l)
        };
        let l = if tools { &mut self.tools_list } else { &mut self.home_list };
        let to = to.clamp(0, n as isize - 1) as usize;
        if to != l.sel {
            l.scroll_from = pos;
            l.scroll_start = now;
            l.sel = to;
        }
    }

    fn list_keys(&mut self, tools: bool, b: Button) -> bool {
        let sel = if tools { self.tools_list.sel } else { self.home_list.sel } as isize;
        match b {
            Button::Up => self.move_list(tools, sel - 1),
            Button::Down => self.move_list(tools, sel + 1),
            Button::L1 => self.move_list(tools, sel - PAGE as isize),
            Button::R1 => self.move_list(tools, sel + PAGE as isize),
            _ => return false,
        }
        true
    }

    fn slide(&mut self, d: SlideDir) {
        if !self.reduce_motion {
            self.slide_req = Some(d);
        }
    }

    fn home_input(&mut self, b: Button, repeat: bool) {
        if self.list_keys(false, b) || repeat {
            return;
        }
        match b {
            Button::A | Button::Right => match self.entries.get(self.home_list.sel).cloned() {
                Some(Entry::Package(r)) => self.open(r, Screen::Home),
                Some(Entry::Tools { .. }) => {
                    self.screen = Screen::Tools;
                    self.slide(SlideDir::Forward);
                }
                None => {}
            },
            Button::Y => self.read_overview(true),
            _ => {}
        }
    }

    fn tools_input(&mut self, b: Button, repeat: bool) {
        if self.list_keys(true, b) || repeat {
            return;
        }
        match b {
            Button::A | Button::Right => {
                if let Some(r) = self.tool_rows.get(self.tools_list.sel).cloned() {
                    self.open(r, Screen::Tools);
                }
            }
            Button::B | Button::Left => {
                if self.query.is_empty() {
                    self.screen = Screen::Home;
                    self.slide(SlideDir::Back);
                } else {
                    self.query.clear();
                    self.search_now();
                }
            }
            Button::L2 => {
                self.query.clear();
                self.search_now();
                self.screen = Screen::Home;
                self.slide(SlideDir::Back);
            }
            Button::Y => self.read_overview(true),
            _ => {}
        }
    }

    /// Details, or the job's progress if that's what it's doing.
    fn open(&mut self, r: Row, from: Screen) {
        self.back_to = from;
        self.row = Some(r.clone());
        self.details = None;
        if self.job.as_ref().is_some_and(|j| j.id == r.id && j.repo == r.repo) {
            self.screen = Screen::Progress;
        } else {
            self.screen = Screen::Details;
            if matches!(r.state, RowState::Available | RowState::Update) {
                self.read_details(&r);
            } else if r.state == RowState::Installed {
                self.sizing = false;
            }
        }
        self.slide(SlideDir::PanelIn);
    }

    fn refresh_details(&mut self) {
        if let Some(r) = self.row.clone()
            && matches!(r.state, RowState::Available | RowState::Update)
        {
            self.details = None;
            self.read_details(&r);
        }
    }

    fn go_back(&mut self) {
        self.screen = self.back_to;
        self.row = None;
        self.details = None;
        self.sizing = false;
        self.slide(SlideDir::PanelOut);
    }

    fn details_input(&mut self, b: Button) {
        let Some(r) = self.row.clone() else { return };
        let here = is_here(&r);
        match b {
            Button::B | Button::Left => self.go_back(),
            Button::L2 => {
                self.go_back();
                self.screen = Screen::Home;
            }
            Button::A => match r.state {
                RowState::Available => self.begin(Op::Install, &r),
                RowState::Update => self.begin(Op::Update, &r),
                RowState::Installed if r.kind != Kind::Tool && !self.needs_files(&r) => {
                    // the shell starts it, like anything from the launcher
                    self.shell.send(format!("launch {}", r.id), |_| {});
                }
                _ => {}
            },
            Button::X if here && r.state != RowState::Elsewhere => self.overlay = Some(Overlay::Confirm(Confirm { kind: ConfirmKind::Remove, row: r, nudge: false })),
            Button::Y if r.can_roll_back => self.overlay = Some(Overlay::Confirm(Confirm { kind: ConfirmKind::Rollback, row: r, nudge: false })),
            _ => {}
        }
    }

    fn begin(&mut self, op: Op, r: &Row) {
        if self.job.as_ref().is_some_and(|j| j.id != r.id) {
            self.report(Problem::new("busy", "One thing at a time", "Something else is being installed. Try again when it's done."));
            return;
        }
        self.job = Some(JobView { repo: r.repo.clone(), id: r.id.clone(), name: r.name.clone(), action: if op == Op::Update { Action::Update } else { Action::Install }, ..JobView::default() });
        self.job_reported = false;
        self.starting = true;
        self.screen = Screen::Progress;
        self.start_job(op, r);
    }

    fn progress_input(&mut self, b: Button) {
        match b {
            // it carries on
            Button::B | Button::Left => {
                self.screen = self.back_to;
                self.row = None;
                self.slide(SlideDir::PanelOut);
            }
            Button::L2 => {
                self.screen = Screen::Home;
                self.row = None;
                self.slide(SlideDir::PanelOut);
            }
            Button::X => {
                if let Some(j) = &mut self.job
                    && !j.cancelling
                    && j.phase != Phase::Installing
                {
                    j.cancelling = true;
                    self.send_simple(Op::Cancel, None);
                }
            }
            _ => {}
        }
    }

    fn confirm_input(&mut self, i: Input) {
        let Some(Overlay::Confirm(c)) = &mut self.overlay else { return };
        let Input::Press(b, false) = i else { return };
        match b {
            Button::Y => {
                let (kind, row) = (c.kind, c.row.clone());
                self.overlay = None;
                let op = if kind == ConfirmKind::Remove { Op::Remove } else { Op::Rollback };
                self.send_simple(op, Some(&row));
                if kind == ConfirmKind::Remove {
                    self.go_back();
                }
            }
            Button::B | Button::L2 => self.overlay = None,
            // the button that got you here does nothing on purpose
            _ => c.nudge = true,
        }
    }

    fn find_input(&mut self, i: Input) {
        let Some(Overlay::Find(f)) = &mut self.overlay else { return };
        match i {
            Input::Char(c) => f.insert(c),
            Input::Press(Button::Y, _) => f.insert(' '),
            Input::Press(Button::B, _) => {
                f.backspace();
            }
            Input::Press(Button::Up, _) => return self.move_list(true, self.tools_list.sel as isize - 1),
            Input::Press(Button::Down, _) => return self.move_list(true, self.tools_list.sel as isize + 1),
            Input::Press(Button::A | Button::L2, false) => {
                self.overlay = None;
                if self.short {
                    self.shell.toggle_keyboard();
                }
                if self.search_due.is_some() {
                    self.search_now();
                }
                return;
            }
            _ => return,
        }
        self.query = f.string();
        self.search_later();
    }

    #[cfg(test)]
    pub fn forget_watcher(&mut self) {
        self.watching = false;
    }

    // --------------------------------------------------------------- questions ----

    /// Is this one of the person's files missing, for what an app says it needs?
    pub fn missing(&self, path: &str) -> bool {
        !expand(path, &self.home).exists()
    }

    /// An app that waits for your own files: it won't show under Apps until they're here.
    pub fn needs_files(&self, r: &Row) -> bool {
        r.requires.iter().any(|p| self.missing(p))
    }

    /// The row the panel is about.
    pub fn selected_row(&self) -> Option<&Row> {
        match self.screen {
            Screen::Tools => self.tool_rows.get(self.tools_list.sel),
            _ => match self.entries.get(self.home_list.sel) {
                Some(Entry::Package(r)) => Some(r),
                _ => None,
            },
        }
    }

    pub fn job_for(&self, r: &Row) -> Option<&JobView> {
        self.job.as_ref().filter(|j| j.id == r.id && j.repo == r.repo)
    }

    /// "3 days ago"
    pub fn age(&self, then: u64) -> String {
        let days = self.now_secs.saturating_sub(then) / 86400;
        match days {
            0 => "today".into(),
            1 => "yesterday".into(),
            2..=60 => format!("{days} days ago"),
            _ => format!("{} months ago", days / 30),
        }
    }
}

pub fn is_here(r: &Row) -> bool {
    matches!(r.state, RowState::Installed | RowState::Update | RowState::Elsewhere)
}

fn entry_key(e: &Entry) -> String {
    match e {
        Entry::Package(r) => format!("{}/{}", r.repo, r.id),
        Entry::Tools { .. } => "/tools".into(),
    }
}

fn unreachable_problem(e: String) -> Problem {
    Problem::new("service", "Can't reach the service", e)
}

pub fn expand(p: &str, home: &Path) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(p),
    }
}
