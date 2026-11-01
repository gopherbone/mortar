// SPDX-License-Identifier: GPL-2.0-or-later
//! The shell's own UI, after the approved motion prototype
//! (plan/mortar-motion): boot, home, the MENU drawer (Apps fan of live
//! cards, Launch grid, Settings), launch/close/switch transitions, the
//! volume OSD, toasts and the screenshot flash.
//!
//! Pure state: the compositor feeds it buttons, the passing of time and a
//! `View` of the apps; it answers with `Out`s (actions, sounds, haptic
//! ticks) and builds a `Scene` for a renderer. Everything is laid out in
//! the prototype's 1024x768 logical pixels.
//!
//! Layers, bottom to top: home or the visible app (+ on-screen keyboard),
//! the launch/close/switch transition, the drawer and its scrim, a setup
//! page, the OSD, a toast, the shutter flash, the boot screen.

use crate::art::{self, Art, Pal, DAY, NIGHT};
use crate::haptics::Haptic;
use crate::motion::{self, Params, Preset, Spring};
use crate::scene::{ts, Layer, Scene, Src, Typo, Xf, B, P};
use crate::sfx::Sfx;
use mortar_setup::ui::SetupUi;
use mortar_ui::gfx::{Color, Face, TextStyle};
use mortar_ui::input::Button;
use mortar_ui::status::Sys;
use serde::{Deserialize, Serialize};

pub const W: f32 = 1024.0;
pub const H: f32 = 768.0;

// ------------------------------------------------------------- settings ----

/// User settings changed from the Settings tab (persisted by the compositor;
/// defaults from the NixOS module).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Prefs {
    pub sounds: bool,
    /// UI sound volume, percent.
    pub sound_volume: u8,
    pub haptics: bool,
    pub motion: Preset,
    pub reduce_motion: bool,
    pub night: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { sounds: true, sound_volume: 60, haptics: true, motion: Preset::Bouncy, reduce_motion: false, night: false }
    }
}

// ----------------------------------------------------------------- view ----

#[derive(Clone, Debug)]
pub struct RunningInfo {
    pub uid: u32,
    pub entry: Option<String>,
    pub name: String,
    pub art: Art,
    /// It has a window with a buffer (something to show).
    pub has_buffer: bool,
    pub closing: bool,
    /// Where its buffer goes on screen when shown alone (x, y, w, h).
    pub geom: [f32; 4],
}

#[derive(Clone, Debug)]
pub struct LauncherInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub art: Art,
    pub running: bool,
}

/// What the compositor knows, rebuilt for each call.
pub struct View<'a> {
    /// Running apps, most recently used first.
    pub running: &'a [RunningInfo],
    pub launcher: &'a [LauncherInfo],
    pub sys: &'a Sys,
    pub volume: Option<u8>,
    /// The app in front (None: the home screen).
    pub current: Option<u32>,
    /// The app in front takes keys (offer the on-screen keyboard).
    pub keyboard_app: bool,
    /// Height of the on-screen keyboard strip when it shows (0: hidden).
    pub osk_h: f32,
    pub host: &'a str,
}

impl View<'_> {
    fn app(&self, uid: u32) -> Option<&RunningInfo> {
        self.running.iter().find(|r| r.uid == uid)
    }
    fn by_entry(&self, id: &str) -> Option<&RunningInfo> {
        self.running.iter().find(|r| r.entry.as_deref() == Some(id))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Out {
    Launch(String),
    Focus(u32),
    Close(u32),
    /// Show the home screen (the front app keeps running behind).
    Home,
    Volume(i32),
    Brightness(i32),
    ShowKeyboard,
    OpenSetup { wifi: bool },
    Sleep,
    Logout,
    Reboot,
    PowerOff,
    Sfx(Sfx),
    Haptic(Haptic),
    /// Settings changed (save and apply).
    Prefs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Apps,
    Launch,
    Settings,
}

const TABS: [Tab; 3] = [Tab::Apps, Tab::Launch, Tab::Settings];

impl Tab {
    fn name(self) -> &'static str {
        match self {
            Tab::Apps => "Apps",
            Tab::Launch => "Launch",
            Tab::Settings => "Settings",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Setting {
    Sounds,
    Haptics,
    Motion,
    ReduceMotion,
    Palette,
    Volume,
    Brightness,
    Keyboard,
    Wifi,
    Remote,
    Sleep,
    Logout,
    Reboot,
    PowerOff,
}

fn settings(keyboard_app: bool) -> Vec<Setting> {
    use Setting::*;
    let mut v = vec![Sounds, Haptics, Motion, ReduceMotion, Palette, Volume, Brightness];
    if keyboard_app {
        v.push(Keyboard);
    }
    v.extend([Wifi, Remote, Sleep, Logout, Reboot, PowerOff]);
    v
}

// ---------------------------------------------------------------- state ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CardKey {
    Home,
    App(u32),
}

struct Card {
    key: CardKey,
    y: Spring,
    gone: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
enum Then {
    Home,
    Focus(u32),
    Launch(String, usize),
}

struct Menu {
    tab: Tab,
    dy: Spring,
    sa: Spring,
    fan_pos: Spring,
    fan_sel: usize,
    cards: Vec<Card>,
    lg_sel: usize,
    lg_scroll: Spring,
    set_sel: usize,
    set_scroll: Spring,
    closing: bool,
    then: Option<Then>,
}

struct TileAnim {
    lift: Spring,
    sc: Spring,
    back: Spring,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct R4 {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

fn lerp(a: f32, b: f32, p: f32) -> f32 {
    a + (b - a) * p
}

#[derive(Clone, Debug, PartialEq)]
enum Target {
    Entry(String),
    Uid(u32),
}

enum Trans {
    /// An app grows out of its tile (from home).
    Launch { target: Target, name: String, art: Art, from: R4, t0: f64 },
    /// The front app shrinks back into its tile.
    Close { uid: u32, art: Art, to: R4 },
    /// Another app slides in from the right over the current screen.
    Swap { target: Target, name: String, art: Art, prev: Option<u32>, t0: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastIcon {
    Star,
    Shot,
    Info,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub icon: ToastIcon,
    pub title: String,
    pub sub: String,
    pub secs: f32,
    pub sound: Option<Sfx>,
    pub haptic: Option<Haptic>,
}

impl Toast {
    pub fn info(title: impl Into<String>) -> Toast {
        Toast { icon: ToastIcon::Info, title: title.into(), sub: String::new(), secs: 2.6, sound: Some(Sfx::Tab), haptic: None }
    }
    pub fn achievement(title: impl Into<String>, sub: impl Into<String>) -> Toast {
        Toast {
            icon: ToastIcon::Star,
            title: title.into(),
            sub: format!("ACHIEVEMENT · {}", sub.into()),
            secs: 3.2,
            sound: Some(Sfx::Unlock),
            haptic: Some(Haptic::Double),
        }
    }
}

struct ToastState {
    toast: Toast,
    y: Spring,
    until: f64,
}

struct Osd {
    x: Spring,
    until: f64,
    value: u8,
    label: &'static str,
}

struct Boot {
    t0: f64,
    blocks: [(Spring, Spring, Spring); 4],
    word: Spring,
    y: Spring,
    fired: u8,
}

const BOOT_START: [(f32, f32, f32); 4] = [(-400.0, -300.0, -40.0), (1400.0, -200.0, 30.0), (-300.0, 1000.0, 25.0), (1300.0, 900.0, -35.0)];
const BOOT_END: [(f32, f32); 4] = [(362.0, 166.0), (512.0, 166.0), (362.0, 316.0), (512.0, 316.0)];

/// Background shapes: (w, h, x, y, round).
const SHAPES: [(f32, f32, f32, f32, bool); 5] = [
    (120.0, 160.0, 760.0, 300.0, true),
    (70.0, 70.0, 900.0, 600.0, false),
    (200.0, 200.0, -60.0, 520.0, true),
    (90.0, 90.0, 560.0, 200.0, false),
    (140.0, 140.0, 300.0, 640.0, true),
];

/// Ambient drift keeps going this long after the last button press.
const DRIFT_SECS: f64 = 30.0;

/// How often the compositor should redraw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pace {
    /// Something moves: every vblank.
    Full,
    /// Only the slow background drift: ~20 fps is plenty.
    Ambient,
    /// Nothing moves: redraw on change only.
    Idle,
}

pub struct ShellUi {
    pub prefs: Prefs,
    t: f64,
    drift_t: f64,
    drift_until: f64,
    focus: usize,
    row_x: Spring,
    tiles: Vec<TileAnim>,
    lp: Spring,
    swap_p: Spring,
    trans: Option<Trans>,
    menu: Option<Menu>,
    page: Option<SetupUi>,
    osd: Option<Osd>,
    toast: Option<ToastState>,
    toasts: Vec<Toast>,
    flash: Option<Spring>,
    boot: Option<Boot>,
    confirm: Option<(Setting, f64)>,
    out: Vec<Out>,
    /// Something changed that a frame must show (even if nothing moves).
    pub dirty: bool,
}

impl ShellUi {
    pub fn new(prefs: Prefs, boot: bool) -> ShellUi {
        let mut ui = ShellUi {
            prefs,
            t: 0.0,
            drift_t: 0.0,
            drift_until: DRIFT_SECS,
            focus: 0,
            row_x: Spring::new(0.0),
            tiles: Vec::new(),
            lp: Spring::new(0.0),
            swap_p: Spring::new(1.0),
            trans: None,
            menu: None,
            page: None,
            osd: None,
            toast: None,
            toasts: Vec::new(),
            flash: None,
            boot: None,
            confirm: None,
            out: Vec::new(),
            dirty: true,
        };
        if boot {
            ui.play_boot();
        }
        ui
    }

    fn params(&self) -> Params {
        Params::new(self.prefs.motion, self.prefs.reduce_motion)
    }

    pub fn pal(&self) -> Pal {
        if self.prefs.night { NIGHT } else { DAY }
    }

    fn fx(&mut self, s: Sfx, h: Option<Haptic>) {
        self.out.push(Out::Sfx(s));
        if let Some(h) = h {
            self.out.push(Out::Haptic(h));
        }
    }

    // ------------------------------------------------------ queries ----

    /// The shell takes the buttons (else they go to the front app).
    pub fn captures_input(&self, has_app: bool) -> bool {
        !has_app || self.boot.is_some() || self.menu.is_some() || self.page.is_some() || self.trans.is_some()
    }

    /// The front app is (mostly) hidden behind the shell's screens.
    pub fn covers_app(&self, has_app: bool) -> bool {
        self.captures_input(has_app)
    }

    /// Anything of the shell's is on screen over the app (or instead of it).
    pub fn layer_visible(&self, has_app: bool) -> bool {
        self.covers_app(has_app) || self.osd.is_some() || self.toast.is_some() || self.flash.is_some()
    }

    pub fn menu_open(&self) -> bool {
        self.menu.as_ref().is_some_and(|m| !m.closing)
    }

    pub fn tab(&self) -> Option<Tab> {
        self.menu.as_ref().map(|m| m.tab)
    }

    pub fn page_mut(&mut self) -> Option<&mut SetupUi> {
        self.page.as_mut()
    }

    pub fn page_open(&self) -> bool {
        self.page.is_some()
    }

    pub fn close_page(&mut self) {
        self.page = None;
        self.dirty = true;
    }

    pub fn open_page(&mut self, p: SetupUi) {
        self.page = Some(p);
        self.dirty = true;
    }

    pub fn booting(&self) -> bool {
        self.boot.is_some()
    }

    /// Live app cards are on screen (their apps should keep drawing).
    pub fn cards_visible(&self) -> bool {
        self.menu.as_ref().is_some_and(|m| m.tab == Tab::Apps) && self.page.is_none()
    }

    pub fn pace(&self, has_app: bool) -> Pace {
        let moving = self.boot.is_some()
            || self.trans.is_some()
            || self.flash.is_some()
            || self.toast.is_some()
            || self.osd.is_some()
            || self.menu.is_some()
            || self.springs_ref().iter().any(|s| !s.rest());
        if moving {
            // a drawer at rest only needs frames for its live cards
            let menu_rest = self.menu.as_ref().is_some_and(|m| {
                !m.closing && [&m.dy, &m.sa, &m.fan_pos, &m.lg_scroll, &m.set_scroll].iter().all(|s| s.rest())
                    && m.cards.iter().all(|c| c.gone.is_none() && c.y.rest())
            });
            let others = self.boot.is_some()
                || self.trans.is_some()
                || self.flash.is_some()
                || self.toast.is_some()
                || self.osd.is_some()
                || self.springs_ref().iter().any(|s| !s.rest());
            if menu_rest && !others {
                return if !has_app && self.drifting() { Pace::Ambient } else { Pace::Idle };
            }
            return Pace::Full;
        }
        if !has_app && self.drifting() {
            return Pace::Ambient;
        }
        Pace::Idle
    }

    fn drifting(&self) -> bool {
        !self.prefs.reduce_motion && self.t < self.drift_until
    }

    /// When the next timed event is due (toast/OSD expiry, confirm reset),
    /// in UI seconds from now.
    pub fn next_deadline(&self) -> Option<f64> {
        let mut d: Option<f64> = None;
        let mut take = |t: f64| d = Some(d.map_or(t, |x: f64| x.min(t)));
        if let Some(o) = &self.osd {
            take(o.until - self.t);
        }
        if let Some(t) = &self.toast {
            take(t.until - self.t);
        }
        if self.drifting() {
            take(self.drift_until - self.t);
        }
        d.map(|v| v.max(0.0))
    }

    fn springs_ref(&self) -> Vec<&Spring> {
        let mut v: Vec<&Spring> = vec![&self.row_x, &self.lp, &self.swap_p];
        for t in &self.tiles {
            v.extend([&t.lift, &t.sc, &t.back]);
        }
        v
    }

    fn springs_mut(&mut self) -> Vec<&mut Spring> {
        let mut v: Vec<&mut Spring> = vec![&mut self.row_x, &mut self.lp, &mut self.swap_p];
        for t in &mut self.tiles {
            v.extend([&mut t.lift, &mut t.sc, &mut t.back]);
        }
        if let Some(m) = &mut self.menu {
            v.extend([&mut m.dy, &mut m.sa, &mut m.fan_pos, &mut m.lg_scroll, &mut m.set_scroll]);
            for c in &mut m.cards {
                v.push(&mut c.y);
            }
        }
        if let Some(o) = &mut self.osd {
            v.push(&mut o.x);
        }
        if let Some(t) = &mut self.toast {
            v.push(&mut t.y);
        }
        if let Some(f) = &mut self.flash {
            v.push(f);
        }
        if let Some(b) = &mut self.boot {
            for (x, y, r) in &mut b.blocks {
                v.extend([x, y, r]);
            }
            v.extend([&mut b.word, &mut b.y]);
        }
        v
    }

    // --------------------------------------------------------- time ----

    /// Advance UI time by `dt` seconds; returns what to do.
    pub fn step(&mut self, dt: f64, v: &View) -> Vec<Out> {
        // timers run on real time; springs never jump more than 50 ms (a
        // long stall must not fling them)
        let real = dt.clamp(0.0, 5.0);
        let dt = dt.clamp(0.0, 0.05);
        if self.drifting() {
            self.drift_t += dt;
        }
        self.t += real;
        self.sync_tiles(v);
        let p = self.params();
        {
            let mut s = self.springs_mut();
            motion::step_all(&mut s, dt as f32, p);
        }
        self.timers(v);
        std::mem::take(&mut self.out)
    }

    fn sync_tiles(&mut self, v: &View) {
        if self.tiles.len() != v.launcher.len() {
            self.tiles = (0..v.launcher.len()).map(|_| TileAnim { lift: Spring::new(0.0), sc: Spring::new(1.0), back: Spring::new(0.0) }).collect();
            self.focus = self.focus.min(v.launcher.len().saturating_sub(1));
            self.set_focus(self.focus as i32, v.launcher.len(), true);
        }
    }

    fn timers(&mut self, v: &View) {
        let now = self.t;
        // boot schedule
        if let Some(b) = &mut self.boot {
            let e = now - b.t0;
            let mut ticks = 0;
            for i in 0..4 {
                if b.fired & (1 << i) == 0 && e >= 0.12 + i as f64 * 0.14 {
                    b.fired |= 1 << i;
                    let (x, y, r) = &mut b.blocks[i];
                    x.set(BOOT_END[i].0);
                    y.set(BOOT_END[i].1);
                    r.set(0.0);
                    ticks += 1;
                }
            }
            let mut chime = false;
            if b.fired & 16 == 0 && e >= 0.82 {
                b.fired |= 16;
                b.word.set(1.0);
                chime = true;
            }
            if b.fired & 32 == 0 && e >= 2.1 {
                b.fired |= 32;
                b.y.set(-800.0);
            }
            let done = e >= 2.7;
            for _ in 0..ticks {
                self.fx(Sfx::Tick, Some(Haptic::Tick));
            }
            if chime {
                self.fx(Sfx::Chime, None);
            }
            if done {
                self.boot = None;
                self.dirty = true;
            }
        }
        // drawer closing: run what was asked once it is out of sight
        if let Some(m) = &self.menu
            && m.closing
            && m.dy.x < -660.0
        {
            let then = self.menu.take().and_then(|m| m.then);
            self.dirty = true;
            if let Some(th) = then {
                self.run_then(th, v);
            }
        }
        // fan: drop cards that flew away, add newly running apps
        if let Some(m) = &mut self.menu {
            let before = m.cards.len();
            m.cards.retain(|c| match (c.key, c.gone) {
                (_, Some(t)) => now - t < 0.38,
                (CardKey::App(u), None) => v.app(u).is_some(),
                (CardKey::Home, None) => true,
            });
            for r in v.running {
                if !m.cards.iter().any(|c| c.key == CardKey::App(r.uid)) {
                    m.cards.push(Card { key: CardKey::App(r.uid), y: Spring::new(0.0), gone: None });
                }
            }
            if m.cards.len() != before {
                let n = m.cards.len();
                m.fan_sel = m.fan_sel.min(n.saturating_sub(1));
                // skip over a card that is flying away
                while m.fan_sel > 0 && m.cards[m.fan_sel].gone.is_some() {
                    m.fan_sel -= 1;
                }
                m.fan_pos.snap(m.fan_sel as f32);
            }
        }
        // transitions
        match &self.trans {
            Some(Trans::Launch { target, t0, .. }) | Some(Trans::Swap { target, t0, .. }) => {
                let is_launch = matches!(self.trans, Some(Trans::Launch { .. }));
                let spring = if is_launch { &self.lp } else { &self.swap_p };
                let anim_done = spring.rest() || spring.x > 0.995;
                let shown = target_uid(target, v).is_some_and(|u| v.current == Some(u) && v.app(u).is_some_and(|a| a.has_buffer));
                if anim_done && shown {
                    self.trans = None;
                    self.dirty = true;
                } else if now - t0 > 20.0 {
                    let name = match &self.trans {
                        Some(Trans::Launch { name, .. } | Trans::Swap { name, .. }) => name.clone(),
                        _ => String::new(),
                    };
                    self.trans = None;
                    self.dirty = true;
                    self.show_toast(Toast::info(format!("{name} did not start")));
                    self.fx(Sfx::Deny, None);
                }
            }
            Some(Trans::Close { .. }) => {
                if self.lp.x < 0.01 && self.lp.v.abs() < 0.5 {
                    self.trans = None;
                    self.dirty = true;
                }
            }
            None => {}
        }
        // OSD
        let mut drop_osd = false;
        if let Some(o) = &mut self.osd {
            if now > o.until {
                o.x.set(140.0);
            }
            drop_osd = o.x.t == 140.0 && o.x.x > 135.0;
        }
        if drop_osd {
            self.osd = None;
            self.dirty = true;
        }
        // toasts
        let mut drop_toast = false;
        if let Some(t) = &mut self.toast {
            if now > t.until {
                t.y.set(-220.0);
            }
            drop_toast = t.y.t == -220.0 && t.y.x < -210.0;
        }
        if drop_toast {
            self.toast = None;
            self.dirty = true;
            if !self.toasts.is_empty() {
                let next = self.toasts.remove(0);
                self.show_toast(next);
            }
        }
        // flash
        if self.flash.as_ref().is_some_and(|f| f.x < 0.01 && f.t == 0.0) {
            self.flash = None;
            self.dirty = true;
        }
        if self.confirm.as_ref().is_some_and(|c| now > c.1) {
            self.confirm = None;
        }
    }

    // ------------------------------------------------------- events ----

    pub fn play_boot(&mut self) {
        let mk = |i: usize| (Spring::new(BOOT_START[i].0), Spring::new(BOOT_START[i].1), Spring::new(BOOT_START[i].2));
        self.boot = Some(Boot { t0: self.t, blocks: [mk(0), mk(1), mk(2), mk(3)], word: Spring::new(0.0), y: Spring::new(0.0), fired: 0 });
        self.dirty = true;
    }

    /// The volume or brightness changed: slide the OSD in.
    pub fn osd(&mut self, label: &'static str, value: u8) {
        let until = self.t + 1.5;
        match &mut self.osd {
            Some(o) if o.label == label => {
                o.x.set(0.0);
                o.until = until;
                o.value = value;
            }
            _ => {
                let mut x = Spring::new(140.0);
                x.set(0.0);
                self.osd = Some(Osd { x, until, value, label });
            }
        }
        self.dirty = true;
    }

    pub fn show_toast(&mut self, t: Toast) {
        // info toasts replace what is up; achievements wait their turn
        if let Some(cur) = &self.toast
            && (t.icon == ToastIcon::Star && cur.toast.icon == ToastIcon::Star)
        {
            self.toasts.push(t);
            return;
        }
        if let Some(s) = t.sound {
            self.out.push(Out::Sfx(s));
        }
        if let Some(h) = t.haptic {
            self.out.push(Out::Haptic(h));
        }
        let mut y = Spring::new(-200.0);
        y.set(0.0);
        let until = self.t + t.secs as f64;
        self.toast = Some(ToastState { toast: t, y, until });
        self.dirty = true;
    }

    /// Take the outs queued by `osd`/`show_toast`/`screenshot` outside
    /// `step` and `button`.
    pub fn take_out(&mut self) -> Vec<Out> {
        std::mem::take(&mut self.out)
    }

    /// The shutter: flash, sound, a long buzz. The toast follows when the
    /// file is written (`show_toast`).
    pub fn screenshot(&mut self) {
        let mut f = Spring::new(1.0);
        f.set(0.0);
        self.flash = Some(f);
        self.fx(Sfx::Shutter, Some(Haptic::Long));
        self.dirty = true;
    }

    /// The compositor could not start an app the UI is animating in.
    pub fn launch_failed(&mut self, msg: &str) {
        if matches!(self.trans, Some(Trans::Launch { .. } | Trans::Swap { .. })) {
            self.trans = None;
        }
        self.show_toast(Toast::info(msg.to_string()));
        self.fx(Sfx::Deny, None);
        self.dirty = true;
    }

    /// A button was pressed (`repeat`: software auto-repeat).
    pub fn button(&mut self, b: Button, repeat: bool, v: &View) -> Vec<Out> {
        self.drift_until = self.t + DRIFT_SECS;
        self.dirty = true;
        if self.boot.is_some() {
            return Vec::new();
        }
        if self.page.is_some() {
            self.page_button(b, v);
        } else if self.menu.is_some() {
            self.menu_button(b, repeat, v);
        } else if self.trans.is_some() {
            self.trans_button(b, v);
        } else if v.current.is_none() {
            self.home_button(b, repeat, v);
        }
        std::mem::take(&mut self.out)
    }

    /// MENU (on its own) toggles the drawer.
    pub fn menu_key(&mut self, v: &View) -> Vec<Out> {
        self.drift_until = self.t + DRIFT_SECS;
        self.dirty = true;
        if self.boot.is_some() {
            return Vec::new();
        }
        if self.page.is_some() {
            self.page = None;
            self.close_menu(None);
        } else if self.menu.as_ref().is_some_and(|m| !m.closing) {
            self.close_menu(None);
        } else {
            if let Some(Trans::Launch { .. } | Trans::Swap { .. }) = &self.trans {
                // still starting: leave it to start in the background
                self.trans = None;
                self.lp.snap(0.0);
                self.swap_p.snap(1.0);
            }
            self.open_menu(v);
        }
        std::mem::take(&mut self.out)
    }

    fn page_button(&mut self, b: Button, _v: &View) {
        if let Some(p) = &mut self.page
            && p.button(b)
        {
            self.page = None;
        }
    }

    fn trans_button(&mut self, b: Button, _v: &View) {
        // B while an app is still starting: back home, it keeps starting
        if b == Button::B && self.lp.x > 0.99 && matches!(self.trans, Some(Trans::Launch { .. })) {
            self.trans = None;
            self.lp.snap(0.0);
            self.fx(Sfx::Close, Some(Haptic::Tick));
        }
    }

    fn set_focus(&mut self, i: i32, n: usize, snap: bool) {
        if n == 0 {
            return;
        }
        self.focus = i.rem_euclid(n as i32) as usize;
        for (j, t) in self.tiles.iter_mut().enumerate() {
            let on = j == self.focus;
            t.lift.set(if on { -18.0 } else { 0.0 });
            t.sc.set(if on { 1.05 } else { 1.0 });
            t.back.set(if on { 1.0 } else { 0.0 });
        }
        let tile_w = 232.0 + 28.0;
        let visible = W - 112.0;
        let want = (-(self.focus as f32 * tile_w - visible / 2.0 + tile_w / 2.0)).max(-(n as f32 * tile_w - 28.0 - visible)).min(0.0);
        self.row_x.set(want);
        if snap {
            self.row_x.snap(want);
            for (j, t) in self.tiles.iter_mut().enumerate() {
                let on = j == self.focus;
                t.lift.snap(if on { -18.0 } else { 0.0 });
                t.sc.snap(if on { 1.05 } else { 1.0 });
                t.back.snap(if on { 1.0 } else { 0.0 });
            }
        }
    }

    fn home_button(&mut self, b: Button, _repeat: bool, v: &View) {
        let n = v.launcher.len();
        match b {
            Button::Left | Button::Right if n > 0 => {
                let d = if b == Button::Left { -1 } else { 1 };
                self.set_focus(self.focus as i32 + d, n, false);
                self.fx(Sfx::Move, Some(Haptic::Tick));
            }
            Button::A | Button::Start if n > 0 => {
                let i = self.focus.min(n - 1);
                self.launch_from_tile(i, v);
            }
            Button::B | Button::Up | Button::Down => self.fx(Sfx::Deny, None),
            Button::L1 | Button::R1 | Button::Select => {
                // shortcuts: open the drawer on Launch/Settings
                self.open_menu(v);
                if let Some(m) = &mut self.menu {
                    m.tab = if b == Button::R1 { Tab::Settings } else { Tab::Launch };
                }
            }
            _ => {}
        }
    }

    fn tile_rect(&self, i: usize) -> R4 {
        let t = self.tiles.get(i);
        let (lift, sc) = t.map(|t| (t.lift.x, t.sc.x)).unwrap_or((0.0, 1.0));
        let x = 56.0 + self.row_x.x + i as f32 * 260.0;
        let (cx, cy) = (x + 116.0, 302.0 + 159.0 + lift);
        R4 { x: cx - 116.0 * sc, y: cy - 159.0 * sc, w: 232.0 * sc, h: 318.0 * sc }
    }

    fn launch_from_tile(&mut self, i: usize, v: &View) {
        let Some(l) = v.launcher.get(i) else { return };
        if !self.tiles.is_empty() && i != self.focus {
            self.set_focus(i as i32, v.launcher.len(), true);
        }
        let from = self.tile_rect(i);
        self.trans = Some(Trans::Launch { target: Target::Entry(l.id.clone()), name: l.name.clone(), art: l.art, from, t0: self.t });
        self.lp.snap(0.0);
        self.lp.set(1.0);
        self.out.push(Out::Launch(l.id.clone()));
        self.fx(Sfx::Open, Some(Haptic::Double));
    }

    fn go_home(&mut self, v: &View) {
        let Some(cur) = v.current else { return };
        let a = v.app(cur);
        let art = a.map(|a| a.art).unwrap_or_default();
        let i = a.and_then(|a| a.entry.as_ref()).and_then(|e| v.launcher.iter().position(|l| &l.id == e));
        if let Some(i) = i {
            self.set_focus(i as i32, v.launcher.len(), true);
        }
        let to = match i {
            Some(i) => self.tile_rect(i),
            None => R4 { x: W / 2.0 - 116.0, y: 302.0, w: 232.0, h: 318.0 },
        };
        self.trans = Some(Trans::Close { uid: cur, art, to });
        self.lp.snap(1.0);
        self.lp.set(0.0);
        self.out.push(Out::Home);
        self.fx(Sfx::Close, Some(Haptic::Tick));
    }

    fn swap_to(&mut self, target: Target, v: &View) {
        let (name, art) = match &target {
            Target::Uid(u) => v.app(*u).map(|a| (a.name.clone(), a.art)).unwrap_or_default(),
            Target::Entry(id) => v.launcher.iter().find(|l| &l.id == id).map(|l| (l.name.clone(), l.art)).unwrap_or_default(),
        };
        match &target {
            Target::Uid(u) => self.out.push(Out::Focus(*u)),
            Target::Entry(id) => self.out.push(Out::Launch(id.clone())),
        }
        self.trans = Some(Trans::Swap { target, name, art, prev: v.current, t0: self.t });
        self.swap_p.snap(0.0);
        self.swap_p.set(1.0);
        self.fx(Sfx::Open, Some(Haptic::Double));
    }

    fn run_then(&mut self, th: Then, v: &View) {
        match th {
            Then::Home => {
                if v.current.is_some() {
                    self.go_home(v);
                }
            }
            Then::Focus(uid) => {
                if v.current != Some(uid) {
                    self.swap_to(Target::Uid(uid), v);
                }
            }
            Then::Launch(id, idx) => {
                let running = v.by_entry(&id).map(|a| a.uid);
                if running.is_some() && running == v.current {
                    return;
                }
                if v.current.is_some() {
                    match running {
                        Some(u) => self.swap_to(Target::Uid(u), v),
                        None => self.swap_to(Target::Entry(id), v),
                    }
                } else {
                    self.set_focus(idx as i32, v.launcher.len(), true);
                    self.launch_from_tile(idx, v);
                }
            }
        }
    }

    fn open_menu(&mut self, v: &View) {
        let mut cards = vec![Card { key: CardKey::Home, y: Spring::new(0.0), gone: None }];
        cards.extend(v.running.iter().map(|r| Card { key: CardKey::App(r.uid), y: Spring::new(0.0), gone: None }));
        // MENU then A flips to the previous app: preselect it
        let fan_sel = match v.current {
            Some(_) if v.running.len() > 1 => 2,
            Some(_) => 1,
            None if !v.running.is_empty() => 1,
            None => 0,
        };
        let mut dy = Spring::new(-680.0);
        dy.set(0.0);
        let mut sa = Spring::new(0.0);
        sa.set(1.0);
        let lg_sel = self.focus.min(v.launcher.len().saturating_sub(1));
        self.menu = Some(Menu {
            tab: Tab::Apps,
            dy,
            sa,
            fan_pos: Spring::new(fan_sel as f32),
            fan_sel,
            cards,
            lg_sel,
            lg_scroll: Spring::new(lg_scroll_for(lg_sel)),
            set_sel: 0,
            set_scroll: Spring::new(0.0),
            closing: false,
            then: None,
        });
        self.confirm = None;
        self.fx(Sfx::Drawer, Some(Haptic::Tick));
    }

    fn close_menu(&mut self, then: Option<Then>) {
        if let Some(m) = &mut self.menu {
            m.dy.set(-700.0);
            m.sa.set(0.0);
            m.closing = true;
            m.then = then;
        }
        self.fx(Sfx::DrawerUp, Some(Haptic::Tick));
    }

    fn menu_button(&mut self, b: Button, repeat: bool, v: &View) {
        let Some(m) = &mut self.menu else { return };
        if m.closing {
            return;
        }
        match b {
            Button::B => {
                self.close_menu(None);
                return;
            }
            Button::L1 | Button::R1 if !repeat => {
                let i = TABS.iter().position(|t| *t == m.tab).unwrap_or(0);
                m.tab = TABS[(i + if b == Button::R1 { 1 } else { 2 }) % 3];
                self.confirm = None;
                self.fx(Sfx::Tab, Some(Haptic::Tick));
                return;
            }
            _ => {}
        }
        match m.tab {
            Tab::Apps => self.apps_button(b, repeat),
            Tab::Launch => self.launch_button(b, repeat, v),
            Tab::Settings => self.settings_button(b, repeat, v),
        }
    }

    fn apps_button(&mut self, b: Button, repeat: bool) {
        let Some(m) = &mut self.menu else { return };
        let n = m.cards.len();
        match b {
            Button::Left | Button::Right => {
                let d: i32 = if b == Button::Right { 1 } else { -1 };
                let mut s = m.fan_sel as i32;
                loop {
                    s += d;
                    if s < 0 || s >= n as i32 || m.cards[s as usize].gone.is_none() {
                        break;
                    }
                }
                let s = s.clamp(0, n as i32 - 1) as usize;
                if s != m.fan_sel && m.cards[s].gone.is_none() {
                    m.fan_sel = s;
                    m.fan_pos.set(s as f32);
                    self.fx(Sfx::Move, Some(Haptic::Tick));
                } else {
                    self.fx(Sfx::Deny, None);
                }
            }
            Button::A if !repeat => {
                let key = m.cards.get(m.fan_sel).map(|c| c.key);
                match key {
                    Some(CardKey::Home) => self.close_menu(Some(Then::Home)),
                    Some(CardKey::App(u)) => self.close_menu(Some(Then::Focus(u))),
                    None => {}
                }
            }
            Button::X if !repeat => {
                let t = self.t;
                match m.cards.get_mut(m.fan_sel) {
                    Some(c) if c.key != CardKey::Home && c.gone.is_none() => {
                        c.gone = Some(t);
                        c.y.set(700.0);
                        let CardKey::App(uid) = c.key else { return };
                        self.out.push(Out::Close(uid));
                        self.fx(Sfx::Close, Some(Haptic::Double));
                    }
                    _ => self.fx(Sfx::Deny, None),
                }
            }
            _ => {}
        }
    }

    fn launch_button(&mut self, b: Button, repeat: bool, v: &View) {
        let Some(m) = &mut self.menu else { return };
        let n = v.launcher.len();
        if n == 0 {
            return;
        }
        let cols = 3;
        let old = m.lg_sel.min(n - 1);
        let new = match b {
            Button::Left => old.saturating_sub(1),
            Button::Right => (old + 1).min(n - 1),
            Button::Up => old.saturating_sub(cols),
            Button::Down => (old + cols).min(n - 1),
            Button::A | Button::Start if !repeat => {
                let id = v.launcher[old].id.clone();
                self.close_menu(Some(Then::Launch(id, old)));
                return;
            }
            _ => return,
        };
        m.lg_sel = new;
        m.lg_scroll.set(lg_scroll_for(new));
        if new != old {
            self.fx(Sfx::Move, Some(Haptic::Tick));
        }
    }

    fn settings_button(&mut self, b: Button, repeat: bool, v: &View) {
        let items = settings(v.keyboard_app);
        let Some(m) = &mut self.menu else { return };
        let n = items.len();
        m.set_sel = m.set_sel.min(n - 1);
        match b {
            Button::Up | Button::Down => {
                let old = m.set_sel;
                m.set_sel = if b == Button::Up { old.saturating_sub(1) } else { (old + 1).min(n - 1) };
                m.set_scroll.set(set_scroll_for(m.set_sel, n));
                if m.set_sel != old {
                    self.confirm = None;
                    self.fx(Sfx::Move, Some(Haptic::Tick));
                }
                return;
            }
            _ => {}
        }
        let it = items[m.set_sel];
        let right = b == Button::Right;
        let horiz = matches!(b, Button::Left | Button::Right);
        let mut changed = true;
        match (it, b) {
            (Setting::Sounds, _) if horiz || b == Button::A => self.prefs.sounds = !self.prefs.sounds,
            (Setting::Haptics, _) if horiz || b == Button::A => self.prefs.haptics = !self.prefs.haptics,
            (Setting::ReduceMotion, _) if horiz || b == Button::A => self.prefs.reduce_motion = !self.prefs.reduce_motion,
            (Setting::Palette, _) if horiz || b == Button::A => self.prefs.night = !self.prefs.night,
            (Setting::Motion, _) if horiz || b == Button::A => {
                let i = Preset::ALL.iter().position(|p| *p == self.prefs.motion).unwrap_or(0) as i32;
                let d = if b == Button::Left { -1 } else { 1 };
                self.prefs.motion = Preset::ALL[(i + d).rem_euclid(3) as usize];
            }
            (Setting::Volume, _) if horiz => {
                changed = false;
                self.out.push(Out::Volume(if right { 5 } else { -5 }));
            }
            (Setting::Brightness, _) if horiz => {
                changed = false;
                self.out.push(Out::Brightness(if right { 10 } else { -10 }));
            }
            (_, Button::A) if !repeat => {
                changed = false;
                match it {
                    Setting::Keyboard => {
                        self.close_menu(None);
                        self.out.push(Out::ShowKeyboard);
                    }
                    Setting::Wifi => self.out.push(Out::OpenSetup { wifi: true }),
                    Setting::Remote => self.out.push(Out::OpenSetup { wifi: false }),
                    Setting::Sleep => self.out.push(Out::Sleep),
                    Setting::Logout if self.confirmed(it, "log out (all apps close)") => self.out.push(Out::Logout),
                    Setting::Reboot if self.confirmed(it, "restart") => self.out.push(Out::Reboot),
                    Setting::PowerOff if self.confirmed(it, "power off") => self.out.push(Out::PowerOff),
                    _ => {}
                }
            }
            _ => changed = false,
        }
        if changed {
            self.out.push(Out::Prefs);
            // the new setting is heard/felt with the new value
            self.fx(Sfx::Tab, Some(Haptic::Tick));
        }
    }

    fn confirmed(&mut self, s: Setting, what: &str) -> bool {
        if self.confirm.is_some_and(|(k, t)| k == s && self.t < t) {
            self.confirm = None;
            return true;
        }
        self.confirm = Some((s, self.t + 4.0));
        self.show_toast(Toast::info(format!("Press A again to {what}")));
        false
    }
}

fn target_uid(t: &Target, v: &View) -> Option<u32> {
    match t {
        Target::Uid(u) => Some(*u),
        Target::Entry(id) => v.by_entry(id).map(|a| a.uid),
    }
}

/// Launch grid scroll: keep the selected row in the two visible rows.
fn lg_scroll_for(sel: usize) -> f32 {
    (sel / 3).saturating_sub(1) as f32 * 150.0
}

/// Settings scroll: five rows show; keep the selection inside them.
fn set_scroll_for(sel: usize, n: usize) -> f32 {
    let first = sel.saturating_sub(3).min(n.saturating_sub(5));
    first as f32 * 76.0
}

// ---------------------------------------------------------------- drawing ----

fn with_a(c: Color, a: f32) -> Color {
    Color(c.0, c.1, c.2, (a.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// Date line for the home screen: "SAT 1 NOV · BRICK".
fn date_line(host: &str) -> String {
    const DAYS: [&str; 7] = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];
    const MONTHS: [&str; 12] = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"];
    // SAFETY: plain libc time calls with a zeroed tm
    let (wd, d, m) = unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        (tm.tm_wday as usize % 7, tm.tm_mday, tm.tm_mon as usize % 12)
    };
    format!("{} {} {} · {}", DAYS[wd], d, MONTHS[m], host.to_ascii_uppercase())
}

/// A string that may contain the arrow glyphs: drawn with the arrows as
/// small triangles (the fonts have no ◀▶▲▼). Returns the width.
fn arrow_text(b: &mut B, x: f32, top: f32, s: &str, st: &TextStyle, draw: bool) -> f32 {
    let aw = st.size * 0.62;
    let mut pen = x;
    let mut buf = String::new();
    let flush = |b: &mut B, pen: &mut f32, buf: &mut String| {
        if !buf.is_empty() {
            let w = if draw { b.text(*pen, top, buf, st) } else { b.measure(buf, st) };
            *pen += w;
            buf.clear();
        }
    };
    for ch in s.chars() {
        if matches!(ch, '◀' | '▶' | '▲' | '▼') {
            flush(b, &mut pen, &mut buf);
            if draw {
                let cy = top + st.line_px() / 2.0;
                let (l, r, t, bt) = (pen + aw * 0.12, pen + aw * 0.88, cy - st.size * 0.3, cy + st.size * 0.3);
                let mid = (l + r) / 2.0;
                let pts: [P; 3] = match ch {
                    '◀' => [(r, t), (r, bt), (l, cy)],
                    '▶' => [(l, t), (l, bt), (r, cy)],
                    '▲' => [(l, bt), (r, bt), (mid, t)],
                    _ => [(l, t), (r, t), (mid, bt)],
                };
                b.poly(&pts, st.color);
            }
            pen += aw;
        } else {
            buf.push(ch);
        }
    }
    flush(b, &mut pen, &mut buf);
    pen - x
}

/// The hint bar: right-aligned (cap, label) pairs.
fn hints(b: &mut B, pal: &Pal, y: f32, items: &[(&str, &str)]) {
    b.rect(0.0, y, W, 64.0, pal.ground);
    b.rect(0.0, y, W, 1.0, pal.ink);
    let cap_st = ts(Face::Mono600, 14.0, pal.ground);
    let lab = ts(Face::Body400, 18.0, pal.soft);
    let mut right = W - 30.0;
    for (cap, label) in items.iter().rev() {
        let lw = b.measure(label, &lab);
        b.text(right - lw, y + 32.0 - lab.line_px() / 2.0, label, &lab);
        right -= lw + 9.0;
        let tw = arrow_text(b, 0.0, 0.0, cap, &cap_st, false);
        let cw = (tw + 18.0).max(30.0);
        b.rrect(right - cw, y + 17.0, cw, 30.0, 15.0, pal.ink);
        arrow_text(b, right - cw + (cw - tw) / 2.0, y + 32.0 - cap_st.line_px() / 2.0, cap, &cap_st, true);
        right -= cw + 26.0;
    }
}

/// A tag: mono label in a filled box with an outline.
fn chip(b: &mut B, right: f32, cy: f32, text: &str, size: f32, pad: (f32, f32), fill: Color, ink: Color) -> f32 {
    let st = ts(Face::Mono400, size, ink);
    let tw = b.measure(text, &st);
    let (w, h) = (tw + pad.0 * 2.0 + 2.0, st.line_px() + pad.1 * 2.0 + 2.0);
    let (x, y) = ((right - w).round(), (cy - h / 2.0).round());
    b.block(x, y, w.round(), h.round(), fill, ink);
    b.text(x + 1.0 + pad.0, y + 1.0 + pad.1, text, &st);
    w
}

fn star_pts(cx: f32, cy: f32, r: f32) -> Vec<P> {
    (0..10)
        .map(|i| {
            let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
            let rr = if i % 2 == 0 { r } else { r * 0.45 };
            (cx + rr * a.cos(), cy + rr * a.sin())
        })
        .collect()
}

impl ShellUi {
    /// Build this frame. `osk` is set when the on-screen keyboard strip
    /// shows under the app.
    pub fn build(&self, typo: &Typo, v: &View, out_w: f32, out_h: f32) -> Scene {
        let pal = self.pal();
        let mut b = B::new(typo, out_w, out_h, Color(0, 0, 0, 255));
        if (out_w, out_h) != (W, H) {
            b.push(Xf::scale(out_w / W, out_h / H));
        }
        // base: what is under any transition
        let base_app = match &self.trans {
            Some(Trans::Launch { .. }) | Some(Trans::Close { .. }) => None,
            Some(Trans::Swap { prev, .. }) => *prev,
            None => v.current,
        };
        match base_app {
            Some(uid) => self.draw_app(&mut b, v, uid, true),
            None => self.draw_home(&mut b, &pal, v),
        }
        self.draw_trans(&mut b, &pal, v);
        if let Some(m) = &self.menu {
            self.draw_menu(&mut b, &pal, v, m);
        }
        if self.page.is_some() {
            b.image(Src::Layer(Layer::Page), 0.0, 0.0, W, H, [0.0, 0.0, 1.0, 1.0]);
        }
        if let Some(o) = &self.osd {
            self.draw_osd(&mut b, &pal, o);
        }
        if let Some(t) = &self.toast {
            self.draw_toast(&mut b, &pal, t);
        }
        if let Some(f) = &self.flash {
            b.rect(0.0, 0.0, W, H, with_a(Color(255, 255, 255, 255), f.x));
        }
        if let Some(bt) = &self.boot {
            self.draw_boot(&mut b, &pal, bt);
        }
        b.finish()
    }

    /// The app in its on-screen place (black around a smaller window), plus
    /// the keyboard strip.
    fn draw_app(&self, b: &mut B, v: &View, uid: u32, osk: bool) {
        b.rect(0.0, 0.0, W, H, Color(0, 0, 0, 255));
        if let Some(a) = v.app(uid) {
            if a.has_buffer {
                let g = a.geom;
                b.image(Src::App(uid), g[0], g[1], g[2], g[3], [0.0, 0.0, 1.0, 1.0]);
            } else {
                self.splash(b, &self.pal(), a.art, &a.name, 0.0);
            }
        }
        if osk && v.osk_h > 0.0 && v.current == Some(uid) {
            b.image(Src::Layer(Layer::Osk), 0.0, H - v.osk_h, W, v.osk_h, [0.0, 0.0, 1.0, 1.0]);
        }
    }

    /// Full-screen accent block shown while an app starts.
    fn splash(&self, b: &mut B, pal: &Pal, art: Art, name: &str, alpha: f32) {
        let accent = art.accent(pal);
        b.push_alpha(1.0 - alpha);
        b.rect(0.0, 0.0, W, H, accent);
        let bob = (self.t * 3.0).sin() as f32 * 5.0;
        art::draw(b, art, W / 2.0, 300.0 + bob, 1.5, *pal, pal.dark);
        let st = ts(Face::SerifItalic, 64.0, pal.dark).line_height(1.0);
        b.text_center(W / 2.0, 470.0, name, &st);
        let dots = ".".repeat(1 + (self.t * 2.0) as usize % 3);
        let st2 = ts(Face::Mono400, 18.0, pal.dark).tracking(0.08);
        b.text_center(W / 2.0, 550.0, &format!("STARTING{dots}"), &st2);
        b.pop_alpha();
    }

    fn draw_home(&self, b: &mut B, pal: &Pal, v: &View) {
        b.rect(0.0, 0.0, W, H, pal.ground);
        // drifting background shapes
        let t = self.drift_t as f32;
        for (i, (w, h, x, y, round)) in SHAPES.iter().enumerate() {
            let ph = i as f32 * 1.7;
            let tx = x + (t * 0.21 + ph).sin() * 40.0;
            let ty = y + (t * 0.17 + ph).cos() * 30.0;
            let rot = ((t * 0.1 + ph).sin() * 12.0).to_radians();
            b.push(Xf::translate(tx, ty).mul(&Xf::about(w / 2.0, h / 2.0, Xf::rotate(rot))));
            if *round {
                b.ellipse(w / 2.0, h / 2.0, w / 2.0, h / 2.0, pal.ground_2);
            } else {
                b.rect(0.0, 0.0, *w, *h, pal.ground_2);
            }
            b.pop();
        }
        // clock, date, hint
        let clock = ts(Face::SerifItalic, 132.0, pal.ink).line_height(0.9).tracking(-0.02);
        b.text(56.0, 80.0, &v.sys.time, &clock);
        let date = ts(Face::Mono400, 18.0, pal.soft).tracking(0.04);
        b.text(62.0, 214.0, &date_line(v.host), &date);
        let hint = ts(Face::Body400, 18.0, pal.soft).line_height(1.4);
        let bold = ts(Face::Body600, 18.0, pal.ink).line_height(1.4);
        let (a, m, c) = ("Press ", "MENU", " anytime");
        let wc = b.measure(c, &hint);
        let wm = b.measure(m, &bold);
        let wa = b.measure(a, &hint);
        let r = W - 56.0;
        b.text(r - wc, 98.0, c, &hint);
        b.text(r - wc - wm, 98.0, m, &bold);
        b.text(r - wc - wm - wa, 98.0, a, &hint);
        b.text_right(r, 98.0 + hint.line_px(), "to switch apps", &hint);

        // tiles (back to front: the focused one last, it lifts over)
        let order: Vec<usize> = (0..v.launcher.len()).filter(|i| *i != self.focus).chain(std::iter::once(self.focus)).filter(|i| *i < v.launcher.len()).collect();
        for i in order {
            self.draw_tile(b, pal, v, i);
        }
        if v.launcher.is_empty() {
            let st = ts(Face::Body400, 24.0, pal.soft);
            b.text(56.0, 420.0, "No apps configured (mortar.shell.apps).", &st);
        }

        // status bar
        b.rect(0.0, 0.0, W, 56.0, pal.ground);
        b.rect(0.0, 55.0, W, 1.0, pal.ink);
        let sb = ts(Face::Mono400, 18.0, pal.soft);
        b.text(30.0, 28.0 - sb.line_px() / 2.0, &v.sys.time, &sb);
        let mut right = W - 30.0;
        let mut net = Vec::new();
        if v.sys.wifi {
            net.push("Wi-Fi".to_string());
        }
        if v.sys.usb_net {
            net.push("USB".to_string());
        }
        if let Some(bat) = v.sys.battery {
            net.push(format!("{bat}%"));
        }
        let net = net.join(" · ");
        if !net.is_empty() {
            right -= b.text_right(right, 28.0 - sb.line_px() / 2.0, &net, &sb) + 10.0;
        }
        if v.sys.ssh_sessions > 0 {
            right -= chip(b, right, 28.0, "SSH", 15.0, (9.0, 2.0), pal.lime, pal.lime_ink) + 10.0;
        }
        if !v.running.is_empty() {
            chip(b, right, 28.0, &format!("{} running", v.running.len()), 15.0, (9.0, 2.0), pal.lavender, pal.lavender_ink);
        }
        hints(b, pal, H - 64.0, &[("◀▶", "Choose"), ("MENU", "Apps"), ("A", "Open")]);
    }

    fn draw_tile(&self, b: &mut B, pal: &Pal, v: &View, i: usize) {
        let l = &v.launcher[i];
        let Some(t) = self.tiles.get(i) else { return };
        let sel = i == self.focus;
        let x = 56.0 + self.row_x.x + i as f32 * 260.0;
        let y = 302.0;
        if x > W + 40.0 || x + 232.0 < -40.0 {
            return;
        }
        let (fg, bg) = if sel { (pal.lime_ink, pal.lime) } else { (pal.ink, pal.tile) };
        b.push(Xf::translate(0.0, t.lift.x).mul(&Xf::about(x + 116.0, y + 159.0, Xf::scale(t.sc.x, t.sc.x))));
        let off = t.back.x * 12.0;
        if off > 0.05 {
            b.block(x + off, y + off, 232.0, 318.0, pal.lavender, pal.ink);
        }
        b.block(x, y, 232.0, 318.0, bg, pal.ink);
        art::draw(b, l.art, x + 116.0, y + 95.0, 1.0, *pal, fg);
        b.rect(x + 1.0, y + 189.0, 230.0, 1.0, fg);
        let nm = ts(Face::SerifItalic, 40.0, fg).line_height(1.0);
        let name = b.fit(&l.name, &nm, 200.0);
        b.text(x + 18.0, y + 206.0, &name, &nm);
        let sub = ts(Face::Body400, 16.0, with_a(fg, 0.8)).line_height(1.3);
        let desc = b.fit(&l.description, &sub, 200.0);
        b.text(x + 18.0, y + 250.0, &desc, &sub);
        if l.running {
            chip(b, x + 232.0 - 12.0, y + 12.0 + 10.0, "RUNNING", 13.0, (7.0, 1.0), pal.lavender, pal.lavender_ink);
        }
        b.pop();
    }

    fn draw_trans(&self, b: &mut B, pal: &Pal, v: &View) {
        match &self.trans {
            Some(Trans::Launch { target, name, art, from, .. }) => {
                let p = self.lp.x;
                let r = R4 { x: lerp(from.x, 0.0, p), y: lerp(from.y, 0.0, p), w: lerp(from.w, W, p), h: lerp(from.h, H, p) };
                b.block(r.x, r.y, r.w, r.h, art.accent(pal), pal.ink);
                let shown = target_uid(target, v).filter(|u| v.current == Some(*u) && v.app(*u).is_some_and(|a| a.has_buffer));
                match shown {
                    Some(uid) => {
                        let a = ((p - 0.55) / 0.35).clamp(0.0, 1.0);
                        if a > 0.0 {
                            b.push_alpha(a);
                            self.app_in_rect(b, v, uid, r);
                            b.pop_alpha();
                        }
                    }
                    None if p > 0.9 => {
                        let a = ((p - 0.9) / 0.1).clamp(0.0, 1.0);
                        b.push_alpha(a);
                        self.splash(b, pal, *art, name, 0.0);
                        b.pop_alpha();
                    }
                    None => {}
                }
            }
            Some(Trans::Close { uid, art, to }) => {
                let p = self.lp.x;
                let r = R4 { x: lerp(to.x, 0.0, p), y: lerp(to.y, 0.0, p), w: lerp(to.w, W, p), h: lerp(to.h, H, p) };
                b.block(r.x, r.y, r.w, r.h, art.accent(pal), pal.ink);
                let a = ((p - 0.6) / 0.3).clamp(0.0, 1.0);
                if a > 0.0 {
                    b.push_alpha(a);
                    self.app_in_rect(b, v, *uid, r);
                    b.pop_alpha();
                }
            }
            Some(Trans::Swap { target, name, art, .. }) => {
                let dx = (1.0 - self.swap_p.x) * W;
                b.push(Xf::translate(dx, 0.0));
                let uid = target_uid(target, v).filter(|u| v.app(*u).is_some_and(|a| a.has_buffer) && v.current == Some(*u));
                match uid {
                    Some(u) => self.draw_app(b, v, u, false),
                    None => self.splash(b, pal, *art, name, 0.0),
                }
                b.rect(-1.0, 0.0, 1.0, H, pal.ink);
                b.pop();
            }
            None => {}
        }
    }

    /// The app's screen squeezed into `r` (launch/close).
    fn app_in_rect(&self, b: &mut B, v: &View, uid: u32, r: R4) {
        let Some(a) = v.app(uid) else { return };
        b.push(Xf::translate(r.x, r.y).mul(&Xf::scale(r.w / W, r.h / H)));
        b.rect(0.0, 0.0, W, H, Color(0, 0, 0, 255));
        let g = a.geom;
        b.image(Src::App(uid), g[0], g[1], g[2], g[3], [0.0, 0.0, 1.0, 1.0]);
        b.pop();
    }

    fn draw_menu(&self, b: &mut B, pal: &Pal, v: &View, m: &Menu) {
        b.rect(0.0, 0.0, W, H, Color(29, 29, 29, (0.42 * m.sa.x.clamp(0.0, 1.0) * 255.0) as u8));
        b.push(Xf::translate(0.0, m.dy.x));
        b.rect(0.0, 0.0, W, 620.0, pal.ground);
        b.rect(0.0, 619.0, W, 1.0, pal.ink);
        match m.tab {
            Tab::Apps => self.draw_fan(b, pal, v, m),
            Tab::Launch => {
                b.clip(Some([0.0, 119.0, W, 437.0]));
                self.draw_grid(b, pal, v, m);
                b.clip(None);
            }
            Tab::Settings => {
                b.clip(Some([0.0, 119.0, W, 437.0]));
                self.draw_settings(b, pal, v, m);
                b.clip(None);
            }
        }
        // header
        b.rect(0.0, 0.0, W, 118.0, pal.ground);
        b.rect(0.0, 118.0, W, 1.0, pal.ink);
        let sub = ts(Face::Mono400, 15.0, pal.soft);
        b.text(40.0, 26.0, &format!("MENU · {} running", v.running.len()), &sub);
        let title = ts(Face::SerifItalic, 54.0, pal.ink).line_height(1.0);
        b.text(40.0, 50.0, m.tab.name(), &title);
        let tab_st = ts(Face::Mono400, 17.0, pal.soft);
        let mut right = W - 40.0;
        for t in TABS.iter().rev() {
            let on = *t == m.tab;
            let st = if on { ts(Face::Mono600, 17.0, pal.lime_ink) } else { tab_st };
            let tw = b.measure(t.name(), &st);
            let (w, h) = (tw + 26.0, st.line_px() + 10.0);
            let (x, y) = ((right - w).round(), (104.0 - h).round());
            if on {
                b.block(x, y, w.round(), h.round(), pal.lime, pal.lime_ink);
            } else {
                b.frame(x, y, w.round(), h.round(), 1.0, pal.ink);
            }
            b.text(x + 13.0, y + 5.0, t.name(), &st);
            right -= w + 10.0;
        }
        let hint_items: Vec<(&str, &str)> = match m.tab {
            Tab::Apps => vec![("L1/R1", "Tabs"), ("◀▶", "Choose"), ("X", "Close app"), ("B", "Back"), ("A", "Switch")],
            Tab::Launch => vec![("L1/R1", "Tabs"), ("◀▶▲▼", "Choose"), ("B", "Back"), ("A", "Open")],
            Tab::Settings => {
                let items = settings(v.keyboard_app);
                let it = items.get(m.set_sel.min(items.len() - 1)).copied();
                let mut h = vec![("L1/R1", "Tabs"), ("▲▼", "Choose")];
                match it {
                    Some(Setting::Keyboard | Setting::Wifi | Setting::Remote | Setting::Sleep | Setting::Logout | Setting::Reboot | Setting::PowerOff) => h.push(("A", "Choose")),
                    _ => h.push(("◀▶", "Change")),
                }
                h.push(("B", "Back"));
                h
            }
        };
        hints(b, pal, 556.0, &hint_items);
        b.pop();
    }

    fn draw_fan(&self, b: &mut B, pal: &Pal, v: &View, m: &Menu) {
        let mut order: Vec<usize> = (0..m.cards.len()).collect();
        order.sort_by(|a, c| {
            let da = (*a as f32 - m.fan_pos.x).abs();
            let dc = (*c as f32 - m.fan_pos.x).abs();
            dc.partial_cmp(&da).unwrap_or(std::cmp::Ordering::Equal)
        });
        for i in order {
            let c = &m.cards[i];
            let dd = i as f32 - m.fan_pos.x;
            let ad = dd.abs();
            if ad > 3.2 {
                continue;
            }
            let (tx, rot, sc, ty) = (dd * 250.0, (dd * 6.0).to_radians(), (1.0 - ad * 0.16).max(0.55), ad * 26.0 + c.y.x);
            let (x0, y0) = (312.0, 142.0);
            let center = (512.0, y0 + 174.0);
            b.push(Xf::translate(tx, ty).mul(&Xf::about(center.0, center.1, Xf::rotate(rot).mul(&Xf::scale(sc, sc)))));
            let alpha = if c.gone.is_some() { (1.0 - c.y.x / 400.0).max(0.0) } else { 1.0 };
            b.push_alpha(alpha);
            let (name, sub) = match c.key {
                CardKey::Home => {
                    b.rect(x0, y0, 400.0, 300.0, pal.lime);
                    let st = ts(Face::SerifItalic, 96.0, pal.lime_ink);
                    b.text_base(x0 + 34.0, y0 + 150.0, &v.sys.time, &st);
                    for k in 0..4 {
                        let (rx, ry) = (x0 + 34.0 + k as f32 * 86.0, y0 + 190.0);
                        b.stroke(&[(rx, ry), (rx + 72.0, ry), (rx + 72.0, ry + 84.0), (rx, ry + 84.0)], true, 1.5, pal.lime_ink);
                    }
                    ("Home".to_string(), "Tiles".to_string())
                }
                CardKey::App(uid) => match v.app(uid) {
                    Some(a) => {
                        b.rect(x0, y0, 400.0, 300.0, Color(0, 0, 0, 255));
                        if a.has_buffer {
                            let s = 400.0 / W;
                            let g = a.geom;
                            b.image(Src::App(uid), x0 + g[0] * s, y0 + g[1] * s, g[2] * s, g[3] * s, [0.0, 0.0, 1.0, 1.0]);
                        } else {
                            b.rect(x0, y0, 400.0, 300.0, a.art.accent(pal));
                            art::draw(b, a.art, x0 + 200.0, y0 + 150.0, 1.0, *pal, pal.dark);
                        }
                        let sub = if a.closing {
                            "Closing…"
                        } else if !a.has_buffer {
                            "Starting…"
                        } else if v.current == Some(uid) {
                            "On screen"
                        } else {
                            "Running"
                        };
                        (a.name.clone(), sub.to_string())
                    }
                    None => (String::new(), String::new()),
                },
            };
            b.frame(x0, y0, 400.0, 300.0, 1.0, pal.ink);
            if i == m.fan_sel && c.gone.is_none() {
                b.frame(x0 - 4.0, y0 - 4.0, 408.0, 308.0, 4.0, pal.lime);
            }
            let nm = ts(Face::SerifItalic, 32.0, pal.ink);
            let base = y0 + 310.0 + b.typo.base_in_line(Face::SerifItalic, 32.0, 1.2);
            let subst = ts(Face::Mono400, 14.0, pal.soft);
            let sw = b.measure(&sub, &subst);
            let name = b.fit(&name, &nm, 390.0 - sw - 12.0);
            b.text_base(x0 + 2.0, base, &name, &nm);
            b.text_base(x0 + 398.0 - sw, base, &sub, &subst);
            b.pop_alpha();
            b.pop();
        }
    }

    fn draw_grid(&self, b: &mut B, pal: &Pal, v: &View, m: &Menu) {
        let cw = (W - 80.0 - 36.0) / 3.0;
        for (i, l) in v.launcher.iter().enumerate() {
            let (col, row) = (i % 3, i / 3);
            let x = 40.0 + col as f32 * (cw + 18.0);
            let y = 140.0 + row as f32 * 150.0 - m.lg_scroll.x;
            if y > 620.0 || y + 132.0 < 100.0 {
                continue;
            }
            let on = i == m.lg_sel;
            let (bg, fg) = if on { (pal.lime, pal.lime_ink) } else { (pal.tile, pal.ink) };
            b.block(x, y, cw, 132.0, bg, pal.ink);
            art::draw(b, l.art, x + 18.0 + 50.0, y + 66.0, 0.5, *pal, fg);
            let st = ts(Face::SerifItalic, 34.0, fg).line_height(1.0);
            let name = b.fit(&l.name, &st, cw - 150.0);
            b.text(x + 18.0 + 100.0 + 16.0, y + 66.0 - 17.0, &name, &st);
            if l.running {
                chip(b, x + cw - 10.0, y + 10.0 + 9.0, "ON", 12.0, (6.0, 1.0), pal.lavender, pal.lavender_ink);
            }
        }
    }

    fn draw_settings(&self, b: &mut B, pal: &Pal, v: &View, m: &Menu) {
        let items = settings(v.keyboard_app);
        let sel = m.set_sel.min(items.len() - 1);
        let onoff = |on: bool| (vec!["On", "Off"], if on { 0 } else { 1 });
        for (i, it) in items.iter().enumerate() {
            let y = 136.0 + i as f32 * 76.0 - m.set_scroll.x;
            if y > 620.0 || y + 76.0 < 100.0 {
                continue;
            }
            let on = i == sel;
            let fg = if on { pal.lime_ink } else { pal.ink };
            if on {
                b.rect(40.0, y, W - 80.0, 76.0, pal.lime);
            } else {
                b.rect(40.0, y + 75.0, W - 80.0, 1.0, pal.faint);
            }
            let (name, val): (&str, Result<(Vec<&str>, usize), String>) = match it {
                Setting::Sounds => ("UI sounds", Ok(onoff(self.prefs.sounds))),
                Setting::Haptics => ("Haptic ticks", Ok(onoff(self.prefs.haptics))),
                Setting::Motion => ("Motion", Ok((Preset::ALL.iter().map(|p| p.name()).collect(), Preset::ALL.iter().position(|p| *p == self.prefs.motion).unwrap_or(0)))),
                Setting::ReduceMotion => ("Reduce motion", Ok((vec!["Off", "On"], self.prefs.reduce_motion as usize))),
                Setting::Palette => ("Palette", Ok((vec!["Day", "Night"], self.prefs.night as usize))),
                Setting::Volume => ("Volume", Err(v.volume.map(|x| format!("{x}%")).unwrap_or_else(|| "n/a".into()))),
                Setting::Brightness => ("Brightness", Err(v.sys.brightness.map(|x| format!("{x}%")).unwrap_or_else(|| "n/a".into()))),
                Setting::Keyboard => ("On-screen keyboard", Err("START in the app ›".into())),
                Setting::Wifi => ("Wi-Fi", Err(if v.sys.wifi { "Connected ›".into() } else { "Not connected ›".into() })),
                Setting::Remote => (
                    "Remote access",
                    Err(if v.sys.sshd_listening {
                        match v.sys.ssh_sessions {
                            0 => "On ›".into(),
                            1 => "On · 1 session ›".into(),
                            n => format!("On · {n} sessions ›"),
                        }
                    } else {
                        "Off ›".into()
                    }),
                ),
                Setting::Sleep => ("Sleep", Err("Power key".into())),
                Setting::Logout => ("Log out", Err("Closes all apps".into())),
                Setting::Reboot => ("Restart", Err("›".into())),
                Setting::PowerOff => ("Power off", Err("›".into())),
            };
            let lab = ts(Face::Body400, 24.0, fg);
            b.text(60.0, y + 38.0 - lab.line_px() / 2.0, name, &lab);
            // value: options with the chosen one bold, others faded
            let st_on = ts(Face::Mono600, 18.0, fg);
            let st_off = ts(Face::Mono400, 18.0, with_a(fg, 0.55));
            let st_txt = ts(Face::Mono400, 18.0, fg);
            let mut segs: Vec<(String, TextStyle)> = Vec::new();
            let adjustable = matches!(it, Setting::Volume | Setting::Brightness) || val.is_ok();
            if on && adjustable {
                segs.push(("◀ ".into(), st_txt));
            }
            match &val {
                Ok((opts, chosen)) => {
                    for (j, o) in opts.iter().enumerate() {
                        if j > 0 {
                            segs.push((" · ".into(), st_off));
                        }
                        segs.push((o.to_string(), if j == *chosen { st_on } else { st_off }));
                    }
                }
                Err(s) => segs.push((s.clone(), st_txt)),
            }
            if on && adjustable {
                segs.push((" ▶".into(), st_txt));
            }
            let total: f32 = segs.iter().map(|(s, st)| arrow_text(b, 0.0, 0.0, s, st, false)).sum();
            let mut x = W - 60.0 - total;
            let top = y + 38.0 - st_txt.line_px() / 2.0;
            for (s, st) in &segs {
                x += arrow_text(b, x, top, s, st, true);
            }
        }
    }

    fn draw_osd(&self, b: &mut B, pal: &Pal, o: &Osd) {
        let x = W - 26.0 - 64.0 + o.x.x;
        let y = 76.0;
        b.block(x, y, 64.0, 260.0, pal.ground, pal.ink);
        let lab = ts(Face::Mono400, 12.0, pal.soft);
        b.text_center(x + 32.0, y + 8.0, o.label, &lab);
        let n = ts(Face::Mono400, 15.0, pal.ink);
        let ny = y + 260.0 - 9.0 - n.line_px();
        b.text_center(x + 32.0, ny, &o.value.to_string(), &n);
        let h = (o.value as f32 * 2.0).min(200.0);
        if h > 0.0 {
            b.block(x + 9.0, ny - 8.0 - h, 46.0, h, pal.lime, pal.lime_ink);
        }
    }

    fn draw_toast(&self, b: &mut B, pal: &Pal, t: &ToastState) {
        let (x, y) = (232.0, 22.0 + t.y.x);
        let dark = pal.dark;
        b.block(x, y, 560.0, 104.0, pal.yellow, pal.ink);
        let (ix, iy) = (x + 19.0, y + 15.0);
        b.block(ix, iy, 74.0, 74.0, pal.lime, dark);
        let (cx, cy) = (ix + 37.0, iy + 37.0);
        match t.toast.icon {
            ToastIcon::Star => b.poly_concave(&star_pts(cx, cy + 2.0, 24.0), dark),
            ToastIcon::Shot => {
                b.frame(cx - 20.0, cy - 16.0, 40.0, 32.0, 3.0, dark);
                b.rect(cx - 9.0, cy - 6.0, 18.0, 12.0, dark);
            }
            ToastIcon::Info => {
                let st = ts(Face::SerifItalic, 48.0, dark).line_height(1.0);
                b.text_center(cx, cy - 26.0, "i", &st);
            }
        }
        let tx = ix + 74.0 + 16.0;
        let title = ts(Face::SerifItalic, 34.0, dark).line_height(1.0);
        let sub = ts(Face::Mono400, 15.0, dark);
        let max = x + 560.0 - 18.0 - tx;
        let tt = b.fit(&t.toast.title, &title, max);
        if t.toast.sub.is_empty() {
            b.text(tx, cy - 17.0, &tt, &title);
        } else {
            let top = cy - (34.0 + sub.line_px() + 4.0) / 2.0;
            b.text(tx, top, &tt, &title);
            let ss = b.fit(&t.toast.sub, &sub, max);
            b.text(tx, top + 38.0, &ss, &sub);
        }
    }

    fn draw_boot(&self, b: &mut B, pal: &Pal, bt: &Boot) {
        b.push(Xf::translate(0.0, bt.y.x));
        b.rect(0.0, 0.0, W, H, pal.ground);
        b.rect(0.0, H, W, 40.0, pal.ground);
        let cols = [pal.lime, pal.lavender, pal.yellow, pal.orange];
        for (i, (x, y, r)) in bt.blocks.iter().enumerate() {
            b.push(Xf::translate(x.x, y.x).mul(&Xf::about(75.0, 75.0, Xf::rotate(r.x.to_radians()))));
            b.block(0.0, 0.0, 150.0, 150.0, cols[i], pal.ink);
            b.pop();
        }
        let w = bt.word.x;
        let st = ts(Face::SerifItalic, 150.0, pal.ink).line_height(1.0);
        let ww = b.measure("Mortar", &st);
        let s = 0.85 + 0.15 * w;
        b.push(Xf::translate(0.0, (1.0 - w) * 40.0).mul(&Xf::about(W / 2.0, 375.0, Xf::scale(s, s))));
        b.push_alpha(w);
        b.text(W / 2.0 - ww / 2.0, 300.0, "Mortar", &st);
        b.pop_alpha();
        b.pop();
        let tag = ts(Face::Mono400, 18.0, pal.soft).tracking(0.08);
        b.text_center(W / 2.0, 470.0, "NIXOS · TRIMUI BRICK", &tag);
        b.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(uid: u32, entry: &str) -> RunningInfo {
        RunningInfo { uid, entry: Some(entry.into()), name: entry.into(), art: Art::Generic, has_buffer: true, closing: false, geom: [0.0, 0.0, W, H] }
    }
    fn launcher() -> Vec<LauncherInfo> {
        ["term", "m8", "pico"].iter().map(|i| LauncherInfo { id: i.to_string(), name: i.to_string(), description: String::new(), art: Art::Generic, running: false }).collect()
    }
    fn view<'a>(running: &'a [RunningInfo], launcher: &'a [LauncherInfo], sys: &'a Sys, current: Option<u32>) -> View<'a> {
        View { running, launcher, sys, volume: Some(50), current, keyboard_app: false, osk_h: 0.0, host: "brick" }
    }
    fn settle(ui: &mut ShellUi, v: &View) -> Vec<Out> {
        let mut out = Vec::new();
        for _ in 0..240 {
            out.extend(ui.step(1.0 / 60.0, v));
        }
        out
    }
    fn actions(o: &[Out]) -> Vec<Out> {
        o.iter().filter(|o| !matches!(o, Out::Sfx(_) | Out::Haptic(_))).cloned().collect()
    }

    #[test]
    fn boot_plays_and_ends() {
        let sys = Sys::demo(None);
        let l = launcher();
        let v = view(&[], &l, &sys, None);
        let mut ui = ShellUi::new(Prefs::default(), true);
        assert!(ui.booting());
        let out = settle(&mut ui, &v);
        assert!(!ui.booting());
        assert_eq!(out.iter().filter(|o| **o == Out::Sfx(Sfx::Tick)).count(), 4);
        assert!(out.contains(&Out::Sfx(Sfx::Chime)));
    }

    #[test]
    fn home_launches_focused_tile() {
        let sys = Sys::demo(None);
        let l = launcher();
        let v = view(&[], &l, &sys, None);
        let mut ui = ShellUi::new(Prefs::default(), false);
        settle(&mut ui, &v);
        ui.button(Button::Right, false, &v);
        let out = ui.button(Button::A, false, &v);
        assert_eq!(actions(&out), vec![Out::Launch("m8".into())]);
        assert!(out.contains(&Out::Haptic(Haptic::Double)));
        assert!(ui.captures_input(false));
        // the app maps and is shown: the transition ends
        let r = [run(5, "m8")];
        let v2 = view(&r, &l, &sys, Some(5));
        settle(&mut ui, &v2);
        assert!(!ui.captures_input(true));
        assert!(!ui.layer_visible(true));
    }

    #[test]
    fn menu_a_switches_to_previous_app() {
        let sys = Sys::demo(None);
        let l = launcher();
        let r = [run(2, "term"), run(1, "m8")];
        let v = view(&r, &l, &sys, Some(2));
        let mut ui = ShellUi::new(Prefs::default(), false);
        ui.menu_key(&v);
        settle(&mut ui, &v);
        ui.button(Button::A, false, &v);
        let out = settle(&mut ui, &v);
        assert_eq!(actions(&out), vec![Out::Focus(1)]);
    }

    #[test]
    fn x_flicks_a_card_away() {
        let sys = Sys::demo(None);
        let l = launcher();
        let r = [run(2, "term"), run(1, "m8")];
        let v = view(&r, &l, &sys, Some(2));
        let mut ui = ShellUi::new(Prefs::default(), false);
        ui.menu_key(&v);
        settle(&mut ui, &v);
        assert_eq!(actions(&ui.button(Button::X, false, &v)), vec![Out::Close(1)]);
        // the app goes away; the fan closes the gap
        let r2 = [run(2, "term")];
        let v2 = view(&r2, &l, &sys, Some(2));
        settle(&mut ui, &v2);
        assert_eq!(ui.menu.as_ref().unwrap().cards.len(), 2);
        // the home card cannot be closed
        ui.button(Button::Left, false, &v2);
        ui.button(Button::Left, false, &v2);
        assert!(actions(&ui.button(Button::X, false, &v2)).is_empty());
    }

    #[test]
    fn home_card_goes_home_with_close_transition() {
        let sys = Sys::demo(None);
        let l = launcher();
        let r = [run(2, "term")];
        let v = view(&r, &l, &sys, Some(2));
        let mut ui = ShellUi::new(Prefs::default(), false);
        ui.menu_key(&v);
        settle(&mut ui, &v);
        ui.button(Button::Left, false, &v);
        ui.button(Button::A, false, &v);
        let mut out = Vec::new();
        for _ in 0..60 {
            out.extend(ui.step(1.0 / 60.0, &v));
            if out.contains(&Out::Home) {
                break;
            }
        }
        assert_eq!(actions(&out), vec![Out::Home]);
        assert!(matches!(ui.trans, Some(Trans::Close { uid: 2, .. })));
    }

    #[test]
    fn settings_change_prefs() {
        let sys = Sys::demo(None);
        let l = launcher();
        let v = view(&[], &l, &sys, None);
        let mut ui = ShellUi::new(Prefs::default(), false);
        ui.menu_key(&v);
        ui.button(Button::L1, false, &v); // Apps -> Settings
        assert_eq!(ui.tab(), Some(Tab::Settings));
        let out = ui.button(Button::Right, false, &v);
        assert!(out.contains(&Out::Prefs));
        assert!(!ui.prefs.sounds);
        ui.button(Button::Down, false, &v);
        ui.button(Button::Down, false, &v);
        ui.button(Button::Right, false, &v);
        assert_eq!(ui.prefs.motion, Preset::Calm);
        // power off needs a second press
        for _ in 0..20 {
            ui.button(Button::Down, false, &v);
        }
        assert!(actions(&ui.button(Button::A, false, &v)).is_empty());
        assert_eq!(actions(&ui.button(Button::A, false, &v)), vec![Out::PowerOff]);
    }

    #[test]
    fn drawer_at_rest_is_not_full_pace() {
        let sys = Sys::demo(None);
        let l = launcher();
        let v = view(&[], &l, &sys, None);
        let mut ui = ShellUi::new(Prefs::default(), false);
        ui.menu_key(&v);
        ui.button(Button::R1, false, &v);
        settle(&mut ui, &v);
        assert_ne!(ui.pace(false), Pace::Full);
        for _ in 0..(40 * 60) {
            ui.step(1.0 / 60.0, &v);
        }
        assert_eq!(ui.pace(false), Pace::Idle);
    }

    #[test]
    fn pace_goes_idle() {
        let sys = Sys::demo(None);
        let l = launcher();
        let r = [run(2, "term")];
        let v = view(&r, &l, &sys, Some(2));
        let mut ui = ShellUi::new(Prefs::default(), false);
        settle(&mut ui, &v);
        assert_eq!(ui.pace(true), Pace::Idle);
        ui.osd("VOL", 50);
        assert_eq!(ui.pace(true), Pace::Full);
        for _ in 0..200 {
            ui.step(1.0 / 60.0, &v);
        }
        assert_eq!(ui.pace(true), Pace::Idle);
        assert!(!ui.layer_visible(true));
    }
}
