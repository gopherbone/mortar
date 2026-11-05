// SPDX-License-Identifier: GPL-2.0-or-later
//! The app's state and what the buttons do. No drawing (view.rs), no window
//! (wl.rs): `App` takes key events and a clock, and says when it wants a
//! frame, so the same code runs under Wayland, in the preview and in tests.

use crate::bg::{Bg, Wake};
use crate::keys::{self, Decoder, Input};
use crate::listing::{self, Clock, Entry, Kind, Sort, human_size, plural};
use crate::ops::{self, Answer, Choice, Clash, Guard, Job, JobKind, JobMsg, JobSpec, Progress, Report, Trash};
use crate::places::{self, Place, PlaceEnv, PlaceKind, PlayConfig};
use crate::preview::{self, Preview};
use crate::shell::Shell;
use crate::storage::{self, Dir, ScanUpdate, Tile, TileKind};
use mortar_ui::gfx::Rect;
use mortar_ui::input::Button;
use resvg::tiny_skia::Pixmap;
use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

pub const W: f32 = 1024.0;
pub const H: f32 = 768.0;
/// The bottom row with the button hints.
pub const HINT_H: f32 = 72.0;
/// The storage mosaic's area.
pub const STORAGE: Rect = Rect { x: 0.0, y: 92.0, w: 1024.0, h: 604.0 };
/// The Places grid: columns, rows and a cell.
pub const PLACE_COLS: usize = 4;
pub const PLACE_ROWS: usize = 3;
pub const PLACE_CELL: (f32, f32) = (256.0, 232.0);
/// How long the selection takes to settle on a row.
const SCROLL_MS: u64 = 110;
const SLIDE_MS: u64 = 190;
/// Wait this long on a selection before reading anything for the panel.
const PREVIEW_DEBOUNCE_MS: u64 = 120;
/// A job that ends sooner than this never shows its progress screen.
const JOB_SHOW_AFTER_MS: u64 = 300;
const TOAST_MS: u64 = 2800;
/// Hold L1+R1 this long to delete forever.
pub const HOLD_MS: u64 = 900;
/// A folder that hasn't arrived after this much shows "Reading".
const NAV_WAIT_MS: u64 = 120;
/// Rows a page moves.
const PAGE: usize = 6;
/// The on-screen keyboard is up when the window is shorter than this.
const SHORT: i32 = 640;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlideDir {
    /// New from the right.
    Forward,
    /// New from the left.
    Back,
    /// New from below, over the old.
    Up,
    /// Old leaves downwards.
    Down,
    /// The panel changes: new from the right, over the rest.
    PanelIn,
    PanelOut,
}

pub struct Slide {
    pub dir: SlideDir,
    pub start: u64,
    pub from: Pixmap,
    pub to: Pixmap,
}

/// What X opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Copy,
    Move,
    Rename,
    Delete,
    NewFolder,
    MarkAll,
    ClearMarks,
    Paste,
    Forget,
    StorageHere,
    Sort,
    Hidden,
    Restore,
    DeleteForever,
    EmptyTrash,
}

impl Act {
    pub fn is_danger(self) -> bool {
        matches!(self, Act::Delete | Act::DeleteForever | Act::EmptyTrash)
    }
}

/// A text field: characters and a cursor.
#[derive(Clone, Debug, Default)]
pub struct Field {
    pub text: Vec<char>,
    pub cursor: usize,
}

impl Field {
    pub fn new(s: &str, cursor: Option<usize>) -> Field {
        let text: Vec<char> = s.chars().collect();
        let cursor = cursor.unwrap_or(text.len()).min(text.len());
        Field { text, cursor }
    }
    pub fn string(&self) -> String {
        self.text.iter().collect()
    }
    pub fn insert(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += 1;
    }
    pub fn backspace(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        self.cursor -= 1;
        self.text.remove(self.cursor);
        true
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum NameFor {
    Rename { path: PathBuf, original: String },
    NewFolder,
}

pub struct Naming {
    pub purpose: NameFor,
    pub field: Field,
    pub error: Option<String>,
    /// B can back out only on a fresh press, so holding B to clear the name
    /// doesn't leave the screen as well.
    pub b_armed: bool,
}

pub struct Confirm {
    pub forever: bool,
    pub sources: Vec<PathBuf>,
    pub names: Vec<String>,
    pub kinds: Vec<Kind>,
    /// Why it can't go in the Trash, if that's why it's forever.
    pub reason: Option<String>,
    /// Emptying the whole Trash.
    pub empty_trash: bool,
    /// When L1+R1 both went down.
    pub held_since: Option<u64>,
    /// A or some other button got pressed: say which one's needed.
    pub nudge: bool,
}

pub enum Overlay {
    Actions { sel: usize, rows: Vec<Act> },
    Confirm(Confirm),
    Naming(Naming),
    /// Jump to a name: the on-screen keyboard or a real one.
    Find(Field),
    Report(Report),
}

pub struct ClashUi {
    pub clash: Clash,
    pub sel: usize,
    pub for_rest: bool,
}

pub struct JobRun {
    pub kind: JobKind,
    pub job: Job,
    pub progress: Progress,
    pub clash: Option<ClashUi>,
    pub cancelling: bool,
    pub started: u64,
    pub shown: bool,
    /// Kinds of the things going, for the tiles.
    pub kinds: Vec<Kind>,
    pub count: usize,
}

pub struct Carry {
    pub moving: bool,
    pub sources: Vec<PathBuf>,
    pub kinds: Vec<Kind>,
}

pub struct StorageLevel {
    pub title: String,
    /// None: the overview.
    pub dir: Option<PathBuf>,
    pub tiles: Vec<Tile>,
    pub rects: Vec<Rect>,
    pub sel: usize,
    /// The person has moved: scan updates don't choose for them any more.
    pub moved: bool,
    pub scanning: bool,
    pub error: Option<String>,
}

pub enum Screen {
    Places,
    Browse,
    Image { index: usize, pic: Option<Result<Arc<Pixmap>, String>> },
    Text { name: String, lines: Option<Result<Vec<String>, String>>, cut: bool, top: usize },
    Storage,
}

/// Where a pending folder change leads.
struct PendingNav {
    dir: PathBuf,
    select: Select,
    slide: Option<SlideDir>,
    since: u64,
    /// Coming from a screen other than the list: commit whatever arrives.
    committed: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Select {
    First,
    Keep,
    Name(OsString),
}

/// Everything the app gets from outside.
pub struct Setup {
    pub env: PlaceEnv,
    pub guard: Guard,
    pub clock: Clock,
    pub play: Option<PlayConfig>,
    pub shell: Shell,
    pub reduce_motion: bool,
    pub wake: Wake,
}

pub struct App {
    pub env: PlaceEnv,
    pub guard: Guard,
    pub clock: Clock,
    pub play: Option<PlayConfig>,
    pub games: Arc<Vec<String>>,
    pub shell: Shell,
    pub reduce_motion: bool,
    wake: Wake,
    tx: Sender<Bg>,
    rx: Receiver<Bg>,
    pub trash: Trash,

    pub now: u64,
    pub dirty: bool,
    pub done: bool,
    decoder: Decoder,

    pub screen: Screen,
    pub overlay: Option<Overlay>,
    pub places: Vec<Place>,
    pub place_facts: HashMap<String, String>,
    pub place_sel: usize,

    pub dir: PathBuf,
    pub entries: Vec<Entry>,
    pub sel: usize,
    /// Where the list is shown, tweening to `sel`.
    pub scroll_from: f32,
    pub scroll_start: u64,
    pub hidden_count: usize,
    pub truncated: bool,
    pub loading: bool,
    pub marks: HashSet<OsString>,
    pub carry: Option<Carry>,
    pub sort: Sort,
    pub show_hidden: bool,
    remembered: HashMap<PathBuf, OsString>,
    nav_gen: Arc<AtomicU64>,
    pending: Option<PendingNav>,

    pub panel: Option<(PathBuf, Preview)>,
    cache: Vec<(PathBuf, i64, Preview)>,
    preview_due: Option<u64>,
    preview_gen: Arc<AtomicU64>,
    view_gen: Arc<AtomicU64>,
    workers: Arc<AtomicU64>,

    pub levels: Vec<StorageLevel>,
    /// Back from Storage: to the list, or to Places.
    storage_from_places: bool,
    scan_gen: Arc<AtomicU64>,

    pub job: Option<JobRun>,
    after_job: Option<(PathBuf, Select)>,
    pub toast: Option<(String, u64)>,

    pub slide: Option<Slide>,
    pub slide_req: Option<SlideDir>,
    pub short: bool,
    pub focused: bool,
    /// A keyboard we opened, to close again.
    keyboard_ours: bool,
}

impl App {
    pub fn new(s: Setup) -> App {
        let (tx, rx) = channel();
        let trash = Trash { root: s.env.trash.clone() };
        let games = Arc::new(s.play.as_ref().map(|p| p.extensions()).unwrap_or_default());
        let places = places::discover(&s.env);
        let mut a = App {
            env: s.env,
            guard: s.guard,
            clock: s.clock,
            play: s.play,
            games,
            shell: s.shell,
            reduce_motion: s.reduce_motion,
            wake: s.wake,
            tx,
            rx,
            trash,
            now: 0,
            dirty: true,
            done: false,
            decoder: Decoder::default(),
            screen: Screen::Places,
            overlay: None,
            places,
            place_facts: HashMap::new(),
            place_sel: 0,
            dir: PathBuf::new(),
            entries: Vec::new(),
            sel: 0,
            scroll_from: 0.0,
            scroll_start: 0,
            hidden_count: 0,
            truncated: false,
            loading: false,
            marks: HashSet::new(),
            carry: None,
            sort: Sort::Name,
            show_hidden: false,
            remembered: HashMap::new(),
            nav_gen: Arc::new(AtomicU64::new(0)),
            pending: None,
            panel: None,
            cache: Vec::new(),
            preview_due: None,
            preview_gen: Arc::new(AtomicU64::new(0)),
            view_gen: Arc::new(AtomicU64::new(0)),
            workers: Arc::new(AtomicU64::new(0)),
            levels: Vec::new(),
            storage_from_places: false,
            scan_gen: Arc::new(AtomicU64::new(0)),
            job: None,
            after_job: None,
            toast: None,
            slide: None,
            slide_req: None,
            short: false,
            focused: true,
            keyboard_ours: false,
        };
        a.refresh_place_facts();
        a
    }

    // ---------------------------------------------------------- workers ----

    /// Run on a thread; a counter says when everything is done (`idle`).
    fn work(&self, f: impl FnOnce(&Sender<Bg>, &Wake) + Send + 'static) {
        let (tx, wake, n) = (self.tx.clone(), self.wake.clone(), self.workers.clone());
        n.fetch_add(1, Ordering::SeqCst);
        std::thread::spawn(move || {
            f(&tx, &wake);
            n.fetch_sub(1, Ordering::SeqCst);
            wake();
        });
    }

    /// Nothing running: tests and the preview wait for this.
    pub fn idle(&self) -> bool {
        self.workers.load(Ordering::SeqCst) == 0 && self.job.is_none() && self.pending.is_none()
    }

    /// Take in what the workers finished. True if the screen changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(m) = self.rx.try_recv() {
            changed = true;
            match m {
                Bg::Listing { generation, dir, result } => self.on_listing(generation, dir, result),
                Bg::Preview { key, generation, preview } => self.on_preview(key, generation, preview),
                Bg::Scan { generation, update } => self.on_scan(generation, update),
                Bg::PlaceFacts(v) => self.place_facts.extend(v),
                Bg::Note(s) => self.say(&s),
                Bg::Image { generation, result } => {
                    if generation == self.view_gen.load(Ordering::SeqCst)
                        && let Screen::Image { pic, .. } = &mut self.screen
                    {
                        *pic = Some(result);
                    }
                }
                Bg::Text { generation, result } => {
                    if generation == self.view_gen.load(Ordering::SeqCst)
                        && let Screen::Text { lines, cut, .. } = &mut self.screen
                    {
                        match result {
                            Ok((l, c)) => {
                                *lines = Some(Ok(l));
                                *cut = c;
                            }
                            Err(e) => *lines = Some(Err(e)),
                        }
                    }
                }
            }
        }
        changed |= self.poll_job();
        if changed {
            self.dirty = true;
        }
        changed
    }

    /// The clock moved. Anything due happens here.
    pub fn tick(&mut self, now: u64) {
        self.now = now;
        if self.toast.as_ref().is_some_and(|t| now >= t.1) {
            self.toast = None;
            self.dirty = true;
        }
        if let Some(p) = &self.pending
            && now >= p.since + NAV_WAIT_MS
            && !p.committed
        {
            self.commit_reading();
        }
        if self.preview_due.is_some_and(|t| now >= t) {
            self.preview_due = None;
            self.start_preview();
        }
        if let Some(Overlay::Confirm(c)) = &self.overlay
            && let Some(t) = c.held_since
        {
            self.dirty = true;
            if now >= t + HOLD_MS {
                self.confirm_go();
            }
        }
        if let Some(j) = &mut self.job
            && !j.shown
            && now >= j.started + JOB_SHOW_AFTER_MS
        {
            j.shown = true;
            self.dirty = true;
        }
    }

    /// When the app next has something to do with no input: None means it
    /// can sleep until something wakes it.
    pub fn next_deadline(&self) -> Option<u64> {
        let mut d: Option<u64> = None;
        let mut add = |t: u64| d = Some(d.map_or(t, |x: u64| x.min(t)));
        if let Some(t) = &self.toast {
            add(t.1);
        }
        if let Some(p) = &self.pending
            && !p.committed
        {
            add(p.since + NAV_WAIT_MS);
        }
        if let Some(t) = self.preview_due {
            add(t);
        }
        if let Some(j) = &self.job
            && !j.shown
        {
            add(j.started + JOB_SHOW_AFTER_MS);
        }
        d
    }

    /// Something is moving: keep the frames coming. False at rest, so an
    /// idle app draws nothing.
    pub fn animating(&self) -> bool {
        if self.slide.is_some() || self.slide_req.is_some() {
            return true;
        }
        if matches!(self.overlay, Some(Overlay::Confirm(Confirm { held_since: Some(_), .. }))) {
            return true;
        }
        matches!(self.screen, Screen::Browse) && !self.reduce_motion && self.now < self.scroll_start + SCROLL_MS
    }

    /// Where the list is drawn: rows from the top, easing towards `sel`.
    pub fn scroll_pos(&self) -> f32 {
        if self.reduce_motion || self.now >= self.scroll_start + SCROLL_MS {
            return self.sel as f32;
        }
        let t = (self.now - self.scroll_start) as f32 / SCROLL_MS as f32;
        let e = 1.0 - (1.0 - t).powi(3);
        self.scroll_from + (self.sel as f32 - self.scroll_from) * e
    }

    pub fn slide_progress(&self) -> Option<f32> {
        let s = self.slide.as_ref()?;
        let t = (self.now.saturating_sub(s.start)) as f32 / SLIDE_MS as f32;
        Some(1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3))
    }

    pub fn slide_done(&self) -> bool {
        self.slide.as_ref().is_some_and(|s| self.now >= s.start + SLIDE_MS)
    }

    fn request_slide(&mut self, d: SlideDir) {
        if !self.reduce_motion && self.focused {
            self.slide_req = Some(d);
        }
        self.dirty = true;
    }

    // -------------------------------------------------------------- nav ----

    pub fn say(&mut self, s: &str) {
        self.toast = Some((s.to_string(), self.now + TOAST_MS));
        self.dirty = true;
    }

    fn refresh_place_facts(&mut self) {
        self.places = places::discover(&self.env);
        self.place_sel = self.place_sel.min(self.places.len().saturating_sub(1));
        let list: Vec<(PlaceKind, String, PathBuf)> = self.places.iter().map(|p| (p.kind, p.name.clone(), p.path.clone())).collect();
        let trash = self.trash.clone();
        self.work(move |tx, _| {
            let mut out = Vec::new();
            for (k, name, p) in list {
                let fact = match k {
                    PlaceKind::Trash => {
                        let n = trash.count();
                        if n == 0 { "Empty".to_string() } else { plural(n, "item") }
                    }
                    PlaceKind::Storage | PlaceKind::Root => match ops::free_space(&p) {
                        Some((free, total)) if k == PlaceKind::Storage => format!("{} of {} free", human_size(free), human_size(total)),
                        _ => "The whole system".to_string(),
                    },
                    PlaceKind::Home | PlaceKind::Removable => match ops::free_space(&p) {
                        Some((free, _)) => format!("{} free", human_size(free)),
                        None => String::new(),
                    },
                    _ => match std::fs::read_dir(&p) {
                        Ok(r) => {
                            let n = r.flatten().filter(|e| e.file_name().as_bytes().first() != Some(&b'.')).count();
                            if n == 0 { "Empty".to_string() } else { plural(n, "item") }
                        }
                        Err(_) => String::new(),
                    },
                };
                out.push((name, fact));
            }
            let _ = tx.send(Bg::PlaceFacts(out));
        });
    }

    /// Go straight to a folder (the preview).
    pub fn go_to(&mut self, dir: PathBuf) {
        self.go(dir, Select::First, Some(SlideDir::Forward));
    }

    pub fn refresh_preview(&mut self) {
        self.preview_soon(true);
    }

    fn read_dir(&mut self, dir: PathBuf) {
        let generation = self.nav_gen.fetch_add(1, Ordering::SeqCst) + 1;
        let (hidden, sort, games, g) = (self.show_hidden, self.sort, self.games.clone(), self.nav_gen.clone());
        self.work(move |tx, wake| {
            let stop = || g.load(Ordering::SeqCst) != generation;
            let result = listing::read(&dir, hidden, sort, &games, &stop);
            let _ = tx.send(Bg::Listing { generation, dir, result });
            wake();
        });
    }

    /// Go to a folder. The screen changes when it has been read (or after a
    /// moment, showing that it's reading).
    fn go(&mut self, dir: PathBuf, select: Select, slide: Option<SlideDir>) {
        if !self.dir.as_os_str().is_empty() && self.dir != dir && !self.marks.is_empty() {
            let n = self.marks.len();
            self.marks.clear();
            self.say(&format!("Cleared {}", plural(n, "mark")));
        }
        if let Some(e) = self.entries.get(self.sel)
            && !self.dir.as_os_str().is_empty()
        {
            self.remembered.insert(self.dir.clone(), e.name.clone());
        }
        self.pending = Some(PendingNav { dir: dir.clone(), select, slide, since: self.now, committed: false });
        self.read_dir(dir);
    }

    /// Show the folder as "reading": it's taking a while.
    fn commit_reading(&mut self) {
        let Some(mut p) = self.pending.take() else { return };
        self.dir = p.dir.clone();
        self.entries.clear();
        self.sel = 0;
        self.scroll_from = 0.0;
        self.scroll_start = 0;
        self.loading = true;
        self.panel = None;
        self.preview_due = None;
        self.screen = Screen::Browse;
        if let Some(d) = p.slide.take() {
            self.request_slide(d);
        }
        p.committed = true;
        self.pending = Some(p);
        self.dirty = true;
    }

    fn on_listing(&mut self, generation: u64, dir: PathBuf, result: Result<listing::Listing, String>) {
        if generation != self.nav_gen.load(Ordering::SeqCst) {
            return;
        }
        let pending = self.pending.take();
        let l = match result {
            Ok(l) => l,
            Err(why) => {
                match pending {
                    // the folder couldn't be opened: stay where you are
                    Some(p) if !p.committed => self.say(&why),
                    _ => {
                        // what you were in has gone
                        self.loading = false;
                        self.say(&why);
                        match dir.parent().filter(|_| dir != self.env.root) {
                            Some(up) => self.go(up.to_path_buf(), Select::First, None),
                            None => self.to_places(),
                        }
                    }
                }
                return;
            }
        };
        let old_name = self.entries.get(self.sel).map(|e| e.name.clone());
        let old_sel = self.sel;
        let select = pending.as_ref().map(|p| p.select.clone()).unwrap_or(Select::Keep);
        let slide = pending.as_ref().and_then(|p| p.slide);
        let moved = self.dir != dir;
        self.dir = dir;
        self.entries = l.entries;
        self.hidden_count = l.hidden_count;
        self.truncated = l.truncated;
        self.loading = false;
        if !matches!(self.screen, Screen::Storage | Screen::Image { .. } | Screen::Text { .. }) || pending.is_some() {
            self.screen = Screen::Browse;
        }
        self.sel = match &select {
            Select::First => 0,
            Select::Name(n) => self.entries.iter().position(|e| e.name == *n).unwrap_or(0),
            Select::Keep => old_name
                .and_then(|n| self.entries.iter().position(|e| e.name == n))
                .unwrap_or(old_sel.min(self.entries.len().saturating_sub(1))),
        };
        // marks of what's gone go too
        let names: HashSet<&OsStr> = self.entries.iter().map(|e| e.name.as_os_str()).collect();
        self.marks.retain(|m| names.contains(m.as_os_str()));
        self.scroll_from = self.sel as f32;
        self.scroll_start = 0;
        if moved || select != Select::Keep {
            self.panel = None;
        }
        self.preview_soon(true);
        if let Some(d) = slide {
            self.request_slide(d);
        }
        self.dirty = true;
    }

    fn to_places(&mut self) {
        self.overlay = None;
        self.scan_gen.fetch_add(1, Ordering::SeqCst);
        if matches!(self.screen, Screen::Places) {
            return;
        }
        self.refresh_place_facts();
        self.pending = None;
        self.nav_gen.fetch_add(1, Ordering::SeqCst);
        if !self.marks.is_empty() {
            self.marks.clear();
        }
        if !self.dir.as_os_str().is_empty()
            && let Some(e) = self.entries.get(self.sel)
        {
            self.remembered.insert(self.dir.clone(), e.name.clone());
        }
        self.dir = PathBuf::new();
        self.screen = Screen::Places;
        self.request_slide(SlideDir::Back);
    }

    /// Up one folder; from a place's top, back to Places.
    fn go_up(&mut self) {
        let top = self.places.iter().any(|p| p.kind != PlaceKind::Root && p.kind != PlaceKind::Storage && p.path == self.dir);
        if top || self.dir == self.env.root || self.dir.parent().is_none() {
            self.to_places();
            return;
        }
        let up = self.dir.parent().unwrap().to_path_buf();
        let name = self.dir.file_name().map(OsStr::to_os_string);
        self.go(up, name.map(Select::Name).unwrap_or(Select::First), Some(SlideDir::Back));
    }

    pub fn selected(&self) -> Option<&Entry> {
        self.entries.get(self.sel)
    }

    pub fn in_trash(&self) -> bool {
        self.dir == self.trash.files()
    }

    pub fn dir_read_only(&self) -> bool {
        self.guard.is_read_only(&self.dir)
    }

    pub fn targets(&self) -> Vec<&Entry> {
        if self.marks.is_empty() {
            self.selected().into_iter().collect()
        } else {
            self.entries.iter().filter(|e| self.marks.contains(&e.name)).collect()
        }
    }

    pub fn marked_size(&self) -> u64 {
        self.entries.iter().filter(|e| self.marks.contains(&e.name) && !e.is_dir).map(|e| e.size).sum()
    }

    fn move_sel(&mut self, to: usize) {
        if self.entries.is_empty() {
            return;
        }
        let to = to.min(self.entries.len() - 1);
        if to == self.sel {
            return;
        }
        self.scroll_from = self.scroll_pos();
        self.scroll_start = self.now;
        self.sel = to;
        self.preview_soon(false);
        self.dirty = true;
    }

    // ---------------------------------------------------------- preview ----

    fn preview_soon(&mut self, now: bool) {
        self.preview_gen.fetch_add(1, Ordering::SeqCst);
        let Some(e) = self.entries.get(self.sel) else {
            self.panel = None;
            self.preview_due = None;
            return;
        };
        let key = self.dir.join(&e.name);
        if let Some(c) = self.cache.iter().find(|c| c.0 == key && c.1 == e.mtime) {
            self.panel = Some((key, c.2.clone()));
            self.preview_due = None;
            return;
        }
        if !self.panel.as_ref().is_some_and(|p| p.0 == key) {
            self.panel = None;
        }
        self.preview_due = Some(if now { self.now } else { self.now + PREVIEW_DEBOUNCE_MS });
    }

    fn start_preview(&mut self) {
        let Some(e) = self.entries.get(self.sel).cloned() else { return };
        let dir = self.dir.clone();
        let generation = self.preview_gen.load(Ordering::SeqCst);
        let (g, games) = (self.preview_gen.clone(), self.games.clone());
        static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
        self.work(move |tx, _| {
            let _one = ONE_AT_A_TIME.lock();
            let stop = || g.load(Ordering::SeqCst) != generation;
            if stop() {
                return;
            }
            let preview = preview::panel(&dir, &e, &games, crate::view::ART, &stop);
            if !stop() {
                let _ = tx.send(Bg::Preview { key: dir.join(&e.name), generation, preview });
            }
        });
    }

    fn on_preview(&mut self, key: PathBuf, generation: u64, preview: Preview) {
        let mtime = self.entries.get(self.sel).map(|e| e.mtime).unwrap_or(0);
        if self.cache.len() >= 16 {
            self.cache.remove(0);
        }
        self.cache.retain(|c| c.0 != key);
        self.cache.push((key.clone(), mtime, preview.clone()));
        if generation == self.preview_gen.load(Ordering::SeqCst) {
            self.panel = Some((key, preview));
        }
    }

    // ------------------------------------------------------------- open ----

    fn open_selected(&mut self) {
        let Some(e) = self.selected().cloned() else { return };
        if self.in_trash() {
            self.open_actions();
            return;
        }
        let path = self.dir.join(&e.name);
        if e.broken {
            return self.say("This link points at nothing.");
        }
        if e.is_dir {
            let sel = self.remembered.get(&path).cloned().map(Select::Name).unwrap_or(Select::First);
            return self.go(path, sel, Some(SlideDir::Forward));
        }
        if e.special {
            return self.say("That's not a regular file. It can't be opened.");
        }
        if self.play.as_ref().is_some_and(|p| p.in_rom_folder(&path)) {
            return self.open_game(&path);
        }
        match e.kind {
            Kind::Image => self.open_image(self.sel),
            Kind::Game => self.open_game(&path),
            Kind::Text => self.open_text(&e, path),
            _ => {
                let is_text = matches!(&self.panel, Some((k, Preview::Text(_))) if *k == path);
                if is_text {
                    self.open_text(&e, path);
                } else {
                    let what = Path::new(&e.name).extension().map(|x| format!(".{} files", x.to_string_lossy())).unwrap_or_else(|| "this file".into());
                    self.say(&format!("Nothing here can open {what}."));
                }
            }
        }
    }

    fn open_game(&mut self, path: &Path) {
        let Some(play) = &self.play else {
            return self.say("There's no emulator on this system.");
        };
        let Some(system) = play.system_for(path).map(String::from) else {
            return self.say("Mortar doesn't know which system plays this.");
        };
        let Some(s) = path.to_str().filter(|s| !s.contains('\n') && s.trim() == *s) else {
            return self.say("This name is too unusual to start from here.");
        };
        let tx = self.tx.clone();
        let wake = self.wake.clone();
        self.shell.send(format!("game {system} {s}"), move |r| {
            if let Err(e) = r {
                let _ = tx.send(Bg::Note(format!("Couldn't start it: {e}")));
                wake();
            }
        });
    }

    fn open_image(&mut self, index: usize) {
        let Some(e) = self.entries.get(index) else { return };
        let p = self.dir.join(&e.name);
        let generation = self.view_gen.fetch_add(1, Ordering::SeqCst) + 1;
        let was_viewer = matches!(self.screen, Screen::Image { .. });
        self.screen = Screen::Image { index, pic: None };
        self.work(move |tx, _| {
            let result = std::panic::catch_unwind(|| preview::load_image(&p, W as u32, H as u32)).unwrap_or_else(|_| Err("This picture is damaged.".into())).map(Arc::new);
            let _ = tx.send(Bg::Image { generation, result });
        });
        if !was_viewer {
            self.request_slide(SlideDir::Up);
        }
        self.dirty = true;
    }

    fn open_text(&mut self, e: &Entry, path: PathBuf) {
        let generation = self.view_gen.fetch_add(1, Ordering::SeqCst) + 1;
        self.screen = Screen::Text { name: e.display.clone(), lines: None, cut: false, top: 0 };
        self.work(move |tx, _| {
            let _ = tx.send(Bg::Text { generation, result: preview::load_text(&path, crate::view::TEXT_COLS) });
        });
        self.request_slide(SlideDir::Up);
    }

    // ---------------------------------------------------------- actions ----

    fn action_rows(&self) -> Vec<Act> {
        let mut v = Vec::new();
        let some = !self.entries.is_empty();
        if self.in_trash() {
            if some {
                v.extend([Act::Restore, Act::DeleteForever, Act::EmptyTrash]);
            }
            v.push(Act::Sort);
            return v;
        }
        let ro = self.dir_read_only();
        if self.carry.is_some() {
            if !ro {
                v.push(Act::Paste);
            }
            v.push(Act::Forget);
            if !ro {
                v.push(Act::NewFolder);
            }
        } else {
            if some {
                v.push(Act::Copy);
                if !ro {
                    v.push(Act::Move);
                    if self.marks.len() <= 1 {
                        v.push(Act::Rename);
                    }
                    v.push(Act::Delete);
                }
            }
            if !ro {
                v.push(Act::NewFolder);
            }
            if some {
                v.push(if self.marks.is_empty() { Act::MarkAll } else { Act::ClearMarks });
            }
        }
        v.extend([Act::StorageHere, Act::Sort, Act::Hidden]);
        v
    }

    fn open_actions(&mut self) {
        let rows = self.action_rows();
        self.overlay = Some(Overlay::Actions { sel: 0, rows });
        self.request_slide(SlideDir::PanelIn);
    }

    fn run_action(&mut self, a: Act) {
        match a {
            Act::Copy | Act::Move => {
                let t = self.targets();
                let sources = t.iter().map(|e| self.dir.join(&e.name)).collect();
                let kinds = t.iter().map(|e| e.kind).collect();
                let n = t.len();
                self.carry = Some(Carry { moving: a == Act::Move, sources, kinds });
                self.marks.clear();
                self.close_overlay();
                self.say(&if n == 1 { "In hand. Go where it belongs, then press Y.".to_string() } else { format!("{n} in hand. Go where they belong, then press Y.") });
            }
            Act::Rename => {
                let Some(e) = self.targets().first().map(|e| (*e).clone()) else { return };
                let path = self.dir.join(&e.name);
                if let Err(w) = self.guard.check_removable(&path) {
                    self.close_overlay();
                    return self.say(&w);
                }
                // the cursor before the extension: it's the name you're changing
                let cursor = if e.is_dir { None } else { e.display.rfind('.').filter(|i| *i > 0).map(|i| e.display[..i].chars().count()) };
                self.overlay = Some(Overlay::Naming(Naming {
                    purpose: NameFor::Rename { path, original: e.display.clone() },
                    field: Field::new(&e.display, cursor),
                    error: None,
                    b_armed: true,
                }));
                self.request_slide(SlideDir::Up);
                self.want_keyboard(true);
            }
            Act::NewFolder => {
                self.overlay = Some(Overlay::Naming(Naming { purpose: NameFor::NewFolder, field: Field::new("New folder", None), error: None, b_armed: true }));
                self.request_slide(SlideDir::Up);
                self.want_keyboard(true);
            }
            Act::Delete | Act::DeleteForever => self.ask_delete(a == Act::DeleteForever),
            Act::EmptyTrash => {
                let sources = self.entries.iter().map(|e| self.dir.join(&e.name)).collect();
                let names = self.entries.iter().take(4).map(|e| self.trash_name(e)).collect();
                let kinds = self.entries.iter().take(200).map(|e| e.kind).collect();
                self.overlay = Some(Overlay::Confirm(Confirm { forever: true, sources, names, kinds, reason: None, empty_trash: true, held_since: None, nudge: false }));
                self.request_slide(SlideDir::Up);
            }
            Act::MarkAll => {
                self.marks = self.entries.iter().map(|e| e.name.clone()).collect();
                self.close_overlay();
            }
            Act::ClearMarks => {
                self.marks.clear();
                self.close_overlay();
            }
            Act::Paste => {
                self.paste();
            }
            Act::Forget => {
                self.carry = None;
                self.close_overlay();
            }
            Act::StorageHere => {
                self.close_overlay();
                self.open_storage(Some(self.dir.clone()), false);
            }
            Act::Sort => {
                self.set_sort(self.sort.next());
            }
            Act::Hidden => {
                self.show_hidden = !self.show_hidden;
                self.read_dir(self.dir.clone());
                self.dirty = true;
            }
            Act::Restore => {
                let sources: Vec<PathBuf> = self.targets().iter().map(|e| self.dir.join(&e.name)).collect();
                let kinds = self.targets().iter().map(|e| e.kind).collect();
                self.close_overlay();
                self.start_job(JobKind::Restore, sources, None, kinds);
            }
        }
    }

    fn set_sort(&mut self, s: Sort) {
        self.sort = s;
        self.read_dir(self.dir.clone());
        self.dirty = true;
    }

    /// The name something had before it went in the Trash.
    pub fn trash_name(&self, e: &Entry) -> String {
        match self.trash.read_info(&e.name) {
            Some(i) => i.origin.file_name().map(listing::display).unwrap_or_else(|| e.display.clone()),
            None => e.display.clone(),
        }
    }

    fn close_overlay(&mut self) {
        let was_actions = matches!(self.overlay, Some(Overlay::Actions { .. }));
        self.overlay = None;
        self.request_slide(if was_actions { SlideDir::PanelOut } else { SlideDir::Down });
    }

    fn ask_delete(&mut self, forever: bool) {
        let t = self.targets();
        let sources: Vec<PathBuf> = t.iter().map(|e| self.dir.join(&e.name)).collect();
        let in_trash = self.in_trash();
        let names: Vec<String> = t.iter().take(4).map(|e| if in_trash { self.trash_name(e) } else { e.display.clone() }).collect();
        let kinds: Vec<Kind> = t.iter().take(200).map(|e| e.kind).collect();
        // anything that can't be removed is said before anyone confirms
        if !in_trash
            && let Some(why) = sources.iter().find_map(|s| self.guard.check_removable(s).err())
        {
            self.close_overlay();
            return self.say(&why);
        }
        let mut forever = forever;
        let mut reason = None;
        if !forever && sources.iter().any(|s| !self.trash.can_take(s)) {
            forever = true;
            reason = Some("This disk has no Trash.".to_string());
        }
        self.overlay = Some(Overlay::Confirm(Confirm { forever, sources, names, kinds, reason, empty_trash: false, held_since: None, nudge: false }));
        self.request_slide(SlideDir::Up);
    }

    fn confirm_go(&mut self) {
        let Some(Overlay::Confirm(c)) = self.overlay.take() else { return };
        let kind = if c.forever { JobKind::Delete } else { JobKind::Trash };
        self.start_job(kind, c.sources, None, c.kinds);
    }

    fn paste(&mut self) {
        let Some(c) = self.carry.take() else { return };
        let kind = if c.moving { JobKind::Move } else { JobKind::Copy };
        self.overlay = None;
        self.start_job(kind, c.sources, Some(self.dir.clone()), c.kinds);
    }

    fn start_job(&mut self, kind: JobKind, sources: Vec<PathBuf>, dest: Option<PathBuf>, kinds: Vec<Kind>) {
        let count = sources.len();
        let spec = JobSpec { kind, sources, dest, trash: Some(self.trash.root.clone()) };
        let job = ops::spawn(spec, self.guard.clone(), self.wake.clone());
        self.job = Some(JobRun { kind, job, progress: Progress::default(), clash: None, cancelling: false, started: self.now, shown: false, kinds, count });
        self.marks.clear();
        self.after_job = Some((self.dir.clone(), Select::Keep));
        self.dirty = true;
    }

    fn poll_job(&mut self) -> bool {
        let Some(j) = self.job.as_mut() else { return false };
        let mut changed = false;
        let mut done = None;
        while let Ok(m) = j.job.rx.try_recv() {
            changed = true;
            match m {
                JobMsg::Progress(p) => j.progress = p,
                JobMsg::Clash(c) => {
                    let sel = 0;
                    j.clash = Some(ClashUi { clash: c, sel, for_rest: false });
                    let shell = self.shell.clone();
                    if !self.focused {
                        shell.attention(Some("A name is taken: copy or skip?"));
                    }
                }
                JobMsg::Done(r) => done = Some(r),
            }
        }
        if let Some(r) = done {
            self.finish_job(r);
        }
        changed
    }

    fn finish_job(&mut self, r: Report) {
        self.job = None;
        self.shell.attention(None);
        let select = r.landed.first().filter(|_| matches!(r.kind, JobKind::Copy | JobKind::Move)).cloned().map(Select::Name).unwrap_or(Select::Keep);
        if matches!(self.screen, Screen::Browse) {
            self.pending = Some(PendingNav { dir: self.dir.clone(), select, slide: None, since: self.now, committed: true });
            self.read_dir(self.dir.clone());
        }
        if r.clean() {
            self.say(&r.headline());
        } else {
            self.overlay = Some(Overlay::Report(r));
        }
        self.after_job = None;
    }

    /// The current job's screen should show: quick ones never do.
    pub fn job_visible(&self) -> bool {
        self.job.as_ref().is_some_and(|j| j.clash.is_some() || j.shown)
    }

    // ---------------------------------------------------------- storage ----

    pub fn open_storage(&mut self, dir: Option<PathBuf>, from_places: bool) {
        self.storage_from_places = from_places;
        self.levels.clear();
        self.push_storage(dir, "Storage".into());
        self.screen = Screen::Storage;
        self.request_slide(SlideDir::Forward);
    }

    fn push_storage(&mut self, dir: Option<PathBuf>, title: String) {
        let title = dir.as_ref().and_then(|d| d.file_name().map(listing::display)).unwrap_or(title);
        self.levels.push(StorageLevel { title, dir: dir.clone(), tiles: Vec::new(), rects: Vec::new(), sel: 0, moved: false, scanning: true, error: None });
        let generation = self.scan_gen.fetch_add(1, Ordering::SeqCst) + 1;
        let (g, games) = (self.scan_gen.clone(), self.games.clone());
        let roots: Vec<(String, PathBuf)> = self.places.iter().filter(|p| matches!(p.kind, PlaceKind::Home | PlaceKind::Games | PlaceKind::Saves | PlaceKind::Bios)).map(|p| (p.name.clone(), p.path.clone())).collect();
        let disk = self.env.root.clone();
        self.work(move |tx, wake| {
            let stop = || g.load(Ordering::SeqCst) != generation;
            let mut emit = |update: ScanUpdate| {
                let _ = tx.send(Bg::Scan { generation, update });
                wake();
            };
            match &dir {
                Some(d) => storage::scan_dir(d, &games, &stop, &mut emit),
                None => storage::scan_overview(&roots, &disk, &games, &stop, &mut emit),
            }
        });
    }

    fn on_scan(&mut self, generation: u64, update: ScanUpdate) {
        if generation != self.scan_gen.load(Ordering::SeqCst) {
            return;
        }
        let Some(l) = self.levels.last_mut() else { return };
        let (tiles, scanning) = match update {
            ScanUpdate::Partial(v) => (v, true),
            ScanUpdate::Done(v) => (v, false),
            ScanUpdate::Failed(e) => {
                l.scanning = false;
                l.error = Some(e);
                return;
            }
        };
        let selected = l.tiles.get(l.sel).map(|t| t.name.clone());
        // the overview is your few folders: none gets gathered away
        l.tiles = if l.dir.is_some() { storage::gather(tiles, 0.015, 40) } else { tiles };
        // nothing thinner than a D-pad can land on
        let total: f64 = l.tiles.iter().map(|t| t.size as f64).sum();
        let sizes: Vec<f64> = l.tiles.iter().map(|t| (t.size as f64).max(total * 0.012)).collect();
        l.rects = storage::squarify(&sizes, STORAGE);
        let first = l.tiles.iter().position(|t| t.path.is_some()).unwrap_or(0);
        l.sel = if l.moved { selected.and_then(|n| l.tiles.iter().position(|t| t.name == n)).unwrap_or(first) } else { first };
        l.scanning = scanning;
    }

    fn storage_button(&mut self, b: Button) {
        let Some(l) = self.levels.last_mut() else { return };
        let dir = match b {
            Button::Up => Some(Dir::Up),
            Button::Down => Some(Dir::Down),
            Button::Left => Some(Dir::Left),
            Button::Right => Some(Dir::Right),
            _ => None,
        };
        if let Some(d) = dir {
            if let Some(n) = storage::nearest(&l.rects, l.sel, d) {
                l.sel = n;
                l.moved = true;
                self.dirty = true;
            }
            return;
        }
        match b {
            Button::A => {
                let Some(t) = l.tiles.get(l.sel).cloned() else { return };
                let from = l.dir.clone();
                match (t.kind, t.path) {
                    (TileKind::Folder, Some(p)) => {
                        self.push_storage(Some(p), t.name);
                        self.request_slide(SlideDir::Forward);
                    }
                    (TileKind::File(_), Some(p)) => {
                        let name = p.file_name().map(OsStr::to_os_string).map(Select::Name).unwrap_or(Select::First);
                        if let Some(d) = p.parent() {
                            self.show_hidden = self.show_hidden || p.file_name().is_some_and(|n| n.as_bytes().first() == Some(&b'.'));
                            self.levels.clear();
                            self.go(d.to_path_buf(), name, Some(SlideDir::Forward));
                        }
                    }
                    (TileKind::Others(_), _) => {
                        if let Some(d) = from {
                            self.sort = Sort::Size;
                            self.levels.clear();
                            self.go(d, Select::First, Some(SlideDir::Forward));
                        }
                    }
                    (TileKind::Free, _) => self.say("That's the room you have left."),
                    (TileKind::System, _) => self.say("That's Mortar itself, and apps. Nothing to tidy by hand."),
                    _ => {}
                }
            }
            Button::B | Button::Left => {
                self.scan_gen.fetch_add(1, Ordering::SeqCst);
                self.levels.pop();
                if self.levels.is_empty() {
                    if self.storage_from_places || self.dir.as_os_str().is_empty() {
                        self.screen = Screen::Places;
                    } else {
                        self.screen = Screen::Browse;
                        self.read_dir(self.dir.clone());
                    }
                    self.request_slide(SlideDir::Back);
                } else {
                    // the level below may not have finished; read it again
                    let l = self.levels.last_mut().unwrap();
                    if l.scanning {
                        let (d, t) = (l.dir.clone(), l.title.clone());
                        self.levels.pop();
                        self.push_storage(d, t);
                    }
                    self.request_slide(SlideDir::Back);
                }
            }
            Button::L2 => self.to_places(),
            _ => {}
        }
    }

    // ------------------------------------------------------------ input ----

    /// A raw key from the window.
    pub fn key(&mut self, code: u16, down: bool, repeat: bool) {
        for i in self.decoder.key(code, down, repeat) {
            self.input(i);
        }
    }

    /// Does this key auto-repeat right now? Only moving around, and deleting
    /// while naming. Nothing that acts on files.
    pub fn repeats(&self, code: u16) -> bool {
        let moving = matches!(code, keys::UP | keys::DOWN | keys::PAGEUP | keys::PAGEDOWN | keys::LEFT | keys::RIGHT);
        match &self.overlay {
            Some(Overlay::Naming(_)) | Some(Overlay::Find(_)) => moving || code == keys::BACKSPACE || code == keys::DELETE,
            Some(Overlay::Confirm(_)) | Some(Overlay::Report(_)) => false,
            Some(Overlay::Actions { .. }) => matches!(code, keys::UP | keys::DOWN),
            None => {
                if self.job.is_some() {
                    return false;
                }
                match self.screen {
                    Screen::Browse | Screen::Text { .. } => matches!(code, keys::UP | keys::DOWN | keys::PAGEUP | keys::PAGEDOWN),
                    _ => false,
                }
            }
        }
    }

    /// Focus went away: nothing stays held.
    pub fn focus(&mut self, on: bool) {
        self.focused = on;
        if !on {
            self.decoder.reset();
            if let Some(Overlay::Confirm(c)) = &mut self.overlay {
                c.held_since = None;
            }
        } else {
            // the world may have changed while it was behind
            if matches!(self.screen, Screen::Browse) && self.job.is_none() && self.overlay.is_none() {
                self.read_dir(self.dir.clone());
            } else if matches!(self.screen, Screen::Places) {
                self.refresh_place_facts();
            }
        }
        self.dirty = true;
    }

    /// The window changed size. Short means the on-screen keyboard is up.
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
            if self.keyboard_ours && !matches!(self.overlay, Some(Overlay::Naming(_))) {
                // what we opened it for is already over
                self.shell.toggle_keyboard();
                return;
            }
            // START from the list: find by name
            if matches!(self.screen, Screen::Browse) && self.overlay.is_none() && self.job.is_none() {
                self.overlay = Some(Overlay::Find(Field::default()));
            }
        } else {
            self.keyboard_ours = false;
            if matches!(self.overlay, Some(Overlay::Find(_))) {
                self.overlay = None;
            }
        }
    }

    fn want_keyboard(&mut self, on: bool) {
        if on {
            if !self.short && self.shell.present() {
                self.keyboard_ours = true;
                self.shell.toggle_keyboard();
            }
        } else if self.short {
            self.shell.toggle_keyboard();
        }
    }

    pub fn input(&mut self, i: Input) {
        self.dirty = true;
        if self.job.is_some() {
            return self.job_input(i);
        }
        if self.pending.as_ref().is_some_and(|p| !p.committed) {
            return;
        }
        // a press ends a slide at once: input always wins over motion
        if matches!(i, Input::Press(..)) {
            self.slide = None;
        }
        match &self.overlay {
            Some(Overlay::Actions { .. }) => return self.actions_input(i),
            Some(Overlay::Confirm(_)) => return self.confirm_input(i),
            Some(Overlay::Naming(_)) => return self.naming_input(i),
            Some(Overlay::Find(_)) => return self.find_input(i),
            Some(Overlay::Report(_)) => {
                if matches!(i, Input::Press(Button::A | Button::B, false)) {
                    self.overlay = None;
                    self.request_slide(SlideDir::Down);
                }
                return;
            }
            None => {}
        }
        match i {
            Input::Press(b, repeat) => match self.screen {
                Screen::Places => self.places_button(b, repeat),
                Screen::Browse => self.browse_button(b, repeat),
                Screen::Image { .. } => self.image_button(b),
                Screen::Text { .. } => self.text_button(b),
                Screen::Storage => {
                    if !repeat {
                        self.storage_button(b)
                    }
                }
            },
            Input::Char(c) => {
                // a real keyboard: start finding by name
                if matches!(self.screen, Screen::Browse) {
                    let mut f = Field::default();
                    f.insert(c);
                    self.overlay = Some(Overlay::Find(f));
                    self.jump_to_match();
                }
            }
            _ => {}
        }
    }

    pub fn places_button(&mut self, b: Button, repeat: bool) {
        if repeat {
            return;
        }
        let rects = crate::view::place_rects(self.places.len());
        let d = match b {
            Button::Up => Some(Dir::Up),
            Button::Down => Some(Dir::Down),
            Button::Left => Some(Dir::Left),
            Button::Right => Some(Dir::Right),
            _ => None,
        };
        if let Some(d) = d {
            if let Some(n) = storage::nearest(&rects, self.place_sel, d) {
                self.place_sel = n;
            }
            return;
        }
        if b != Button::A {
            return;
        }
        let Some(p) = self.places.get(self.place_sel).cloned() else { return };
        match p.kind {
            PlaceKind::Storage => self.open_storage(None, true),
            PlaceKind::Trash => {
                let _ = std::fs::create_dir_all(self.trash.files());
                let _ = std::fs::create_dir_all(self.trash.info());
                self.go(p.path, Select::First, Some(SlideDir::Forward));
            }
            _ => {
                let sel = self.remembered.get(&p.path).cloned().map(Select::Name).unwrap_or(Select::First);
                self.go(p.path, sel, Some(SlideDir::Forward));
            }
        }
    }

    fn browse_button(&mut self, b: Button, repeat: bool) {
        let n = self.entries.len();
        match b {
            Button::Up if self.sel > 0 => self.move_sel(self.sel - 1),
            Button::Down => self.move_sel(self.sel + 1),
            Button::L1 => self.move_sel(self.sel.saturating_sub(PAGE)),
            Button::R1 => self.move_sel(self.sel + PAGE),
            _ if repeat => {}
            Button::A => self.open_selected(),
            Button::Right => {
                if self.selected().is_some_and(|e| e.is_dir) && !self.in_trash() {
                    self.open_selected();
                }
            }
            Button::B | Button::Left => self.go_up(),
            Button::L2 => self.to_places(),
            Button::X => self.open_actions(),
            Button::Y => {
                if self.carry.is_some() {
                    if self.dir_read_only() {
                        self.say("This place is read-only.");
                    } else if !self.in_trash() {
                        self.paste();
                    }
                } else if n > 0 {
                    let name = self.entries[self.sel].name.clone();
                    if !self.marks.remove(&name) {
                        self.marks.insert(name);
                    }
                    self.move_sel(self.sel + 1);
                }
            }
            Button::R2 => self.set_sort(self.sort.next()),
            Button::Select => {
                if !self.in_trash() {
                    self.open_storage(Some(self.dir.clone()), false);
                }
            }
            _ => {}
        }
    }

    fn image_button(&mut self, b: Button) {
        let Screen::Image { index, .. } = self.screen else { return };
        let step = |from: usize, forward: bool, e: &[Entry]| -> Option<usize> {
            let mut i = from;
            loop {
                i = if forward { i + 1 } else { i.checked_sub(1)? };
                let x = e.get(i)?;
                if x.kind == Kind::Image {
                    return Some(i);
                }
            }
        };
        match b {
            Button::Left | Button::L1 | Button::Up => {
                if let Some(i) = step(index, false, &self.entries) {
                    self.sel = i;
                    self.scroll_from = i as f32;
                    self.open_image(i);
                    self.request_slide(SlideDir::Back);
                }
            }
            Button::Right | Button::R1 | Button::Down => {
                if let Some(i) = step(index, true, &self.entries) {
                    self.sel = i;
                    self.scroll_from = i as f32;
                    self.open_image(i);
                    self.request_slide(SlideDir::Forward);
                }
            }
            Button::B | Button::A => {
                self.view_gen.fetch_add(1, Ordering::SeqCst);
                self.screen = Screen::Browse;
                self.preview_soon(true);
                self.request_slide(SlideDir::Down);
            }
            Button::L2 => self.to_places(),
            _ => {}
        }
    }

    fn text_button(&mut self, b: Button) {
        let page = crate::view::TEXT_ROWS;
        let Screen::Text { lines, top, .. } = &mut self.screen else { return };
        let n = lines.as_ref().and_then(|l| l.as_ref().ok()).map(|l| l.len()).unwrap_or(0);
        let max = n.saturating_sub(page);
        match b {
            Button::Up => *top = top.saturating_sub(1),
            Button::Down => *top = (*top + 1).min(max),
            Button::L1 | Button::Left => *top = top.saturating_sub(page),
            Button::R1 | Button::Right => *top = (*top + page).min(max),
            Button::B | Button::A => {
                self.view_gen.fetch_add(1, Ordering::SeqCst);
                self.screen = Screen::Browse;
                self.preview_soon(true);
                self.request_slide(SlideDir::Down);
            }
            Button::L2 => self.to_places(),
            _ => {}
        }
    }

    fn actions_input(&mut self, i: Input) {
        let Input::Press(b, _) = i else { return };
        let Some(Overlay::Actions { sel, rows }) = &mut self.overlay else { return };
        let n = rows.len();
        match b {
            Button::Up => *sel = (*sel + n - 1) % n,
            Button::Down => *sel = (*sel + 1) % n,
            Button::A => {
                let a = rows[*sel];
                self.run_action(a);
            }
            Button::Left | Button::Right => {
                let a = rows[*sel];
                match a {
                    Act::Sort => {
                        let s = if b == Button::Right { self.sort.next() } else { self.sort.prev() };
                        self.set_sort(s);
                    }
                    Act::Hidden => self.run_action(Act::Hidden),
                    _ => {}
                }
            }
            Button::B | Button::X => self.close_overlay(),
            Button::L2 => self.to_places(),
            _ => {}
        }
    }

    /// Y for the Trash. A does nothing here on purpose: you got here with A,
    /// and a second press by accident must not do it. Deleting forever takes
    /// L1 and R1 held together.
    fn confirm_input(&mut self, i: Input) {
        let now = self.now;
        let Some(Overlay::Confirm(c)) = &mut self.overlay else { return };
        match i {
            Input::Shoulders { l1, r1 } => {
                if !c.forever {
                    return;
                }
                if l1 && r1 {
                    c.held_since.get_or_insert(now);
                } else {
                    c.held_since = None;
                }
            }
            Input::Press(Button::Y, false) if !c.forever => self.confirm_go(),
            Input::Press(Button::B, false) => self.close_overlay(),
            Input::Press(Button::L1 | Button::R1, _) => {}
            Input::Press(_, false) => c.nudge = true,
            _ => {}
        }
    }

    fn naming_input(&mut self, i: Input) {
        let Some(Overlay::Naming(n)) = &mut self.overlay else { return };
        match i {
            Input::Char(c) => {
                n.field.insert(c);
                n.error = None;
            }
            Input::Press(Button::Y, _) => {
                // Space, on the pad and the OSK's X
                n.field.insert(' ');
                n.error = None;
            }
            Input::Press(Button::B, repeat) => {
                if !n.field.backspace() && !repeat && n.b_armed {
                    self.want_keyboard(false);
                    self.close_overlay();
                    return;
                }
                n.b_armed = !repeat && n.field.text.is_empty();
                n.error = None;
            }
            Input::Press(Button::Left, _) => n.field.cursor = n.field.cursor.saturating_sub(1),
            Input::Press(Button::Right, _) => n.field.cursor = (n.field.cursor + 1).min(n.field.text.len()),
            Input::Home => n.field.cursor = 0,
            Input::End => n.field.cursor = n.field.text.len(),
            Input::Delete => {
                if n.field.cursor < n.field.text.len() {
                    n.field.text.remove(n.field.cursor);
                }
            }
            Input::Press(Button::L2, _) => {
                self.want_keyboard(false);
                self.close_overlay();
            }
            Input::Press(Button::A, false) => self.naming_done(),
            _ => {}
        }
    }

    fn naming_done(&mut self) {
        let Some(Overlay::Naming(n)) = &mut self.overlay else { return };
        let name = n.field.string();
        let name = name.trim().to_string();
        let result = match &n.purpose {
            NameFor::Rename { original, .. } if *original == name => {
                // no change: leave a name that isn't UTF-8 alone
                self.want_keyboard(false);
                self.close_overlay();
                return;
            }
            NameFor::Rename { path, .. } => ops::rename(&self.guard, path, &name),
            NameFor::NewFolder => ops::new_folder(&self.guard, &self.dir, &name),
        };
        match result {
            Ok(p) => {
                self.want_keyboard(false);
                self.overlay = None;
                let select = p.file_name().map(OsStr::to_os_string).map(Select::Name).unwrap_or(Select::Keep);
                self.pending = Some(PendingNav { dir: self.dir.clone(), select, slide: None, since: self.now, committed: true });
                self.read_dir(self.dir.clone());
                self.request_slide(SlideDir::Down);
            }
            Err(e) => n.error = Some(e),
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
            Input::Press(Button::Up, _) if self.sel > 0 => return self.move_sel(self.sel - 1),
            Input::Press(Button::Down, _) => return self.move_sel(self.sel + 1),
            Input::Press(Button::A | Button::L2, false) => {
                self.overlay = None;
                self.want_keyboard(false);
                return;
            }
            _ => return,
        }
        self.jump_to_match();
    }

    fn jump_to_match(&mut self) {
        let Some(Overlay::Find(f)) = &self.overlay else { return };
        let q = f.string().to_lowercase();
        if q.is_empty() {
            return;
        }
        let hit = self.entries.iter().position(|e| e.display.to_lowercase().starts_with(&q)).or_else(|| self.entries.iter().position(|e| e.display.to_lowercase().contains(&q)));
        if let Some(i) = hit {
            self.move_sel(i);
        }
    }

    fn job_input(&mut self, i: Input) {
        let Some(j) = &mut self.job else { return };
        let Input::Press(b, repeat) = i else { return };
        if repeat {
            return;
        }
        if let Some(c) = &mut j.clash {
            let n = c.clash.options.len();
            match b {
                Button::Left => c.sel = (c.sel + n - 1) % n,
                Button::Right => c.sel = (c.sel + 1) % n,
                Button::Y => c.for_rest = !c.for_rest,
                Button::A => {
                    let a = Answer { choice: Some(c.clash.options[c.sel]), for_rest: c.for_rest };
                    let _ = j.job.answers.send(a);
                    j.clash = None;
                    self.shell.attention(None);
                }
                Button::B => {
                    j.cancelling = true;
                    j.job.cancel.store(true, Ordering::Relaxed);
                    let _ = j.job.answers.send(Answer { choice: None, for_rest: false });
                    j.clash = None;
                    self.shell.attention(None);
                }
                _ => {}
            }
            return;
        }
        if b == Button::B {
            j.cancelling = true;
            j.job.cancel.store(true, Ordering::Relaxed);
        }
    }
}

pub fn choice_word(c: Choice) -> &'static str {
    match c {
        Choice::Skip => "Skip it",
        Choice::KeepBoth => "Keep both",
        Choice::Replace => "Replace",
        Choice::Merge => "Merge folders",
    }
}
