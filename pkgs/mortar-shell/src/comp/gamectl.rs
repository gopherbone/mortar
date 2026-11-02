// SPDX-License-Identifier: GPL-2.0-or-later
//! The compositor side of games (games.rs, gamepages.rs): one app per
//! game, the library and pictures read off the event loop, requests to the
//! games' control sockets, hibernation and the save before suspend.
//!
//! Two worker threads: `games-lib` runs `mortar-play list --json` (only
//! when a watched folder's modification time changed) and decodes resume
//! and slot pictures; `games-ctl` talks to the games' sockets
//! (`$XDG_RUNTIME_DIR/mortar-play-<pid>.sock`, found by pid ancestry like
//! windows are). Results are picked up by `games_poll` on the loop.

use super::state::Shell;
use crate::config::{AppEntry, Category, GamepadMode};
use crate::games::{self, Awake, GameCmd, GameRef, GameStatus, GamesView, Library, Pic};
use crate::shellui::Toast;
use resvg::tiny_skia::Pixmap;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

enum LibJob {
    List(Vec<String>),
    Thumb(PathBuf),
}

enum Done {
    List(Result<Library, String>),
    Thumb(PathBuf, Option<Pixmap>),
    Ctl(u32, GameCmd, Result<serde_json::Value, String>),
}

/// The library is looked at again at most this often while the MENU shows.
const LIST_EVERY: Duration = Duration::from_secs(2);
/// Decoded pictures kept (resume and slot pictures, ~300 KB each).
const PIC_CACHE: usize = 16;
/// Hibernation is considered this often.
const HIBERNATE_EVERY: Duration = Duration::from_secs(2);
/// A game younger than this is still starting (never put to sleep).
const STARTING: Duration = Duration::from_secs(8);

pub struct GamesState {
    pub view: GamesView,
    lib_tx: Sender<LibJob>,
    ctl_tx: Sender<(u32, PathBuf, GameCmd)>,
    done: Arc<Mutex<Vec<Done>>>,
    last_list: Option<Instant>,
    pending: HashSet<PathBuf>,
    next_pic: u32,
    socks: HashMap<u32, PathBuf>,
    /// Games waking up: their sleeping picture until their first frame.
    waking: HashMap<u32, u32>,
    last_hibernate: Instant,
}

/// A request to a game's control socket: one JSON line each way.
pub fn request(sock: &Path, line: &str, timeout: Duration) -> Result<serde_json::Value, String> {
    let mut s = UnixStream::connect(sock).map_err(|e| format!("{}: {e}", sock.display()))?;
    s.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    s.set_write_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    s.write_all(line.as_bytes()).and_then(|_| s.write_all(b"\n")).map_err(|e| e.to_string())?;
    let mut out = String::new();
    BufReader::new(&s).read_line(&mut out).map_err(|e| e.to_string())?;
    serde_json::from_str(out.trim()).map_err(|e| format!("reply: {e}"))
}

fn mtimes(dirs: &[PathBuf]) -> Vec<Option<SystemTime>> {
    dirs.iter().map(|d| std::fs::metadata(d).and_then(|m| m.modified()).ok()).collect()
}

/// A PNG (mortar-play's thumbnails) as an opaque pixmap.
pub fn load_png(path: &Path) -> Option<Pixmap> {
    let f = std::fs::File::open(path).ok()?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(f));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info().ok()?;
    let mut buf = vec![0; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width, info.height);
    let mut pm = Pixmap::new(w, h)?;
    let px = &buf[..info.buffer_size()];
    let ch = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        _ => return None,
    };
    for (o, i) in pm.data_mut().chunks_exact_mut(4).zip(px.chunks_exact(ch)) {
        let (r, g, b) = if ch >= 3 { (i[0], i[1], i[2]) } else { (i[0], i[0], i[0]) };
        o.copy_from_slice(&[r, g, b, 255]);
    }
    Some(pm)
}

impl GamesState {
    pub fn new(library_app: Option<String>) -> GamesState {
        let done: Arc<Mutex<Vec<Done>>> = Arc::default();
        let (lib_tx, lib_rx) = channel::<LibJob>();
        let d = done.clone();
        let r = std::thread::Builder::new().name("games-lib".into()).spawn(move || {
            let mut watch: Vec<PathBuf> = Vec::new();
            let mut seen: Vec<Option<SystemTime>> = Vec::new();
            while let Ok(job) = lib_rx.recv() {
                let out = match job {
                    LibJob::List(argv) => {
                        // nothing moved since the last answer: keep it
                        if !watch.is_empty() && mtimes(&watch) == seen {
                            continue;
                        }
                        let r = std::process::Command::new(&argv[0])
                            .args(&argv[1..])
                            .stderr(std::process::Stdio::null())
                            .output()
                            .map_err(|e| format!("{}: {e}", argv[0]))
                            .and_then(|o| Library::parse(&String::from_utf8_lossy(&o.stdout)));
                        if let Ok(lib) = &r {
                            watch = lib.watch.clone();
                            seen = mtimes(&watch);
                        }
                        Done::List(r)
                    }
                    LibJob::Thumb(p) => {
                        let pm = load_png(&p);
                        Done::Thumb(p, pm)
                    }
                };
                if let Ok(mut v) = d.lock() {
                    v.push(out);
                }
            }
        });
        if let Err(e) = r {
            tracing::warn!("games-lib thread: {e}");
        }
        let (ctl_tx, ctl_rx) = channel::<(u32, PathBuf, GameCmd)>();
        let d = done.clone();
        let r = std::thread::Builder::new().name("games-ctl".into()).spawn(move || {
            while let Ok((uid, sock, cmd)) = ctl_rx.recv() {
                // saving a big state takes a moment
                let r = request(&sock, &cmd.json(), Duration::from_secs(5));
                if let Ok(mut v) = d.lock() {
                    v.push(Done::Ctl(uid, cmd, r));
                }
            }
        });
        if let Err(e) = r {
            tracing::warn!("games-ctl thread: {e}");
        }
        GamesState {
            view: GamesView { library_app, ..Default::default() },
            lib_tx,
            ctl_tx,
            done,
            last_list: None,
            pending: HashSet::new(),
            next_pic: 1,
            socks: HashMap::new(),
            waking: HashMap::new(),
            last_hibernate: Instant::now(),
        }
    }

    fn pic_id(&mut self) -> u32 {
        let id = self.next_pic;
        self.next_pic += 1;
        id
    }

    /// A waking game's sleeping picture.
    pub fn waking_pic(&self, uid: u32) -> Option<u32> {
        self.waking.get(&uid).copied()
    }
}

impl Shell {
    /// Start a game as its own app; a game that runs (or sleeps) is
    /// switched to (or woken).
    pub fn launch_game(&mut self, g: &GameRef) -> Result<u32, String> {
        let gc = self.cfg.games.clone().ok_or("games are not set up (mortar.play)")?;
        let key = g.key();
        if let Some(a) = self.apps.by_entry(&key) {
            let uid = a.uid;
            if a.sleeping() {
                self.wake_game(uid)?;
            } else if !a.windows.is_empty() {
                self.show_app(uid);
            } else {
                self.pending_focus = Some(uid);
            }
            return Ok(uid);
        }
        let e = AppEntry {
            id: key,
            name: g.title(),
            description: g.system_name.clone(),
            icon: mortar_ui::icons::Icon::Pad,
            command: g.argv(&gc.run),
            env: gc.env.clone(),
            gamepad: GamepadMode::Forward,
            single_instance: true,
            art: Some(crate::art::Art::Games),
            category: Some(Category::Games),
            requires: None,
        };
        self.launch_entry(e)
    }

    /// Wake a sleeping game: its process starts again (it resumes from its
    /// `auto` state); its picture stands in until its first frame.
    pub fn wake_game(&mut self, uid: u32) -> Result<(), String> {
        let pic = self.apps.get(uid).and_then(|a| a.asleep);
        let (env, pads) = (self.launch_env(), &self.pads);
        let le = crate::apps::LaunchEnv { wayland_display: &self.socket_name, extra: env };
        self.apps.respawn(uid, &le, |slot| {
            let bl = pads.blacklist(slot);
            vec![("SDL_JOYSTICK_BLACKLIST_DEVICES".into(), bl.clone()), ("SDL_GAMECONTROLLER_IGNORE_DEVICES".into(), bl)]
        })?;
        tracing::info!(uid, "game woken");
        if let (Some(g), Some(p)) = (self.games.as_mut(), pic) {
            g.waking.insert(uid, p);
            g.socks.remove(&uid);
        }
        self.pending_focus = Some(uid);
        self.ui_dirty = true;
        Ok(())
    }

    /// The control socket of game app `uid`.
    fn game_socket(&mut self, uid: u32) -> Option<PathBuf> {
        if let Some(p) = self.games.as_ref()?.socks.get(&uid).filter(|p| p.exists()) {
            return Some(p.clone());
        }
        let dir = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
        let found = std::fs::read_dir(&dir).ok()?.flatten().find_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let pid: u32 = name.strip_prefix("mortar-play-")?.strip_suffix(".sock")?.parse().ok()?;
            (self.apps.for_pid(pid, crate::apps::proc_parent) == Some(uid)).then(|| e.path())
        })?;
        self.games.as_mut()?.socks.insert(uid, found.clone());
        Some(found)
    }

    /// A request from the game panel (answered in `games_poll`).
    pub fn game_ctl(&mut self, uid: u32, cmd: GameCmd) {
        match self.game_socket(uid) {
            Some(sock) => {
                if let Some(g) = &self.games {
                    let _ = g.ctl_tx.send((uid, sock, cmd));
                }
            }
            None => {
                tracing::warn!(uid, ?cmd, "no control socket");
                if cmd != GameCmd::ListStates {
                    self.sui.show_toast(Toast::info("The game is not answering"));
                }
            }
        }
    }

    /// Keep the library and pictures fresh while the MENU shows; take the
    /// workers' answers.
    pub fn games_poll(&mut self) {
        // (every tick: nothing here is copied or built unless needed)
        if self.cfg.games.is_none() || self.games.is_none() {
            return;
        }
        let wants = self.sui.wants_games();
        let wanted = if self.sui.menu_open() { self.with_view(|ui, v| ui.wanted_pics(v)) } else { Vec::new() };
        let Some(g) = self.games.as_mut() else { return };
        if wants
            && g.last_list.is_none_or(|t| t.elapsed() >= LIST_EVERY)
            && let Some(gc) = self.cfg.games.as_ref()
        {
            g.last_list = Some(Instant::now());
            let _ = g.lib_tx.send(LibJob::List(gc.list.clone()));
        }
        for p in &wanted {
            if !g.view.thumbs.contains_key(p) && g.pending.insert(p.clone()) {
                let _ = g.lib_tx.send(LibJob::Thumb(p.clone()));
            }
        }
        // a bounded cache: drop pictures nobody wants
        if g.view.thumbs.len() > PIC_CACHE {
            let drop: Vec<PathBuf> = g.view.thumbs.keys().filter(|k| !wanted.contains(k)).take(g.view.thumbs.len() - PIC_CACHE).cloned().collect();
            for k in drop {
                if let Some(p) = g.view.thumbs.remove(&k) {
                    self.cpu_imgs.pics.remove(&p.id);
                }
            }
        }
        // a woken game drew its first frame: its picture goes
        let drawn: Vec<u32> = g.waking.keys().copied().filter(|u| self.apps.get(*u).is_none_or(|a| !a.windows.is_empty())).collect();
        for u in drawn {
            if let Some(p) = g.waking.remove(&u) {
                self.cpu_imgs.pics.remove(&p);
            }
        }
        let dones = g.done.lock().map(|mut d| std::mem::take(&mut *d)).unwrap_or_default();
        for d in dones {
            self.game_done(d);
        }
    }

    fn game_done(&mut self, d: Done) {
        let Some(g) = self.games.as_mut() else { return };
        match d {
            Done::List(Ok(lib)) => {
                if g.view.lib.as_ref() != Some(&lib) {
                    tracing::info!(systems = lib.shelves().len(), recent = lib.recent.len(), "games library");
                    // pictures may have changed with it
                    for (_, p) in g.view.thumbs.drain() {
                        self.cpu_imgs.pics.remove(&p.id);
                    }
                    g.view.lib = Some(lib);
                    self.ui_dirty = true;
                }
            }
            Done::List(Err(e)) => tracing::warn!("games library: {e}"),
            Done::Thumb(path, pm) => {
                g.pending.remove(&path);
                if let Some(pm) = pm {
                    let id = g.pic_id();
                    g.view.thumbs.insert(path, Pic { id, w: pm.width(), h: pm.height() });
                    self.cpu_imgs.pics.insert(id, pm);
                    self.ui_dirty = true;
                }
            }
            Done::Ctl(uid, cmd, r) => self.game_reply(uid, cmd, r),
        }
    }

    fn game_reply(&mut self, uid: u32, cmd: GameCmd, r: Result<serde_json::Value, String>) {
        let r = r.and_then(|v| {
            if v.get("ok").and_then(|o| o.as_bool()) == Some(true) {
                Ok(v)
            } else {
                Err(v.get("error").and_then(|e| e.as_str()).unwrap_or("failed").to_string())
            }
        });
        let name = self.apps.get(uid).map(|a| a.name.clone()).unwrap_or_default();
        tracing::info!(uid, ?cmd, ok = r.is_ok(), "game request");
        let Some(g) = self.games.as_mut() else { return };
        match (cmd, r) {
            (GameCmd::ListStates, Ok(v)) => {
                if let Some(st) = GameStatus::parse(&v) {
                    g.view.status.insert(uid, st);
                    self.ui_dirty = true;
                }
            }
            (GameCmd::Save(n), Ok(_)) => {
                // its picture changed: decode it again
                if let Some(t) = g.view.status.get(&uid).and_then(|s| s.slots[n as usize].as_ref()).and_then(|s| s.thumb.clone())
                    && let Some(p) = g.view.thumbs.remove(&t)
                {
                    self.cpu_imgs.pics.remove(&p.id);
                }
                self.sui.show_toast(Toast { sub: name, ..Toast::info(format!("Saved to slot {}", n + 1)) });
                self.game_ctl(uid, GameCmd::ListStates);
            }
            (GameCmd::FastForward(_), Ok(v)) => {
                let on = v.get("on").and_then(|o| o.as_bool()).unwrap_or(false);
                if let Some(st) = g.view.status.get_mut(&uid) {
                    st.fast_forward = on;
                }
                self.sui.show_toast(Toast { sub: name, sound: None, ..Toast::info(if on { "Fast-forward on" } else { "Fast-forward off" }) });
            }
            (GameCmd::NextDisc, Ok(v)) => {
                let (i, n) = (v.get("disc").and_then(|x| x.as_u64()).unwrap_or(0), v.get("of").and_then(|x| x.as_u64()).unwrap_or(0));
                if let Some(st) = g.view.status.get_mut(&uid) {
                    st.discs = Some((i as u32, n as u32));
                }
                self.sui.show_toast(Toast { sub: name, ..Toast::info(format!("Disc {i} of {n}")) });
            }
            (GameCmd::Display(_), Ok(v)) => {
                if let Some(st) = g.view.status.get_mut(&uid) {
                    st.display = v.get("display").and_then(crate::games::DisplayInfo::parse);
                }
                self.ui_dirty = true;
            }
            (GameCmd::KeepDisplay(game), Ok(v)) => {
                let d = v.get("display").and_then(crate::games::DisplayInfo::parse);
                let msg = match &d {
                    Some(d) if game => format!("{} kept for this game", d.label),
                    Some(d) => format!("{} kept for all {} games", d.label, d.system_name),
                    None => "Display kept".to_string(),
                };
                if let Some(st) = g.view.status.get_mut(&uid) {
                    st.display = d;
                }
                self.sui.show_toast(Toast { sub: name, sound: None, ..Toast::info(msg) });
            }
            (GameCmd::Load(n), Ok(_)) => tracing::info!(uid, slot = n + 1, "state loaded"),
            (GameCmd::Quit { .. } | GameCmd::SaveAuto | GameCmd::Reset, Ok(_)) => {}
            (GameCmd::ListStates, Err(e)) => tracing::warn!(uid, "list_states: {e}"),
            (GameCmd::Quit { .. }, Err(e)) => tracing::warn!(uid, "quit: {e} (it is stopped instead)"),
            (_, Err(e)) => self.sui.show_toast(Toast { sub: name, ..Toast::info(e) }),
        }
        let outs = self.sui.take_out();
        self.handle_outs(outs);
        self.needs_redraw = true;
    }

    /// Games that run (not asleep, not closing), most recent first.
    fn awake_games(&self) -> Vec<Awake> {
        let vis = self.visible_app();
        self.apps
            .ordered()
            .into_iter()
            .filter_map(|u| self.apps.get(u))
            .filter(|a| a.is_game() && a.child.is_some() && a.asleep.is_none() && a.closing.is_none())
            .enumerate()
            .map(|(rank, a)| Awake {
                uid: a.uid,
                rank,
                front: Some(a.uid) == vis || Some(a.uid) == self.pending_focus,
                starting: a.windows.is_empty() || a.started.is_none_or(|t| t.elapsed() < STARTING),
            })
            .collect()
    }

    /// Too many games, or memory low: the least recently used background
    /// game saves and sleeps (one at a time).
    pub fn hibernate_tick(&mut self) {
        let Some(gc) = self.cfg.games.as_ref() else { return };
        let (max, low) = (gc.max_games, gc.hibernate_below_mb);
        let Some(g) = self.games.as_mut() else { return };
        if g.last_hibernate.elapsed() < HIBERNATE_EVERY {
            return;
        }
        g.last_hibernate = Instant::now();
        // one going to sleep at a time
        if self.apps.list.iter().any(|a| a.asleep.is_some() && a.child.is_some()) {
            return;
        }
        let awake = self.awake_games();
        if awake.is_empty() {
            return;
        }
        let mem = std::fs::read_to_string("/proc/meminfo").ok().and_then(|s| games::mem_available_mb(&s));
        if let Some(uid) = games::hibernate_pick(&awake, max, mem, low) {
            tracing::info!(uid, awake = awake.len(), mem_mb = ?mem, "putting a game to sleep");
            self.hibernate(uid);
        }
    }

    /// Save and quit a background game; it stays in Running, asleep, with
    /// its last frame.
    pub fn hibernate(&mut self, uid: u32) {
        let Some(sock) = self.game_socket(uid) else {
            tracing::warn!(uid, "hibernate: no control socket");
            return;
        };
        // its last frame, half size (it stands in for the window)
        let pm = self.app_surface(uid).and_then(|s| super::ui::surface_buffer(&s)).and_then(|(b, _)| super::ui::buffer_pixmap(&b, 2));
        let Some(g) = self.games.as_mut() else { return };
        let id = g.pic_id();
        match pm {
            Some(pm) => {
                self.cpu_imgs.pics.insert(id, pm);
            }
            None => tracing::warn!(uid, "hibernate: no picture"),
        }
        let _ = g.ctl_tx.send((uid, sock, GameCmd::Quit { quiet: true }));
        g.view.status.remove(&uid);
        if let Some(a) = self.apps.get_mut(uid) {
            a.asleep = Some(id);
            // stopped if it does not quit by itself
            a.closing = Some(Instant::now());
        }
        self.ui_dirty = true;
    }

    /// Before suspending: every running game writes its resume state
    /// (at most ~500 ms in all; the requests run in parallel).
    pub fn save_games_for_suspend(&mut self) {
        let uids: Vec<u32> = self.apps.list.iter().filter(|a| a.is_game() && a.child.is_some() && a.asleep.is_none()).map(|a| a.uid).collect();
        let socks: Vec<(u32, PathBuf)> = uids.into_iter().filter_map(|u| Some((u, self.game_socket(u)?))).collect();
        if socks.is_empty() {
            return;
        }
        let t0 = Instant::now();
        let (tx, rx) = channel();
        for (uid, sock) in &socks {
            let (tx, uid, sock) = (tx.clone(), *uid, sock.clone());
            std::thread::spawn(move || {
                let r = request(&sock, &GameCmd::SaveAuto.json(), Duration::from_millis(450));
                let _ = tx.send((uid, r.is_ok()));
            });
        }
        drop(tx);
        let deadline = t0 + Duration::from_millis(500);
        let mut saved = 0;
        while let Ok((_, ok)) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            saved += ok as usize;
            if Instant::now() >= deadline {
                break;
            }
        }
        tracing::info!(games = socks.len(), saved, ms = t0.elapsed().as_millis() as u64, "saved games before suspend");
    }
}
