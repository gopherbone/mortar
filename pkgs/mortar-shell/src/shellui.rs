// SPDX-License-Identifier: GPL-2.0-or-later
//! The shell's own UI, after the approved motion prototype
//! (plan/mortar-motion) and cross menu (plan/mortar-cross): boot, home, the
//! MENU cross (cross.rs: Running, Apps, Games, Music, Quick, Power), the
//! settings screen, launch/close/switch transitions, the volume OSD, toasts
//! and the screenshot flash.
//!
//! Pure state: the compositor feeds it buttons, the passing of time and a
//! `View` of the apps; it answers with `Out`s (actions, sounds, haptic
//! ticks) and builds a `Scene` for a renderer. Everything is laid out in
//! the prototype's 1024x768 logical pixels.
//!
//! Layers, bottom to top: home or the visible app (+ on-screen keyboard),
//! the MENU and its veil, the launch/close/switch transition, the settings
//! screen, the time zone picker, a setup page, the power menu, the OSD, a
//! toast, the shutter flash, the boot screen, the power-key hold
//! indicator, and the black of sleep.

#[path = "cross.rs"]
mod cross;
pub use cross::Cat;

use crate::art::{self, Art, Pal, DAY, NIGHT};
use crate::config::Category;
use crate::radio::Radio;
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

/// User settings changed from the settings screen (persisted by the compositor;
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
    /// Sleep suspends to RAM once the screen is dark (else screen off only).
    pub suspend: bool,
    /// Sleep after this many minutes without input (0: never).
    pub idle_minutes: u32,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { sounds: true, sound_volume: 60, haptics: true, motion: Preset::Bouncy, reduce_motion: false, night: false, suspend: false, idle_minutes: 0 }
    }
}

/// Settings > Sleep when idle: the choices, in minutes (0: never).
pub const IDLE_CHOICES: [u32; 6] = [0, 1, 2, 5, 10, 30];
const IDLE_LABELS: [&str; 6] = ["Off", "1m", "2m", "5m", "10m", "30m"];

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
    /// Where it shows in the MENU besides Apps.
    pub category: Category,
    /// Opening it while it runs switches to it.
    pub single_instance: bool,
}

/// The rfkill switches (None: the device has none of that kind).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Radios {
    pub wifi: Option<bool>,
    pub bluetooth: Option<bool>,
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
    /// The current time zone ("Europe/Lisbon", "UTC").
    pub timezone: &'a str,
    pub radios: Radios,
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
    /// Close the app, then start its entry again.
    Restart(u32),
    /// Show the home screen (the front app keeps running behind).
    Home,
    Volume(i32),
    Brightness(i32),
    /// Switch a radio on or off.
    Radio(Radio, bool),
    ShowKeyboard,
    OpenSetup { wifi: bool },
    /// Open the time zone picker (the compositor loads the zones).
    OpenTimeZone,
    SetTimeZone(String),
    Sleep,
    Logout,
    Reboot,
    PowerOff,
    Sfx(Sfx),
    Haptic(Haptic),
    /// Settings changed (save and apply).
    Prefs,
}

/// Rows of the settings screen (MENU > Quick > Settings…). Volume,
/// brightness, sleep and power live in the MENU itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Setting {
    Sounds,
    Haptics,
    Motion,
    ReduceMotion,
    Palette,
    TimeZone,
    Wifi,
    Remote,
    SleepMode,
    IdleSleep,
    Logout,
}

const SETTINGS: [Setting; 11] = {
    use Setting::*;
    [Sounds, Haptics, Motion, ReduceMotion, Palette, TimeZone, Wifi, Remote, SleepMode, IdleSleep, Logout]
};

/// The settings screen: a list that slides in from the right.
struct SettingsPanel {
    sel: usize,
    scroll: Spring,
    x: Spring,
    closing: bool,
}

// ---------------------------------------------------------------- state ----

/// A zone and its current UTC offset (seconds).
#[derive(Clone, Debug, PartialEq)]
pub struct TzZone {
    pub id: String,
    pub offset: i32,
}

#[derive(Clone, Debug, PartialEq)]
enum TzRow {
    /// A region and how many zones it has ("UTC": none, it is a zone).
    Region(String, usize),
    Zone(usize),
}

/// Settings > Time zone: regions, then the zones of one.
struct TzPicker {
    zones: Vec<TzZone>,
    current: String,
    region: Option<String>,
    sel: usize,
    scroll: Spring,
    /// Slide in/out from the right.
    x: Spring,
    /// Fade/slide of the list when the level changes.
    swap: Spring,
    closing: bool,
}

const TZ_ROWS: usize = 7;

impl TzPicker {
    fn rows(&self) -> Vec<TzRow> {
        match &self.region {
            None => {
                let mut regions: Vec<(String, usize)> = Vec::new();
                for z in &self.zones {
                    if z.id == "UTC" {
                        continue;
                    }
                    let r = crate::tz::region(&z.id);
                    match regions.iter_mut().find(|(n, _)| n == r) {
                        Some(e) => e.1 += 1,
                        None => regions.push((r.to_string(), 1)),
                    }
                }
                regions.sort();
                let mut v: Vec<TzRow> = regions.into_iter().map(|(n, c)| TzRow::Region(n, c)).collect();
                v.push(TzRow::Region("UTC".into(), 0));
                v
            }
            Some(r) => {
                let mut idx: Vec<usize> = (0..self.zones.len()).filter(|i| crate::tz::region(&self.zones[*i].id) == r && self.zones[*i].id != "UTC").collect();
                idx.sort_by_key(|i| crate::tz::pretty(&self.zones[*i].id).0);
                idx.into_iter().map(TzRow::Zone).collect()
            }
        }
    }
    fn scroll_for(sel: usize, n: usize) -> f32 {
        sel.saturating_sub(3).min(n.saturating_sub(TZ_ROWS)) as f32 * 76.0
    }
    fn select(&mut self, sel: usize, n: usize, snap: bool) {
        self.sel = sel.min(n.saturating_sub(1));
        let t = Self::scroll_for(self.sel, n);
        if snap { self.scroll.snap(t) } else { self.scroll.set(t) }
    }
}

/// Sleep: the screen fades to black, then the compositor switches the
/// backlight off and stops drawing; any button wakes it.
struct SleepSt {
    a: Spring,
    waking: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SleepPhase {
    Awake,
    Fading,
    /// Fully black: backlight off, nothing drawn.
    Dark,
    Waking,
}

/// The power menu (hold the power key): Sleep, Restart, Power off.
struct PowerMenu {
    sel: usize,
    x: Spring,
    /// A row waiting for its confirming second A.
    confirm: Option<usize>,
    closing: bool,
}

const POWER_ROWS: [(&str, &str); 3] = [("Sleep", "Screen off · any button wakes"), ("Restart", "Apps close"), ("Power off", "Apps close")];

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
    /// An app grows out of its home tile or MENU row, over `prev` (the app
    /// that was in front, or home).
    Launch { target: Target, name: String, art: Art, from: R4, prev: Option<u32>, t0: f64 },
    /// The front app shrinks back into its tile.
    Close { uid: u32, art: Art, to: R4 },
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
    trans: Option<Trans>,
    menu: Option<cross::Cross>,
    /// Each MENU category's selected row (kept between openings).
    cross_sel: [usize; 6],
    settings: Option<SettingsPanel>,
    page: Option<SetupUi>,
    tz: Option<TzPicker>,
    sleep: Option<SleepSt>,
    power: Option<PowerMenu>,
    /// Power key held: progress to the power menu (0..1).
    power_hold: Option<f32>,
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
            trans: None,
            menu: None,
            cross_sel: [0; 6],
            settings: None,
            page: None,
            tz: None,
            sleep: None,
            power: None,
            power_hold: None,
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
        !has_app
            || self.boot.is_some()
            || self.menu.is_some()
            || self.settings.is_some()
            || self.page.is_some()
            || self.tz.is_some()
            || self.trans.is_some()
            || self.sleep.is_some()
            || self.power.is_some()
    }

    /// The front app is (mostly) hidden behind the shell's screens.
    pub fn covers_app(&self, has_app: bool) -> bool {
        self.captures_input(has_app)
    }

    /// Anything of the shell's is on screen over the app (or instead of it).
    pub fn layer_visible(&self, has_app: bool) -> bool {
        self.covers_app(has_app) || self.osd.is_some() || self.toast.is_some() || self.flash.is_some() || self.power_hold.is_some()
    }

    pub fn menu_open(&self) -> bool {
        self.menu.as_ref().is_some_and(|m| !m.closing)
    }

    /// The MENU's category (while it shows).
    pub fn menu_category(&self) -> Option<Cat> {
        self.menu.as_ref().filter(|m| !m.closing).map(|m| m.cat)
    }

    pub fn settings_open(&self) -> bool {
        self.settings.as_ref().is_some_and(|s| !s.closing)
    }

    pub fn page_mut(&mut self) -> Option<&mut SetupUi> {
        self.page.as_mut()
    }

    pub fn page_open(&self) -> bool {
        self.page.is_some()
    }

    /// Fade to black (sleep). The compositor turns the backlight off once
    /// the screen is dark.
    pub fn sleep(&mut self) {
        if self.sleep.as_ref().is_some_and(|s| !s.waking) {
            return;
        }
        self.fx(Sfx::DrawerUp, Some(Haptic::Tick));
        self.power = None;
        let mut a = Spring::new(self.sleep.as_ref().map(|s| s.a.x).unwrap_or(0.0));
        a.k_mul = 0.3; // a slow fade: sleep, not a cut
        a.set(1.0);
        self.sleep = Some(SleepSt { a, waking: false });
        self.dirty = true;
    }

    /// Wake: the screen comes back (fading in from black).
    pub fn wake(&mut self) {
        if let Some(s) = &mut self.sleep
            && !s.waking
        {
            s.waking = true;
            s.a.k_mul = 1.6;
            s.a.set(0.0);
            self.drift_until = self.t + DRIFT_SECS;
            self.fx(Sfx::Drawer, Some(Haptic::Tick));
            self.dirty = true;
        }
    }

    pub fn sleep_phase(&self) -> SleepPhase {
        match &self.sleep {
            None => SleepPhase::Awake,
            Some(s) if s.waking => SleepPhase::Waking,
            Some(s) if s.a.x > 0.995 => SleepPhase::Dark,
            Some(_) => SleepPhase::Fading,
        }
    }

    pub fn open_power_menu(&mut self) {
        let mut x = Spring::new(0.0);
        x.set(1.0);
        self.power = Some(PowerMenu { sel: 0, x, confirm: None, closing: false });
        self.power_hold = None;
        self.fx(Sfx::Drawer, Some(Haptic::Long));
        self.dirty = true;
    }

    pub fn power_menu_open(&self) -> bool {
        self.power.as_ref().is_some_and(|p| !p.closing)
    }

    pub fn close_power_menu(&mut self) {
        if let Some(p) = &mut self.power
            && !p.closing
        {
            p.closing = true;
            p.x.set(0.0);
            self.fx(Sfx::DrawerUp, None);
        }
    }

    /// The power key is held: show progress towards the power menu.
    pub fn set_power_hold(&mut self, p: Option<f32>) {
        if p != self.power_hold {
            self.power_hold = p;
            self.dirty = true;
        }
    }

    fn power_button(&mut self, b: Button, repeat: bool) {
        let Some(p) = &mut self.power else { return };
        if p.closing {
            return;
        }
        match b {
            Button::Up | Button::Down => {
                let old = p.sel;
                p.sel = if b == Button::Up { old.saturating_sub(1) } else { (old + 1).min(POWER_ROWS.len() - 1) };
                p.confirm = None;
                if p.sel != old {
                    self.fx(Sfx::Move, Some(Haptic::Tick));
                }
            }
            Button::A if !repeat => match p.sel {
                0 => self.sleep(),
                i if p.confirm == Some(i) => {
                    self.out.push(if i == 1 { Out::Reboot } else { Out::PowerOff });
                    self.fx(Sfx::Close, Some(Haptic::Long));
                }
                i => {
                    p.confirm = Some(i);
                    self.fx(Sfx::Deny, Some(Haptic::Double));
                }
            },
            Button::B if !repeat => {
                if p.confirm.take().is_none() {
                    self.close_power_menu();
                }
            }
            _ => {}
        }
    }

    /// Show the time zone picker with the system's zones.
    pub fn open_tz(&mut self, zones: Vec<TzZone>, current: String) {
        let mut x = Spring::new(W);
        x.set(0.0);
        let mut p = TzPicker { zones, current, region: None, sel: 0, scroll: Spring::new(0.0), x, swap: Spring::new(1.0), closing: false };
        let rows = p.rows();
        let cur_region = crate::tz::region(&p.current).to_string();
        let sel = rows.iter().position(|r| matches!(r, TzRow::Region(n, _) if *n == cur_region)).unwrap_or(0);
        p.select(sel, rows.len(), true);
        self.tz = Some(p);
        self.fx(Sfx::Tab, Some(Haptic::Tick));
        self.dirty = true;
    }

    pub fn tz_open(&self) -> bool {
        self.tz.is_some()
    }

    fn close_tz(&mut self) {
        if let Some(p) = &mut self.tz {
            p.closing = true;
            p.x.set(W);
        }
    }

    fn tz_button(&mut self, b: Button, repeat: bool) {
        let Some(p) = &mut self.tz else { return };
        if p.closing {
            return;
        }
        let rows = p.rows();
        let n = rows.len();
        let old = p.sel;
        match b {
            Button::Up | Button::Down | Button::L1 | Button::R1 => {
                let new = match b {
                    Button::Up => old.saturating_sub(1),
                    Button::Down => (old + 1).min(n.saturating_sub(1)),
                    Button::L1 => old.saturating_sub(TZ_ROWS),
                    _ => (old + TZ_ROWS).min(n.saturating_sub(1)),
                };
                p.select(new, n, false);
                if new != old {
                    self.fx(Sfx::Move, Some(Haptic::Tick));
                } else {
                    self.fx(Sfx::Deny, None);
                }
            }
            Button::A | Button::Right if !repeat => match rows.get(old) {
                Some(TzRow::Region(r, _)) if r == "UTC" => {
                    self.out.push(Out::SetTimeZone("UTC".into()));
                    self.close_tz();
                }
                Some(TzRow::Region(r, _)) => {
                    p.region = Some(r.clone());
                    let zr = p.rows();
                    let cur = zr.iter().position(|z| matches!(z, TzRow::Zone(i) if p.zones[*i].id == p.current)).unwrap_or(0);
                    p.select(cur, zr.len(), true);
                    p.swap.snap(0.0);
                    p.swap.set(1.0);
                    self.fx(Sfx::Tab, Some(Haptic::Tick));
                }
                Some(TzRow::Zone(i)) => {
                    let id = p.zones[*i].id.clone();
                    self.out.push(Out::SetTimeZone(id));
                    self.close_tz();
                }
                None => {}
            },
            Button::B | Button::Left if !repeat => match p.region.take() {
                Some(r) => {
                    let rr = p.rows();
                    let sel = rr.iter().position(|x| matches!(x, TzRow::Region(n, _) if *n == r)).unwrap_or(0);
                    p.select(sel, rr.len(), true);
                    p.swap.snap(0.0);
                    p.swap.set(1.0);
                    self.fx(Sfx::Tab, Some(Haptic::Tick));
                }
                None => {
                    self.close_tz();
                    self.fx(Sfx::DrawerUp, Some(Haptic::Tick));
                }
            },
            _ => {}
        }
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

    pub fn pace(&self, has_app: bool) -> Pace {
        // asleep and dark: nothing at all
        if self.sleep_phase() == SleepPhase::Dark {
            return Pace::Idle;
        }
        // layers that animate for as long as they show
        let timed = self.boot.is_some()
            || self.trans.is_some()
            || self.flash.is_some()
            || self.toast.is_some()
            || self.osd.is_some()
            || self.power_hold.is_some();
        // layers that can sit still: moving only while their springs are
        let menu_moving = self.menu.as_ref().is_some_and(|m| m.moving());
        let settings_moving = self.settings.as_ref().is_some_and(|p| p.closing || !(p.scroll.rest() && p.x.rest()));
        let tz_moving = self.tz.as_ref().is_some_and(|p| p.closing || !(p.scroll.rest() && p.x.rest() && p.swap.rest()));
        let power_moving = self.power.as_ref().is_some_and(|p| p.closing || !p.x.rest());
        let sleep_moving = self.sleep.as_ref().is_some_and(|s| !s.a.rest());
        let springs = self.springs_ref().iter().any(|s| !s.rest());
        if timed || menu_moving || settings_moving || tz_moving || power_moving || sleep_moving || springs {
            return Pace::Full;
        }
        // home visible (no app, nothing opaque over it): the slow drift
        let over_home = self.menu.is_none() && self.settings.is_none() && self.tz.is_none() && self.power.is_none() && self.sleep.is_none() && self.page.is_none();
        if !has_app && over_home && self.drifting() {
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
        let mut v: Vec<&Spring> = vec![&self.row_x, &self.lp];
        for t in &self.tiles {
            v.extend([&t.lift, &t.sc, &t.back]);
        }
        v
    }

    fn springs_mut(&mut self) -> Vec<&mut Spring> {
        let mut v: Vec<&mut Spring> = vec![&mut self.row_x, &mut self.lp];
        for t in &mut self.tiles {
            v.extend([&mut t.lift, &mut t.sc, &mut t.back]);
        }
        if let Some(m) = &mut self.menu {
            v.extend(m.springs());
        }
        if let Some(p) = &mut self.settings {
            v.extend([&mut p.scroll, &mut p.x]);
        }
        if let Some(p) = &mut self.tz {
            v.extend([&mut p.scroll, &mut p.x, &mut p.swap]);
        }
        if let Some(p) = &mut self.power {
            v.push(&mut p.x);
        }
        if let Some(sl) = &mut self.sleep {
            v.push(&mut sl.a);
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
        // the MENU: in step with the apps; gone once faded out
        self.sync_menu(v);
        if self.settings.as_ref().is_some_and(|p| p.closing && p.x.x > W - 8.0) {
            self.settings = None;
            self.dirty = true;
        }
        // power menu closed; sleep woken
        if self.power.as_ref().is_some_and(|p| p.closing && p.x.x < 0.02) {
            self.power = None;
            self.dirty = true;
        }
        if self.sleep.as_ref().is_some_and(|s| s.waking && s.a.x < 0.01) {
            self.sleep = None;
            self.dirty = true;
        }
        // time zone picker slid away
        if self.tz.as_ref().is_some_and(|p| p.closing && p.x.x > W - 8.0) {
            self.tz = None;
            self.dirty = true;
        }
        // transitions
        match &self.trans {
            Some(Trans::Launch { target, t0, .. }) => {
                let anim_done = self.lp.rest() || self.lp.x > 0.995;
                let shown = target_uid(target, v).is_some_and(|u| v.current == Some(u) && v.app(u).is_some_and(|a| a.has_buffer));
                if anim_done && shown {
                    self.trans = None;
                    self.dirty = true;
                } else if now - t0 > 20.0 {
                    let name = match &self.trans {
                        Some(Trans::Launch { name, .. }) => name.clone(),
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

    /// Drop the OSD at once (the screenshot chord's first volume key
    /// started it; it must not be in the picture).
    pub fn hide_osd(&mut self) {
        if self.osd.take().is_some() {
            self.dirty = true;
        }
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
        if matches!(self.trans, Some(Trans::Launch { .. })) {
            self.trans = None;
        }
        self.show_toast(Toast::info(msg.to_string()));
        self.fx(Sfx::Deny, None);
        self.dirty = true;
    }

    /// The compositor started entry `id` as app `uid`: the transition
    /// waits for that app (a second instance has the same entry).
    pub fn launched(&mut self, id: &str, uid: u32) {
        if let Some(Trans::Launch { target, .. }) = &mut self.trans
            && *target == Target::Entry(id.to_string())
        {
            *target = Target::Uid(uid);
        }
    }

    /// A button was pressed (`repeat`: software auto-repeat).
    pub fn button(&mut self, b: Button, repeat: bool, v: &View) -> Vec<Out> {
        self.drift_until = self.t + DRIFT_SECS;
        self.dirty = true;
        if self.boot.is_some() {
            return Vec::new();
        }
        if self.sleep.is_some() {
            // any button wakes, and does nothing else
            self.wake();
            return std::mem::take(&mut self.out);
        }
        if self.power.is_some() {
            self.power_button(b, repeat);
            return std::mem::take(&mut self.out);
        }
        if self.tz.is_some() {
            self.tz_button(b, repeat);
        } else if self.page.is_some() {
            self.page_button(b, v);
        } else if self.settings.is_some() {
            self.settings_button(b, repeat, v);
        } else if self.menu.is_some() {
            self.menu_button(b, repeat, v);
        } else if self.trans.is_some() {
            self.trans_button(b, v);
        } else if v.current.is_none() {
            self.home_button(b, repeat, v);
        }
        std::mem::take(&mut self.out)
    }

    /// MENU (on its own) opens the cross menu, or closes it and whatever
    /// was opened from it.
    pub fn menu_key(&mut self, v: &View) -> Vec<Out> {
        self.drift_until = self.t + DRIFT_SECS;
        self.dirty = true;
        if self.boot.is_some() {
            return Vec::new();
        }
        if self.sleep.is_some() {
            self.wake();
            return std::mem::take(&mut self.out);
        }
        if self.power.is_some() {
            self.close_power_menu();
            return std::mem::take(&mut self.out);
        }
        let sub = self.tz.take().is_some() | self.page.take().is_some() | self.settings.take().is_some();
        if sub || self.menu.as_ref().is_some_and(|m| !m.closing) {
            self.close_menu();
        } else {
            if let Some(Trans::Launch { .. }) = &self.trans {
                // still starting: leave it to start in the background
                self.trans = None;
                self.lp.snap(0.0);
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
                // shortcuts: the MENU on Apps (L1, SELECT) or Quick (R1)
                self.open_menu_on(if b == Button::R1 { Cat::Quick } else { Cat::Apps }, v);
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
        self.trans = Some(Trans::Launch { target: Target::Entry(l.id.clone()), name: l.name.clone(), art: l.art, from, prev: None, t0: self.t });
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

    /// MENU > Quick > Settings…: the settings screen slides in.
    fn open_settings(&mut self) {
        let mut x = Spring::new(W);
        x.set(0.0);
        self.settings = Some(SettingsPanel { sel: 0, scroll: Spring::new(0.0), x, closing: false });
        self.confirm = None;
        self.fx(Sfx::Tab, Some(Haptic::Tick));
    }

    fn close_settings(&mut self) {
        if let Some(p) = &mut self.settings {
            p.closing = true;
            p.x.set(W);
        }
        self.fx(Sfx::DrawerUp, Some(Haptic::Tick));
    }

    fn settings_button(&mut self, b: Button, repeat: bool, _v: &View) {
        let Some(p) = &mut self.settings else { return };
        if p.closing {
            return;
        }
        let n = SETTINGS.len();
        match b {
            Button::Up | Button::Down => {
                let old = p.sel;
                p.sel = if b == Button::Up { old.saturating_sub(1) } else { (old + 1).min(n - 1) };
                p.scroll.set(set_scroll_for(p.sel, n));
                if p.sel != old {
                    self.confirm = None;
                    self.fx(Sfx::Tick, Some(Haptic::Tick));
                } else {
                    self.fx(Sfx::Deny, None);
                }
                return;
            }
            Button::B if !repeat => {
                self.close_settings();
                return;
            }
            _ => {}
        }
        let it = SETTINGS[p.sel];
        let horiz = matches!(b, Button::Left | Button::Right);
        let mut changed = true;
        match (it, b) {
            (Setting::Sounds, _) if horiz || b == Button::A => self.prefs.sounds = !self.prefs.sounds,
            (Setting::Haptics, _) if horiz || b == Button::A => self.prefs.haptics = !self.prefs.haptics,
            (Setting::ReduceMotion, _) if horiz || b == Button::A => self.prefs.reduce_motion = !self.prefs.reduce_motion,
            (Setting::Palette, _) if horiz || b == Button::A => self.prefs.night = !self.prefs.night,
            (Setting::SleepMode, _) if horiz || b == Button::A => self.prefs.suspend = !self.prefs.suspend,
            (Setting::IdleSleep, _) if horiz || b == Button::A => {
                let n = IDLE_CHOICES.len() as i32;
                let i = IDLE_CHOICES.iter().position(|m| *m == self.prefs.idle_minutes).unwrap_or(0) as i32;
                let d = if b == Button::Left { -1 } else { 1 };
                self.prefs.idle_minutes = IDLE_CHOICES[(i + d).rem_euclid(n) as usize];
            }
            (Setting::Motion, _) if horiz || b == Button::A => {
                let i = Preset::ALL.iter().position(|p| *p == self.prefs.motion).unwrap_or(0) as i32;
                let d = if b == Button::Left { -1 } else { 1 };
                self.prefs.motion = Preset::ALL[(i + d).rem_euclid(3) as usize];
            }
            (Setting::TimeZone, Button::A | Button::Right) if !repeat => {
                changed = false;
                self.out.push(Out::OpenTimeZone);
            }
            (Setting::Wifi | Setting::Remote, Button::A | Button::Right) if !repeat => {
                changed = false;
                self.out.push(Out::OpenSetup { wifi: it == Setting::Wifi });
            }
            (Setting::Logout, Button::A) if !repeat => {
                changed = false;
                if self.confirmed(it, "log out (all apps close)") {
                    self.out.push(Out::Logout);
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

/// Settings scroll: seven rows show; keep the selection inside them.
fn set_scroll_for(sel: usize, n: usize) -> f32 {
    sel.saturating_sub(3).min(n.saturating_sub(7)) as f32 * 76.0
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
            Some(Trans::Launch { prev, .. }) => prev.filter(|u| v.app(*u).is_some()),
            Some(Trans::Close { .. }) => None,
            None => v.current,
        };
        match base_app {
            Some(uid) => self.draw_app(&mut b, v, uid, true),
            None => self.draw_home(&mut b, &pal, v),
        }
        // the MENU fades out under the transition it started
        if let Some(m) = &self.menu {
            self.draw_menu(&mut b, &pal, v, m);
        }
        self.draw_trans(&mut b, &pal, v);
        if let Some(p) = &self.settings {
            self.draw_settings(&mut b, &pal, v, p);
        }
        if let Some(p) = &self.tz {
            self.draw_tz(&mut b, &pal, p);
        }
        if self.page.is_some() {
            b.image(Src::Layer(Layer::Page), 0.0, 0.0, W, H, [0.0, 0.0, 1.0, 1.0]);
        }
        if let Some(p) = &self.power {
            self.draw_power(&mut b, &pal, p);
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
        if let Some(h) = self.power_hold {
            self.draw_power_hold(&mut b, &pal, h);
        }
        if let Some(sl) = &self.sleep {
            b.rect(0.0, 0.0, W, H, with_a(Color(0, 0, 0, 255), sl.a.x));
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

    /// The settings screen (MENU > Quick > Settings…), full screen.
    fn draw_settings(&self, b: &mut B, pal: &Pal, v: &View, p: &SettingsPanel) {
        b.push(Xf::translate(p.x.x, 0.0));
        b.block(0.0, 0.0, W + 2.0, H, pal.ground, pal.ink);
        let n = SETTINGS.len();
        let sel = p.sel.min(n - 1);
        let onoff = |on: bool| (vec!["On", "Off"], if on { 0 } else { 1 });
        b.clip(Some([0.0, 119.0, W, H - 64.0 - 119.0]));
        for (i, it) in SETTINGS.iter().enumerate() {
            let y = 136.0 + i as f32 * 76.0 - p.scroll.x;
            if y > H || y + 76.0 < 100.0 {
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
                Setting::SleepMode => ("Sleep", Ok((vec!["Suspend", "Screen off"], if self.prefs.suspend { 0 } else { 1 }))),
                Setting::IdleSleep => (
                    "Sleep when idle",
                    Ok((IDLE_LABELS.to_vec(), IDLE_CHOICES.iter().position(|m| *m == self.prefs.idle_minutes).unwrap_or(0))),
                ),
                Setting::TimeZone => ("Time zone", Err(format!("{} · {} ›", crate::tz::pretty(v.timezone).0, v.sys.time))),
                Setting::Wifi => ("Wi-Fi networks", Err(if v.sys.wifi { "Connected ›".into() } else { "Not connected ›".into() })),
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
                Setting::Logout => ("Log out", Err("Closes all apps".into())),
            };
            let lab = ts(Face::Body400, 24.0, fg);
            b.text(60.0, y + 38.0 - lab.line_px() / 2.0, name, &lab);
            // value: options with the chosen one bold, others faded
            let st_on = ts(Face::Mono600, 18.0, fg);
            let st_off = ts(Face::Mono400, 18.0, with_a(fg, 0.55));
            let st_txt = ts(Face::Mono400, 18.0, fg);
            let mut segs: Vec<(String, TextStyle)> = Vec::new();
            let adjustable = val.is_ok();
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
        b.clip(None);
        // header
        b.rect(0.0, 0.0, W, 118.0, pal.ground);
        b.rect(0.0, 118.0, W, 1.0, pal.ink);
        let sub = ts(Face::Mono400, 15.0, pal.soft);
        b.text(40.0, 26.0, "MENU · QUICK · SETTINGS", &sub);
        b.text_right(W - 40.0, 26.0, &format!("{} / {}", sel + 1, n), &sub);
        let title = ts(Face::SerifItalic, 54.0, pal.ink).line_height(1.0);
        b.text(40.0, 50.0, "Settings", &title);
        let last = match SETTINGS[sel] {
            Setting::TimeZone | Setting::Wifi | Setting::Remote | Setting::Logout => ("A", "Open"),
            _ => ("◀▶", "Change"),
        };
        let last = if SETTINGS[sel] == Setting::Logout { ("A", "Log out") } else { last };
        hints(b, pal, H - 64.0, &[("▲▼", "Choose"), last, ("B", "Back"), ("MENU", "Close")]);
        b.pop();
    }
    fn draw_tz(&self, b: &mut B, pal: &Pal, p: &TzPicker) {
        b.push(Xf::translate(p.x.x, 0.0));
        b.block(0.0, 0.0, W + 2.0, H, pal.ground, pal.ink);
        let rows = p.rows();
        let n = rows.len();
        let sel = p.sel.min(n.saturating_sub(1));
        b.clip(Some([0.0, 119.0, W, H - 64.0 - 119.0]));
        b.push(Xf::translate((1.0 - p.swap.x) * 60.0, 0.0));
        b.push_alpha(p.swap.x.clamp(0.0, 1.0));
        let cur_region = crate::tz::region(&p.current).to_string();
        for (i, r) in rows.iter().enumerate() {
            let y = 136.0 + i as f32 * 76.0 - p.scroll.x;
            if y > H || y + 76.0 < 100.0 {
                continue;
            }
            let on = i == sel;
            let fg = if on { pal.lime_ink } else { pal.ink };
            let soft = if on { pal.lime_ink } else { pal.soft };
            if on {
                b.rect(40.0, y, W - 80.0, 76.0, pal.lime);
            } else {
                b.rect(40.0, y + 75.0, W - 80.0, 1.0, pal.faint);
            }
            let lab = ts(Face::Body400, 24.0, fg);
            let mono = ts(Face::Mono400, 18.0, soft);
            let ty = y + 38.0 - lab.line_px() / 2.0;
            let my = y + 38.0 - mono.line_px() / 2.0;
            let (is_cur, right_txt) = match r {
                TzRow::Region(name, c) => {
                    b.text(60.0, ty, name, &lab);
                    let t = if name == "UTC" { "Coordinated Universal Time".to_string() } else { format!("{c} zones ›") };
                    (*name == cur_region || (name == "UTC" && p.current == "UTC"), t)
                }
                TzRow::Zone(zi) => {
                    let z = &p.zones[*zi];
                    let (city, sub) = crate::tz::pretty(&z.id);
                    let w = b.text(60.0, ty, &city, &lab);
                    if let Some(sub) = sub {
                        let st = ts(Face::Mono400, 15.0, soft);
                        b.text(60.0 + w + 14.0, y + 38.0 - st.line_px() / 2.0, &sub.to_uppercase(), &st);
                    }
                    (z.id == p.current, format!("{}   {}", crate::tz::hhmm_at(z.offset), crate::tz::fmt_offset(z.offset)))
                }
            };
            let w = b.text_right(W - 60.0, my, &right_txt, &mono);
            if is_cur {
                chip(b, W - 60.0 - w - 16.0, y + 38.0, "CURRENT", 13.0, (7.0, 1.0), pal.lavender, pal.lavender_ink);
            }
        }
        b.pop_alpha();
        b.pop();
        b.clip(None);
        // header
        b.rect(0.0, 0.0, W, 118.0, pal.ground);
        b.rect(0.0, 118.0, W, 1.0, pal.ink);
        let crumb = match &p.region {
            Some(_) => "SETTINGS · TIME ZONE · CITY",
            None => "SETTINGS · TIME ZONE · REGION",
        };
        let sub = ts(Face::Mono400, 15.0, pal.soft);
        b.text(40.0, 26.0, crumb, &sub);
        b.text_right(W - 40.0, 26.0, &format!("{} / {}", sel + 1, n), &sub);
        let title = ts(Face::SerifItalic, 54.0, pal.ink).line_height(1.0);
        b.text(40.0, 50.0, p.region.as_deref().unwrap_or("Time zone"), &title);
        let now = format!("{} · {}", crate::tz::pretty(&p.current).0, mortar_ui::status::clock());
        let st = ts(Face::Mono600, 17.0, pal.lime_ink);
        let tw = b.measure(&now, &st);
        let (w, h) = (tw + 26.0, st.line_px() + 10.0);
        let (x, y) = ((W - 40.0 - w).round(), (104.0 - h).round());
        b.block(x, y, w.round(), h.round(), pal.lime, pal.lime_ink);
        b.text(x + 13.0, y + 5.0, &now, &st);
        let last = if p.region.is_some() { ("A", "Set") } else { ("A", "Open") };
        hints(b, pal, H - 64.0, &[("▲▼", "Choose"), ("L1/R1", "Page"), ("B", "Back"), last]);
        b.pop();
    }

    fn draw_power(&self, b: &mut B, pal: &Pal, p: &PowerMenu) {
        let x = p.x.x.clamp(0.0, 1.2);
        b.rect(0.0, 0.0, W, H, Color(29, 29, 29, (0.5 * x.min(1.0) * 255.0) as u8));
        let (pw, ph) = (560.0, 470.0);
        let (px, py) = ((W - pw) / 2.0, (H - ph) / 2.0);
        let s = 0.9 + 0.1 * x;
        b.push(Xf::about(W / 2.0, H / 2.0, Xf::scale(s, s)));
        b.push_alpha(x.min(1.0));
        b.block(px, py, pw, ph, pal.ground, pal.ink);
        let sub = ts(Face::Mono400, 15.0, pal.soft);
        b.text(px + 32.0, py + 26.0, "POWER KEY", &sub);
        let title = ts(Face::SerifItalic, 54.0, pal.ink).line_height(1.0);
        b.text(px + 32.0, py + 50.0, "Power", &title);
        b.rect(px, py + 120.0, pw, 1.0, pal.ink);
        for (i, (name, desc)) in POWER_ROWS.iter().enumerate() {
            let y = py + 136.0 + i as f32 * 84.0;
            let on = i == p.sel;
            let confirm = p.confirm == Some(i);
            let (bg, fg) = if confirm {
                (Some(pal.orange), pal.dark)
            } else if on {
                (Some(pal.lime), pal.lime_ink)
            } else {
                (None, pal.ink)
            };
            if let Some(bg) = bg {
                b.block(px + 24.0, y, pw - 48.0, 72.0, bg, if confirm { pal.dark } else { pal.lime_ink });
            } else {
                b.rect(px + 24.0, y + 71.0, pw - 48.0, 1.0, pal.faint);
            }
            let lab = ts(Face::Body500, 26.0, fg);
            b.text(px + 46.0, y + 36.0 - lab.line_px() / 2.0, name, &lab);
            let d = if confirm {
                format!("Press A again to {}", name.to_lowercase())
            } else if i == 0 && self.prefs.suspend {
                "Suspend · power key wakes".to_string()
            } else {
                desc.to_string()
            };
            let st = ts(Face::Mono400, 15.0, if on || confirm { fg } else { pal.soft });
            b.text_right(px + pw - 46.0, y + 36.0 - st.line_px() / 2.0, &d, &st);
        }
        let cap = ts(Face::Mono600, 13.0, pal.ground);
        let lab = ts(Face::Body400, 16.0, pal.soft);
        let mut xr = px + pw - 28.0;
        for (c, l) in [("▲▼", "Choose"), ("B", "Cancel"), ("A", "Select")].iter().rev() {
            let lw = b.measure(l, &lab);
            b.text(xr - lw, py + ph - 44.0, l, &lab);
            xr -= lw + 8.0;
            let tw = arrow_text(b, 0.0, 0.0, c, &cap, false);
            let cw = (tw + 16.0).max(26.0);
            b.rrect(xr - cw, py + ph - 44.0 + 1.0, cw, 24.0, 12.0, pal.ink);
            arrow_text(b, xr - cw + (cw - tw) / 2.0, py + ph - 44.0 + 13.0 - cap.line_px() / 2.0, c, &cap, true);
            xr -= cw + 20.0;
        }
        b.pop_alpha();
        b.pop();
    }

    /// Holding the power key: a pill that fills towards the power menu.
    fn draw_power_hold(&self, b: &mut B, pal: &Pal, h: f32) {
        let (w, hh) = (340.0, 56.0);
        let (x, y) = ((W - w) / 2.0, H - 64.0 - 24.0 - hh);
        b.block(x, y, w, hh, pal.ground, pal.ink);
        b.rect(x + 1.0, y + hh - 7.0, (w - 2.0) * h.clamp(0.0, 1.0), 6.0, pal.lime);
        let st = ts(Face::Mono600, 15.0, pal.ink).tracking(0.06);
        b.text_center(W / 2.0, y + 24.0 - st.line_px() / 2.0, "HOLD FOR POWER MENU", &st);
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
        [("term", Category::Apps), ("m8", Category::Music), ("pico", Category::Games)]
            .iter()
            .map(|(i, c)| LauncherInfo { id: i.to_string(), name: i.to_string(), description: String::new(), art: Art::Generic, running: false, category: *c, single_instance: true })
            .collect()
    }
    fn view<'a>(running: &'a [RunningInfo], launcher: &'a [LauncherInfo], sys: &'a Sys, current: Option<u32>) -> View<'a> {
        View {
            running,
            launcher,
            sys,
            volume: Some(50),
            current,
            keyboard_app: false,
            osk_h: 0.0,
            host: "brick",
            timezone: "Europe/Lisbon",
            radios: Radios { wifi: Some(true), bluetooth: None },
        }
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
    fn press(ui: &mut ShellUi, v: &View, bs: &[Button]) -> Vec<Out> {
        bs.iter().flat_map(|b| actions(&ui.button(*b, false, v))).collect()
    }
    fn sel_row(ui: &ShellUi) -> Option<cross::Row> {
        let m = ui.menu.as_ref()?;
        m.sel_key(ui.cross_sel[m.cat as usize])
    }
    /// MENU, then to Quick (R1 jumps to Power, ◀ is Quick).
    fn to_quick(ui: &mut ShellUi, v: &View) {
        ui.menu_key(v);
        press(ui, v, &[Button::R1, Button::Left]);
        assert_eq!(ui.menu_category(), Some(Cat::Quick));
    }
    fn to_settings(ui: &mut ShellUi, v: &View) {
        to_quick(ui, v);
        press(ui, v, &[Button::Down; 8]);
        assert_eq!(sel_row(ui), Some(cross::Row::Settings));
        ui.button(Button::A, false, v);
        assert!(ui.settings_open());
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
        assert_eq!(ui.menu_category(), Some(Cat::Running));
        // Home, the app on screen, the previous one (selected)
        assert_eq!(sel_row(&ui), Some(cross::Row::App(1)));
        assert_eq!(actions(&ui.button(Button::A, false, &v)), vec![Out::Focus(1)]);
        // it zooms out of its row over the app that was in front
        assert!(matches!(ui.trans, Some(Trans::Launch { target: Target::Uid(1), prev: Some(2), .. })));
        let v2 = view(&r, &l, &sys, Some(1));
        settle(&mut ui, &v2);
        assert!(ui.menu.is_none() && ui.trans.is_none());
        assert!(!ui.captures_input(true));
    }

    #[test]
    fn x_closes_a_running_app() {
        let sys = Sys::demo(None);
        let l = launcher();
        let r = [run(2, "term"), run(1, "m8")];
        let v = view(&r, &l, &sys, Some(2));
        let mut ui = ShellUi::new(Prefs::default(), false);
        ui.menu_key(&v);
        settle(&mut ui, &v);
        assert_eq!(actions(&ui.button(Button::X, false, &v)), vec![Out::Close(1)]);
        assert_eq!(ui.toast.as_ref().map(|t| t.toast.title.as_str()), Some("Closed"));
        // its row goes at once (the app takes a moment to exit)
        assert_eq!(sel_row(&ui), Some(cross::Row::App(2)));
        let r2 = [run(2, "term")];
        let v2 = view(&r2, &l, &sys, Some(2));
        settle(&mut ui, &v2);
        assert_eq!(sel_row(&ui), Some(cross::Row::App(2)));
        // Home cannot be closed
        ui.button(Button::Up, false, &v2);
        assert_eq!(sel_row(&ui), Some(cross::Row::Home));
        assert!(actions(&ui.button(Button::X, false, &v2)).is_empty());
    }

    #[test]
    fn home_row_goes_home_with_close_transition() {
        let sys = Sys::demo(None);
        let l = launcher();
        let r = [run(2, "term")];
        let v = view(&r, &l, &sys, Some(2));
        let mut ui = ShellUi::new(Prefs::default(), false);
        ui.menu_key(&v);
        settle(&mut ui, &v);
        assert_eq!(sel_row(&ui), Some(cross::Row::App(2)));
        ui.button(Button::Up, false, &v);
        assert_eq!(actions(&ui.button(Button::A, false, &v)), vec![Out::Home]);
        assert!(matches!(ui.trans, Some(Trans::Close { uid: 2, .. })));
    }

    #[test]
    fn categories_remember_their_selection() {
        let sys = Sys::demo(None);
        let l = launcher();
        let v = view(&[], &l, &sys, None);
        let mut ui = ShellUi::new(Prefs::default(), false);
        ui.menu_key(&v);
        assert_eq!(cross::cats(&v).len(), 6);
        press(&mut ui, &v, &[Button::Right, Button::Down, Button::Down]);
        assert_eq!(ui.menu_category(), Some(Cat::Apps));
        assert_eq!(sel_row(&ui), Some(cross::Row::Launch("pico".into())));
        press(&mut ui, &v, &[Button::Right]);
        assert_eq!(ui.menu_category(), Some(Cat::Games));
        assert_eq!(sel_row(&ui), Some(cross::Row::Launch("pico".into())));
        press(&mut ui, &v, &[Button::R1]);
        assert_eq!(ui.menu_category(), Some(Cat::Power));
        press(&mut ui, &v, &[Button::L1]);
        assert_eq!(ui.menu_category(), Some(Cat::Running));
        // closed and opened again: Apps is still on its third row
        ui.menu_key(&v);
        settle(&mut ui, &v);
        assert!(ui.menu.is_none());
        ui.menu_key(&v);
        press(&mut ui, &v, &[Button::Right]);
        assert_eq!(sel_row(&ui), Some(cross::Row::Launch("pico".into())));
        assert_eq!(press(&mut ui, &v, &[Button::A]), vec![Out::Launch("pico".into())]);
        // Games and Music only show with something in them
        let l2: Vec<LauncherInfo> = l.into_iter().filter(|x| x.category == Category::Apps).collect();
        let v2 = view(&[], &l2, &sys, None);
        assert_eq!(cross::cats(&v2), vec![Cat::Running, Cat::Apps, Cat::Quick, Cat::Power]);
    }

    #[test]
    fn launching_a_running_app_switches_to_it() {
        let sys = Sys::demo(None);
        let l = launcher();
        let r = [run(2, "term"), run(1, "m8")];
        let v = view(&r, &l, &sys, Some(2));
        let mut ui = ShellUi::new(Prefs::default(), false);
        ui.menu_key(&v);
        assert_eq!(press(&mut ui, &v, &[Button::Right, Button::Down, Button::A]), vec![Out::Focus(1)]);
        // the one in front: the MENU just closes
        ui.trans = None;
        settle(&mut ui, &v);
        ui.menu_key(&v);
        press(&mut ui, &v, &[Button::Right, Button::Up]);
        assert!(press(&mut ui, &v, &[Button::A]).is_empty());
        assert!(!ui.menu_open());
    }

    #[test]
    fn quick_values_and_toggles() {
        let sys = Sys::demo(None);
        let l = launcher();
        let r = [run(2, "term")];
        let v = view(&r, &l, &sys, Some(2));
        let mut ui = ShellUi::new(Prefs::default(), false);
        to_quick(&mut ui, &v);
        assert_eq!(sel_row(&ui), Some(cross::Row::Brightness));
        // ◀▶ always change category, also on a value row
        assert!(press(&mut ui, &v, &[Button::Left]).is_empty());
        assert_eq!(ui.menu_category(), Some(Cat::Music));
        press(&mut ui, &v, &[Button::Right]);
        assert_eq!(ui.menu_category(), Some(Cat::Quick));
        // A: adjust mode; ◀▶ change the value; A or B leave it
        assert!(press(&mut ui, &v, &[Button::A]).is_empty());
        assert_eq!(press(&mut ui, &v, &[Button::Right]), vec![Out::Brightness(10)]);
        assert_eq!(press(&mut ui, &v, &[Button::Left]), vec![Out::Brightness(-10)]);
        assert!(press(&mut ui, &v, &[Button::B]).is_empty());
        assert!(ui.menu_open(), "B leaves adjust mode, not the MENU");
        press(&mut ui, &v, &[Button::Right]);
        assert_eq!(ui.menu_category(), Some(Cat::Power));
        press(&mut ui, &v, &[Button::Left]);
        // ▲▼ in adjust mode leave it and move
        assert_eq!(press(&mut ui, &v, &[Button::Down, Button::A, Button::Left]), vec![Out::Volume(-10)]);
        assert!(press(&mut ui, &v, &[Button::Down]).is_empty());
        assert_eq!(sel_row(&ui), Some(cross::Row::Wifi));
        assert_eq!(ui.menu_category(), Some(Cat::Quick));
        // toggles: A switches; ◀▶ still change category
        assert_eq!(press(&mut ui, &v, &[Button::A]), vec![Out::Radio(Radio::Wifi, false)]);
        press(&mut ui, &v, &[Button::Right]);
        assert_eq!(ui.menu_category(), Some(Cat::Power));
        press(&mut ui, &v, &[Button::Left]);
        // no Bluetooth switch on this device: nothing to toggle
        assert!(press(&mut ui, &v, &[Button::Down, Button::A]).is_empty());
        // Sleep closes the MENU and sleeps
        assert_eq!(press(&mut ui, &v, &[Button::Down, Button::A]), vec![Out::Sleep]);
        assert!(!ui.menu_open());
    }

    #[test]
    fn power_needs_a_second_a() {
        let sys = Sys::demo(None);
        let l = launcher();
        let v = view(&[], &l, &sys, None);
        let mut ui = ShellUi::new(Prefs::default(), false);
        ui.menu_key(&v);
        press(&mut ui, &v, &[Button::R1, Button::Down]);
        assert_eq!(sel_row(&ui), Some(cross::Row::Restart));
        assert!(press(&mut ui, &v, &[Button::A]).is_empty());
        assert_eq!(press(&mut ui, &v, &[Button::A]), vec![Out::Reboot]);
        // moving away forgets the first press
        assert!(press(&mut ui, &v, &[Button::Down, Button::A, Button::Up, Button::Down, Button::A]).is_empty());
        assert_eq!(press(&mut ui, &v, &[Button::A]), vec![Out::PowerOff]);
    }

    #[test]
    fn options_popup() {
        let sys = Sys::demo(None);
        let l = launcher();
        let r = [run(2, "term"), run(1, "m8")];
        let v = view(&r, &l, &sys, Some(2));
        let mut ui = ShellUi::new(Prefs::default(), false);
        ui.menu_key(&v);
        assert_eq!(press(&mut ui, &v, &[Button::Y, Button::Down, Button::A]), vec![Out::Restart(1)]);
        assert!(ui.menu_open());
        // B backs out of the popup, not the MENU
        assert!(press(&mut ui, &v, &[Button::Y, Button::B]).is_empty());
        assert!(ui.menu_open());
        assert_eq!(press(&mut ui, &v, &[Button::Y, Button::Up, Button::A]), vec![Out::Close(1)]);
        // Y on Home: nothing
        ui.button(Button::Up, false, &v);
        ui.button(Button::Up, false, &v);
        assert!(press(&mut ui, &v, &[Button::Y, Button::A]).iter().all(|o| *o == Out::Home));
    }

    #[test]
    fn settings_change_prefs() {
        let sys = Sys::demo(None);
        let l = launcher();
        let v = view(&[], &l, &sys, None);
        let mut ui = ShellUi::new(Prefs::default(), false);
        to_settings(&mut ui, &v);
        let out = ui.button(Button::Right, false, &v);
        assert!(out.contains(&Out::Prefs));
        assert!(!ui.prefs.sounds);
        press(&mut ui, &v, &[Button::Down, Button::Down, Button::Right]);
        assert_eq!(ui.prefs.motion, Preset::Calm);
        // log out needs a second press
        press(&mut ui, &v, &[Button::Down; 10]);
        assert!(press(&mut ui, &v, &[Button::A]).is_empty());
        assert_eq!(press(&mut ui, &v, &[Button::A]), vec![Out::Logout]);
        // Sleep when idle, then Sleep (Suspend / Screen off), above Log out
        assert!(!ui.prefs.suspend);
        ui.button(Button::Up, false, &v);
        assert!(ui.button(Button::Right, false, &v).contains(&Out::Prefs));
        assert_eq!(ui.prefs.idle_minutes, 1);
        press(&mut ui, &v, &[Button::Left, Button::Left]);
        assert_eq!(ui.prefs.idle_minutes, 30, "wraps around");
        press(&mut ui, &v, &[Button::Up, Button::A]);
        assert!(ui.prefs.suspend);
        // B: back to the MENU; MENU closes everything
        ui.button(Button::B, false, &v);
        settle(&mut ui, &v);
        assert!(!ui.settings_open() && ui.menu_open());
        ui.button(Button::A, false, &v);
        assert!(ui.settings_open());
        ui.menu_key(&v);
        settle(&mut ui, &v);
        assert!(ui.settings.is_none() && ui.menu.is_none());
    }

    #[test]
    fn menu_at_rest_is_not_full_pace() {
        let sys = Sys::demo(None);
        let l = launcher();
        let v = view(&[], &l, &sys, None);
        let mut ui = ShellUi::new(Prefs::default(), false);
        ui.menu_key(&v);
        ui.button(Button::Right, false, &v);
        settle(&mut ui, &v);
        assert_ne!(ui.pace(false), Pace::Full);
        for _ in 0..(40 * 60) {
            ui.step(1.0 / 60.0, &v);
        }
        assert_eq!(ui.pace(false), Pace::Idle);
    }

    #[test]
    fn time_zone_picker() {
        let sys = Sys::demo(None);
        let l = launcher();
        let v = view(&[], &l, &sys, None);
        let mut ui = ShellUi::new(Prefs::default(), false);
        to_settings(&mut ui, &v);
        let tz_row = SETTINGS.iter().position(|s| *s == Setting::TimeZone).unwrap();
        for _ in 0..tz_row {
            ui.button(Button::Down, false, &v);
        }
        assert_eq!(actions(&ui.button(Button::A, false, &v)), vec![Out::OpenTimeZone]);
        let zones = ["Europe/Lisbon", "Europe/Berlin", "America/Argentina/Buenos_Aires", "America/New_York", "UTC"]
            .iter()
            .map(|z| TzZone { id: z.to_string(), offset: 0 })
            .collect();
        ui.open_tz(zones, "Europe/Lisbon".into());
        // regions: America, Europe (current, preselected), UTC
        let p = ui.tz.as_ref().unwrap();
        assert_eq!(p.rows().len(), 3);
        assert_eq!(p.sel, 1);
        ui.button(Button::A, false, &v); // into Europe, on Lisbon
        let p = ui.tz.as_ref().unwrap();
        assert_eq!(p.rows(), vec![TzRow::Zone(1), TzRow::Zone(0)]); // Berlin, Lisbon
        assert_eq!(p.sel, 1);
        ui.button(Button::Up, false, &v);
        assert_eq!(actions(&ui.button(Button::A, false, &v)), vec![Out::SetTimeZone("Europe/Berlin".into())]);
        settle(&mut ui, &v);
        assert!(!ui.tz_open());
        assert!(ui.settings_open() && ui.menu_open());
    }

    #[test]
    fn sleep_and_power_menu() {
        let sys = Sys::demo(None);
        let l = launcher();
        let r = [run(2, "term")];
        let v = view(&r, &l, &sys, Some(2));
        let mut ui = ShellUi::new(Prefs::default(), false);
        settle(&mut ui, &v);
        assert!(!ui.captures_input(true));
        ui.sleep();
        assert!(ui.captures_input(true), "asleep: buttons do not reach the app");
        settle(&mut ui, &v);
        assert_eq!(ui.sleep_phase(), SleepPhase::Dark);
        assert_eq!(ui.pace(true), Pace::Idle);
        // a button wakes and does nothing else
        let out = ui.button(Button::A, false, &v);
        assert!(actions(&out).is_empty());
        assert_eq!(ui.sleep_phase(), SleepPhase::Waking);
        settle(&mut ui, &v);
        assert_eq!(ui.sleep_phase(), SleepPhase::Awake);
        assert!(!ui.captures_input(true));
        // power menu: restart needs a second A; B backs out of the confirm
        ui.open_power_menu();
        ui.button(Button::Down, false, &v);
        assert!(actions(&ui.button(Button::A, false, &v)).is_empty());
        ui.button(Button::B, false, &v);
        assert!(ui.power_menu_open());
        assert!(actions(&ui.button(Button::A, false, &v)).is_empty());
        assert_eq!(actions(&ui.button(Button::A, false, &v)), vec![Out::Reboot]);
        // Sleep from the menu
        ui.button(Button::Up, false, &v);
        ui.button(Button::A, false, &v);
        settle(&mut ui, &v);
        assert_eq!(ui.sleep_phase(), SleepPhase::Dark);
        assert!(!ui.power_menu_open());
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
