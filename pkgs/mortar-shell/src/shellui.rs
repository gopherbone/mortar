// SPDX-License-Identifier: GPL-2.0-or-later
//! The shell's own UI, after the approved motion prototype
//! (plan/mortar-motion) and Cross Menu 2 (plan/mortar-cross-2): boot, the
//! MENU (cross.rs: the Running stack, its top card the dashboard, home.rs
//! and widgets.rs, which is home with no app in front; then
//! Apps, Games, Music, Settings and its glass pages, menupages.rs),
//! launch/close/switch transitions, the volume OSD, toasts, first-time
//! tips and the screenshot flash.
//!
//! Pure state: the compositor feeds it buttons, the passing of time and a
//! `View` of the apps; it answers with `Out`s (actions, sounds, haptic
//! ticks) and builds a `Scene` for a renderer. Everything is laid out in
//! the prototype's 1024x768 logical pixels.
//!
//! Layers, bottom to top: the plain ground (home) or the visible app (+ on-screen keyboard),
//! the MENU and its veil (with its glass pages), the launch/close/switch
//! transition, a setup page, the power menu, the OSD, a toast, the shutter
//! flash, the boot screen, the power-key hold indicator, and the black of
//! sleep.

#[path = "cross.rs"]
mod cross;
#[path = "menupages.rs"]
mod menupages;
#[path = "gamepages.rs"]
mod gamepages;
#[path = "home.rs"]
mod home;
pub use cross::Cat;
#[cfg(test)]
use menupages::SubKind;
use cross::Glass;

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

/// The ground-colour veil over the blurred app behind the MENU (light
/// enough that its colours and layout show, with the blur its text does
/// not read). Over dark backdrops it is raised (`veil_for`), up to
/// VEIL_MAX.
const FROST_VEIL: f32 = 0.62;
const VEIL_MAX: f32 = 0.9;

/// WCAG relative luminance of an sRGB colour (channels 0..1).
fn luminance(c: [f32; 3]) -> f32 {
    let lin = |v: f32| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
    0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
}

fn rgb(c: Color) -> [f32; 3] {
    [c.0 as f32 / 255.0, c.1 as f32 / 255.0, c.2 as f32 / 255.0]
}

/// The lightest veil (from FROST_VEIL up) over a backdrop of mean colour
/// `mean` that gives the palette's ink-soft text at least 4.5:1 against
/// the veiled backdrop. Light backdrops keep 62%; black needs ~85%.
pub fn veil_for(mean: [f32; 3], pal: &Pal) -> f32 {
    let (g, ls) = (rgb(pal.ground), luminance(rgb(pal.soft)));
    let mut v = FROST_VEIL;
    while v < VEIL_MAX {
        let c: [f32; 3] = std::array::from_fn(|i| mean[i] * (1.0 - v) + g[i] * v);
        let l = luminance(c);
        let ratio = (l.max(ls) + 0.05) / (l.min(ls) + 0.05);
        if ratio >= 4.5 {
            return v;
        }
        v += 0.01;
    }
    VEIL_MAX
}

pub const W: f32 = 1024.0;
pub const H: f32 = 768.0;

// ------------------------------------------------------------- settings ----

/// User settings changed in MENU > Settings (persisted by the compositor;
/// defaults from the NixOS module), plus what the shell remembers for the
/// person: the first-time tips already shown and what was on screen
/// lately (the dashboard's Recently closed).
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
    /// The MENU's screensaver: seconds idle on the cross before its
    /// pattern fills the screen (0: never).
    pub screensaver: u32,
    /// First-time tips already shown (TIPS ids).
    pub tips_seen: Vec<String>,
    /// Entries on screen lately, newest first (Recently closed).
    pub recent: Vec<String>,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            sounds: true,
            sound_volume: 60,
            haptics: true,
            motion: Preset::Bouncy,
            reduce_motion: false,
            night: false,
            suspend: false,
            idle_minutes: 0,
            screensaver: 60,
            tips_seen: Vec::new(),
            recent: Vec::new(),
        }
    }
}

/// Settings > Sleep when idle: the choices, in minutes (0: never).
pub const IDLE_CHOICES: [u32; 6] = [0, 1, 2, 5, 10, 30];
const IDLE_LABELS: [&str; 6] = ["Off", "1m", "2m", "5m", "10m", "30m"];

/// Settings > Screensaver: seconds idle on the cross (0: never).
pub const SAVER_CHOICES: [u32; 5] = [0, 30, 60, 120, 300];
const SAVER_LABELS: [&str; 5] = ["Off", "30s", "1m", "2m", "5m"];
/// The screensaver grows (and shrinks) over this long; after SAVER_DIM s
/// full screen it dims and redraws at 20 fps.
const SAVER_GROW: f32 = 1.1;
const SAVER_DIM: f64 = 180.0;

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
    /// A game (mortar-play, one app per game): Y opens the game panel.
    pub game: bool,
    /// A game put to sleep: its picture stands in for the window.
    pub asleep: bool,
    /// Its last frame (`Src::Pic`), while asleep or waking.
    pub pic: Option<u32>,
}

/// An app's state, in the same words everywhere (Running, the cross, Home).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppState {
    OnScreen,
    Background,
    Starting,
    Closing,
    Asleep,
}

impl AppState {
    pub fn of(a: &RunningInfo, v: &View) -> AppState {
        if a.closing {
            AppState::Closing
        } else if a.asleep {
            AppState::Asleep
        } else if !a.has_buffer {
            AppState::Starting
        } else if v.current == Some(a.uid) {
            AppState::OnScreen
        } else {
            AppState::Background
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            AppState::OnScreen => "on screen",
            AppState::Background => "in the background",
            AppState::Starting => "starting",
            AppState::Closing => "closing",
            AppState::Asleep => "asleep",
        }
    }
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
    /// How far it is still below its place while it slides (0: up).
    pub osk_drop: f32,
    pub host: &'a str,
    /// The current time zone ("Europe/Lisbon", "UTC").
    pub timezone: &'a str,
    pub radios: Radios,
    /// Over-the-air updates (None: mortar.update is off).
    pub update: Option<&'a UpdateView>,
    /// The default sink is muted.
    pub muted: bool,
    /// Wi-Fi and Bluetooth from mortar-setupd (None: no setup service).
    pub net: Option<&'a NetState>,
    /// The running apps' PipeWire streams (the Mixer, Now playing).
    pub mixer: &'a [MixChan],
    /// mortar-play's library and the game apps' states (None: no games).
    pub games: Option<&'a crate::games::GamesView>,
    /// Home and Settings > About: attention, widgets, system facts.
    pub dash: Dash<'a>,
}

/// What the dashboard and About show.
#[derive(Clone, Copy, Debug, Default)]
pub struct Dash<'a> {
    /// "Needs you" (attention.rs), oldest first.
    pub attention: &'a [crate::attention::Item],
    /// The Home widgets (config `home.widgets`).
    pub widgets: &'a [crate::widgets::Widget],
    /// The command widgets' last results, by widget index.
    pub outputs: &'a [Option<crate::widgets::Output>],
    pub about: Option<&'a crate::about::About>,
}

/// mortar-setupd's view of Wi-Fi (iwd) and Bluetooth (BlueZ).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NetState {
    pub wifi: mortar_setup::control::WifiView,
    pub bt: mortar_setup::control::BtView,
    /// The service did not answer.
    pub error: Option<String>,
}

/// One app's sound in the Mixer: its PipeWire output streams together.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MixChan {
    pub uid: u32,
    /// Percent, on wpctl's (cubic) scale.
    pub level: u8,
    pub muted: bool,
    /// A stream is running (making sound), not idle or corked.
    pub playing: bool,
}

/// Settings > Updates, from mortar-update's status file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpdateView {
    /// Short status for the row ("Up to date", "Restart to update").
    pub message: String,
    /// Staged: restarting applies it.
    pub ready: bool,
    /// A check or download is running.
    pub busy: bool,
    pub auto_check: bool,
}

impl View<'_> {
    pub(crate) fn app(&self, uid: u32) -> Option<&RunningInfo> {
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
    /// A mortar-setup page (Wi-Fi's WPS and full list, Remote access).
    OpenSetup(SetupPage),
    /// Open Settings > Time zone (the compositor loads the zones, then
    /// calls `open_tz`).
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
    /// Save what the shell remembers (tips shown, the last app) without
    /// applying anything.
    Remember,
    /// Settings > Updates: check now.
    UpdateCheck,
    /// Settings > Auto-check daily.
    UpdateAuto(bool),
    /// A request to mortar-setupd (Wi-Fi, Bluetooth), sent off the loop.
    Setup(mortar_setup::control::Request),
    /// Join a secured network: the setup service's password keyboard.
    WifiPassword(String),
    /// The Mixer: an app's streams to a level (percent), muted or not.
    MixLevel(u32, u8),
    MixMute(u32, bool),
    /// The default sink muted or not.
    MasterMute(bool),
    /// Start a game as its own app (or switch to it if it runs).
    LaunchGame(crate::games::GameRef),
    /// A request to a game's control socket (the game panel).
    GameCtl(u32, crate::games::GameCmd),
}

/// mortar-setup's own pages the MENU opens: Wi-Fi's WPS and full list
/// ("Other network…"), and Settings > Remote access.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupPage {
    Wifi,
    Remote,
}

// ---------------------------------------------------------------- state ----

/// A zone and its current UTC offset (seconds).
#[derive(Clone, Debug, PartialEq)]
pub struct TzZone {
    pub id: String,
    pub offset: i32,
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
    /// An app grows out of its dashboard tile or MENU row (or card), over
    /// `prev` (the app that was in front, or home).
    Launch { target: Target, name: String, art: Art, from: R4, prev: Option<u32>, t0: f64 },
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OsdKind {
    Volume,
    Brightness,
}

/// The volume / brightness pill. Its life is timed, not animated: it
/// shows until `until`, fades for OSD_FADE, and is gone then whether or
/// not frames were drawn in between.
struct Osd {
    kind: OsdKind,
    value: u8,
    muted: bool,
    /// Fade and scale in (0..1).
    a: Spring,
    /// The bar (0..1), a small spring per step.
    fill: Spring,
    until: f64,
}

/// The OSD stays this long after the last change, then fades this long.
const OSD_SECS: f64 = 1.2;
const OSD_FADE: f64 = 0.2;

struct Boot {
    t0: f64,
    blocks: [(Spring, Spring, Spring); 4],
    word: Spring,
    y: Spring,
    fired: u8,
}

const BOOT_START: [(f32, f32, f32); 4] = [(-400.0, -300.0, -40.0), (1400.0, -200.0, 30.0), (-300.0, 1000.0, 25.0), (1300.0, 900.0, -35.0)];
const BOOT_END: [(f32, f32); 4] = [(362.0, 166.0), (512.0, 166.0), (362.0, 316.0), (512.0, 316.0)];

/// The charging line's highlight moves this long after the last press.
const DRIFT_SECS: f64 = 30.0;

/// How often the compositor should redraw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pace {
    /// Something moves: every vblank.
    Full,
    /// Only the charging line's slow highlight on the dashboard: ~20 fps.
    Ambient,
    /// Only the MENU's background pattern moves: ~30 fps.
    Pattern,
    /// Nothing moves: redraw on change only.
    Idle,
}

pub struct ShellUi {
    pub prefs: Prefs,
    t: f64,
    drift_t: f64,
    drift_until: f64,
    /// The dashboard is on screen and the battery charges (the charging
    /// line moves).
    charging_home: bool,
    home: home::Home,
    /// The app just sent home: its frosted picture fades out behind the
    /// stack (1 -> 0).
    leaving: Option<(u32, Spring)>,
    lp: Spring,
    /// The veil over the blurred backdrop: FROST_VEIL, raised over dark
    /// backdrops so ink-soft text keeps 4.5:1 (`set_backdrop`).
    veil: Spring,
    trans: Option<Trans>,
    menu: Option<cross::Cross>,
    /// Each MENU category's selected row (kept between openings).
    cross_sel: [usize; 5],
    page: Option<SetupUi>,
    /// The zones the compositor loaded for Settings > Time zone.
    tz_zones: Vec<TzZone>,
    sleep: Option<SleepSt>,
    power: Option<PowerMenu>,
    /// Power key held: progress to the power menu (0..1).
    power_hold: Option<f32>,
    osd: Option<Osd>,
    toast: Option<ToastState>,
    toasts: Vec<Toast>,
    flash: Option<Spring>,
    boot: Option<Boot>,
    /// The last button press (UI time), and the screensaver: wanted, how
    /// far it has grown (0..1), when it filled the screen.
    last_input: f64,
    saver_on: bool,
    saver_p: f32,
    saver_full: f64,
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
            charging_home: false,
            home: home::Home::default(),
            leaving: None,
            lp: Spring::new(0.0),
            veil: Spring::new(FROST_VEIL),
            trans: None,
            menu: None,
            cross_sel: [0; 5],
            page: None,
            tz_zones: Vec::new(),
            sleep: None,
            power: None,
            power_hold: None,
            osd: None,
            toast: None,
            toasts: Vec::new(),
            flash: None,
            boot: None,
            last_input: 0.0,
            saver_on: false,
            saver_p: 0.0,
            saver_full: 0.0,
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
            || self.page.is_some()
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

    /// The stack is on the dashboard card (the Running page, settled or
    /// on its way there, nothing over it).
    pub fn dash_selected(&self) -> bool {
        self.menu.as_ref().is_some_and(|m| !m.closing && !m.on_cross() && m.run_sel == 0 && m.ctx.is_none() && m.top().is_none() && m.gpanel.is_none())
    }

    /// In the dashboard's tiles (for `ctl state`).
    pub fn in_tiles(&self) -> bool {
        self.home.tiles
    }

    /// The dashboard shows (its command widgets run, Now playing looks at
    /// the streams).
    pub fn dashboard_visible(&self) -> bool {
        self.page.is_none() && self.sleep.is_none() && self.menu.as_ref().is_some_and(|m| !m.closing && m.page_x.x < 0.99 && m.run_scroll.x < 1.0 && m.top().is_none())
    }

    /// Settings shows the volume and brightness in its rows (no OSD).
    pub fn levels_on_screen(&self) -> bool {
        self.menu.as_ref().is_some_and(|m| !m.closing && m.on_cross() && (m.cat == Cat::Settings || m.top().is_some()))
    }

    /// The MENU's category (while it shows).
    pub fn menu_category(&self) -> Option<Cat> {
        self.menu.as_ref().filter(|m| !m.closing).map(|m| if m.on_cross() { m.cat } else { Cat::Running })
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
            || self.power_hold.is_some()
            // the OSD only while it moves (fading in or out, its bar)
            || self.osd.as_ref().is_some_and(|o| self.t > o.until || !(o.a.rest() && o.fill.rest()));
        // layers that can sit still: moving only while their springs are
        let menu_moving = self.menu.as_ref().is_some_and(|m| m.moving());
        let power_moving = self.power.as_ref().is_some_and(|p| p.closing || !p.x.rest());
        let sleep_moving = self.sleep.as_ref().is_some_and(|s| !s.a.rest());
        let springs = self.springs_ref().iter().any(|s| !s.rest());
        if timed || menu_moving || power_moving || sleep_moving || springs {
            return Pace::Full;
        }
        // the dashboard on screen and charging: the charging line's slow
        // highlight
        let _ = has_app;
        if self.power.is_none() && self.dashboard_visible() && self.drifting() {
            return Pace::Ambient;
        }
        // the screensaver growing or shrinking; full for a long while: it
        // dims and slows down
        if self.saver_p > 0.0 && (self.saver_p < 1.0 || !self.saver_on) {
            return Pace::Full;
        }
        if self.saver_on && self.saver_p >= 1.0 && !self.prefs.reduce_motion && self.t - self.saver_full > SAVER_DIM {
            return Pace::Ambient;
        }
        // the cross's pattern moves while it shows (Reduce motion: still)
        if !self.prefs.reduce_motion && self.page.is_none() && self.menu.as_ref().is_some_and(|m| m.pattern_visible()) {
            return Pace::Pattern;
        }
        Pace::Idle
    }

    fn drifting(&self) -> bool {
        self.charging_home && !self.prefs.reduce_motion && self.t < self.drift_until
    }

    /// When the next timed event is due (toast/OSD expiry, confirm reset),
    /// in UI seconds from now.
    pub fn next_deadline(&self) -> Option<f64> {
        let mut d: Option<f64> = None;
        let mut take = |t: f64| d = Some(d.map_or(t, |x: f64| x.min(t)));
        if let Some(o) = &self.osd {
            take(if self.t <= o.until { o.until - self.t } else { o.until + OSD_FADE - self.t });
        }
        if let Some(t) = &self.toast {
            take(t.until - self.t);
        }
        if let Some(at) = self.saver_due() {
            take(at - self.t);
        }
        if self.drifting() {
            take(self.drift_until - self.t);
        }
        d.map(|v| v.max(0.0))
    }

    fn springs_ref(&self) -> Vec<&Spring> {
        let mut v: Vec<&Spring> = vec![&self.lp, &self.veil];
        v.extend(self.leaving.as_ref().map(|l| &l.1));
        v
    }

    fn springs_mut(&mut self) -> Vec<&mut Spring> {
        let mut v: Vec<&mut Spring> = vec![&mut self.lp, &mut self.veil];
        v.extend(self.leaving.as_mut().map(|l| &mut l.1));
        if let Some(m) = &mut self.menu {
            v.extend(m.springs());
        }
        if let Some(p) = &mut self.power {
            v.push(&mut p.x);
        }
        if let Some(sl) = &mut self.sleep {
            v.push(&mut sl.a);
        }
        if let Some(o) = &mut self.osd {
            v.extend([&mut o.a, &mut o.fill]);
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
        self.charging_home = v.sys.charging && self.dashboard_visible();
        self.sync_home(v);
        self.ensure_home(v);
        let p = self.params();
        {
            let mut s = self.springs_mut();
            motion::step_all(&mut s, dt as f32, p);
        }
        // the pattern only while it is on screen (nothing when the MENU is shut
        // or covered)
        if self.page.is_none() {
            self.step_pattern(dt as f32);
        }
        self.step_saver(dt as f32);
        self.timers(v);
        self.tips(v);
        std::mem::take(&mut self.out)
    }

    /// Home is the stack on the dashboard: with no app in front (and none
    /// on its way), it is always there. Under the boot screen it is simply
    /// there; otherwise (the last app went, a start went to the
    /// background) it fades in like the MENU.
    fn ensure_home(&mut self, v: &View) {
        if v.current.is_none() && self.menu.is_none() && self.trans.is_none() {
            // at start (with or without the boot screen) and while dark it
            // is simply there
            let snap = self.boot.is_some() || self.sleep.is_some() || self.t < 0.25;
            self.open_home(v, snap);
        }
        if self.leaving.as_ref().is_some_and(|l| l.1.x < 0.01) {
            self.leaving = None;
            self.dirty = true;
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
        self.sync_sub();
        // power menu closed; sleep woken
        if self.power.as_ref().is_some_and(|p| p.closing && p.x.x < 0.02) {
            self.power = None;
            self.dirty = true;
        }
        if self.sleep.as_ref().is_some_and(|s| s.waking && s.a.x < 0.01) {
            self.sleep = None;
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
            None => {}
        }
        // OSD
        // OSD: hidden by the clock alone (no spring has to arrive anywhere)
        let mut drop_osd = false;
        if let Some(o) = &mut self.osd {
            if now > o.until {
                o.a.set(0.0);
            }
            drop_osd = now > o.until + OSD_FADE;
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
    }

    // ------------------------------------------------------- events ----

    pub fn play_boot(&mut self) {
        let mk = |i: usize| (Spring::new(BOOT_START[i].0), Spring::new(BOOT_START[i].1), Spring::new(BOOT_START[i].2));
        self.boot = Some(Boot { t0: self.t, blocks: [mk(0), mk(1), mk(2), mk(3)], word: Spring::new(0.0), y: Spring::new(0.0), fired: 0 });
        self.dirty = true;
    }

    /// The volume or brightness changed: show the OSD (or update it at
    /// once: the number jumps, the bar springs).
    pub fn osd(&mut self, kind: OsdKind, value: u8, muted: bool) {
        let until = self.t + OSD_SECS;
        let level = if muted { 0.0 } else { value.min(100) as f32 / 100.0 };
        let reduce = self.prefs.reduce_motion;
        match &mut self.osd {
            Some(o) if o.kind == kind => {
                o.a.set(1.0);
                o.until = until;
                o.value = value;
                o.muted = muted;
                o.fill.set(level);
            }
            _ => {
                let mut a = Spring::new(0.0);
                a.k_mul = 3.5; // in and out in ~120 ms
                a.set(1.0);
                let mut fill = Spring::new(level);
                fill.k_mul = 2.0;
                if reduce {
                    a.snap(1.0);
                }
                self.osd = Some(Osd { kind, value, muted, a, fill, until });
            }
        }
        self.dirty = true;
    }

    pub fn osd_kind(&self) -> Option<OsdKind> {
        self.osd.as_ref().map(|o| o.kind)
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
        self.last_input = self.t;
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
        if self.saver_wake() {
            return std::mem::take(&mut self.out);
        }
        if self.page.is_some() {
            self.page_button(b, v);
        } else if self.menu.is_some() {
            self.menu_button(b, repeat, v);
        } else if self.trans.is_some() {
            self.trans_button(b, v);
        } else if v.current.is_none() {
            // home before the stack came up (it always does)
            self.open_home(v, true);
            self.menu_button(b, repeat, v);
        }
        std::mem::take(&mut self.out)
    }

    /// MENU (on its own) opens the MENU over the app, or closes it and
    /// whatever was opened from it; at home it comes back to the
    /// dashboard.
    pub fn menu_key(&mut self, v: &View) -> Vec<Out> {
        self.drift_until = self.t + DRIFT_SECS;
        self.last_input = self.t;
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
        if self.saver_wake() {
            return std::mem::take(&mut self.out);
        }
        // everything closes at once: a setup page, the glass pages, the MENU
        let sub = self.page.take().is_some();
        if sub || self.menu.as_ref().is_some_and(|m| !m.closing) {
            self.leave_menu(v);
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

    /// A on the dashboard card over an app: home. The app goes to the
    /// background (its card stays in the stack, right below), its frosted
    /// picture fades out behind the stack, and the stack stays.
    fn go_home(&mut self, v: &View) {
        let Some(cur) = v.current else { return };
        let mut s = Spring::new(1.0);
        s.set(0.0);
        if self.prefs.reduce_motion {
            s.snap(0.0);
        }
        self.leaving = v.app(cur).filter(|a| a.has_buffer).map(|_| (cur, s));
        self.out.push(Out::Home);
        self.fx(Sfx::Close, Some(Haptic::Tick));
    }


    /// An app asked for attention (attention.rs): one toast when it is
    /// raised, never for updates of its text.
    pub fn attention_raised(&mut self, name: &str, text: &str) {
        let mut t = Toast::info(format!("{name} needs you"));
        t.sub = text.to_string();
        t.secs = 3.6;
        t.haptic = Some(Haptic::Double);
        self.show_toast(t);
    }
}

fn target_uid(t: &Target, v: &View) -> Option<u32> {
    match t {
        Target::Uid(u) => Some(*u),
        Target::Entry(id) => v.by_entry(id).map(|a| a.uid),
    }
}

// ---------------------------------------------------------------- drawing ----

fn with_a(c: Color, a: f32) -> Color {
    Color(c.0, c.1, c.2, (a.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// Points along a circle from `a0` to `a1` degrees (y down).
fn arc_pts(cx: f32, cy: f32, r: f32, a0: f32, a1: f32) -> Vec<P> {
    let n = ((a1 - a0).abs() / 8.0).ceil().max(2.0) as usize;
    (0..=n)
        .map(|i| {
            let a = (a0 + (a1 - a0) * i as f32 / n as f32).to_radians();
            (cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}

/// A rounded rectangle as horizontal spans (y0, y1, inset from each
/// side), `n` per rounded end: an image cut to its shape.
fn rrect_spans(y: f32, w: f32, h: f32, r: f32, n: usize) -> Vec<(f32, f32, f32)> {
    let r = r.min(w / 2.0).min(h / 2.0);
    let inset = |d: f32| r - (r * r - d * d).max(0.0).sqrt();
    let mut v = Vec::with_capacity(2 * n + 1);
    for i in 0..n {
        let (y0, y1) = (y + r * i as f32 / n as f32, y + r * (i + 1) as f32 / n as f32);
        v.push((y0, y1, inset(r - ((y0 + y1) / 2.0 - y))));
    }
    v.push((y + r, y + h - r, 0.0));
    for i in 0..n {
        let (y0, y1) = (y + h - r + r * i as f32 / n as f32, y + h - r + r * (i + 1) as f32 / n as f32);
        v.push((y0, y1, inset((y0 + y1) / 2.0 - (y + h - r))));
    }
    v
}

/// The OSD's speaker: waves by level, crossed out when muted (None).
fn osd_speaker(b: &mut B, cx: f32, cy: f32, ink: Color, level: Option<u8>) {
    let body = [(cx - 13.0, cy - 5.0), (cx - 6.0, cy - 5.0), (cx + 3.0, cy - 13.0), (cx + 3.0, cy + 13.0), (cx - 6.0, cy + 5.0), (cx - 13.0, cy + 5.0)];
    b.poly_concave(&body, ink);
    match level {
        None => {
            b.stroke(&[(cx + 8.0, cy - 6.0), (cx + 18.0, cy + 6.0)], false, 2.2, ink);
            b.stroke(&[(cx + 18.0, cy - 6.0), (cx + 8.0, cy + 6.0)], false, 2.2, ink);
        }
        Some(l) => {
            if l > 0 {
                b.stroke(&arc_pts(cx + 3.0, cy, 7.0, -45.0, 45.0), false, 2.2, ink);
            }
            if l >= 50 {
                b.stroke(&arc_pts(cx + 3.0, cy, 13.0, -45.0, 45.0), false, 2.2, ink);
            }
        }
    }
}

fn osd_sun(b: &mut B, cx: f32, cy: f32, ink: Color) {
    let c = crate::scene::ellipse_pts(cx, cy, 6.5, 6.5, 28);
    b.stroke(&c, true, 2.2, ink);
    for k in 0..8 {
        let t = (k as f32 * 45.0).to_radians();
        let (co, si) = (t.cos(), t.sin());
        b.stroke(&[(cx + co * 11.0, cy + si * 11.0), (cx + co * 15.5, cy + si * 15.5)], false, 2.2, ink);
    }
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

/// The status bar's battery level: plain text (pushed onto `net`), a lime
/// chip with a bolt while charging, yellow at 15 % and orange at 5 % (the
/// warning levels). Returns the new right edge.
fn battery_status(b: &mut B, pal: &Pal, v: &View, mut right: f32, net: &mut Vec<String>) -> f32 {
    if let Some(bat) = v.sys.battery {
        let txt = format!("{bat}%");
        if v.sys.charging {
            let w = chip(b, right, 28.0, &txt, 15.0, (9.0, 2.0), pal.lime, pal.lime_ink);
            b.poly_concave(&bolt_pts(right - w - 11.0, 28.0, 18.0), pal.ink);
            right -= w + 32.0;
        } else if bat <= 15 {
            let fill = if bat <= 5 { pal.orange } else { pal.yellow };
            right -= chip(b, right, 28.0, &txt, 15.0, (9.0, 2.0), fill, pal.dark) + 10.0;
        } else {
            net.push(txt);
        }
    }
    right
}

/// A lightning bolt (the charging mark), `h` tall, centred on (cx, cy);
/// the same shape as the toolkit's BatCharge icon.
fn bolt_pts(cx: f32, cy: f32, h: f32) -> Vec<P> {
    [(12.4, 8.6), (7.6, 12.6), (11.2, 12.6), (9.6, 15.4), (14.4, 11.4), (10.8, 11.4)]
        .iter()
        .map(|(x, y)| (cx + (x - 11.0) / 6.8 * h, cy + (y - 12.0) / 6.8 * h))
        .collect()
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
            None => v.current,
        };
        // the app behind the MENU or the power menu: frosted (blurred
        // under a light veil) as they open (an app still starting has
        // nothing to blur; at home the stack sits on the plain ground)
        let frost_app = base_app.and_then(|u| v.app(u)).filter(|a| a.has_buffer).map(|a| (a.uid, a.geom));
        let backdrop = frost_app.map(|(uid, g)| (Src::Blur(uid), g));
        let frost = if backdrop.is_some() { self.frost() } else { 0.0 };
        match base_app {
            // fully frosted: the sharp screen would be drawn for nothing
            _ if frost >= 0.999 => b.rect(0.0, 0.0, W, H, Color(0, 0, 0, 255)),
            Some(uid) => self.draw_app(&mut b, v, uid, true),
            None => b.rect(0.0, 0.0, W, H, pal.ground),
        }
        if let Some((src, g)) = backdrop
            && frost > 0.0
        {
            b.push_alpha(frost);
            b.image(src, g[0], g[1], g[2], g[3], [0.0, 0.0, 1.0, 1.0]);
            b.pop_alpha();
            b.rect(0.0, 0.0, W, H, with_a(pal.ground, frost * self.veil.x));
        }
        // the app just sent home: its frosted picture fades out
        if base_app.is_none()
            && let Some((uid, s)) = &self.leaving
            && let Some(a) = v.app(*uid).filter(|a| a.has_buffer)
        {
            let k = s.x.clamp(0.0, 1.0);
            let g = a.geom;
            b.push_alpha(k);
            b.image(Src::Blur(*uid), g[0], g[1], g[2], g[3], [0.0, 0.0, 1.0, 1.0]);
            b.rect(0.0, 0.0, W, H, with_a(pal.ground, self.veil.x));
            b.pop_alpha();
        }
        // the MENU fades out under the transition it started
        if let Some(m) = &self.menu {
            let g = Glass { src: backdrop.filter(|_| frost > 0.0), veil: frost * self.veil.x };
            self.draw_menu(&mut b, &pal, v, m, &g);
        }
        self.draw_trans(&mut b, &pal, v);
        if self.page.is_some() {
            b.image(Src::Layer(Layer::Page), 0.0, 0.0, W, H, [0.0, 0.0, 1.0, 1.0]);
        }
        if let Some(p) = &self.power {
            self.draw_power(&mut b, &pal, p, frost > 0.0);
        }
        if let Some(o) = &self.osd {
            self.draw_osd(&mut b, &pal, o, if frost > 0.0 { None } else { frost_app });
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

    /// The blurred backdrop's mean colour (sRGB 0..1, from the renderer's
    /// quarter-size blur; None: not measured). The veil eases towards what
    /// keeps ink-soft legible over it; with nothing frosted on screen (or
    /// Reduce motion) it moves at once, so opening the MENU never shows it
    /// sliding.
    pub fn set_backdrop(&mut self, mean: Option<[f32; 3]>) {
        let t = mean.map(|m| veil_for(m, &self.pal())).unwrap_or(FROST_VEIL);
        if (t - self.veil.t).abs() < 0.005 {
            return;
        }
        if self.prefs.reduce_motion || self.frost() <= 0.0 {
            self.veil.snap(t);
        } else {
            self.veil.k_mul = 0.5;
            self.veil.set(t);
        }
        self.dirty = true;
    }

    /// The screensaver may come: the cross shows on its own (not Running,
    /// not a glass page or popup, nothing over it) and it is not off.
    fn saver_allowed(&self) -> bool {
        self.prefs.screensaver > 0
            && self.boot.is_none()
            && self.sleep.is_none()
            && self.power.is_none()
            && self.trans.is_none()
            && self.page.is_none()
            && self.menu.as_ref().is_some_and(|m| m.saver_ok())
    }

    /// When the screensaver is due (UI time), while it may come.
    fn saver_due(&self) -> Option<f64> {
        (self.saver_allowed() && !self.saver_on).then(|| self.last_input + self.prefs.screensaver as f64)
    }

    /// Grow the screensaver after the idle delay; shrink it when it may no
    /// longer show.
    fn step_saver(&mut self, dt: f32) {
        let allowed = self.saver_allowed();
        if !allowed {
            self.saver_on = false;
        } else if !self.saver_on && self.t >= self.last_input + self.prefs.screensaver as f64 {
            self.saver_on = true;
            self.saver_full = self.t + SAVER_GROW as f64;
        }
        let target = if self.saver_on { 1.0 } else { 0.0 };
        let was = self.saver_p;
        self.saver_p = if self.prefs.reduce_motion || !allowed && target == 0.0 && self.menu.is_none() {
            target
        } else if self.saver_on {
            (self.saver_p + dt / SAVER_GROW).min(1.0)
        } else {
            (self.saver_p - dt / SAVER_GROW).max(0.0)
        };
        if self.saver_p != was {
            self.dirty = true;
        }
    }

    /// A press while the screensaver shows: it shrinks back, and the press
    /// does nothing else.
    fn saver_wake(&mut self) -> bool {
        if !self.saver_on && self.saver_p <= 0.0 {
            return false;
        }
        self.saver_on = false;
        self.fx(Sfx::Tab, Some(Haptic::Tick));
        true
    }

    /// How far the screensaver has grown (0: the strip; 1: the screen),
    /// and how dim it is (after a long while).
    pub(super) fn saver(&self) -> (f32, f32) {
        let e = self.saver_p * self.saver_p * (3.0 - 2.0 * self.saver_p);
        let dim = if self.saver_p >= 1.0 { ((self.t - self.saver_full - SAVER_DIM) / 8.0).clamp(0.0, 1.0) as f32 * 0.35 } else { 0.0 };
        (e, dim)
    }

    /// Put the veil at its target now (previews).
    pub fn snap_veil(&mut self) {
        self.veil.snap(self.veil.t);
    }

    /// How much of the frosted backdrop shows: it follows the MENU's (or
    /// the power menu's) open spring; with Reduce motion it is simply on
    /// while they show. None while a full-screen panel hides the app.
    fn frost(&self) -> f32 {
        if self.page.is_some() {
            return 0.0;
        }
        let menu = self.menu.as_ref().map(|m| m.open());
        let power = self.power.as_ref().map(|p| p.x.x.clamp(0.0, 1.0));
        if menu.is_none() && power.is_none() {
            return 0.0;
        }
        if self.prefs.reduce_motion {
            return 1.0;
        }
        menu.unwrap_or(0.0).max(power.unwrap_or(0.0))
    }

    /// The app in its on-screen place (black around a smaller window), plus
    /// the keyboard strip.
    fn draw_app(&self, b: &mut B, v: &View, uid: u32, osk: bool) {
        b.rect(0.0, 0.0, W, H, Color(0, 0, 0, 255));
        if let Some(a) = v.app(uid) {
            if a.has_buffer {
                let g = a.geom;
                b.image(Src::App(uid), g[0], g[1], g[2], g[3], [0.0, 0.0, 1.0, 1.0]);
                // a keyboard app shorter than the screen (the keyboard is
                // sliding, or it closed and the app has not drawn at full
                // height yet): its bottom row continues down, not black
                let bottom = g[1] + g[3];
                if v.keyboard_app && v.current == Some(uid) && g[1] <= 0.5 && g[3] >= 1.0 && bottom < H - 0.5 {
                    b.image(Src::App(uid), g[0], bottom, g[2], H - bottom, [0.0, 1.0 - 1.0 / g[3], 1.0, 1.0]);
                }
            } else {
                self.splash(b, &self.pal(), a.art, &a.name, 0.0);
            }
        }
        if osk && v.osk_h > 0.0 && v.current == Some(uid) {
            b.image(Src::Layer(Layer::Osk), 0.0, H - v.osk_h + v.osk_drop, W, v.osk_h, [0.0, 0.0, 1.0, 1.0]);
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
                        // a game waking from sleep: its last frame while it
                        // restarts, so the restart does not show
                        match target_uid(target, v).and_then(|u| v.app(u)).and_then(|a| a.pic) {
                            Some(pic) => b.image(Src::Pic(pic), 0.0, 0.0, W, H, [0.0, 0.0, 1.0, 1.0]),
                            None => self.splash(b, pal, *art, name, 0.0),
                        }
                        b.pop_alpha();
                    }
                    None => {}
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

    fn draw_power(&self, b: &mut B, pal: &Pal, p: &PowerMenu, frosted: bool) {
        let x = p.x.x.clamp(0.0, 1.2);
        // over a frosted app a lighter scrim is enough
        let scrim = if frosted { 0.2 } else { 0.5 };
        b.rect(0.0, 0.0, W, H, Color(29, 29, 29, (scrim * x.min(1.0) * 255.0) as u8));
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

    /// The OSD: a frosted-glass pill at the right edge (style of
    /// plan/mortar-cross-2: light ground at 42% over a blur of the app,
    /// a light top edge, ink content, no hard border). `app`: the app
    /// behind (uid, geometry) to blur; None: a flat 85% ground.
    fn draw_osd(&self, b: &mut B, pal: &Pal, o: &Osd, app: Option<(u32, [f32; 4])>) {
        let a = o.a.x.clamp(0.0, 1.0);
        if a < 0.01 {
            return;
        }
        let (w, h) = (72.0, 300.0);
        let (x, y) = (W - 24.0 - w, (H - h) / 2.0);
        let r = w / 2.0;
        let s = 0.9 + 0.1 * a;
        b.push(Xf::about(x + w / 2.0, y + h / 2.0, Xf::scale(s, s)));
        b.push_alpha(a);
        match app {
            Some((uid, g)) if g[2] > 0.0 && g[3] > 0.0 => {
                // the blurred app, cut to the pill in horizontal spans
                for (y0, y1, inset) in rrect_spans(y, w, h, r, 10) {
                    let (x0, x1) = (x + inset, x + w - inset);
                    let uv = [(x0 - g[0]) / g[2], (y0 - g[1]) / g[3], (x1 - g[0]) / g[2], (y1 - g[1]) / g[3]];
                    let uv = uv.map(|c| c.clamp(0.0, 1.0));
                    b.image(Src::Blur(uid), x0, y0, x1 - x0, y1 - y0, uv);
                }
                b.rrect(x, y, w, h, r, with_a(pal.ground, 0.42));
            }
            _ => b.rrect(x, y, w, h, r, with_a(pal.ground, 0.85)),
        }
        // a faint outline so it holds over light apps, a light top edge,
        // a faint shade under the bottom
        b.stroke(&crate::scene::rrect_pts(x, y, w, h, r), true, 1.0, with_a(pal.ink, 0.12));
        let top = arc_pts(x + r, y + r, r - 0.5, 180.0, 360.0);
        b.stroke(&top, false, 1.5, Color(255, 255, 255, 190));
        let bottom = arc_pts(x + r, y + h - r, r - 0.5, 0.0, 180.0);
        b.stroke(&bottom, false, 1.0, with_a(pal.ink, 0.15));
        // icon, bar, number
        let cx = x + w / 2.0;
        let ink = pal.ink;
        match o.kind {
            OsdKind::Volume => osd_speaker(b, cx, y + 40.0, ink, if o.muted { None } else { Some(o.value) }),
            OsdKind::Brightness => osd_sun(b, cx, y + 40.0, ink),
        }
        let (bx, by, bw, bh) = (cx - 7.0, y + 76.0, 14.0, 172.0);
        b.rect(bx, by, bw, bh, with_a(ink, 0.14));
        let fh = (o.fill.x.clamp(0.0, 1.0) * bh).round();
        if fh > 0.0 {
            b.rect(bx, by + bh - fh, bw, fh, ink);
        }
        let n = ts(Face::Mono600, 18.0, ink);
        let label = if o.muted { "off".to_string() } else { o.value.to_string() };
        b.text_center(cx, y + h - 40.0, &label, &n);
        b.pop_alpha();
        b.pop();
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
        RunningInfo { uid, entry: Some(entry.into()), name: entry.into(), art: Art::Generic, has_buffer: true, closing: false, geom: [0.0, 0.0, W, H], game: false, asleep: false, pic: None }
    }
    /// Prefs with every tip already shown (no tip toasts in the way).
    fn quiet() -> Prefs {
        Prefs { tips_seen: ["menu", "keyboard", "game", "controls"].map(String::from).to_vec(), ..Prefs::default() }
    }
    fn launcher() -> Vec<LauncherInfo> {
        [("term", Category::Apps), ("files", Category::Apps), ("m8", Category::Music), ("pico", Category::Games)]
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
            osk_drop: 0.0,
            host: "brick",
            timezone: "Europe/Lisbon",
            radios: Radios { wifi: Some(true), bluetooth: None },
            update: None,
            muted: false,
            net: None,
            mixer: &[],
            games: None,
            dash: Dash::default(),
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
        m.sel_key(ui.cross_sel[m.cat.i()])
    }
    /// MENU, then R1: the cross on Settings.
    fn to_quick(ui: &mut ShellUi, v: &View) {
        ui.menu_key(v);
        press(ui, v, &[Button::R1]);
        assert_eq!(ui.menu_category(), Some(Cat::Settings));
    }
    /// MENU > Settings > a page (its row, then A).
    fn to_page(ui: &mut ShellUi, v: &View, k: SubKind) -> Vec<Out> {
        to_quick(ui, v);
        let i = cross::settings_rows(v).iter().position(|r| *r == cross::Row::Page(k)).expect("no such row");
        press(ui, v, &vec![Button::Up; 12]);
        press(ui, v, &vec![Button::Down; i]);
        assert_eq!(sel_row(ui), Some(cross::Row::Page(k)));
        let out = press(ui, v, &[Button::A]);
        if k != SubKind::TimeZone {
            assert_eq!(ui.menu_sub(), Some(k));
        }
        out
    }
    /// Where the UI is, for the B / MENU table.
    fn place(ui: &ShellUi) -> String {
        if ui.power_menu_open() {
            return if ui.power.as_ref().is_some_and(|p| p.confirm.is_some()) { "power-menu:confirm".into() } else { "power-menu".into() };
        }
        if ui.page.is_some() {
            return "setup-page".into();
        }
        let Some(m) = ui.menu.as_ref().filter(|m| !m.closing) else { return "closed".into() };
        if let Some(u) = ui.game_panel() {
            return format!("game-panel:{u}");
        }
        if let Some(sys) = ui.game_page() {
            return format!("game-page:{sys}");
        }
        if m.ctx.is_some() {
            return "options".into();
        }
        let pages: Vec<&str> = ui.menu_pages().iter().map(|k| k.name()).collect();
        if !pages.is_empty() {
            let top = m.top().unwrap();
            let extra = if top.adjusting() { ":adjust" } else { "" };
            return format!("page:{}{extra}", pages.join("/"));
        }
        match ui.menu_category() {
            Some(Cat::Running) if ui.in_tiles() => "tiles".into(),
            Some(Cat::Running) if ui.dash_selected() => "dashboard".into(),
            Some(Cat::Running) => "running".into(),
            Some(c) => format!("cross:{}{}", c.name(), if m.adjusting { ":adjust" } else { "" }),
            None => "closed".into(),
        }
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
    fn home_is_a_dashboard() {
        let sys = Sys::demo(None);
        let l = launcher();
        let ws = crate::widgets::defaults();
        let dash = Dash { widgets: &ws, ..Dash::default() };
        let v = View { dash, ..view(&[], &l, &sys, None) };
        let mut ui = ShellUi::new(quiet(), false);
        settle(&mut ui, &v);
        // home is the stack on the dashboard card, at rest
        assert!(ui.dash_selected() && ui.dashboard_visible());
        assert_eq!(ui.menu_category(), Some(Cat::Running));
        assert_eq!(ui.pace(false), Pace::Idle, "a still screen: nothing drifts");
        // nothing to do yet: A does nothing (it is not a launcher), B and
        // MENU stay; ▶ is the way to the apps
        assert!(press(&mut ui, &v, &[Button::A, Button::B, Button::Down]).is_empty());
        assert!(actions(&ui.menu_key(&v)).is_empty());
        assert!(ui.dash_selected() && !ui.in_tiles() && ui.menu_open());
        let m = ui.menu.as_ref().unwrap();
        assert_eq!(ui.menu_hints(m, &v), vec![("▶", "Apps")]);
        // Recently closed: A enters the tiles, A starts it again, growing
        // from its tile inside the card
        ui.prefs.recent = vec!["m8".into()];
        settle(&mut ui, &v);
        press(&mut ui, &v, &[Button::A]);
        assert!(ui.in_tiles());
        assert_eq!(ui.tiles_hints(&v), vec![("◀▶▲▼", "Choose"), ("A", "Open"), ("B", "Back")]);
        // B leaves the tiles, A goes back in
        press(&mut ui, &v, &[Button::B]);
        assert!(!ui.in_tiles() && ui.dash_selected());
        press(&mut ui, &v, &[Button::A]);
        assert_eq!(press(&mut ui, &v, &[Button::A]), vec![Out::Launch("m8".into())]);
        assert!(matches!(&ui.trans, Some(Trans::Launch { from, .. }) if from.y >= cross::RUN_Y && from.w < W));
        assert!(!ui.menu_open(), "the stack fades under the launch");
        // apps running (in the background): the stack shows them below the
        // dashboard; ▼ A switches to the last one
        let r = [run(5, "m8"), run(6, "term")];
        let v2 = View { dash, ..view(&r, &l, &sys, None) };
        let mut ui = ShellUi::new(quiet(), false);
        settle(&mut ui, &v2);
        assert!(ui.dash_selected());
        assert!(ui.live_previews().is_empty(), "the cards next to the dashboard are tabs: nothing live, no frames");
        assert_eq!(ui.pace(false), Pace::Idle);
        press(&mut ui, &v2, &[Button::Down]);
        assert_eq!(sel_row(&ui), Some(cross::Row::App(5)));
        // MENU (or B) at home: back to the dashboard
        assert!(actions(&ui.menu_key(&v2)).is_empty());
        assert!(ui.dash_selected() && ui.menu_open());
        press(&mut ui, &v2, &[Button::Down]);
        assert_eq!(press(&mut ui, &v2, &[Button::A]), vec![Out::Focus(5)]);
        // Needs you: A into the tiles (on Needs you first), A switches to it
        let att = [crate::attention::Item { app: "term".into(), text: "Approve: ls".into(), seq: 1 }];
        let v3 = View { dash: Dash { attention: &att, ..dash }, ..view(&r, &l, &sys, None) };
        let mut ui = ShellUi::new(quiet(), false);
        settle(&mut ui, &v3);
        press(&mut ui, &v3, &[Button::A]);
        assert_eq!(press(&mut ui, &v3, &[Button::A]), vec![Out::Focus(6)]);
        // the app on screen is remembered for Recently closed
        let v4 = View { dash, ..view(&r, &l, &sys, Some(6)) };
        let out = ui.step(0.01, &v4);
        assert_eq!(ui.prefs.recent.first().map(String::as_str), Some("term"));
        assert!(out.contains(&Out::Remember));
        // R1 from home: Settings; B comes back to the dashboard
        let mut ui = ShellUi::new(quiet(), false);
        settle(&mut ui, &v);
        press(&mut ui, &v, &[Button::R1]);
        assert_eq!(ui.menu_category(), Some(Cat::Settings));
        press(&mut ui, &v, &[Button::B]);
        assert!(ui.dash_selected() && ui.menu_open());
    }

    /// Home at rest draws the same frame (nothing to redraw), and a widget
    /// update changes only its own tile.
    #[test]
    fn idle_dashboard_damages_nothing_and_a_widget_its_tile() {
        // needs the fonts (baked into the package build; else skipped)
        let Ok(fonts) = mortar_ui::gfx::Fonts::load(&mortar_ui::gfx::font_dir()) else { return };
        let typo = Typo::new(fonts);
        let sys = Sys::demo(Some("08:42".into()));
        let l = launcher();
        let r = [run(5, "m8"), run(6, "term")];
        let mut ws = crate::widgets::defaults();
        ws.push(crate::widgets::Widget { title: Some("Up".into()), command: vec!["true".into()], ..crate::widgets::Widget::new(crate::widgets::Kind::Command) });
        let mut outs: Vec<Option<crate::widgets::Output>> = vec![None; ws.len()];
        outs[7] = Some(crate::widgets::parse_output("3 days"));
        let mk = |o: &[Option<crate::widgets::Output>]| -> Scene {
            let v = View { dash: Dash { widgets: &ws, outputs: o, ..Dash::default() }, ..view(&r, &l, &sys, None) };
            let mut ui = ShellUi::new(quiet(), false);
            settle(&mut ui, &v);
            assert_eq!(ui.pace(false), Pace::Idle);
            assert!(ui.dashboard_visible() && ui.live_previews().is_empty());
            let s = ui.build(&typo, &v, W, H);
            assert!(s.apps().is_empty(), "no live app picture at rest: an app's frames do not redraw home");
            s
        };
        let infos = |s: &Scene| {
            let mut ink = |t: &crate::scene::TextRun| {
                let lay = typo.layout(&t.s, t.face, t.size, t.tracking);
                let end = lay.last().map_or(0.0, |g| g.1) + t.size;
                Some([t.x - 2.0, t.base - t.size * 1.2, t.x + end, t.base + t.size * 0.4])
            };
            let mut out = Vec::new();
            crate::damage::infos(s, &mut ink, &mut out);
            out
        };
        let a = infos(&mk(&outs));
        let mut g = crate::damage::Grid::new(W, H);
        g.diff(&a, &infos(&mk(&outs)));
        assert_eq!(g.rects(), Some(Vec::new()), "the same frame: nothing to redraw");
        // the 1x1 command tile (row 2, column 3) gets a new value
        outs[7] = Some(crate::widgets::parse_output("4 days"));
        let mut g = crate::damage::Grid::new(W, H);
        g.diff(&a, &infos(&mk(&outs)));
        let rects = g.rects().expect("a small change");
        assert!(!rects.is_empty());
        let slot = crate::widgets::layout(&ws).into_iter().find(|s| s.i == 7).unwrap();
        assert_eq!((slot.row, slot.col), (2, 3));
        let tile_x = W - 40.0 - (W - 80.0 - 48.0) / 4.0;
        for rc in rects {
            assert!(rc[0] >= tile_x - 32.0 && rc[1] >= cross::RUN_Y + 200.0 && rc[1] + rc[3] <= cross::RUN_Y + cross::RUN_H + 32.0, "outside the tile: {rc:?}");
        }
    }

    #[test]
    fn home_comes_back_by_itself() {
        let sys = Sys::demo(None);
        let l = launcher();
        // the boot screen slides up off the dashboard
        let v = view(&[], &l, &sys, None);
        let mut ui = ShellUi::new(quiet(), true);
        ui.step(0.01, &v);
        assert!(ui.booting() && ui.dash_selected());
        // the last app exits: home comes back
        let r = [run(2, "term")];
        let v1 = view(&r, &l, &sys, Some(2));
        let mut ui = ShellUi::new(quiet(), false);
        settle(&mut ui, &v1);
        assert!(ui.menu.is_none());
        settle(&mut ui, &v);
        assert!(ui.dash_selected());
        // an app shown by itself (over SSH) while home rests: it shows
        let v2 = view(&r, &l, &sys, Some(2));
        settle(&mut ui, &v2);
        assert!(ui.menu.is_none());
    }

    #[test]
    fn menu_a_switches_to_previous_app() {
        let sys = Sys::demo(None);
        let l = launcher();
        let r = [run(2, "term"), run(1, "m8")];
        let v = view(&r, &l, &sys, Some(2));
        let mut ui = ShellUi::new(quiet(), false);
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
        let mut ui = ShellUi::new(quiet(), false);
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
    fn dashboard_card_goes_home() {
        let sys = Sys::demo(None);
        let l = launcher();
        let r = [run(2, "term")];
        let v = view(&r, &l, &sys, Some(2));
        let mut ui = ShellUi::new(quiet(), false);
        ui.menu_key(&v);
        settle(&mut ui, &v);
        // the one app, the dashboard above it
        assert_eq!(sel_row(&ui), Some(cross::Row::App(2)));
        ui.button(Button::Up, false, &v);
        assert_eq!(sel_row(&ui), Some(cross::Row::Home));
        let m = ui.menu.as_ref().unwrap();
        assert!(ui.menu_hints(m, &v).contains(&("A", "Go home")));
        // A: the app goes to the background; the stack stays, its frosted
        // picture fades out behind it
        assert_eq!(actions(&ui.button(Button::A, false, &v)), vec![Out::Home]);
        assert!(ui.menu_open() && ui.leaving.is_some() && ui.trans.is_none());
        let home = view(&r, &l, &sys, None);
        settle(&mut ui, &home);
        assert!(ui.dash_selected() && ui.menu_open() && ui.leaving.is_none());
        // its card is right below; MENU now stays home
        press(&mut ui, &home, &[Button::Down]);
        assert_eq!(sel_row(&ui), Some(cross::Row::App(2)));
        ui.menu_key(&home);
        assert!(ui.dash_selected() && ui.menu_open());
    }

    #[test]
    fn categories_remember_their_selection() {
        let sys = Sys::demo(None);
        let l = launcher();
        let v = view(&[], &l, &sys, None);
        let mut ui = ShellUi::new(quiet(), false);
        ui.menu_key(&v);
        assert_eq!(cross::cats(&v), vec![Cat::Apps, Cat::Games, Cat::Music, Cat::Settings]);
        // Apps lists tools only: games and music apps are not repeated there
        assert_eq!(cross::rows(Cat::Apps, &v), vec![cross::Row::Launch("term".into()), cross::Row::Launch("files".into())]);
        // ▶ from Running: the cross on Apps
        press(&mut ui, &v, &[Button::Right, Button::Down]);
        assert_eq!(ui.menu_category(), Some(Cat::Apps));
        assert_eq!(sel_row(&ui), Some(cross::Row::Launch("files".into())));
        press(&mut ui, &v, &[Button::Right]);
        assert_eq!(ui.menu_category(), Some(Cat::Games));
        assert_eq!(sel_row(&ui), Some(cross::Row::Launch("pico".into())));
        // Music starts with the Mixer
        press(&mut ui, &v, &[Button::Right]);
        assert_eq!(sel_row(&ui), Some(cross::Row::Mixer));
        press(&mut ui, &v, &[Button::R1]);
        assert_eq!(ui.menu_category(), Some(Cat::Settings));
        press(&mut ui, &v, &[Button::L1]);
        assert_eq!(ui.menu_category(), Some(Cat::Running));
        // ◀ from Apps goes back to Running too; ◀ on Running goes nowhere
        press(&mut ui, &v, &[Button::Right, Button::Left]);
        assert_eq!(ui.menu_category(), Some(Cat::Running));
        assert!(press(&mut ui, &v, &[Button::Left, Button::L1]).is_empty());
        assert_eq!(ui.menu_category(), Some(Cat::Running));
        // back home and out again: Apps is still on its second row
        press(&mut ui, &v, &[Button::Right]);
        ui.menu_key(&v);
        settle(&mut ui, &v);
        assert!(ui.dash_selected());
        press(&mut ui, &v, &[Button::Right]);
        assert_eq!(sel_row(&ui), Some(cross::Row::Launch("files".into())));
        assert_eq!(press(&mut ui, &v, &[Button::A]), vec![Out::Launch("files".into())]);
        // Games only shows with something in it; Music always has the Mixer
        let l2: Vec<LauncherInfo> = l.into_iter().filter(|x| x.category == Category::Apps).collect();
        let v2 = view(&[], &l2, &sys, None);
        assert_eq!(cross::cats(&v2), vec![Cat::Apps, Cat::Music, Cat::Settings]);
    }

    #[test]
    fn running_page_scrolls_and_flips() {
        let sys = Sys::demo(None);
        let l = launcher();
        let r = [run(2, "term"), run(1, "m8")];
        let v = view(&r, &l, &sys, Some(2));
        let mut ui = ShellUi::new(quiet(), false);
        ui.menu_key(&v);
        // Home, the app on screen, the previous one (selected)
        assert_eq!(sel_row(&ui), Some(cross::Row::App(1)));
        assert_eq!(ui.live_previews(), vec![2, 1], "the selected window and its neighbour run live");
        assert!(press(&mut ui, &v, &[Button::Down]).is_empty(), "the end of the stack");
        press(&mut ui, &v, &[Button::Up]);
        assert_eq!(sel_row(&ui), Some(cross::Row::App(2)));
        press(&mut ui, &v, &[Button::Up]);
        assert_eq!(sel_row(&ui), Some(cross::Row::Home));
        // R1 from Running: straight to Settings; its pattern shows and moves
        press(&mut ui, &v, &[Button::R1]);
        assert_eq!(ui.menu_category(), Some(Cat::Settings));
        settle(&mut ui, &v);
        assert!(ui.live_previews().is_empty(), "no live windows once the cross covers them");
        assert_eq!(ui.pace(true), Pace::Pattern);
        let before = ui.menu.as_ref().unwrap().patterns.t;
        ui.step(0.03, &v);
        assert!(ui.menu.as_ref().unwrap().patterns.t > before, "it moves");
        // ▲▼ nudge it; a category change slides the old one out
        let n0 = ui.menu.as_ref().unwrap().patterns.nudge.t;
        press(&mut ui, &v, &[Button::Down]);
        assert!(ui.menu.as_ref().unwrap().patterns.nudge.t > n0);
        press(&mut ui, &v, &[Button::Left]);
        assert_eq!(ui.pace(true), Pace::Full, "the patterns slide");
        settle(&mut ui, &v);
        press(&mut ui, &v, &[Button::Right]);
        settle(&mut ui, &v);
        // Reduce motion freezes it, and nothing redraws
        ui.prefs.reduce_motion = true;
        let still = ui.menu.as_ref().unwrap().patterns.t;
        ui.step(0.03, &v);
        assert_eq!(ui.menu.as_ref().unwrap().patterns.t, still);
        assert_eq!(ui.pace(true), Pace::Idle);
        ui.prefs.reduce_motion = false;
        // closed: gone, nothing simulated
        ui.menu_key(&v);
        settle(&mut ui, &v);
        assert!(ui.menu.is_none());
        assert_eq!(ui.pace(true), Pace::Idle);
    }

    #[test]
    fn launching_a_running_app_switches_to_it() {
        let sys = Sys::demo(None);
        let l = launcher();
        let r = [run(2, "term"), run(1, "m8")];
        let v = view(&r, &l, &sys, Some(2));
        let mut ui = ShellUi::new(quiet(), false);
        ui.menu_key(&v);
        // M8 is under Music (after the Mixer)
        assert_eq!(press(&mut ui, &v, &[Button::Right, Button::Right, Button::Right, Button::Down, Button::A]), vec![Out::Focus(1)]);
        // the one in front: the MENU just closes
        ui.trans = None;
        settle(&mut ui, &v);
        ui.menu_key(&v);
        press(&mut ui, &v, &[Button::Right, Button::Left, Button::Right]);
        assert_eq!(sel_row(&ui), Some(cross::Row::Launch("term".into())));
        assert!(press(&mut ui, &v, &[Button::A]).is_empty());
        assert!(!ui.menu_open());
    }

    #[test]
    fn settings_values_and_pages() {
        let sys = Sys::demo(None);
        let l = launcher();
        let r = [run(2, "term")];
        let v = view(&r, &l, &sys, Some(2));
        let mut ui = ShellUi::new(quiet(), false);
        to_quick(&mut ui, &v);
        assert_eq!(sel_row(&ui), Some(cross::Row::Brightness));
        // ◀▶ change category, also on a value row
        assert!(press(&mut ui, &v, &[Button::Left]).is_empty());
        assert_eq!(ui.menu_category(), Some(Cat::Music));
        press(&mut ui, &v, &[Button::Right]);
        assert_eq!(ui.menu_category(), Some(Cat::Settings));
        // A: adjust mode; ◀▶ change the value; A or B leave it
        assert!(press(&mut ui, &v, &[Button::A]).is_empty());
        assert_eq!(press(&mut ui, &v, &[Button::Right]), vec![Out::Brightness(10)]);
        assert_eq!(press(&mut ui, &v, &[Button::Left]), vec![Out::Brightness(-10)]);
        assert!(press(&mut ui, &v, &[Button::B]).is_empty());
        assert!(ui.menu_open(), "B leaves adjust mode, not the MENU");
        // ▲▼ in adjust mode leave it and move
        assert_eq!(press(&mut ui, &v, &[Button::Down, Button::A, Button::Left]), vec![Out::Volume(-10)]);
        assert!(press(&mut ui, &v, &[Button::Down]).is_empty());
        // the one place for settings: a page per group
        let keys: Vec<cross::Row> = cross::settings_rows(&v);
        use cross::Row::*;
        assert_eq!(
            keys,
            vec![Brightness, Volume, Page(SubKind::Display), Page(SubKind::Sound), Page(SubKind::Wifi), Page(SubKind::Bluetooth), Page(SubKind::TimeZone), Remote, Page(SubKind::About), Page(SubKind::Power)]
        );
        // the pages: A opens, B closes (the MENU stays)
        for kind in [SubKind::Display, SubKind::Sound, SubKind::Wifi, SubKind::Bluetooth] {
            assert_eq!(sel_row(&ui), Some(Page(kind)));
            press(&mut ui, &v, &[Button::A]);
            assert_eq!(ui.menu_sub(), Some(kind));
            press(&mut ui, &v, &[Button::B]);
            assert_eq!(ui.menu_sub(), None);
            assert!(ui.menu_open());
            // the page is still sliding out: the next press is the cross's
            press(&mut ui, &v, &[Button::Down]);
        }
        // Time zone: the compositor loads the zones; Remote access: mortar-setup's page
        assert_eq!(press(&mut ui, &v, &[Button::A]), vec![Out::OpenTimeZone]);
        press(&mut ui, &v, &[Button::Down]);
        assert_eq!(press(&mut ui, &v, &[Button::A]), vec![Out::OpenSetup(SetupPage::Remote)]);
        // Updates only with mortar.update; Keyboard only with a keyboard app
        let u = UpdateView::default();
        let vu = View { update: Some(&u), keyboard_app: true, ..view(&r, &l, &sys, Some(2)) };
        let k = cross::settings_rows(&vu);
        assert_eq!(k[2], Keyboard);
        assert!(k.contains(&Page(SubKind::Updates)));
    }

    #[test]
    fn power_page_needs_a_second_a() {
        let sys = Sys::demo(None);
        let l = launcher();
        let v = view(&[], &l, &sys, None);
        let mut ui = ShellUi::new(quiet(), false);
        to_page(&mut ui, &v, SubKind::Power);
        // Sleep, Sleep mode, Sleep when idle, Restart, Power off, Log out
        press(&mut ui, &v, &[Button::Down; 3]);
        assert!(press(&mut ui, &v, &[Button::A]).is_empty());
        assert_eq!(press(&mut ui, &v, &[Button::A]), vec![Out::Reboot]);
        // moving away forgets the first press
        assert!(press(&mut ui, &v, &[Button::Down, Button::A, Button::Up, Button::Down, Button::A]).is_empty());
        assert_eq!(press(&mut ui, &v, &[Button::A]), vec![Out::PowerOff]);
        press(&mut ui, &v, &[Button::Down]);
        assert!(press(&mut ui, &v, &[Button::A]).is_empty());
        assert_eq!(press(&mut ui, &v, &[Button::A]), vec![Out::Logout]);
        // sleep mode and sleep when idle are choices (A: the next one)
        press(&mut ui, &v, &[Button::Up; 4]);
        assert!(!ui.prefs.suspend);
        assert_eq!(press(&mut ui, &v, &[Button::A]), vec![Out::Prefs]);
        assert!(ui.prefs.suspend);
        press(&mut ui, &v, &[Button::Down]);
        assert_eq!(press(&mut ui, &v, &[Button::Right]), vec![Out::Prefs], "▶ also picks the next");
        assert_eq!(ui.prefs.idle_minutes, 1);
        // Sleep closes the MENU and sleeps
        press(&mut ui, &v, &[Button::Up, Button::Up]);
        assert_eq!(press(&mut ui, &v, &[Button::A]), vec![Out::Sleep]);
        assert!(!ui.menu_open());
    }

    fn net_demo() -> NetState {
        use mortar_setup::control::{BtDeviceView, BtView, NetView, WifiView};
        let n = |ssid: &str, sec: &str, known: bool, connected: bool| NetView { ssid: ssid.into(), bars: 3, security: sec.into(), known, connected };
        NetState {
            wifi: WifiView {
                available: true,
                state: "connected".into(),
                ssid: Some("Home".into()),
                address: Some("192.0.2.7".into()),
                networks: vec![n("Home", "psk", true, true), n("Open", "open", false, false), n("Locked", "psk", false, false)],
                ..Default::default()
            },
            bt: BtView {
                available: true,
                powered: true,
                devices: vec![
                    BtDeviceView { address: "AA".into(), name: "Pad".into(), kind: "gamepad".into(), paired: true, connected: true, ..Default::default() },
                    BtDeviceView { address: "BB".into(), name: "Keys".into(), kind: "keyboard".into(), paired: false, ..Default::default() },
                ],
                ..Default::default()
            },
            error: None,
        }
    }

    #[test]
    fn wifi_and_bluetooth_pages() {
        use mortar_setup::control::Request;
        let sys = Sys::demo(None);
        let l = launcher();
        let net = net_demo();
        let v = View { net: Some(&net), radios: Radios { wifi: Some(true), bluetooth: Some(true) }, ..view(&[], &l, &sys, None) };
        let mut ui = ShellUi::new(quiet(), false);
        // opening Wi-Fi scans
        assert_eq!(to_page(&mut ui, &v, SubKind::Wifi), vec![Out::Setup(Request::WifiScan)]);
        assert!(ui.wants_net());
        // toggle, connected network, Details, Forget, Open, Locked, Other
        assert_eq!(press(&mut ui, &v, &[Button::A]), vec![Out::Radio(Radio::Wifi, false)]);
        press(&mut ui, &v, &[Button::Down, Button::Down, Button::Down]);
        assert_eq!(press(&mut ui, &v, &[Button::A]), vec![Out::Setup(Request::WifiForget { ssid: "Home".into() })]);
        // forgetting goes back to the switch (the network leaves the list)
        press(&mut ui, &v, &[Button::Down; 4]);
        assert_eq!(press(&mut ui, &v, &[Button::A]), vec![Out::Setup(Request::WifiConnect { ssid: "Open".into(), passphrase: None })]);
        press(&mut ui, &v, &[Button::Down]);
        // a secured network it does not know: the password keyboard
        assert_eq!(press(&mut ui, &v, &[Button::A]), vec![Out::WifiPassword("Locked".into())]);
        press(&mut ui, &v, &[Button::Down]);
        assert_eq!(press(&mut ui, &v, &[Button::A]), vec![Out::OpenSetup(SetupPage::Wifi)]);
        assert!(press(&mut ui, &v, &[Button::Down]).is_empty(), "the end of the list");
        press(&mut ui, &v, &[Button::B]);
        settle(&mut ui, &v);
        // Bluetooth: toggle, the paired pad (A disconnects, X forgets),
        // Pair new device (scan), the keyboard found (A pairs)
        press(&mut ui, &v, &[Button::Down, Button::A]);
        assert_eq!(ui.menu_sub(), Some(SubKind::Bluetooth));
        press(&mut ui, &v, &[Button::Down]);
        assert_eq!(press(&mut ui, &v, &[Button::A]), vec![Out::Setup(Request::BtDisconnect { address: "AA".into() })]);
        assert_eq!(press(&mut ui, &v, &[Button::X]), vec![Out::Setup(Request::BtForget { address: "AA".into() })]);
        press(&mut ui, &v, &[Button::Down]);
        assert_eq!(press(&mut ui, &v, &[Button::A]), vec![Out::Setup(Request::BtScan { on: true })]);
        press(&mut ui, &v, &[Button::Down]);
        assert_eq!(press(&mut ui, &v, &[Button::A]), vec![Out::Setup(Request::BtPair { address: "BB".into() })]);
        // the toggle: off with rfkill
        press(&mut ui, &v, &[Button::Up, Button::Up, Button::Up]);
        assert_eq!(press(&mut ui, &v, &[Button::A]), vec![Out::Radio(Radio::Bluetooth, false)]);
    }

    #[test]
    fn mixer_channels() {
        let sys = Sys::demo(None);
        let l = launcher();
        let r = [run(2, "term"), run(1, "m8")];
        let mix = [MixChan { uid: 1, level: 70, muted: false, playing: true }];
        let v = View { mixer: &mix, ..view(&r, &l, &sys, Some(2)) };
        let mut ui = ShellUi::new(quiet(), false);
        ui.menu_key(&v);
        press(&mut ui, &v, &[Button::Right, Button::Right, Button::Right]);
        assert_eq!(ui.menu_category(), Some(Cat::Music));
        press(&mut ui, &v, &[Button::A]);
        assert!(ui.wants_mixer());
        // Master, term (front, selected; no sound), m8, UI sounds
        assert!(press(&mut ui, &v, &[Button::Up]).is_empty(), "no stream: nothing to set");
        press(&mut ui, &v, &[Button::Right]);
        assert_eq!(press(&mut ui, &v, &[Button::Up]), vec![Out::MixLevel(1, 75)]);
        assert_eq!(press(&mut ui, &v, &[Button::A]), vec![Out::MixMute(1, true)]);
        press(&mut ui, &v, &[Button::Left, Button::Left]);
        assert_eq!(press(&mut ui, &v, &[Button::Down]), vec![Out::Volume(-5)]);
        assert_eq!(press(&mut ui, &v, &[Button::A]), vec![Out::MasterMute(true)]);
        // UI sounds are the shell's own setting
        press(&mut ui, &v, &[Button::Right, Button::Right, Button::Right]);
        assert_eq!(press(&mut ui, &v, &[Button::Down]), vec![Out::Prefs]);
        assert_eq!(ui.prefs.sound_volume, 55);
        assert_eq!(press(&mut ui, &v, &[Button::A]), vec![Out::Prefs]);
        assert!(!ui.prefs.sounds);
        // ◀ on Master goes back, like on every page
        press(&mut ui, &v, &[Button::Left, Button::Left, Button::Left]);
        assert_eq!(ui.menu_sub(), Some(SubKind::Mixer));
        press(&mut ui, &v, &[Button::Left]);
        assert_eq!(ui.menu_sub(), None);
    }

    #[test]
    fn options_popup() {
        let sys = Sys::demo(None);
        let l = launcher();
        let r = [run(2, "term"), run(1, "m8")];
        let v = view(&r, &l, &sys, Some(2));
        let mut ui = ShellUi::new(quiet(), false);
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
    fn display_and_sound_pages() {
        let sys = Sys::demo(None);
        let l = launcher();
        let v = view(&[], &l, &sys, None);
        let mut ui = ShellUi::new(quiet(), false);
        to_page(&mut ui, &v, SubKind::Display);
        // Brightness (a level), Palette, Motion, Reduce motion, Screensaver
        assert!(press(&mut ui, &v, &[Button::A]).is_empty());
        assert_eq!(press(&mut ui, &v, &[Button::Right]), vec![Out::Brightness(10)]);
        assert!(press(&mut ui, &v, &[Button::A]).is_empty(), "A: done");
        assert_eq!(press(&mut ui, &v, &[Button::Down, Button::A]), vec![Out::Prefs]);
        assert!(ui.prefs.night);
        press(&mut ui, &v, &[Button::Down, Button::A]);
        assert_eq!(ui.prefs.motion, Preset::Calm);
        press(&mut ui, &v, &[Button::Right]);
        assert_eq!(ui.prefs.motion, Preset::Snappy);
        press(&mut ui, &v, &[Button::Down, Button::A]);
        assert!(ui.prefs.reduce_motion);
        assert_eq!(ui.prefs.screensaver, 60);
        press(&mut ui, &v, &[Button::Down, Button::A]);
        assert_eq!(ui.prefs.screensaver, 120);
        // ◀ goes back to the cross
        press(&mut ui, &v, &[Button::Left]);
        assert_eq!(ui.menu_sub(), None);
        assert!(ui.menu_open());
        ui.prefs.reduce_motion = false;
        settle(&mut ui, &v);
        // Sound: Volume, Mixer ›, UI sounds, UI sound level, Vibration
        press(&mut ui, &v, &[Button::Down]);
        press(&mut ui, &v, &[Button::A]);
        assert_eq!(ui.menu_sub(), Some(SubKind::Sound));
        press(&mut ui, &v, &[Button::Down, Button::A]);
        assert_eq!(ui.menu_pages(), vec![SubKind::Sound, SubKind::Mixer]);
        press(&mut ui, &v, &[Button::B]);
        assert_eq!(ui.menu_pages(), vec![SubKind::Sound], "B: one level back");
        assert_eq!(press(&mut ui, &v, &[Button::Down, Button::A]), vec![Out::Prefs]);
        assert!(!ui.prefs.sounds);
        press(&mut ui, &v, &[Button::Down, Button::A]);
        assert_eq!(press(&mut ui, &v, &[Button::Right]), vec![Out::Prefs]);
        assert_eq!(ui.prefs.sound_volume, 70);
        press(&mut ui, &v, &[Button::B]);
        assert_eq!(ui.menu_sub(), Some(SubKind::Sound), "B leaves adjust mode first");
        press(&mut ui, &v, &[Button::Down, Button::A]);
        assert!(!ui.prefs.haptics);
        // MENU closes everything (at home: back to the dashboard)
        ui.menu_key(&v);
        settle(&mut ui, &v);
        assert!(ui.menu_pages().is_empty() && ui.dash_selected());
    }

    #[test]
    fn settings_updates() {
        let sys = Sys::demo(None);
        let l = launcher();
        let mut u = UpdateView { message: "Up to date".into(), ready: false, busy: false, auto_check: true };
        let mut ui = ShellUi::new(quiet(), false);
        {
            let v = View { update: Some(&u), ..view(&[], &l, &sys, None) };
            to_page(&mut ui, &v, SubKind::Updates);
            assert_eq!(press(&mut ui, &v, &[Button::A]), vec![Out::UpdateCheck]);
            assert_eq!(press(&mut ui, &v, &[Button::Down, Button::A]), vec![Out::UpdateAuto(false)]);
            press(&mut ui, &v, &[Button::Up]);
        }
        // staged: A twice restarts
        u.ready = true;
        u.message = "Restart to update".into();
        let v = View { update: Some(&u), ..view(&[], &l, &sys, None) };
        assert!(press(&mut ui, &v, &[Button::A]).is_empty());
        assert_eq!(press(&mut ui, &v, &[Button::A]), vec![Out::Reboot]);
        // no update module: no row
        let v0 = view(&[], &l, &sys, None);
        assert!(!cross::settings_rows(&v0).contains(&cross::Row::Page(SubKind::Updates)));
    }

    #[test]
    fn frosted_backdrop_follows_the_menu() {
        let sys = Sys::demo(None);
        let l = launcher();
        let r = [run(2, "term")];
        let v = view(&r, &l, &sys, Some(2));
        let mut ui = ShellUi::new(quiet(), false);
        assert_eq!(ui.frost(), 0.0);
        ui.menu_key(&v);
        assert_eq!(ui.frost(), 0.0, "eases in with the open spring");
        ui.step(0.05, &v);
        let early = ui.frost();
        assert!(early > 0.0 && early < 1.0);
        settle(&mut ui, &v);
        assert_eq!(ui.frost(), 1.0);
        // a glass page keeps the frost (it is frosted itself)
        press(&mut ui, &v, &[Button::R1, Button::Down, Button::Down, Button::A]);
        assert_eq!(ui.menu_sub(), Some(SubKind::Display));
        settle(&mut ui, &v);
        assert_eq!(ui.frost(), 1.0);
        ui.menu_key(&v);
        settle(&mut ui, &v);
        assert_eq!(ui.frost(), 0.0, "closed: nothing extra drawn");
        // Reduce motion: on at once, not animated
        ui.prefs.reduce_motion = true;
        ui.menu_key(&v);
        assert_eq!(ui.frost(), 1.0);
        ui.menu_key(&v);
        settle(&mut ui, &v);
        // the power menu frosts the app too
        ui.prefs.reduce_motion = false;
        ui.open_power_menu();
        settle(&mut ui, &v);
        assert_eq!(ui.frost(), 1.0);
    }

    #[test]
    fn menu_at_rest_is_not_full_pace() {
        let sys = Sys::demo(None);
        let l = launcher();
        let v = view(&[], &l, &sys, None);
        let mut ui = ShellUi::new(quiet(), false);
        ui.menu_key(&v);
        ui.button(Button::Right, false, &v);
        settle(&mut ui, &v);
        assert_ne!(ui.pace(false), Pace::Full);
        for _ in 0..(40 * 60) {
            ui.step(1.0 / 60.0, &v);
        }
        // only the pattern moves (30 fps); with Reduce motion nothing does
        assert_eq!(ui.pace(false), Pace::Pattern);
        ui.prefs.reduce_motion = true;
        assert_eq!(ui.pace(false), Pace::Idle);
    }

    #[test]
    fn time_zone_pages() {
        let sys = Sys::demo(None);
        let l = launcher();
        let v = view(&[], &l, &sys, None);
        let mut ui = ShellUi::new(quiet(), false);
        assert_eq!(to_page(&mut ui, &v, SubKind::TimeZone), vec![Out::OpenTimeZone]);
        let zones = ["Europe/Lisbon", "Europe/Berlin", "America/Argentina/Buenos_Aires", "America/New_York", "UTC"]
            .iter()
            .map(|z| TzZone { id: z.to_string(), offset: 0 })
            .collect();
        ui.open_tz(zones, "Europe/Lisbon".into());
        assert_eq!(ui.menu_sub(), Some(SubKind::TimeZone));
        // regions: America, Europe (current, preselected), UTC; A or ▶ opens one
        press(&mut ui, &v, &[Button::Right]);
        assert_eq!(ui.menu_pages(), vec![SubKind::TimeZone, SubKind::TzCities]);
        // B: back to the regions, ▶ again: on Lisbon (current); ▲ Berlin
        press(&mut ui, &v, &[Button::B]);
        assert_eq!(ui.menu_pages(), vec![SubKind::TimeZone]);
        press(&mut ui, &v, &[Button::A, Button::Up]);
        assert_eq!(press(&mut ui, &v, &[Button::A]), vec![Out::SetTimeZone("Europe/Berlin".into())]);
        // set: both pages slide away, back on Settings
        settle(&mut ui, &v);
        assert!(ui.menu_pages().is_empty());
        assert_eq!(ui.menu_category(), Some(Cat::Settings));
        // UTC is a zone of its own
        ui.out.push(Out::Sfx(Sfx::Tick));
        ui.open_tz(vec![TzZone { id: "UTC".into(), offset: 0 }, TzZone { id: "Europe/Lisbon".into(), offset: 0 }], "UTC".into());
        assert_eq!(press(&mut ui, &v, &[Button::A]), vec![Out::SetTimeZone("UTC".into())]);
    }

    /// B goes back one level and closes the MENU only at the top; MENU
    /// closes everything. Every screen, one row each.
    #[test]
    fn b_and_menu_on_every_screen() {
        use Button::*;
        let sys = Sys::demo(None);
        let l = launcher();
        let r = [run(2, "term"), run(1, "m8")];
        let u = UpdateView { message: "Up to date".into(), ..Default::default() };
        let v = View { update: Some(&u), ..view(&r, &l, &sys, Some(2)) };
        let zones = || vec![TzZone { id: "Europe/Lisbon".into(), offset: 0 }, TzZone { id: "UTC".into(), offset: 0 }];
        let page = |k: SubKind| move |ui: &mut ShellUi, v: &View| {
            to_page(ui, v, k);
        };
        type Setup = Box<dyn Fn(&mut ShellUi, &View)>;
        let table: Vec<(&str, Setup, &str, &str)> = vec![
            ("Running", Box::new(|ui, v| { ui.menu_key(v); }), "closed", "closed"),
            ("the cross", Box::new(|ui, v| { ui.menu_key(v); press(ui, v, &[Right]); }), "closed", "closed"),
            ("the cross, adjusting", Box::new(|ui, v| { to_quick(ui, v); press(ui, v, &[A]); }), "cross:Settings", "closed"),
            ("the options popup", Box::new(|ui, v| { ui.menu_key(v); press(ui, v, &[Y]); }), "running", "closed"),
            ("Display", Box::new(page(SubKind::Display)), "cross:Settings", "closed"),
            ("Display, adjusting", Box::new(|ui, v| { to_page(ui, v, SubKind::Display); press(ui, v, &[A]); }), "page:display", "closed"),
            ("Sound", Box::new(page(SubKind::Sound)), "cross:Settings", "closed"),
            ("Sound > Mixer", Box::new(|ui, v| { to_page(ui, v, SubKind::Sound); press(ui, v, &[Down, A]); }), "page:sound", "closed"),
            ("Mixer (Music)", Box::new(|ui, v| { ui.menu_key(v); press(ui, v, &[Right, Right, Right, A]); }), "cross:Music", "closed"),
            ("Wi-Fi", Box::new(page(SubKind::Wifi)), "cross:Settings", "closed"),
            ("Bluetooth", Box::new(page(SubKind::Bluetooth)), "cross:Settings", "closed"),
            ("Time zone", Box::new(move |ui, v| { to_page(ui, v, SubKind::TimeZone); ui.open_tz(zones(), "Europe/Lisbon".into()); }), "cross:Settings", "closed"),
            ("Time zone > cities", Box::new(move |ui, v| { to_page(ui, v, SubKind::TimeZone); ui.open_tz(zones(), "Europe/Lisbon".into()); press(ui, v, &[Up, A]); }), "page:timezone", "closed"),
            ("Updates", Box::new(page(SubKind::Updates)), "cross:Settings", "closed"),
            ("About", Box::new(page(SubKind::About)), "cross:Settings", "closed"),
            ("About > Controls", Box::new(|ui, v| { to_page(ui, v, SubKind::About); press(ui, v, &[Down; 5]); press(ui, v, &[A]); }), "page:about", "closed"),
            ("Power", Box::new(page(SubKind::Power)), "cross:Settings", "closed"),
            ("Power, confirming", Box::new(|ui, v| { to_page(ui, v, SubKind::Power); press(ui, v, &[Down, Down, Down, A]); }), "cross:Settings", "closed"),
            ("the power menu", Box::new(|ui, _| ui.open_power_menu()), "closed", "closed"),
            ("the power menu, confirming", Box::new(|ui, v| { ui.open_power_menu(); press(ui, v, &[Down, A]); }), "power-menu", "closed"),
        ];
        for (name, setup, after_b, after_menu) in &table {
            for (key, want) in [("B", after_b), ("MENU", after_menu)] {
                let mut ui = ShellUi::new(quiet(), false);
                setup(&mut ui, &v);
                let before = place(&ui);
                assert_ne!(before, "closed", "{name}: set up");
                if key == "B" {
                    ui.button(B, false, &v);
                } else {
                    ui.menu_key(&v);
                }
                assert_eq!(place(&ui), **want, "{name} ({before}): {key}");
            }
        }
        // the games (mortar-play): a system's page and the game panel
        let lib = crate::games::Library::parse(
            r#"{"systems":[{"id":"gbc","name":"Game Boy Color","tag":"GBC","order":1,"count":2,"games":[{"path":"/r/a.gbc","name":"A"},{"path":"/r/b.gbc","name":"B"}]}],"recent":[]}"#,
        )
        .ok();
        let gv = crate::games::GamesView { lib, ..Default::default() };
        let mut gr = run(7, "game:gbc:/r/a.gbc");
        gr.game = true;
        let rg = [gr, run(2, "term")];
        let vg = View { games: Some(&gv), ..view(&rg, &l, &sys, Some(2)) };
        let game_table: Vec<(&str, Setup, &str, &str)> = vec![
            ("Games > a system", Box::new(|ui, v| { ui.menu_key(v); press(ui, v, &[Right, Right, A]); }), "cross:Games", "closed"),
            ("the game panel", Box::new(|ui, v| { ui.menu_key(v); press(ui, v, &[Y]); }), "running", "closed"),
        ];
        for (name, setup, after_b, after_menu) in &game_table {
            for (key, want) in [("B", after_b), ("MENU", after_menu)] {
                let mut ui = ShellUi::new(quiet(), false);
                setup(&mut ui, &vg);
                let before = place(&ui);
                assert!(before.starts_with("game-"), "{name}: set up ({before})");
                if key == "B" {
                    ui.button(B, false, &vg);
                } else {
                    ui.menu_key(&vg);
                }
                assert_eq!(place(&ui), **want, "{name} ({before}): {key}");
            }
        }
        // ◀ on a glass page goes back too (not while adjusting)
        let mut ui = ShellUi::new(quiet(), false);
        to_page(&mut ui, &v, SubKind::About);
        press(&mut ui, &v, &[Left]);
        assert_eq!(place(&ui), "cross:Settings");
        // Home (no app in front): B goes back one level as anywhere, but
        // the top is the dashboard, and MENU always comes back to it
        let att = [crate::attention::Item { app: "term".into(), text: "Approve: ls".into(), seq: 1 }];
        let vh = View { update: Some(&u), dash: Dash { attention: &att, widgets: &crate::widgets::defaults(), ..Dash::default() }, ..view(&r, &l, &sys, None) };
        let home_table: Vec<(&str, Setup, &str, &str)> = vec![
            ("Home (the dashboard)", Box::new(|ui, v| { ui.step(0.01, v); }), "dashboard", "dashboard"),
            ("the dashboard's tiles", Box::new(|ui, v| { ui.step(0.01, v); press(ui, v, &[A]); }), "dashboard", "dashboard"),
            ("Running, an app", Box::new(|ui, v| { ui.step(0.01, v); press(ui, v, &[Down]); }), "dashboard", "dashboard"),
            ("the cross", Box::new(|ui, v| { ui.step(0.01, v); press(ui, v, &[Right]); }), "dashboard", "dashboard"),
            ("the cross, adjusting", Box::new(|ui, v| { to_quick(ui, v); press(ui, v, &[A]); }), "cross:Settings", "dashboard"),
            ("the options popup", Box::new(|ui, v| { ui.step(0.01, v); press(ui, v, &[Down, Y]); }), "running", "dashboard"),
            ("Display", Box::new(page(SubKind::Display)), "cross:Settings", "dashboard"),
            ("Sound > Mixer", Box::new(|ui, v| { to_page(ui, v, SubKind::Sound); press(ui, v, &[Down, A]); }), "page:sound", "dashboard"),
            ("the power menu", Box::new(|ui, v| { ui.step(0.01, v); ui.open_power_menu(); }), "dashboard", "dashboard"),
        ];
        for (name, setup, after_b, after_menu) in &home_table {
            for (key, want) in [("B", after_b), ("MENU", after_menu)] {
                let mut ui = ShellUi::new(quiet(), false);
                setup(&mut ui, &vh);
                let before = place(&ui);
                if key == "B" {
                    ui.button(B, false, &vh);
                } else {
                    ui.menu_key(&vh);
                }
                assert_eq!(place(&ui), **want, "home: {name} ({before}): {key}");
            }
        }
    }

    #[test]
    fn attention_and_tips() {
        let sys = Sys::demo(None);
        let l = launcher();
        let r = [run(2, "term")];
        let v = view(&r, &l, &sys, Some(2));
        let mut ui = ShellUi::new(Prefs::default(), false);
        // the first app on screen: one tip, once ever
        let out = ui.step(0.01, &v);
        assert!(out.contains(&Out::Remember));
        assert_eq!(ui.toast.as_ref().map(|t| t.toast.title.as_str()), Some("MENU switches apps"));
        assert_eq!(ui.prefs.tips_seen, vec!["menu".to_string()]);
        ui.toast = None;
        ui.step(0.01, &v);
        assert!(ui.toast.is_none(), "not again");
        // a keyboard app: its own tip
        let vk = View { keyboard_app: true, ..view(&r, &l, &sys, Some(2)) };
        ui.step(0.01, &vk);
        assert_eq!(ui.toast.as_ref().map(|t| t.toast.title.as_str()), Some("START opens the keyboard"));
        // an app needs you: one toast with its text
        ui.attention_raised("Claude", "Approve: ls");
        let t = &ui.toast.as_ref().unwrap().toast;
        assert_eq!((t.title.as_str(), t.sub.as_str()), ("Claude needs you", "Approve: ls"));
    }

    #[test]
    fn sleep_and_power_menu() {
        let sys = Sys::demo(None);
        let l = launcher();
        let r = [run(2, "term")];
        let v = view(&r, &l, &sys, Some(2));
        let mut ui = ShellUi::new(quiet(), false);
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
    fn veil_keeps_subtitles_legible() {
        let contrast = |mean: [f32; 3], v: f32, pal: &Pal| {
            let g = rgb(pal.ground);
            let c: [f32; 3] = std::array::from_fn(|i| mean[i] * (1.0 - v) + g[i] * v);
            let (l, s) = (luminance(c), luminance(rgb(pal.soft)));
            (l.max(s) + 0.05) / (l.min(s) + 0.05)
        };
        // light backdrops keep the usual 62%
        assert_eq!(veil_for([0.85, 0.88, 0.9], &DAY), FROST_VEIL);
        // black (a dark app, the M8) needs more, about 85%, and gets 4.5:1
        let v = veil_for([0.0, 0.0, 0.0], &DAY);
        assert!((0.8..=0.9).contains(&v), "{v}");
        assert!(contrast([0.0; 3], v, &DAY) >= 4.5);
        let mid = veil_for([0.3, 0.3, 0.3], &DAY);
        assert!(mid > FROST_VEIL && mid < v, "{mid}");
        // night: a light backdrop is the hard case
        assert!(contrast([1.0; 3], veil_for([1.0; 3], &NIGHT), &NIGHT) >= 4.5);
        // the UI eases to it while the frost shows, at once when it does not
        let sys = Sys::demo(None);
        let l = launcher();
        let r = [run(2, "term")];
        let v = view(&r, &l, &sys, Some(2));
        let mut ui = ShellUi::new(quiet(), false);
        ui.set_backdrop(Some([0.0; 3]));
        assert!(ui.veil.x > 0.8, "closed: no animation");
        ui.set_backdrop(Some([0.9; 3]));
        ui.menu_key(&v);
        settle(&mut ui, &v);
        ui.set_backdrop(Some([0.0; 3]));
        assert!(ui.veil.x < 0.7 && ui.veil.t > 0.8, "eases over the open MENU");
        assert_eq!(ui.pace(true), Pace::Full);
        settle(&mut ui, &v);
        assert!((ui.veil.x - ui.veil.t).abs() < 0.01);
    }

    #[test]
    fn full_screensaver_holds_still_at_the_pattern_pace() {
        // at full size it stays there (it once stepped back and forth every
        // frame: a flicker, and 60 fps)
        let sys = Sys::demo(None);
        let l = launcher();
        let v = view(&[], &l, &sys, None);
        let mut ui = ShellUi::new(Prefs { screensaver: 30, ..quiet() }, false);
        ui.menu_key(&v);
        press(&mut ui, &v, &[Button::Right]);
        for i in 0..(40 * 40) {
            ui.step(if i % 3 == 0 { 0.008 } else { 1.0 / 30.0 }, &v);
            if ui.t > 33.0 {
                assert_eq!(ui.saver().0, 1.0);
                assert_eq!(ui.pace(false), Pace::Pattern);
            }
        }
    }

    #[test]
    fn screensaver_grows_and_a_press_brings_the_menu_back() {
        let sys = Sys::demo(None);
        let l = launcher();
        let v = view(&[], &l, &sys, None);
        let mut ui = ShellUi::new(Prefs { screensaver: 30, ..quiet() }, false);
        ui.menu_key(&v);
        // on Running it never comes
        for _ in 0..(40 * 30) {
            ui.step(1.0 / 30.0, &v);
        }
        assert_eq!(ui.saver().0, 0.0, "not on Running");
        // on the cross: 30 s idle, then it grows over the screen
        press(&mut ui, &v, &[Button::Right]);
        settle(&mut ui, &v);
        assert!(ui.next_deadline().is_some_and(|d| d > 20.0 && d < 30.0), "the shell wakes for it: {:?}", ui.next_deadline());
        for _ in 0..(25 * 30) {
            ui.step(1.0 / 30.0, &v);
        }
        assert_eq!(ui.saver().0, 0.0, "not yet");
        for _ in 0..(3 * 30) {
            ui.step(1.0 / 30.0, &v);
        }
        assert!(ui.saver().0 >= 0.999, "full screen");
        assert!(ui.menu_open());
        assert_eq!(ui.pace(false), Pace::Pattern, "the saver moves at the pattern's pace");
        // a press brings the menu back and does nothing else
        assert!(press(&mut ui, &v, &[Button::A]).is_empty());
        assert_eq!(ui.menu_category(), Some(Cat::Apps));
        assert_eq!(ui.pace(false), Pace::Full, "it shrinks back");
        settle(&mut ui, &v);
        assert_eq!(ui.saver().0, 0.0);
        // a long while full screen: it dims and slows to 20 fps
        for _ in 0..(32 * 30) {
            ui.step(1.0 / 30.0, &v);
        }
        for _ in 0..(200 * 2) {
            ui.step(0.5, &v);
        }
        assert!(ui.saver().1 > 0.3, "dimmed");
        assert_eq!(ui.pace(false), Pace::Ambient);
        // MENU also only wakes it
        ui.menu_key(&v);
        assert!(ui.menu_open());
        // Reduce motion: a still frame, at once
        ui.prefs.reduce_motion = true;
        for _ in 0..(32 * 30) {
            ui.step(1.0 / 30.0, &v);
        }
        assert_eq!(ui.saver().0, 1.0);
        // a glass page never gets it
        ui.prefs.reduce_motion = false;
        ui.button(Button::A, false, &v);
        settle(&mut ui, &v);
        press(&mut ui, &v, &[Button::R1, Button::Down, Button::Down, Button::A]);
        assert_eq!(ui.menu_sub(), Some(SubKind::Display));
        for _ in 0..(40 * 30) {
            ui.step(1.0 / 30.0, &v);
        }
        assert_eq!(ui.saver().0, 0.0, "not over a glass page");
    }

    #[test]
    fn pace_goes_idle() {
        let sys = Sys::demo(None);
        let l = launcher();
        let r = [run(2, "term")];
        let v = view(&r, &l, &sys, Some(2));
        let mut ui = ShellUi::new(quiet(), false);
        settle(&mut ui, &v);
        assert_eq!(ui.pace(true), Pace::Idle);
        ui.osd(OsdKind::Volume, 50, false);
        assert_eq!(ui.pace(true), Pace::Full);
        for _ in 0..200 {
            ui.step(1.0 / 60.0, &v);
        }
        assert_eq!(ui.pace(true), Pace::Idle);
        assert!(!ui.layer_visible(true));
    }

    #[test]
    fn osd_goes_away_without_frames() {
        let sys = Sys::demo(None);
        let l = launcher();
        let r = [run(2, "term")];
        let v = view(&r, &l, &sys, Some(2));
        let mut ui = ShellUi::new(quiet(), false);
        settle(&mut ui, &v);
        ui.osd(OsdKind::Volume, 50, false);
        // the compositor sleeps until the deadline the UI gives it
        let d = ui.next_deadline().unwrap();
        assert!(d <= OSD_SECS + 1e-9);
        // no frames in between: one late step, then the fade's deadline
        ui.step(d + 0.01, &v);
        let d2 = ui.next_deadline().unwrap();
        assert!(d2 <= OSD_FADE + 1e-9);
        ui.step(d2 + 0.01, &v);
        assert!(ui.osd.is_none(), "gone within {:.2} s", d + d2 + 0.02);
        assert!(d + d2 + 0.02 < 1.5);
        assert!(!ui.layer_visible(true));
        // still fading and the bar at rest: the pill does not keep the
        // compositor at full frame rate while it just sits there
        ui.osd(OsdKind::Volume, 50, false);
        for _ in 0..30 {
            ui.step(1.0 / 60.0, &v);
        }
        assert_eq!(ui.pace(true), Pace::Idle);
        // steps update it at once; held keys keep it up; muted: empty bar
        ui.osd(OsdKind::Volume, 0, true);
        let o = ui.osd.as_ref().unwrap();
        assert!(o.muted && o.fill.t == 0.0);
    }
}
