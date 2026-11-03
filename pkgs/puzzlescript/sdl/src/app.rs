// SPDX-License-Identifier: MIT
//! Screens and game lifecycle: game browser, loading, play, level select, pause menu, compile
//! errors. Platform-free: it reads abstract buttons (input::State) and draws into a Canvas.
use crate::audio::Audio;
use crate::ffi::{self, Game, PSStorage, Session};
use crate::gfx::{self, Canvas, GameRender};
use crate::input::{Btn, State};
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::time::Duration;

// theme
const BG: u32 = 0x14161a;
const PANEL: u32 = 0x23272e;
const ROW_HI: u32 = 0x343b47;
const ACCENT: u32 = 0xf2c14e;
const TEXT: u32 = 0xe8e8e8;
const DIM: u32 = 0x8a9099;
const ERR: u32 = 0xff6b5e;

const HOLD_SHOW: f32 = 0.25;
const HOLD_RESTART: f32 = 1.0;

pub struct Entry {
    pub file: String,
    pub title: String,
    pub author: Option<String>,
    pub has_save: bool,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Screen {
    Browser,
    Loading,
    Play,
    Levels,
    Menu,
    Error,
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Item {
    Resume,
    Restart,
    Undo,
    LevelSelect,
    TitleScreen,
    Games,
    Sound,
    Fullscreen,
    Quit,
}

impl Item {
    fn label(self, app: &App) -> String {
        match self {
            Item::Resume => "Resume".into(),
            Item::Restart => "Restart level".into(),
            Item::Undo => "Undo".into(),
            Item::LevelSelect => "Level select".into(),
            Item::TitleScreen => "Title screen".into(),
            Item::Games => "Back to games".into(),
            Item::Sound => format!("Sound: {}", if app.audio.muted { "off" } else { "on" }),
            Item::Fullscreen => "Toggle fullscreen".into(),
            Item::Quit => "Quit".into(),
        }
    }
}

/// Everything belonging to one open game. Field order is drop order: the session refers to
/// the game and the storage block, so it goes first.
struct Loaded {
    session: Session,
    render: GameRender,
    game: Game,
    _storage: Box<PSStorage>,
    _save_dir: Box<CString>,
    file: String,
}

pub struct App {
    pub games_dir: PathBuf,
    pub data_dir: PathBuf,
    pub audio: Audio,
    pub screen: Screen,
    pub quit: bool,
    pub want_fullscreen_toggle: bool,
    games: Vec<Entry>,
    sel: usize,
    scroll: f32,
    loaded: Option<Loaded>,
    error: Option<(String, Vec<String>)>,
    err_scroll: i32,
    ls_sel: usize,
    ls_scroll: f32,
    menu_items: Vec<Item>,
    menu_sel: usize,
    menu_from: Screen,
    b_held: Option<f32>, // seconds B has been held in play (None: not held / consumed)
    repeat: Option<(Btn, f32)>,
    toast: Option<(String, f32)>,
    dirty: bool,
}

// ---- storage hooks (PSStorage over files: <data>/saves/<game file>/<key>) ------------------

fn key_file(ctx: *mut c_void, key: *const c_char) -> PathBuf {
    let dir = unsafe { &*(ctx as *const CString) };
    let key = unsafe { CStr::from_ptr(key) }.to_string_lossy();
    let mut k: String = key.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    if k.is_empty() {
        k = "_level".into(); // "" is the current-level key
    }
    PathBuf::from(dir.to_string_lossy().into_owned()).join(k)
}

unsafe extern "C" fn st_get(ctx: *mut c_void, key: *const c_char) -> *mut c_char {
    match std::fs::read_to_string(key_file(ctx, key)) {
        Ok(s) => ffi::ps_string(&s),
        Err(_) => std::ptr::null_mut(),
    }
}

unsafe extern "C" fn st_set(ctx: *mut c_void, key: *const c_char, value: *const c_char) {
    let p = key_file(ctx, key);
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let v = CStr::from_ptr(value).to_bytes();
    if let Err(e) = std::fs::write(&p, v) {
        eprintln!("save {}: {e}", p.display());
    }
}

unsafe extern "C" fn st_remove(ctx: *mut c_void, key: *const c_char) {
    let _ = std::fs::remove_file(key_file(ctx, key));
}

// ---- game list -------------------------------------------------------------------------------

/// title/author from the prelude (before OBJECTS), skipping (nested) comments.
fn scan_meta(src: &str) -> (Option<String>, Option<String>) {
    let (mut title, mut author) = (None, None);
    let mut depth = 0;
    for raw in src.lines() {
        let mut line = String::new();
        for c in raw.chars() {
            match c {
                '(' => depth += 1,
                ')' if depth > 0 => depth -= 1,
                _ if depth == 0 => line.push(c),
                _ => {}
            }
        }
        let l = line.trim();
        let mut it = l.splitn(2, char::is_whitespace);
        let word = it.next().unwrap_or("").to_ascii_lowercase();
        let rest = it.next().unwrap_or("").trim();
        match word.as_str() {
            "objects" => break,
            "title" if title.is_none() && !rest.is_empty() => title = Some(rest.to_string()),
            "author" if author.is_none() && !rest.is_empty() => author = Some(rest.to_string()),
            _ => {}
        }
    }
    (title, author)
}

pub fn scan_games(dir: &Path, data: &Path) -> Vec<Entry> {
    let mut v = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        eprintln!("games directory {} not readable", dir.display());
        return v;
    };
    for e in rd.flatten() {
        let file = e.file_name().to_string_lossy().into_owned();
        if !file.to_ascii_lowercase().ends_with(".txt") {
            continue;
        }
        let mut head = Vec::new();
        if let Ok(f) = std::fs::File::open(e.path()) {
            use std::io::Read;
            let _ = f.take(8192).read_to_end(&mut head);
        }
        let (title, author) = scan_meta(&String::from_utf8_lossy(&head));
        let save = data.join("saves").join(&file);
        let has_save = std::fs::read_dir(&save).map(|mut d| d.next().is_some()).unwrap_or(false);
        let title = title.unwrap_or_else(|| file[..file.len() - 4].to_string());
        v.push(Entry { file, title, author, has_save });
    }
    v.sort_by_key(|e| e.title.to_lowercase());
    v
}

fn target_scroll(sel: usize, count: usize, visible: usize) -> f32 {
    let t = sel as f32 - (visible / 2) as f32;
    t.min(count as f32 - visible as f32).max(0.0)
}

fn ease(pos: &mut f32, target: f32) -> bool {
    let d = target - *pos;
    if d.abs() < 0.01 {
        let changed = *pos != target;
        *pos = target;
        return changed;
    }
    *pos += d * 0.35;
    true
}

/// UI scale: 4 at 1024x768.
fn ui_scale(cv: &Canvas) -> i32 {
    (cv.w / 256).min(cv.h / 192).max(1)
}

impl App {
    pub fn new(games_dir: PathBuf, data_dir: PathBuf, audio: Audio) -> App {
        let games = scan_games(&games_dir, &data_dir);
        let mut app = App {
            games_dir,
            data_dir,
            audio,
            screen: Screen::Browser,
            quit: false,
            want_fullscreen_toggle: false,
            games,
            sel: 0,
            scroll: 0.0,
            loaded: None,
            error: None,
            err_scroll: 0,
            ls_sel: 0,
            ls_scroll: 0.0,
            menu_items: Vec::new(),
            menu_sel: 0,
            menu_from: Screen::Browser,
            b_held: None,
            repeat: None,
            toast: None,
            dirty: true,
        };
        if let Ok(s) = std::fs::read_to_string(app.data_dir.join("settings.txt")) {
            for l in s.lines() {
                if let Some(f) = l.strip_prefix("last=") {
                    app.select_file(f);
                }
                if l == "sound=off" {
                    app.audio.muted = true;
                }
            }
        }
        app.scroll = target_scroll(app.sel, app.games.len(), 6);
        app
    }

    pub fn select_file(&mut self, f: &str) -> bool {
        if let Some(i) = self.games.iter().position(|g| g.file == f) {
            self.sel = i;
            self.scroll = target_scroll(i, self.games.len(), 6);
            self.dirty = true;
            return true;
        }
        false
    }

    fn save_settings(&self) {
        let mut s = String::new();
        if let Some(g) = self.games.get(self.sel) {
            s += &format!("last={}\n", g.file);
        }
        s += if self.audio.muted { "sound=off\n" } else { "sound=on\n" };
        let _ = std::fs::create_dir_all(&self.data_dir);
        let _ = std::fs::write(self.data_dir.join("settings.txt"), s);
    }

    pub fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), 1.5));
        self.dirty = true;
    }

    fn go(&mut self, s: Screen) {
        self.screen = s;
        self.dirty = true;
    }

    // ---- game lifecycle ----------------------------------------------------------------------

    fn close_game(&mut self) {
        self.loaded = None;
        self.error = None;
        self.audio.set_game(Vec::new());
        // refresh "continue" markers
        let file = self.games.get(self.sel).map(|g| g.file.clone());
        self.games = scan_games(&self.games_dir, &self.data_dir);
        if let Some(f) = file {
            self.select_file(&f);
        }
    }

    fn open_game(&mut self) {
        self.close_game();
        let Some(e) = self.games.get(self.sel) else {
            self.go(Screen::Browser);
            return;
        };
        let file = e.file.clone();
        let src = match std::fs::read(self.games_dir.join(&file)) {
            Ok(s) => s,
            Err(err) => {
                self.error = Some((file, vec![format!("can't read file: {err}")]));
                self.go(Screen::Error);
                return;
            }
        };
        let t0 = std::time::Instant::now();
        let game = Game::compile(&src);
        eprintln!("compiled {file} in {} ms ({} errors)", t0.elapsed().as_millis(), game.error_count());
        if game.error_count() > 0 {
            self.error = Some((file, game.errors()));
            self.err_scroll = 0;
            self.go(Screen::Error);
            return;
        }
        self.audio.set_game(game.sound_seeds());
        self.audio.pregenerate(Duration::from_millis(400));
        let dir = self.data_dir.join("saves").join(&file);
        let save_dir = Box::new(CString::new(dir.to_string_lossy().into_owned()).unwrap());
        let storage = Box::new(PSStorage {
            ctx: &*save_dir as *const CString as *mut c_void,
            get: Some(st_get),
            set: Some(st_set),
            remove: Some(st_remove),
        });
        let session = unsafe { Session::new(&game, &*storage, None) };
        let render = GameRender::new(&game);
        self.loaded = Some(Loaded { session, render, game, _storage: storage, _save_dir: save_dir, file });
        self.b_held = None;
        self.go(Screen::Play);
    }

    /// Script hook: open `file` straight at its n-th playable level (past the end: the last).
    pub fn play_level(&mut self, file: &str, n: usize) {
        if !self.select_file(file) {
            eprintln!("script: no game {file}");
            return;
        }
        self.open_game();
        if let Some(l) = &mut self.loaded {
            let playable: Vec<usize> =
                l.game.levels().iter().enumerate().filter(|(_, d)| !d.isMessage).map(|(i, _)| i).collect();
            if let Some(&i) = playable.get(n).or(playable.last()) {
                l.session.goto_level(i);
            }
        }
        self.dirty = true;
    }

    pub fn open_menu(&mut self) {
        self.menu_from = if self.screen == Screen::Menu { self.menu_from } else { self.screen };
        self.menu_items = match self.menu_from {
            Screen::Play | Screen::Levels => vec![
                Item::Resume,
                Item::Restart,
                Item::Undo,
                Item::LevelSelect,
                Item::TitleScreen,
                Item::Games,
                Item::Sound,
                Item::Fullscreen,
                Item::Quit,
            ],
            _ => vec![Item::Resume, Item::Sound, Item::Fullscreen, Item::Quit],
        };
        self.menu_sel = 0;
        self.go(Screen::Menu);
    }

    fn open_level_select(&mut self) {
        let Some(l) = &self.loaded else { return };
        let levels = l.game.levels();
        if levels.is_empty() {
            return;
        }
        let mut s = l.session.level_index().max(0) as usize;
        if s >= levels.len() {
            s = 0;
        }
        while s + 1 < levels.len() && levels[s].isMessage {
            s += 1;
        }
        self.ls_sel = s;
        self.ls_scroll = target_scroll(s, levels.len(), 8);
        self.go(Screen::Levels);
    }

    // ---- per-frame update ----------------------------------------------------------------------

    /// Up/down list navigation with held-key repeat.
    fn nav(&mut self, inp: &State, dt: f32) -> i32 {
        let mut d = 0;
        for (b, v) in [(Btn::Up, -1), (Btn::Down, 1)] {
            if inp.was_pressed(b) {
                self.repeat = Some((b, 0.35));
                d = v;
            }
        }
        if d == 0 {
            if let Some((b, t)) = self.repeat {
                if !inp.held(b) {
                    self.repeat = None;
                } else if t - dt <= 0.0 {
                    self.repeat = Some((b, 0.07));
                    d = if b == Btn::Up { -1 } else { 1 };
                } else {
                    self.repeat = Some((b, t - dt));
                }
            }
        }
        d
    }

    pub fn update(&mut self, inp: &State, dt: f32) {
        if let Some((_, t)) = &mut self.toast {
            *t -= dt;
            if *t <= 0.0 {
                self.toast = None;
                self.dirty = true;
            }
        }
        match self.screen {
            Screen::Browser => self.update_browser(inp, dt),
            Screen::Loading => {
                // drawn once with the loading text, then the (possibly slow) compile runs
                if !self.dirty {
                    self.open_game();
                }
            }
            Screen::Play => self.update_play(inp, dt),
            Screen::Levels => self.update_levels(inp, dt),
            Screen::Menu => self.update_menu(inp, dt),
            Screen::Error => {
                if inp.was_pressed(Btn::A) || inp.was_pressed(Btn::B) || inp.was_pressed(Btn::Start) {
                    self.close_game();
                    self.go(Screen::Browser);
                }
                if inp.held(Btn::Down) {
                    self.err_scroll += 8;
                    self.dirty = true;
                }
                if inp.held(Btn::Up) && self.err_scroll > 0 {
                    self.err_scroll = (self.err_scroll - 8).max(0);
                    self.dirty = true;
                }
            }
        }
        if self.screen == Screen::Play {
            // keep sounds generating in the background between turns
            if inp.pressed.is_empty() {
                self.audio.pregenerate(Duration::from_millis(2));
            }
        }
    }

    fn update_browser(&mut self, inp: &State, dt: f32) {
        let n = self.games.len();
        let mut d = self.nav(inp, dt);
        if inp.was_pressed(Btn::L) || inp.was_pressed(Btn::Left) {
            d = -6;
        }
        if inp.was_pressed(Btn::R) || inp.was_pressed(Btn::Right) {
            d = 6;
        }
        if d != 0 && n > 0 {
            self.sel = (self.sel as i32 + d).clamp(0, n as i32 - 1) as usize;
            self.dirty = true;
        }
        if ease(&mut self.scroll, target_scroll(self.sel, n, 6)) {
            self.dirty = true;
        }
        if inp.was_pressed(Btn::A) && n > 0 {
            self.save_settings();
            self.go(Screen::Loading);
        }
        if inp.was_pressed(Btn::Start) {
            self.open_menu();
        }
    }

    fn update_levels(&mut self, inp: &State, dt: f32) {
        let Some(l) = &self.loaded else { return };
        let n = l.game.levels().len();
        let is_msg: Vec<bool> = l.game.levels().iter().map(|d| d.isMessage).collect();
        let mut d = self.nav(inp, dt);
        if inp.was_pressed(Btn::L) || inp.was_pressed(Btn::Left) {
            d = -8;
        }
        if inp.was_pressed(Btn::R) || inp.was_pressed(Btn::Right) {
            d = 8;
        }
        if d != 0 {
            let mut s = (self.ls_sel as i32 + d).clamp(0, n as i32 - 1);
            let step = d.signum();
            while s >= 0 && (s as usize) < n && is_msg[s as usize] {
                s += step;
            }
            if s >= 0 && (s as usize) < n {
                self.ls_sel = s as usize;
                self.dirty = true;
            }
        }
        if ease(&mut self.ls_scroll, target_scroll(self.ls_sel, n, 8)) {
            self.dirty = true;
        }
        if inp.was_pressed(Btn::A) && !is_msg[self.ls_sel] {
            let i = self.ls_sel;
            self.loaded.as_mut().unwrap().session.goto_level(i);
            self.go(Screen::Play);
        } else if inp.was_pressed(Btn::B) || inp.was_pressed(Btn::Select) {
            self.go(Screen::Play);
        } else if inp.was_pressed(Btn::Start) {
            self.open_menu();
        }
    }

    fn update_menu(&mut self, inp: &State, dt: f32) {
        let n = self.menu_items.len();
        let d = self.nav(inp, dt);
        if d != 0 {
            self.menu_sel = (self.menu_sel as i32 + d).rem_euclid(n as i32) as usize;
            self.dirty = true;
        }
        if inp.was_pressed(Btn::B) || inp.was_pressed(Btn::Start) {
            self.go(self.menu_from);
            return;
        }
        if !inp.was_pressed(Btn::A) {
            return;
        }
        let back = self.menu_from;
        match self.menu_items[self.menu_sel] {
            Item::Resume => self.go(back),
            Item::Restart => {
                if let Some(l) = &mut self.loaded {
                    l.session.tap(ffi::KEY_RESTART);
                }
                self.go(Screen::Play);
            }
            Item::Undo => {
                if let Some(l) = &mut self.loaded {
                    l.session.tap(ffi::KEY_UNDO);
                }
                self.go(Screen::Play);
            }
            Item::LevelSelect => self.open_level_select(),
            Item::TitleScreen => {
                if let Some(l) = &mut self.loaded {
                    l.session.goto_title();
                }
                self.go(Screen::Play);
            }
            Item::Games => {
                self.close_game();
                self.go(Screen::Browser);
            }
            Item::Sound => {
                self.audio.muted = !self.audio.muted;
                self.save_settings();
                self.dirty = true;
            }
            Item::Fullscreen => {
                self.want_fullscreen_toggle = true;
                self.go(back);
            }
            Item::Quit => self.quit = true,
        }
    }

    fn update_play(&mut self, inp: &State, dt: f32) {
        if inp.was_pressed(Btn::Start) {
            self.release_game_keys();
            self.open_menu();
            return;
        }
        if inp.was_pressed(Btn::Select) {
            self.release_game_keys();
            self.open_level_select();
            return;
        }
        let Some(l) = &mut self.loaded else { return };
        let s = &mut l.session;
        const MAP: [(Btn, i32); 5] = [
            (Btn::Up, ffi::KEY_UP),
            (Btn::Down, ffi::KEY_DOWN),
            (Btn::Left, ffi::KEY_LEFT),
            (Btn::Right, ffi::KEY_RIGHT),
            (Btn::A, ffi::KEY_ACTION),
        ];
        for (b, k) in MAP {
            if inp.was_pressed(b) {
                s.key_down(k);
            }
            if inp.released.contains(&b) {
                s.key_up(k);
            }
        }
        let text = s.in_text_mode();
        if inp.was_pressed(Btn::X) {
            s.tap(ffi::KEY_UNDO);
        }
        if inp.was_pressed(Btn::Y) {
            s.tap(ffi::KEY_RESTART);
        }
        // B: undo; held for a second: restart (with a progress bar)
        let old = self.b_held;
        if inp.was_pressed(Btn::B) {
            s.tap(ffi::KEY_UNDO);
            self.b_held = Some(0.0);
        } else if let Some(t) = self.b_held {
            if !inp.held(Btn::B) {
                self.b_held = None;
            } else {
                let t = t + dt;
                self.b_held = Some(t);
                if t >= HOLD_RESTART && !text {
                    s.tap(ffi::KEY_RESTART);
                    self.b_held = None; // consumed: nothing more until B is pressed again
                }
            }
        }
        if old.map_or(false, |t| t > HOLD_SHOW) || self.b_held.map_or(false, |t| t > HOLD_SHOW) {
            self.dirty = true;
        }
        let l = self.loaded.as_mut().unwrap();
        l.session.update(dt as f64);
        for seed in l.session.take_sounds() {
            self.audio.play(seed);
        }
        if l.session.take_dirty() {
            self.dirty = true;
        }
    }

    fn release_game_keys(&mut self) {
        if let Some(l) = &mut self.loaded {
            for k in [ffi::KEY_UP, ffi::KEY_DOWN, ffi::KEY_LEFT, ffi::KEY_RIGHT, ffi::KEY_ACTION] {
                l.session.key_up(k);
            }
        }
        self.b_held = None;
    }

    // ---- drawing -----------------------------------------------------------------------------

    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn draw(&mut self, cv: &mut Canvas) {
        match self.screen {
            Screen::Browser => self.draw_browser(cv),
            Screen::Loading => self.draw_loading(cv),
            Screen::Play => self.draw_play(cv),
            Screen::Levels => self.draw_levels(cv),
            Screen::Error => self.draw_error(cv),
            Screen::Menu => {
                match self.menu_from {
                    Screen::Play => self.draw_play(cv),
                    Screen::Levels => self.draw_levels(cv),
                    _ => self.draw_browser(cv),
                }
                self.draw_menu(cv);
            }
        }
        if let Some((msg, _)) = &self.toast {
            let s = ui_scale(cv) - 1;
            let w = gfx::text_width(msg, s) + 8 * s;
            let x = (cv.w - w) / 2;
            let y = cv.h - 22 * s;
            cv.fill(x, y, w, 14 * s, PANEL);
            cv.frame(x, y, w, 14 * s, (s / 2).max(1), ACCENT);
            cv.text_centered(msg, cv.w / 2, y + s, s, TEXT);
        }
    }

    fn header(cv: &mut Canvas, title: &str, note: &str) -> i32 {
        let s = ui_scale(cv);
        let hb = 16 * s;
        cv.fill(0, 0, cv.w, hb, PANEL);
        cv.fill(0, hb - s / 2, cv.w, (s / 2).max(1), ACCENT);
        let ty = hb / 2 - 13 * s / 2;
        cv.text(title, 4 * s, ty, s, ACCENT);
        let ns = (s * 3 / 4).max(1);
        let w = gfx::text_width(note, ns);
        cv.text(note, cv.w - 4 * s - w, hb / 2 - 13 * ns / 2, ns, DIM);
        hb
    }

    fn footer(cv: &mut Canvas, hint: &str) -> i32 {
        let s = ui_scale(cv);
        let ns = (s * 3 / 4).max(1);
        let fh = 14 * ns;
        cv.fill(0, cv.h - fh, cv.w, fh, PANEL);
        cv.text_centered(&gfx::fit(hint, cv.w - 4 * s, ns), cv.w / 2, cv.h - fh + ns, ns, DIM);
        fh
    }

    fn scrollbar(cv: &mut Canvas, y0: i32, h: i32, pos: f32, visible: usize, total: usize) {
        if total <= visible {
            return;
        }
        let s = ui_scale(cv);
        let th = (h * visible as i32 / total as i32).max(4 * s);
        let ty = y0 + ((h - th) as f32 * pos / (total - visible) as f32) as i32;
        cv.fill(cv.w - 2 * s, y0, 2 * s, h, PANEL);
        cv.fill(cv.w - 2 * s, ty, 2 * s, th, DIM);
    }

    fn draw_browser(&mut self, cv: &mut Canvas) {
        cv.clear(BG);
        let s = ui_scale(cv);
        let ss = (s * 3 / 4).max(1);
        let n = self.games.len();
        let top = 16 * s;
        let fh = 14 * ss;
        let row_h = 25 * s;
        let first = self.scroll.floor() as usize;
        let frac = self.scroll - first as f32;
        let visible = ((cv.h - top - fh) / row_h) as usize;
        for i in first..(first + visible + 2).min(n) {
            let y = (top + s) as f32 + ((i - first) as f32 - frac) * row_h as f32;
            let y = y as i32;
            let g = &self.games[i];
            let hi = i == self.sel;
            if hi {
                cv.fill(2 * s, y, cv.w - 6 * s, row_h - s, ROW_HI);
                cv.fill(2 * s, y, s, row_h - s, ACCENT);
            }
            let title = gfx::fit(&g.title, cv.w - 14 * s, s);
            cv.text(&title, 5 * s, y + s, s, if hi { ACCENT } else { TEXT });
            let by = match &g.author {
                Some(a) => format!("by {a}"),
                None => g.file.clone(),
            };
            let note = if g.has_save { "continue" } else { "" };
            let nw = gfx::text_width(note, ss);
            cv.text(&gfx::fit(&by, cv.w - 20 * s - nw, ss), 5 * s, y + 12 * s, ss, DIM);
            if g.has_save {
                cv.text(note, cv.w - 6 * s - nw, y + 12 * s, ss, if hi { ACCENT } else { DIM });
            }
        }
        if n == 0 {
            let msg = format!("No games in {}", self.games_dir.display());
            for (i, l) in gfx::wrap(&msg, ((cv.w / (6 * ss)) - 4) as usize).iter().enumerate() {
                cv.text(l, 4 * s, top + 8 * s + i as i32 * 12 * ss, ss, TEXT);
            }
        }
        Self::scrollbar(cv, top, cv.h - top - fh, self.scroll, visible, n);
        Self::header(cv, "PUZZLESCRIPT", &format!("{}/{}", if n > 0 { self.sel + 1 } else { 0 }, n));
        Self::footer(cv, "A: play   START: menu   L/R: page");
    }

    fn draw_loading(&mut self, cv: &mut Canvas) {
        cv.clear(BG);
        let s = ui_scale(cv);
        let t = self.games.get(self.sel).map(|g| g.title.clone()).unwrap_or_default();
        cv.text_centered(&gfx::fit(&t, cv.w - 8 * s, s), cv.w / 2, cv.h / 2 - 14 * s, s, ACCENT);
        cv.text_centered("loading...", cv.w / 2, cv.h / 2 + 2 * s, (s * 3 / 4).max(1), DIM);
    }

    fn draw_play(&mut self, cv: &mut Canvas) {
        let Some(l) = &mut self.loaded else {
            cv.clear(BG);
            return;
        };
        l.render.draw(cv, &l.game, &l.session);
        if let Some(t) = self.b_held {
            if t > HOLD_SHOW && !l.session.in_text_mode() {
                let frac = ((t - HOLD_SHOW) / (HOLD_RESTART - HOLD_SHOW)).min(1.0);
                let s = ui_scale(cv);
                let ss = (s * 3 / 4).max(1);
                let (bw, bh) = (cv.w / 2, 3 * s);
                let (x, y) = ((cv.w - bw) / 2, cv.h - 8 * s);
                cv.fill(x - s, y - 14 * ss - s, bw + 2 * s, 14 * ss + bh + 2 * s, PANEL);
                cv.text_centered("restart", cv.w / 2, y - 14 * ss, ss, TEXT);
                cv.fill(x, y, bw, bh, ROW_HI);
                cv.fill(x, y, (bw as f32 * frac) as i32, bh, ACCENT);
            }
        }
    }

    fn draw_levels(&mut self, cv: &mut Canvas) {
        cv.clear(BG);
        let Some(l) = &self.loaded else { return };
        let s = ui_scale(cv);
        let ss = (s * 3 / 4).max(1);
        let levels = l.game.levels();
        let n = levels.len();
        let current = l.session.level_index();
        let top = 16 * s;
        let fh = 14 * ss;
        let row_h = 14 * s;
        let first = self.ls_scroll.floor() as usize;
        let frac = self.ls_scroll - first as f32;
        let visible = ((cv.h - top - fh) / row_h) as usize;
        let mut num = levels[..first.min(n)].iter().filter(|d| !d.isMessage).count();
        for i in first..(first + visible + 2).min(n) {
            let y = (top + s) as f32 + ((i - first) as f32 - frac) * row_h as f32;
            let y = y as i32;
            let d = &levels[i];
            if d.isMessage {
                let m = ffi::cstr(d.message).unwrap_or_default();
                let m = format!("\u{201c}{}\u{201d}", m.trim());
                cv.text(&gfx::fit(&m, cv.w - 20 * s, ss), 10 * s, y + s + (s - ss) * 6, ss, DIM);
                continue;
            }
            num += 1;
            let hi = i == self.ls_sel;
            if hi {
                cv.fill(2 * s, y, cv.w - 6 * s, row_h - s, ROW_HI);
                cv.fill(2 * s, y, s, row_h - s, ACCENT);
            }
            cv.text(&format!("Level {num}"), 5 * s, y + s, s, if hi { ACCENT } else { TEXT });
            if i as i32 == current {
                let w = gfx::text_width("current", ss);
                cv.text("current", cv.w - 6 * s - w, y + s + (s - ss) * 6, ss, if hi { ACCENT } else { DIM });
            }
        }
        Self::scrollbar(cv, top, cv.h - top - fh, self.ls_scroll, visible, n);
        let title = l.game.title().unwrap_or_else(|| l.file.clone());
        let title = gfx::fit(&title, cv.w * 2 / 3, s);
        Self::header(cv, &title, "levels");
        Self::footer(cv, "A: play level   B: back");
    }

    fn draw_menu(&mut self, cv: &mut Canvas) {
        cv.shade(0, 0, cv.w, cv.h, 0x000000, 0.6);
        let s = ui_scale(cv);
        let labels: Vec<String> = self.menu_items.iter().map(|i| i.label(self)).collect();
        let row_h = 14 * s;
        let w = labels.iter().map(|l| gfx::text_width(l, s)).max().unwrap_or(0) + 16 * s;
        let h = row_h * labels.len() as i32 + 4 * s;
        let (x, y) = ((cv.w - w) / 2, (cv.h - h) / 2);
        cv.fill(x, y, w, h, PANEL);
        cv.frame(x, y, w, h, (s / 2).max(1), ACCENT);
        for (i, lab) in labels.iter().enumerate() {
            let ry = y + 2 * s + i as i32 * row_h;
            let hi = i == self.menu_sel;
            if hi {
                cv.fill(x + s, ry, w - 2 * s, row_h, ROW_HI);
                cv.text(">", x + 3 * s, ry + s, s, ACCENT);
            }
            cv.text(lab, x + 9 * s, ry + s, s, if hi { ACCENT } else { TEXT });
        }
    }

    fn draw_error(&mut self, cv: &mut Canvas) {
        cv.clear(BG);
        let s = ui_scale(cv);
        let ss = (s * 3 / 4).max(1);
        let Some((file, errs)) = &self.error else { return };
        let top = 16 * s;
        let cols = ((cv.w - 8 * s) / (6 * ss)) as usize;
        let mut y = top + 4 * s - self.err_scroll;
        cv.text(&gfx::fit(file, cv.w - 8 * s, s), 4 * s, y, s, TEXT);
        y += 16 * s;
        for e in errs.iter().take(200) {
            for line in gfx::wrap(e, cols) {
                if y > top - 12 * ss && y < cv.h {
                    cv.text(&line, 4 * s, y, ss, ERR);
                }
                y += 11 * ss;
            }
            y += 4 * ss;
        }
        let n = errs.len();
        Self::header(cv, "CAN'T COMPILE", &format!("{n} error{}", if n == 1 { "" } else { "s" }));
        Self::footer(cv, "B: back   up/down: scroll");
    }
}
