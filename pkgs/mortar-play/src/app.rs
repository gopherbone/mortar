// SPDX-License-Identifier: GPL-2.0-or-later
//! The frontend: library screens, launching, the game loop (pacing, audio,
//! fast-forward, pause), the in-game menu, quitting with saves.

use crate::audio::Mixer;
use crate::config::Config;
use crate::input::{Button, Chord, Held};
use crate::library::{self, Game, Recents, Shelf};
use crate::platform::{PEvent, Platform};
use crate::session::{SLOTS, SavePaths, Session, Slot};
use crate::ui::{self, HomeEntry, MenuAction};
use crate::video::{self, Frame};
use mortar_ui::gfx::{Fonts, Ui, font_dir};
use mortar_ui::icons::Icon;
use mortar_ui::input::{Event as UiEvent, Repeater};
use mortar_ui::status::Sys;
use mortar_ui::toolkit::{ListState, PillKind, hint};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static TERM: AtomicBool = AtomicBool::new(false);

extern "C" fn on_term(_: libc::c_int) {
    TERM.store(true, Ordering::SeqCst);
}

/// SIGTERM/SIGINT (the shell closing us, Ctrl-C) quit cleanly: SRAM and the
/// resume state are written first.
pub fn install_signal_handlers() {
    unsafe {
        libc::signal(libc::SIGTERM, on_term as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_term as *const () as libc::sighandler_t);
    }
}

/// Frame clock: sleeps until the next frame is due.
pub struct Pacer {
    next: Instant,
    period: Duration,
    pub enabled: bool,
}

impl Pacer {
    pub fn new(fps: f64) -> Pacer {
        Pacer { next: Instant::now(), period: Duration::from_secs_f64(1.0 / fps.max(1.0)), enabled: true }
    }
    pub fn set_fps(&mut self, fps: f64) {
        self.period = Duration::from_secs_f64(1.0 / fps.max(1.0));
    }
    pub fn reset(&mut self) {
        self.next = Instant::now();
    }
    pub fn wait(&mut self) {
        if !self.enabled {
            return;
        }
        let now = Instant::now();
        if self.next > now {
            std::thread::sleep(self.next - now);
        } else if now - self.next > self.period * 4 {
            self.next = now; // fell behind (load, stall): don't race to catch up
        }
        self.next += self.period;
    }
}

#[derive(Clone, Debug)]
enum Target {
    Continue(PathBuf, String),
    Recent,
    System(usize),
}

#[derive(Clone, Debug)]
enum Source {
    System(usize),
    Recent,
}

#[derive(Clone, Debug)]
enum Then {
    Back,
    /// Start the game anyway (optional BIOS missing).
    Launch(String, Option<PathBuf>),
}

enum Screen {
    Home,
    Games { source: Source, list: ListState },
    Message { crumb: String, title: String, lines: Vec<String>, tag: Option<(String, PillKind)>, then: Then, back: Box<Screen> },
    Playing,
    Menu,
}

/// Scripted input for headless runs and tests: one step per loop
/// iteration (= one game frame while playing).
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Wait(u32),
    Press(Button),
    Hold(Button),
    Release(Button),
    Shot(PathBuf),
    Quit,
}

pub fn parse_script(s: &str) -> Result<Vec<Step>, String> {
    let mut v = Vec::new();
    for part in s.split([',', ';', '\n']).map(str::trim).filter(|p| !p.is_empty()) {
        let (cmd, arg) = part.split_once([' ', ':']).map(|(a, b)| (a, b.trim())).unwrap_or((part, ""));
        let btn = || Button::parse(arg).ok_or_else(|| format!("script: unknown button '{arg}'"));
        v.push(match cmd {
            "wait" => Step::Wait(arg.parse().map_err(|_| format!("script: wait '{arg}'"))?),
            "press" => Step::Press(btn()?),
            "hold" => Step::Hold(btn()?),
            "release" => Step::Release(btn()?),
            "shot" => Step::Shot(PathBuf::from(arg)),
            "quit" => Step::Quit,
            _ => return Err(format!("script: unknown step '{part}'")),
        });
    }
    Ok(v)
}

struct MenuState {
    list: ListState,
    slot: u8,
    toast: Option<(String, Instant)>,
    paused: Frame,
    aspect: f32,
}

pub struct App {
    cfg: Config,
    pf: Platform,
    mixer: Mixer,
    ui: Ui,
    shelves: Vec<Shelf>,
    recents: Recents,
    sys: Sys,
    sys_read: Instant,
    home_list: ListState,
    screen: Screen,
    /// Where Quit (from the in-game menu) returns to.
    after_game: Option<Screen>,
    session: Option<Session>,
    held: Held,
    rep: Repeater,
    chords: Vec<Chord>,
    visible: bool,
    dirty: bool,
    thumbs: HashMap<PathBuf, Option<Frame>>,
    bios_warned: HashSet<String>,
    under_shell: bool,
    /// Started for one game (`run`): quitting the game quits the app.
    single: bool,
    quit: bool,
    pacer: Pacer,
    menu: MenuState,
    script: Vec<Step>,
    script_wait: u32,
    headless: bool,
    /// Frames emulated (for logs).
    pub frames: u64,
    /// The control socket (inside mortar-shell): the shell's game panel.
    control: Option<crate::control::Control>,
    /// Fast-forward switched on from the shell's game panel (until off).
    ff_on: bool,
    /// The quit was asked for quietly (the shell putting the game to
    /// sleep): no "Saved" toast.
    quiet_quit: bool,
}

impl App {
    pub fn new(cfg: Config, pf: Platform, audio: Box<dyn crate::audio::AudioOut>) -> Result<App, String> {
        let fonts = Fonts::load(&font_dir())?;
        let (w, h) = pf.display.size();
        let ui = Ui::new(w, h, fonts);
        let _ = mortar_ui::theme::set(mortar_ui::theme::Theme::by_name(&cfg.theme).unwrap_or(mortar_ui::theme::LIGHT));
        let mixer = Mixer::new(audio, cfg.audio_latency_ms);
        let recents = Recents::load(&cfg.data_dir());
        let chords = cfg.hotkeys.menu.iter().filter_map(|c| Chord::parse(c)).collect();
        let headless = pf.display.name() == "headless";
        let under_shell = std::env::var_os("MORTAR_APP").is_some();
        let mut app = App {
            shelves: library::scan(&cfg),
            cfg,
            pf,
            mixer,
            ui,
            recents,
            sys: if headless { Sys::demo(Some("12:00".into())) } else { Sys::read() },
            sys_read: Instant::now(),
            home_list: ListState::default(),
            screen: Screen::Home,
            after_game: None,
            session: None,
            held: Held::default(),
            rep: Repeater::default(),
            chords,
            visible: true,
            dirty: true,
            thumbs: HashMap::new(),
            bios_warned: HashSet::new(),
            under_shell,
            single: false,
            quit: false,
            pacer: Pacer::new(60.0),
            menu: MenuState { list: ListState::default(), slot: 0, toast: None, paused: Frame::default(), aspect: 0.0 },
            script: Vec::new(),
            script_wait: 0,
            headless,
            frames: 0,
            control: None,
            ff_on: false,
            quiet_quit: false,
        };
        if app.under_shell && !headless {
            match crate::control::Control::open() {
                Ok(c) => {
                    eprintln!("mortar-play: control socket {}", c.path.display());
                    app.control = Some(c);
                }
                Err(e) => eprintln!("mortar-play: control socket: {e}"),
            }
        }
        app.pacer.enabled = !headless;
        app.recents.prune();
        Ok(app)
    }

    pub fn set_script(&mut self, s: Vec<Step>) {
        self.script = s;
        self.script.reverse();
    }

    // ------------------------------------------------------------ loop ----

    pub fn run(&mut self) {
        let mut evs = Vec::new();
        while !self.quit {
            if TERM.load(Ordering::SeqCst) {
                self.on_quit();
                break;
            }
            evs.clear();
            self.pf.poll(&mut evs);
            self.script_step(&mut evs);
            for e in evs.drain(..) {
                self.event(e);
            }
            self.control_step();
            if self.quit {
                break;
            }
            match self.screen {
                Screen::Playing if self.visible => self.step_game(),
                _ => self.step_ui(),
            }
        }
    }

    fn script_step(&mut self, evs: &mut Vec<PEvent>) {
        if self.script_wait > 0 {
            self.script_wait -= 1;
            return;
        }
        // run every non-waiting step now; a press is released next iteration
        while let Some(s) = self.script.pop() {
            match s {
                Step::Wait(n) => {
                    self.script_wait = n.saturating_sub(1);
                    return;
                }
                Step::Press(b) => {
                    // held for this iteration and the next, released after
                    evs.push(PEvent::Press(b));
                    self.script.push(Step::Release(b));
                    self.script.push(Step::Wait(1));
                    return;
                }
                Step::Hold(b) => evs.push(PEvent::Press(b)),
                Step::Release(b) => evs.push(PEvent::Release(b)),
                Step::Shot(p) => {
                    if self.dirty && !matches!(self.screen, Screen::Playing) {
                        self.draw();
                    }
                    if let Some(f) = self.pf.display.snapshot() {
                        if let Err(e) = f.save_png(&p) {
                            eprintln!("mortar-play: {e}");
                        } else {
                            eprintln!("mortar-play: screenshot {}", p.display());
                        }
                    }
                }
                Step::Quit => {
                    evs.push(PEvent::Quit);
                    return;
                }
            }
        }
        if self.headless && self.script.is_empty() && self.script_wait == 0 {
            // a headless run ends with its script
            evs.push(PEvent::Quit);
        }
    }

    fn event(&mut self, e: PEvent) {
        match e {
            PEvent::Quit => self.on_quit(),
            PEvent::Visible(v) => {
                if v != self.visible {
                    self.visible = v;
                    if self.session.is_some() {
                        eprintln!("mortar-play: {}", if v { "visible again, resuming" } else { "hidden, paused" });
                    }
                    let playing = matches!(self.screen, Screen::Playing);
                    self.mixer.out.set_paused(!v || !playing);
                    self.pacer.reset();
                    self.dirty = true;
                }
            }
            PEvent::ReleaseAll => {
                self.held.clear();
                self.rep = Repeater::default();
            }
            PEvent::Press(b) => {
                if !self.held.down(b) {
                    return;
                }
                if matches!(self.screen, Screen::Playing) {
                    if self.chords.iter().any(|c| c.fires(b, &self.held)) {
                        self.open_menu();
                    }
                } else {
                    self.rep.press(b);
                    self.button(b);
                }
            }
            PEvent::Release(b) => {
                self.held.up(b);
                self.rep.release(b);
            }
        }
    }

    fn on_quit(&mut self) {
        if let Some(mut s) = self.session.take() {
            let saved = s.finish();
            self.saved_toast(&s, saved);
        }
        self.quit = true;
    }

    /// Inside the shell: a "Saved" toast once the resume state is written
    /// (not when the shell put the game to sleep itself).
    fn saved_toast(&self, s: &Session, saved: bool) {
        if saved && self.under_shell && !self.quiet_quit && !self.headless {
            let (name, tag) = s.title();
            crate::control::shell_toast("Saved", &format!("{name} · {tag}"));
        }
    }

    /// Requests from the shell's game panel (control.rs).
    fn control_step(&mut self) {
        let Some(c) = &self.control else { return };
        for p in c.poll() {
            let v = match p.req.clone() {
                Ok(r) => self.control_request(r),
                Err(e) => serde_json::json!({ "ok": false, "error": e }),
            };
            p.reply(v);
            if self.quit {
                return;
            }
        }
    }

    fn control_request(&mut self, req: crate::control::Request) -> serde_json::Value {
        use crate::control::{Request, parse_on, parse_slot};
        use serde_json::json;
        let err = |e: String| json!({ "ok": false, "error": e });
        let slot_of = |n: Option<u8>| n.map(Slot::N).unwrap_or(Slot::Auto);
        if let Request::Quit { quiet } = req {
            self.quiet_quit = quiet;
            let saved = match self.session.take() {
                Some(mut s) => {
                    let saved = s.finish();
                    self.saved_toast(&s, saved);
                    saved
                }
                None => false,
            };
            self.quit = true;
            return json!({ "ok": true, "saved": saved });
        }
        let ff_on = self.ff_on;
        let Some(s) = self.session.as_mut() else { return err("no game is running".into()) };
        match req {
            Request::ListStates => {
                let slot = |sl: Slot| -> serde_json::Value {
                    let thumb = s.slot_paths(sl).map(|p| p.1).filter(|t| t.exists());
                    match s.state_time(sl).and_then(|t| t.elapsed().ok()) {
                        Some(d) => json!({ "age_s": d.as_secs(), "thumb": thumb }),
                        None => serde_json::Value::Null,
                    }
                };
                let slots: Vec<serde_json::Value> = (0..crate::session::SLOTS).map(|n| slot(Slot::N(n))).collect();
                let (name, tag) = s.title();
                let discs = s.core.disk_count().map(|(i, n)| json!([i + 1, n]));
                json!({
                    "ok": true, "game": name, "tag": tag, "system": s.sys.id,
                    "slots": slots, "auto": slot(Slot::Auto),
                    "discs": discs, "fast_forward": ff_on,
                    "can_save": s.saves.is_some(),
                    "display": display_json(s),
                })
            }
            Request::SaveState { slot } => match parse_slot(&slot).and_then(|n| s.save_state(slot_of(n))) {
                Ok(()) => json!({ "ok": true }),
                Err(e) => err(e),
            },
            Request::LoadState { slot } => {
                let r = parse_slot(&slot).and_then(|n| {
                    let sl = slot_of(n);
                    if !s.has_state(sl) {
                        return Err("that slot is empty".to_string());
                    }
                    s.load_state(sl)
                });
                match r {
                    Ok(()) => {
                        if matches!(self.screen, Screen::Menu) {
                            self.resume();
                        }
                        self.pacer.reset();
                        json!({ "ok": true })
                    }
                    Err(e) => err(e),
                }
            }
            Request::Reset => {
                s.reset();
                json!({ "ok": true })
            }
            Request::FastForward { on } => match parse_on(&on, ff_on) {
                Ok(v) => {
                    self.ff_on = v;
                    json!({ "ok": true, "on": v })
                }
                Err(e) => err(e),
            },
            Request::ChangeDisc { to } => {
                if to.as_deref().is_some_and(|t| t != "next") {
                    return err("change_disc: only next".into());
                }
                match s.core.disk_next() {
                    Some(i) => json!({ "ok": true, "disc": i + 1, "of": s.core.disk_count().map(|d| d.1).unwrap_or(0) }),
                    None => err("this game has one disc".into()),
                }
            }
            Request::Display { step, keep } => {
                if let Some(n) = step {
                    s.step_view(n.signum());
                }
                let kept = match keep.as_deref() {
                    None => Ok(()),
                    Some("game") => s.keep_view(true),
                    Some("system") => s.keep_view(false),
                    Some(k) => Err(format!("display: keep \"game\" or \"system\", not \"{k}\"")),
                };
                match kept {
                    Ok(()) => json!({ "ok": true, "display": display_json(s) }),
                    Err(e) => err(e),
                }
            }
            Request::Quit { .. } => unreachable!(),
        }
    }

    fn step_ui(&mut self) {
        let mut reps = Vec::new();
        self.rep.poll(&mut reps);
        for r in reps {
            if let UiEvent::Press { button, .. } = r {
                self.button(button);
            }
        }
        if let Some((_, t)) = &self.menu.toast
            && t.elapsed() > Duration::from_secs(2)
        {
            self.menu.toast = None;
            self.dirty = true;
        }
        if !self.headless && self.sys_read.elapsed() > Duration::from_secs(15) {
            self.sys = Sys::read();
            self.sys_read = Instant::now();
            self.dirty = true;
        }
        if self.dirty {
            self.draw();
        }
        if !self.headless {
            self.pf.wait(Duration::from_millis(16));
        }
    }

    fn step_game(&mut self) {
        let hot = self.cfg.hotkeys.clone();
        let Some(s) = self.session.as_mut() else {
            self.screen = Screen::Home;
            return;
        };
        let ff = self.ff_on || s.sys.fast_forward_button(&hot).is_some_and(|b| self.held.is(b));
        let runs = if ff { hot.fast_forward_speed.max(2) } else { 1 };
        self.pacer.wait();
        let mut new_frame = false;
        for _ in 0..runs {
            if s.run(&self.held, ff, Some(self.mixer.occupancy())) {
                self.mixer.set_core_rate(s.sample_rate());
                self.pacer.set_fps(s.fps());
            }
            self.frames += 1;
            new_frame |= s.host().frame_new;
            if !ff {
                self.mixer.push(s.audio());
            }
            if s.host().shutdown {
                break;
            }
        }
        for m in self.session.as_mut().unwrap().core.host().messages.drain(..) {
            eprintln!("mortar-play: core says: {}", m.text);
        }
        let s = self.session.as_mut().unwrap();
        if new_frame || self.headless {
            let f = s.display_frame();
            let (sw, sh) = self.pf.display.size();
            let dst = video::layout(s.view, f.w, f.h, s.aspect(), sw, sh);
            self.pf.display.present_game(&f, dst, s.view.filter);
        }
        s.tick_autosave();
        if s.host().shutdown {
            self.end_game();
        }
    }

    // -------------------------------------------------------- launching ----

    fn launch(&mut self, sys_id: &str, path: &Path, force: bool) {
        self.launch_opt(sys_id, Some(path), force)
    }

    fn launch_opt(&mut self, sys_id: &str, path: Option<&Path>, force: bool) {
        let Some(sys) = self.cfg.system(sys_id).cloned() else { return };
        let back = std::mem::replace(&mut self.screen, Screen::Home);
        if !force {
            let missing: Vec<_> = sys.missing_bios(&self.cfg.bios_dir(), path.and_then(Path::parent)).into_iter().cloned().collect();
            let required = missing.iter().any(|b| b.required);
            if !missing.is_empty() && (required || !self.bios_warned.contains(&sys.id)) {
                self.bios_warned.insert(sys.id.clone());
                let mut lines = vec![if required {
                    format!("{} needs a BIOS file to start games. Mortar never ships BIOS files; copy your own here:", sys.name)
                } else {
                    format!("Some {} games need a BIOS file; the built-in replacement plays most. For the rest, copy your own here:", sys.name)
                }];
                lines.push(self.cfg.bios_dir().to_string_lossy().into_owned());
                for b in &missing {
                    let d = if b.description.is_empty() { String::new() } else { format!("{}: ", b.description) };
                    lines.push(format!("{d}{}", b.files.join(" or ")));
                }
                lines.push("From your computer, over SSH:".into());
                lines.push(format!("scp {} root@mortar.local:{}/", missing[0].files.first().cloned().unwrap_or_default(), self.cfg.bios_dir().display()));
                let mut hints = vec![hint("B", "Back")];
                if !required {
                    hints.push(hint("A", "Play anyway"));
                }
                self.screen = Screen::Message {
                    crumb: format!("Games / {}", sys.tag),
                    title: "BIOS missing".into(),
                    lines,
                    tag: Some(("BIOS".into(), if required { PillKind::Orange } else { PillKind::Amber })),
                    then: if required { Then::Back } else { Then::Launch(sys.id.clone(), path.map(Path::to_path_buf)) },
                    back: Box::new(back),
                };
                self.dirty = true;
                return;
            }
        }
        // a screen while the core loads (big PS1 images take a moment)
        self.loading(&sys.name, path);
        match Session::start(&self.cfg, &sys, path) {
            Ok(s) => {
                eprintln!(
                    "mortar-play: {} {} · {} · {:.3} fps · {:.0} Hz{}",
                    s.core.info.name,
                    s.core.info.version,
                    path.and_then(Path::file_name).unwrap_or_default().to_string_lossy(),
                    s.fps(),
                    s.sample_rate(),
                    if s.resumed { " · resumed" } else { "" }
                );
                self.mixer.set_core_rate(s.sample_rate());
                self.mixer.out.clear();
                self.mixer.out.set_paused(false);
                self.pacer.set_fps(s.fps());
                self.pacer.reset();
                if let Some(p) = path {
                    self.recents.touch(&sys.id, p);
                }
                self.session = Some(s);
                self.after_game = Some(back);
                self.held.clear();
                self.screen = Screen::Playing;
            }
            Err(e) => {
                eprintln!("mortar-play: {e}");
                self.screen = Screen::Message {
                    crumb: format!("Games / {}", sys.tag),
                    title: "Can't start".into(),
                    lines: vec![e, format!("Core: {}", sys.core)],
                    tag: Some(("Error".into(), PillKind::Orange)),
                    then: Then::Back,
                    back: Box::new(back),
                };
                self.dirty = true;
            }
        }
    }

    /// Start one game and quit when it ends (`mortar-play run`).
    pub fn launch_single(&mut self, sys_id: &str, path: Option<&Path>) -> Result<(), String> {
        self.cfg.system(sys_id).ok_or_else(|| format!("unknown system '{sys_id}'"))?;
        self.single = true;
        self.launch_opt(sys_id, path, false);
        Ok(())
    }

    fn loading(&mut self, system: &str, path: Option<&Path>) {
        let (name, _) = match path {
            Some(p) => library::display_name(&p.file_stem().unwrap_or_default().to_string_lossy()),
            None => (system.to_string(), vec![]),
        };
        let status = self.sys.bar();
        ui::message(
            &mut self.ui,
            &ui::MessageView { status: &status, crumb: system, title: &name, lines: &["Starting…".to_string()], hints: vec![], tag: None },
        );
        self.pf.display.present_ui(&self.ui);
    }

    fn end_game(&mut self) {
        if let Some(mut s) = self.session.take() {
            let saved = s.finish();
            self.saved_toast(&s, saved);
        }
        self.ff_on = false;
        self.mixer.out.clear();
        self.mixer.out.set_paused(true);
        self.thumbs.clear();
        self.shelves = library::scan(&self.cfg);
        self.screen = self.after_game.take().unwrap_or(Screen::Home);
        self.held.clear();
        self.dirty = true;
        if self.single {
            self.quit = true;
        }
    }

    // ------------------------------------------------------------ menu ----

    fn open_menu(&mut self) {
        let Some(s) = self.session.as_mut() else { return };
        self.menu.paused = s.display_frame().into_owned();
        self.menu.aspect = s.aspect();
        self.menu.list = ListState::default();
        self.menu.toast = None;
        self.mixer.out.set_paused(true);
        self.held.clear();
        self.rep = Repeater::default();
        self.screen = Screen::Menu;
        self.dirty = true;
    }

    fn menu_items(&self) -> Vec<(MenuAction, String, Option<String>)> {
        let Some(s) = &self.session else { return Vec::new() };
        let slot = Slot::N(self.menu.slot);
        let when = |t: Option<std::time::SystemTime>| t.map(|t| format!("Saved {}", ago(t))).unwrap_or_else(|| "Empty".into());
        let mut v = vec![
            (MenuAction::Resume, "Resume".to_string(), None),
            (MenuAction::Save, "Save state".to_string(), Some(when(s.state_time(slot)))),
            (MenuAction::Load, "Load state".to_string(), Some(when(s.state_time(slot)))),
        ];
        if let Some((i, n)) = s.core.disk_count()
            && n > 1
        {
            v.push((MenuAction::Disc, "Change disc".into(), Some(format!("Disc {} of {n}", i + 1))));
        }
        v.push((MenuAction::Display, "Display".into(), Some(display_desc(s))));
        v.push((MenuAction::Reset, "Reset".into(), None));
        v.push((MenuAction::Quit, "Save and quit".into(), Some("Resumes here next time".into())));
        v
    }

    fn menu_select(&mut self) {
        let items = self.menu_items();
        let Some((a, _, _)) = items.get(self.menu.list.sel).cloned() else { return };
        let slot = self.menu.slot;
        let Some(s) = self.session.as_mut() else { return };
        let toast = |m: &mut MenuState, t: String| m.toast = Some((t, Instant::now()));
        match a {
            MenuAction::Resume => self.resume(),
            MenuAction::Save => match s.save_state(Slot::N(slot)) {
                Ok(()) => toast(&mut self.menu, format!("Saved to slot {}", slot + 1)),
                Err(e) => toast(&mut self.menu, e),
            },
            MenuAction::Load => {
                if !s.has_state(Slot::N(slot)) {
                    toast(&mut self.menu, format!("Slot {} is empty", slot + 1));
                } else {
                    match s.load_state(Slot::N(slot)) {
                        Ok(()) => self.resume(),
                        Err(e) => toast(&mut self.menu, e),
                    }
                }
            }
            MenuAction::Disc => {
                if let Some(i) = s.core.disk_next() {
                    toast(&mut self.menu, format!("Disc {} inserted", i + 1));
                }
            }
            MenuAction::Display => match s.keep_view(true) {
                Ok(()) => toast(&mut self.menu, format!("{} kept for this game", s.view.label())),
                Err(e) => toast(&mut self.menu, e),
            },
            MenuAction::Reset => {
                s.reset();
                self.resume();
            }
            MenuAction::Quit => self.end_game(),
        }
        self.dirty = true;
    }

    fn resume(&mut self) {
        self.screen = Screen::Playing;
        self.held.clear();
        self.mixer.out.set_paused(false);
        self.pacer.reset();
    }

    // ---------------------------------------------------------- buttons ----

    fn button(&mut self, b: Button) {
        self.dirty = true;
        match &mut self.screen {
            Screen::Home => {
                let targets = self.home_targets();
                let n = targets.len();
                match b {
                    Button::Up => self.home_list.up(n),
                    Button::Down => self.home_list.down(n),
                    Button::A | Button::Start => match targets.get(self.home_list.sel).map(|t| t.0.clone()) {
                        Some(Target::Continue(p, sys)) => self.launch(&sys, &p, false),
                        Some(Target::Recent) => self.screen = Screen::Games { source: Source::Recent, list: ListState::default() },
                        Some(Target::System(i)) if self.cfg.systems[i].no_content => {
                            let id = self.cfg.systems[i].id.clone();
                            self.launch_opt(&id, None, false);
                        }
                        Some(Target::System(i)) => {
                            if self.shelves[i].games.is_empty() {
                                let sys = &self.cfg.systems[i];
                                let place = if self.shelves[i].dirs.len() > 1 { "one of these folders" } else { "this folder" };
                                let mut lines = vec![format!("Copy {} games into {place}:", sys.name)];
                                lines.extend(self.shelves[i].dirs.iter().map(|d| d.to_string_lossy().into_owned()));
                                lines.push(format!("Files: {}, or .zip / .7z archives.", sys.extensions.join(", ")));
                                lines.push("From your computer, over SSH:".into());
                                lines.push(format!("scp -r GAMES root@mortar.local:{}/", self.shelves[i].dirs[0].display()));
                                self.screen = Screen::Message {
                                    crumb: format!("Games / {}", sys.tag),
                                    title: "No games yet".into(),
                                    lines,
                                    tag: None,
                                    then: Then::Back,
                                    back: Box::new(Screen::Home),
                                };
                            } else {
                                self.screen = Screen::Games { source: Source::System(i), list: ListState::default() };
                            }
                        }
                        None => {}
                    },
                    Button::B if !self.under_shell => self.quit = true,
                    _ => {}
                }
            }
            Screen::Games { source, list } => {
                let games = Self::games_of(&self.shelves, &self.recents, &self.cfg, source);
                let n = games.len();
                match b {
                    Button::Up => list.up(n),
                    Button::Down => list.down(n),
                    Button::L1 | Button::Left => list.sel = list.sel.saturating_sub(8),
                    Button::R1 | Button::Right => list.sel = (list.sel + 8).min(n.saturating_sub(1)),
                    Button::B => self.screen = Screen::Home,
                    Button::A | Button::Start => {
                        if let Some(g) = games.get(list.sel).cloned() {
                            self.launch(&g.system, &g.path, false);
                        }
                    }
                    _ => {}
                }
            }
            Screen::Message { then, back, .. } => match b {
                Button::B => {
                    let back = std::mem::replace(back.as_mut(), Screen::Home);
                    self.screen = back;
                    if self.single && self.session.is_none() {
                        self.quit = true;
                    }
                }
                Button::A | Button::Start => {
                    if let Then::Launch(sys, path) = then.clone() {
                        let back = std::mem::replace(back.as_mut(), Screen::Home);
                        self.screen = back;
                        self.launch_opt(&sys, path.as_deref(), true);
                    }
                }
                _ => {}
            },
            Screen::Menu => {
                let items = self.menu_items();
                let n = items.len();
                let on_display = items.get(self.menu.list.sel).is_some_and(|i| i.0 == MenuAction::Display);
                match b {
                    Button::Up => self.menu.list.up(n),
                    Button::Down => self.menu.list.down(n),
                    Button::Left | Button::Right if on_display => {
                        if let Some(s) = self.session.as_mut() {
                            s.step_view(if b == Button::Right { 1 } else { -1 });
                        }
                    }
                    Button::X if on_display => {
                        if let Some(s) = self.session.as_mut() {
                            let t = match s.keep_view(false) {
                                Ok(()) => format!("{} kept for all {} games", s.view.label(), s.sys.name),
                                Err(e) => e,
                            };
                            self.menu.toast = Some((t, Instant::now()));
                        }
                    }
                    Button::Left => self.menu.slot = (self.menu.slot + SLOTS - 1) % SLOTS,
                    Button::Right => self.menu.slot = (self.menu.slot + 1) % SLOTS,
                    Button::A => self.menu_select(),
                    Button::B | Button::Menu | Button::F1 => self.resume(),
                    _ => {}
                }
            }
            Screen::Playing => {}
        }
    }

    // ------------------------------------------------------------- data ----

    fn home_targets(&self) -> Vec<(Target, HomeEntry)> {
        let mut v = Vec::new();
        if let Some(r) = self.recents.list.first()
            && let Some(sys) = self.cfg.system(&r.system)
        {
            let (name, _) = library::display_name(&r.path.file_stem().unwrap_or_default().to_string_lossy());
            v.push((
                Target::Continue(r.path.clone(), r.system.clone()),
                HomeEntry { icon: Icon::Skip, title: name, desc: format!("Continue · {}", sys.name), pill: Some((sys.tag.clone(), PillKind::Ok)), dimmed: false },
            ));
        }
        if self.recents.list.len() > 1 {
            v.push((
                Target::Recent,
                HomeEntry { icon: Icon::File, title: "Recently played".into(), desc: format!("{} games", self.recents.list.len()), pill: None, dimmed: false },
            ));
        }
        let mut empty = Vec::new();
        for (i, (sys, shelf)) in self.cfg.systems.iter().zip(&self.shelves).enumerate() {
            if sys.no_content {
                v.push((Target::System(i), HomeEntry { icon: Icon::Pad, title: sys.name.clone(), desc: "Play".into(), pill: Some((sys.tag.clone(), PillKind::Lavender)), dimmed: false }));
                continue;
            }
            let n = shelf.games.len();
            let e = HomeEntry {
                icon: if n > 0 { Icon::Pad } else { Icon::Folder },
                title: sys.name.clone(),
                desc: if n == 0 { "No games yet".into() } else if n == 1 { "1 game".into() } else { format!("{n} games") },
                pill: Some((sys.tag.clone(), if n > 0 { PillKind::Lavender } else { PillKind::Off })),
                dimmed: n == 0,
            };
            if n > 0 { v.push((Target::System(i), e)) } else { empty.push((Target::System(i), e)) }
        }
        v.extend(empty);
        v
    }

    fn games_of(shelves: &[Shelf], recents: &Recents, cfg: &Config, source: &Source) -> Vec<Game> {
        match source {
            Source::System(i) => shelves[*i].games.clone(),
            Source::Recent => recents
                .list
                .iter()
                .filter(|r| cfg.system(&r.system).is_some())
                .map(|r| {
                    let (name, tags) = library::display_name(&r.path.file_stem().unwrap_or_default().to_string_lossy());
                    let tag = cfg.system(&r.system).map(|s| s.tag.clone()).unwrap_or_default();
                    Game { system: r.system.clone(), path: r.path.clone(), name, tags: std::iter::once(tag).chain(tags).collect() }
                })
                .collect(),
        }
    }

    fn resume_thumb(&mut self, sys_id: &str, path: &Path) -> Option<Frame> {
        let sys = self.cfg.system(sys_id)?;
        let thumb = SavePaths::new(&self.cfg, sys, path).thumb(Slot::Auto);
        self.thumbs.entry(thumb.clone()).or_insert_with(|| Frame::load_png(&thumb)).clone()
    }

    // ------------------------------------------------------------- draw ----

    fn draw(&mut self) {
        self.dirty = false;
        let status = self.sys.bar();
        match &self.screen {
            Screen::Home => {
                let targets = self.home_targets();
                self.home_list.clamp(targets.len());
                let entries: Vec<HomeEntry> = targets.iter().map(|t| t.1.clone()).collect();
                let sel = targets.get(self.home_list.sel).map(|t| t.0.clone());
                let mut preview = None;
                let mut info = Vec::new();
                match &sel {
                    Some(Target::Continue(p, sys)) => {
                        if let Some(f) = self.resume_thumb(sys, p) {
                            let (name, _) = library::display_name(&p.file_stem().unwrap_or_default().to_string_lossy());
                            let sname = self.cfg.system(sys).map(|s| s.name.clone()).unwrap_or_default();
                            preview = Some((f, name, sname));
                        }
                    }
                    Some(Target::System(i)) => {
                        let sys = &self.cfg.systems[*i];
                        info.push(if self.shelves[*i].games.is_empty() { "Copy games to".to_string() } else { "Games folder".to_string() });
                        info.push(self.shelves[*i].dirs.first().map(|d| d.display().to_string()).unwrap_or_default());
                        let missing = sys.missing_bios(&self.cfg.bios_dir(), None);
                        if !missing.is_empty() {
                            info.push(format!("BIOS {} goes in", missing[0].files.first().cloned().unwrap_or_default()));
                            info.push(self.cfg.bios_dir().display().to_string());
                        }
                    }
                    _ => {}
                }
                if entries.is_empty() {
                    info.push("No systems configured.".into());
                }
                let v = ui::HomeView {
                    status: &status,
                    entries: &entries,
                    preview: preview.as_ref().map(|(f, a, b)| (f, a.clone(), b.clone())),
                    info,
                    can_quit: !self.under_shell,
                };
                let mut list = self.home_list.clone();
                ui::home(&mut self.ui, &v, &mut list);
                self.home_list = list;
            }
            Screen::Games { source, list } => {
                let source = source.clone();
                let mut list = list.clone();
                let games = Self::games_of(&self.shelves, &self.recents, &self.cfg, &source);
                list.clamp(games.len());
                let sel = games.get(list.sel).cloned();
                let preview = sel.as_ref().and_then(|g| self.resume_thumb(&g.system, &g.path));
                let rows: Vec<(String, String, bool)> = games
                    .iter()
                    .map(|g| {
                        let resumable = self.cfg.system(&g.system).is_some_and(|s| SavePaths::new(&self.cfg, s, &g.path).state(Slot::Auto).exists());
                        (g.name.clone(), g.tags.first().cloned().unwrap_or_default(), resumable)
                    })
                    .collect();
                let (name, tag) = match &source {
                    Source::System(i) => (self.cfg.systems[*i].name.clone(), self.cfg.systems[*i].tag.clone()),
                    Source::Recent => ("Recently played".to_string(), "Recent".to_string()),
                };
                let mut info = Vec::new();
                if let Some(g) = &sel {
                    info.push(g.path.file_name().unwrap_or_default().to_string_lossy().into_owned());
                }
                let v = ui::GamesView { status: &status, system: &name, tag: &tag, games: &rows, preview: preview.as_ref(), info };
                ui::games(&mut self.ui, &v, &mut list);
                if let Screen::Games { list: l, .. } = &mut self.screen {
                    *l = list;
                }
            }
            Screen::Message { crumb, title, lines, tag, then, .. } => {
                let mut hints = vec![hint("B", "Back")];
                if matches!(then, Then::Launch(..)) {
                    hints.push(hint("A", "Play anyway"));
                }
                let v = ui::MessageView { status: &status, crumb, title, lines, hints, tag: tag.as_ref().map(|(t, k)| (t.as_str(), *k)) };
                ui::message(&mut self.ui, &v);
            }
            Screen::Menu => {
                let items = self.menu_items();
                let Some(s) = &self.session else { return };
                let slot = Slot::N(self.menu.slot);
                let on_slot = matches!(items.get(self.menu.list.sel).map(|i| i.0), Some(MenuAction::Save | MenuAction::Load));
                let thumb = if on_slot { s.state_thumb(slot) } else { None };
                let (picture, aspect, caption) = if on_slot {
                    match &thumb {
                        Some(_) => (thumb.as_ref(), self.menu.aspect, format!("Slot {} · {}", self.menu.slot + 1, s.state_time(slot).map(ago).unwrap_or_default())),
                        None => (None, 0.0, format!("Slot {} is empty", self.menu.slot + 1)),
                    }
                } else {
                    (Some(&self.menu.paused), self.menu.aspect, "Paused".to_string())
                };
                let (name, _) = library::display_name(&s.game.as_ref().and_then(|p| p.file_stem()).unwrap_or_default().to_string_lossy());
                let v = ui::MenuView {
                    status: &status,
                    system: &s.sys.name,
                    game: &name,
                    items: &items,
                    slot: self.menu.slot,
                    display: s.view.label(),
                    picture,
                    aspect,
                    caption,
                    toast: self.menu.toast.as_ref().map(|(t, _)| t.as_str()),
                };
                let mut list = self.menu.list.clone();
                ui::game_menu(&mut self.ui, &v, &mut list);
                self.menu.list = list;
            }
            Screen::Playing => return,
        }
        self.pf.display.present_ui(&self.ui);
    }
}

/// The Display choice for the shell's game panel.
fn display_json(s: &Session) -> serde_json::Value {
    serde_json::json!({
        "label": s.view.label(), "key": s.view.key(), "scope": s.view_scope.name(),
        "system_name": s.sys.name,
    })
}

/// Where the Display choice comes from, for the menu row.
fn display_desc(s: &Session) -> String {
    use crate::display::Scope;
    match s.view_scope {
        Scope::Default => format!("{} default", s.sys.name),
        Scope::System => format!("Kept for all {} games", s.sys.name),
        Scope::Game => "Kept for this game".into(),
        Scope::Session => "A: keep for this game · X: for all".into(),
    }
}

/// "just now", "5 min ago", "3 h ago", "2 days ago".
pub fn ago(t: std::time::SystemTime) -> String {
    let s = t.elapsed().map(|d| d.as_secs()).unwrap_or(0);
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86400 => format!("{} h ago", s / 3600),
        _ => format!("{} days ago", s / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_parse() {
        let s = parse_script("wait 60, press a; hold r2,release r2\nshot /tmp/x.png, quit").unwrap();
        assert_eq!(s[0], Step::Wait(60));
        assert_eq!(s[1], Step::Press(Button::A));
        assert_eq!(s[2], Step::Hold(Button::R2));
        assert_eq!(s[4], Step::Shot(PathBuf::from("/tmp/x.png")));
        assert_eq!(s[5], Step::Quit);
        assert!(parse_script("jump").is_err());
    }

    #[test]
    fn ago_text() {
        let now = std::time::SystemTime::now();
        assert_eq!(ago(now), "just now");
        assert_eq!(ago(now - Duration::from_secs(300)), "5 min ago");
    }
}
