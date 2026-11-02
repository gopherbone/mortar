// SPDX-License-Identifier: GPL-2.0-or-later
//! The MENU's glass pages (plan/mortar-cross-2.html): frosted panels that
//! slide in from the right over the cross, one on top of the other (a
//! stack: Settings > Sound > Mixer, Settings > Time zone > Europe). B or ◀
//! goes back one page, MENU closes everything.
//!
//! - Display: brightness, palette (Day/Night), motion, reduce motion, the
//!   screensaver (Off, 30 s to 5 min idle on the cross).
//! - Sound: volume, the Mixer, UI sounds and their level, vibration.
//! - Wi-Fi: on/off (rfkill, radio.rs), the connected network with its
//!   signal (Details, Forget), nearby networks to join, "Other network…".
//!   Through mortar-setupd (iwd): scan, connect, forget; secured networks
//!   it does not know open the setup password keyboard.
//! - Bluetooth: on/off (rfkill and BlueZ power), paired devices (A connects
//!   or disconnects, X forgets), "Pair new device" (discovery) and the
//!   devices found, A pairs. Through mortar-setupd (BlueZ, its agent).
//! - Time zone: regions, then their cities (the compositor loads the zones).
//! - Updates (with mortar.update): check now / restart to update,
//!   auto-check daily.
//! - About: version and generation, storage, battery, temperature, the
//!   device, and Controls (every button and chord).
//! - Power: Sleep, sleep mode, sleep when idle, Restart, Power off and Log
//!   out (the last three need a second A).
//! - Mixer: Master, a fader per running app (its PipeWire streams), UI
//!   sounds. ◀▶ pick a channel (◀ on Master goes back), ▲▼ ±5, A mutes.
//!
//! Rows: a level (A, then ◀▶ change it in steps of 10; A or B when done), a
//! choice (A or ▶: the next value), a link (A or ▶ opens the page), an
//! action, a fact (A: the whole text in a toast).
//!
//! A child module of `shellui` next to `cross`.

use super::cross::{draw_icon, glass, Cross, Glass, Ic, BAR_H, HINT_H};
use super::*;
use mortar_setup::control::Request;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubKind {
    Wifi,
    Bluetooth,
    Mixer,
    Display,
    Sound,
    TimeZone,
    /// A region's cities (the page's `region`).
    TzCities,
    Updates,
    About,
    Controls,
    Power,
}

impl SubKind {
    pub fn name(self) -> &'static str {
        match self {
            SubKind::Wifi => "wifi",
            SubKind::Bluetooth => "bluetooth",
            SubKind::Mixer => "mixer",
            SubKind::Display => "display",
            SubKind::Sound => "sound",
            SubKind::TimeZone => "timezone",
            SubKind::TzCities => "timezone-cities",
            SubKind::Updates => "updates",
            SubKind::About => "about",
            SubKind::Controls => "controls",
            SubKind::Power => "power",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            SubKind::Wifi => "Wi-Fi",
            SubKind::Bluetooth => "Bluetooth",
            SubKind::Mixer => "Mixer",
            SubKind::Display => "Display",
            SubKind::Sound => "Sound",
            SubKind::TimeZone | SubKind::TzCities => "Time zone",
            SubKind::Updates => "Updates",
            SubKind::About => "About",
            SubKind::Controls => "Controls",
            SubKind::Power => "Power",
        }
    }

    pub(super) fn icon(self) -> Ic {
        match self {
            SubKind::Wifi => Ic::Wifi,
            SubKind::Bluetooth => Ic::Bt,
            SubKind::Mixer => Ic::Music,
            SubKind::Display => Ic::Screen,
            SubKind::Sound => Ic::Vol,
            SubKind::TimeZone | SubKind::TzCities => Ic::Clock,
            SubKind::Updates => Ic::Update,
            SubKind::About => Ic::Info,
            SubKind::Controls => Ic::Pad,
            SubKind::Power => Ic::Power,
        }
    }
}

/// A level row: A, then ◀▶.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Lv {
    Brightness,
    Volume,
    UiVolume,
}

/// A choice row: A or ▶ picks the next value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OptK {
    Palette,
    Motion,
    ReduceMotion,
    Screensaver,
    UiSounds,
    Haptics,
    SleepMode,
    IdleSleep,
    AutoUpdate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ActK {
    Sleep,
    Restart,
    PowerOff,
    Logout,
    Update,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FactK {
    Version,
    Storage,
    Battery,
    Temp,
    Device,
}

/// A row of a page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SRow {
    Label(&'static str),
    /// Greyed out: why there is nothing to do.
    Note(String, String),
    Toggle,
    Conn,
    Details,
    Forget,
    Join(String),
    Other,
    /// A paired Bluetooth device (address).
    Dev(String),
    Scan,
    /// A device found by the scan (address).
    Found(String),
    Level(Lv),
    Opt(OptK),
    Link(SubKind),
    Act(ActK),
    Fact(FactK),
    /// A line of the Controls page (index into CONTROLS).
    Key(usize),
    /// A time zone region and how many zones it has ("UTC": none).
    Region(String, usize),
    /// A zone (index into the loaded zones).
    Zone(usize),
}

impl SRow {
    fn selectable(&self) -> bool {
        !matches!(self, SRow::Label(_) | SRow::Note(..))
    }
    fn confirms(&self) -> bool {
        matches!(self, SRow::Act(ActK::Restart | ActK::PowerOff | ActK::Logout))
    }
}

/// Every button and chord (About > Controls): (group, keys, what, detail).
pub(super) const CONTROLS: &[(&str, &str, &str, &str)] = &[
    ("Everywhere", "MENU", "Open or close the menu", "From any app; closes every page at once"),
    ("Everywhere", "MENU then A", "Back to the app before", "The menu opens on the last app"),
    ("Everywhere", "VOL+ + VOL−", "Volume; both together: screenshot", "Saved in ~/Pictures/Screenshots"),
    ("Everywhere", "POWER", "Tap: sleep or wake", "Any button also wakes it"),
    ("Everywhere", "hold POWER", "The power menu", "Sleep, Restart, Power off"),
    ("Games", "SELECT + MENU", "The game's own menu", "MENU alone opens Mortar's"),
    ("In the menu", "◀▶ ▲▼", "Move; ◀ from Apps: Running", "B goes back one step"),
    ("In the menu", "L1 / R1", "Running / Settings", "From anywhere in the cross"),
    ("In the menu", "X / Y", "Close app / options", "On a window in Running"),
    ("Home", "L1 / SELECT", "The menu on Apps", "R1: on Settings"),
    ("Terminals", "START", "The on-screen keyboard", "Pad: A Enter, B Backspace, X Tab"),
    ("Terminals", "L2 / R2", "Esc / hold for Ctrl", "SELECT: Ctrl+C; L1 R1: Page up/down"),
    ("On-screen keyboard", "A / B", "Type / Backspace", "X Space, Y Enter"),
    ("On-screen keyboard", "L1", "Shift; twice: caps lock", "R2: Ctrl for one key, L2: Esc"),
    ("On-screen keyboard", "▲", "Into the suggestions", "From the top row; ◀▶ choose, A types"),
    ("On-screen keyboard", "R1", "Type the suggestion", "The first one from the keys"),
    ("On-screen keyboard", "START", "Hide the keyboard", "The app gets its full height back"),
];

pub(super) struct Sub {
    pub kind: SubKind,
    sel: usize,
    sel_key: Option<SRow>,
    /// 0: in place; 1: off to the right.
    x: Spring,
    pub closing: bool,
    mix_sel: usize,
    /// TzCities: which region.
    region: Option<String>,
    /// A level row is being adjusted (◀▶ change it).
    adjusting: bool,
    /// A row waiting for its confirming A.
    confirm: Option<SRow>,
}

impl Sub {
    fn new(kind: SubKind, reduce: bool) -> Sub {
        let mut x = Spring::new(1.0);
        x.k_mul = 0.9;
        x.set(0.0);
        if reduce {
            x.snap(0.0);
        }
        Sub { kind, sel: 0, sel_key: None, x, closing: false, mix_sel: 0, region: None, adjusting: false, confirm: None }
    }

    pub fn springs(&mut self) -> Vec<&mut Spring> {
        vec![&mut self.x]
    }

    pub fn moving(&self) -> bool {
        self.closing || !self.x.rest()
    }

    /// Gone after sliding out.
    pub fn gone(&self) -> bool {
        self.closing && self.x.x > 0.98
    }

    #[cfg(test)]
    pub fn adjusting(&self) -> bool {
        self.adjusting
    }
}

/// Wi-Fi is on (rfkill; no switch: whatever iwd says).
fn wifi_on(v: &View) -> bool {
    v.radios.wifi.unwrap_or_else(|| v.net.is_some_and(|n| n.wifi.available))
}

fn bt_on(v: &View) -> bool {
    v.radios.bluetooth != Some(false) && v.net.is_some_and(|n| n.bt.available && n.bt.powered)
}

/// Settings > Wi-Fi's subtitle.
pub(super) fn wifi_summary(v: &View) -> String {
    if !wifi_on(v) {
        return if v.radios.wifi.is_none() && v.net.is_none() { "No Wi-Fi switch".into() } else { "Off".into() };
    }
    match v.net.map(|n| &n.wifi) {
        Some(w) if w.state == "connected" => format!("Connected · {}", w.ssid.clone().unwrap_or_default()),
        Some(w) if w.state == "connecting" => format!("Joining {}…", w.ssid.clone().unwrap_or_default()),
        Some(_) => "On · not connected".into(),
        None if v.sys.wifi => "Connected".into(),
        None => "On".into(),
    }
}

pub(super) fn bt_summary(v: &View) -> String {
    if v.radios.bluetooth == Some(false) {
        return "Off".into();
    }
    match v.net.map(|n| &n.bt) {
        Some(b) if b.available && !b.powered => "Off".into(),
        Some(b) if b.available => b.devices.iter().find(|d| d.connected).map(|d| d.name.clone()).unwrap_or_else(|| "On · nothing connected".into()),
        _ if v.radios.bluetooth.is_none() => "No Bluetooth switch".into(),
        _ => "On".into(),
    }
}

fn wifi_rows(v: &View) -> Vec<SRow> {
    let mut r = vec![SRow::Toggle];
    if !wifi_on(v) {
        return r;
    }
    let Some(n) = v.net else {
        r.push(SRow::Note("Networks need the setup service".into(), "mortar.setup.enable".into()));
        return r;
    };
    let w = &n.wifi;
    if !w.available {
        r.push(SRow::Note("Wi-Fi is not ready".into(), n.error.clone().unwrap_or_else(|| "No answer from iwd yet".into())));
        return r;
    }
    let conn = w.ssid.clone().filter(|_| matches!(w.state.as_str(), "connected" | "connecting" | "roaming"));
    if let Some(ssid) = &conn {
        r.extend([SRow::Label("Connected"), SRow::Conn, SRow::Details]);
        if w.networks.iter().any(|x| x.ssid == *ssid && x.known) || w.state == "connected" {
            r.push(SRow::Forget);
        }
    }
    r.push(SRow::Label("Nearby"));
    for x in w.networks.iter().filter(|x| Some(&x.ssid) != conn.as_ref() && !x.ssid.is_empty()).take(12) {
        r.push(SRow::Join(x.ssid.clone()));
    }
    r.push(SRow::Other);
    r
}

fn bt_rows(v: &View) -> Vec<SRow> {
    let mut r = vec![SRow::Toggle];
    let Some(n) = v.net else {
        r.push(SRow::Note("Devices need the setup service".into(), "mortar.setup.enable".into()));
        return r;
    };
    let b = &n.bt;
    if v.radios.bluetooth == Some(false) {
        return r;
    }
    if !b.available {
        r.push(SRow::Note("Bluetooth is not ready".into(), n.error.clone().unwrap_or_else(|| "No answer from BlueZ yet".into())));
        return r;
    }
    if !b.powered {
        return r;
    }
    let paired: Vec<_> = b.devices.iter().filter(|d| d.paired).collect();
    if !paired.is_empty() {
        r.push(SRow::Label("Your devices"));
        r.extend(paired.iter().map(|d| SRow::Dev(d.address.clone())));
    }
    r.push(SRow::Label("New device"));
    r.push(SRow::Scan);
    r.extend(b.devices.iter().filter(|d| !d.paired).take(10).map(|d| SRow::Found(d.address.clone())));
    r
}

/// The time zone regions ("UTC" last).
fn tz_regions(zones: &[TzZone]) -> Vec<SRow> {
    let mut regions: Vec<(String, usize)> = Vec::new();
    for z in zones {
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
    let mut v: Vec<SRow> = regions.into_iter().map(|(n, c)| SRow::Region(n, c)).collect();
    v.push(SRow::Region("UTC".into(), 0));
    v
}

fn tz_cities(zones: &[TzZone], region: &str) -> Vec<SRow> {
    let mut idx: Vec<usize> = (0..zones.len()).filter(|i| crate::tz::region(&zones[*i].id) == region && zones[*i].id != "UTC").collect();
    idx.sort_by_key(|i| crate::tz::pretty(&zones[*i].id).0);
    idx.into_iter().map(SRow::Zone).collect()
}

fn controls_rows() -> Vec<SRow> {
    let mut r = Vec::new();
    let mut group = "";
    for (i, (g, ..)) in CONTROLS.iter().enumerate() {
        if *g != group {
            group = g;
            r.push(SRow::Label(g));
        }
        r.push(SRow::Key(i));
    }
    r
}

/// A mixer channel: Master, an app, or the shell's own sounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Chan {
    Master,
    App(u32),
    Ui,
}

fn channels(v: &View) -> Vec<Chan> {
    std::iter::once(Chan::Master).chain(v.running.iter().map(|r| Chan::App(r.uid))).chain(std::iter::once(Chan::Ui)).collect()
}

/// (level, muted, has sound) of a channel.
fn chan_state(c: Chan, v: &View, prefs: &Prefs) -> (u8, bool, bool) {
    match c {
        Chan::Master => (v.volume.unwrap_or(0), v.muted, v.volume.is_some()),
        Chan::App(u) => match v.mixer.iter().find(|m| m.uid == u) {
            Some(m) => (m.level, m.muted, true),
            None => (0, false, false),
        },
        Chan::Ui => (prefs.sound_volume, !prefs.sounds, true),
    }
}

/// A choice row's values and the chosen one.
fn opt_values(k: OptK, prefs: &Prefs, v: &View) -> (Vec<&'static str>, usize) {
    let onoff = |b: bool| (vec!["On", "Off"], if b { 0 } else { 1 });
    match k {
        OptK::Palette => (vec!["Day", "Night"], prefs.night as usize),
        OptK::Motion => (Preset::ALL.iter().map(|p| p.name()).collect(), Preset::ALL.iter().position(|p| *p == prefs.motion).unwrap_or(0)),
        OptK::ReduceMotion => onoff(prefs.reduce_motion),
        OptK::Screensaver => (SAVER_LABELS.to_vec(), SAVER_CHOICES.iter().position(|m| *m == prefs.screensaver).unwrap_or(2)),
        OptK::UiSounds => onoff(prefs.sounds),
        OptK::Haptics => onoff(prefs.haptics),
        OptK::SleepMode => (vec!["Suspend", "Screen off"], if prefs.suspend { 0 } else { 1 }),
        OptK::IdleSleep => (IDLE_LABELS.to_vec(), IDLE_CHOICES.iter().position(|m| *m == prefs.idle_minutes).unwrap_or(0)),
        OptK::AutoUpdate => onoff(v.update.is_some_and(|u| u.auto_check)),
    }
}

const ROW_H: f32 = 84.0;
const LBL_H: f32 = 44.0;
const ROWS_TOP: f32 = 176.0;
const ROWS_BOTTOM: f32 = H - HINT_H;
const CH_W: f32 = 168.0;
const CH_GAP: f32 = 18.0;
/// L1/R1 in long lists.
const PAGE_ROWS: usize = 6;

impl ShellUi {
    fn top_sub(&self) -> Option<&Sub> {
        self.menu.as_ref()?.subs.iter().rev().find(|s| !s.closing)
    }

    fn top_sub_mut(&mut self) -> Option<&mut Sub> {
        self.menu.as_mut()?.subs.iter_mut().rev().find(|s| !s.closing)
    }

    /// Push a page over the cross (or over the page in front).
    pub(super) fn open_sub(&mut self, kind: SubKind, v: &View) {
        let reduce = self.prefs.reduce_motion;
        let Some(m) = &mut self.menu else { return };
        let mut s = Sub::new(kind, reduce);
        if kind == SubKind::Mixer {
            // start on the app in front, else Master
            let ch = channels(v);
            s.mix_sel = v.current.and_then(|u| ch.iter().position(|c| *c == Chan::App(u))).unwrap_or(0);
        }
        m.subs.push(s);
        if kind == SubKind::Wifi && wifi_on(v) && v.net.is_some() {
            self.out.push(Out::Setup(Request::WifiScan));
        }
        self.fx(Sfx::Tab, Some(Haptic::Tick));
    }

    /// The compositor loaded the zones (after Out::OpenTimeZone): the time
    /// zone page opens on the current zone's region.
    pub fn open_tz(&mut self, zones: Vec<TzZone>, current: String) {
        self.tz_zones = zones;
        let reduce = self.prefs.reduce_motion;
        let Some(m) = self.menu.as_mut().filter(|m| !m.closing) else { return };
        let mut s = Sub::new(SubKind::TimeZone, reduce);
        let region = crate::tz::region(&current).to_string();
        let rows = tz_regions(&self.tz_zones);
        let key = rows.iter().find(|r| matches!(r, SRow::Region(n, _) if *n == region || (current == "UTC" && n == "UTC"))).cloned();
        s.sel = key.as_ref().and_then(|k| rows.iter().position(|r| r == k)).unwrap_or(0);
        s.sel_key = key;
        m.subs.push(s);
        self.fx(Sfx::Tab, Some(Haptic::Tick));
        self.dirty = true;
    }

    /// Pop the page in front.
    fn close_sub(&mut self, v: &View) {
        let reduce = self.prefs.reduce_motion;
        let Some(s) = self.top_sub_mut() else { return };
        s.closing = true;
        s.x.set(1.0);
        if reduce {
            s.x.snap(1.0);
        }
        if s.kind == SubKind::Bluetooth && v.net.is_some_and(|n| n.bt.discovering) {
            self.out.push(Out::Setup(Request::BtScan { on: false }));
        }
        self.fx(Sfx::DrawerUp, Some(Haptic::Tick));
    }

    /// The pages in front, bottom to top (for `ctl state`).
    pub fn menu_pages(&self) -> Vec<SubKind> {
        self.menu.as_ref().map(|m| m.subs.iter().filter(|s| !s.closing).map(|s| s.kind).collect()).unwrap_or_default()
    }

    /// The page in front (for `ctl state`).
    pub fn menu_sub(&self) -> Option<SubKind> {
        self.top_sub().map(|s| s.kind)
    }

    /// The compositor should keep mortar-setupd's state fresh (Settings or
    /// a Wi-Fi/Bluetooth page shows).
    pub fn wants_net(&self) -> bool {
        let Some(m) = self.menu.as_ref().filter(|m| !m.closing && m.on_cross()) else { return false };
        match self.top_sub() {
            Some(s) => matches!(s.kind, SubKind::Wifi | SubKind::Bluetooth),
            None => m.cat == Cat::Settings,
        }
    }

    /// The Mixer page shows: poll the streams.
    pub fn wants_mixer(&self) -> bool {
        self.menu_sub() == Some(SubKind::Mixer)
    }

    /// Drop pages that slid out.
    pub(super) fn sync_sub(&mut self) {
        if let Some(m) = &mut self.menu {
            let n = m.subs.len();
            m.subs.retain(|s| !s.gone());
            if m.subs.len() != n {
                self.dirty = true;
            }
        }
    }

    /// The rows of a page.
    fn page_rows(&self, s: &Sub, v: &View) -> Vec<SRow> {
        match s.kind {
            SubKind::Wifi => wifi_rows(v),
            SubKind::Bluetooth => bt_rows(v),
            SubKind::Mixer => Vec::new(),
            SubKind::Display => vec![SRow::Level(Lv::Brightness), SRow::Opt(OptK::Palette), SRow::Opt(OptK::Motion), SRow::Opt(OptK::ReduceMotion), SRow::Opt(OptK::Screensaver)],
            SubKind::Sound => vec![SRow::Level(Lv::Volume), SRow::Link(SubKind::Mixer), SRow::Opt(OptK::UiSounds), SRow::Level(Lv::UiVolume), SRow::Opt(OptK::Haptics)],
            SubKind::TimeZone => tz_regions(&self.tz_zones),
            SubKind::TzCities => tz_cities(&self.tz_zones, s.region.as_deref().unwrap_or("")),
            SubKind::Updates => match v.update {
                Some(_) => vec![SRow::Act(ActK::Update), SRow::Opt(OptK::AutoUpdate)],
                None => vec![SRow::Note("Updates are off".into(), "mortar.update.enable".into())],
            },
            SubKind::About => vec![
                SRow::Fact(FactK::Version),
                SRow::Fact(FactK::Storage),
                SRow::Fact(FactK::Battery),
                SRow::Fact(FactK::Temp),
                SRow::Fact(FactK::Device),
                SRow::Link(SubKind::Controls),
            ],
            SubKind::Controls => controls_rows(),
            SubKind::Power => vec![
                SRow::Act(ActK::Sleep),
                SRow::Opt(OptK::SleepMode),
                SRow::Opt(OptK::IdleSleep),
                SRow::Act(ActK::Restart),
                SRow::Act(ActK::PowerOff),
                SRow::Act(ActK::Logout),
            ],
        }
    }

    /// The selected row's index among the selectable ones.
    fn cur_of(s: &Sub, sel_rows: &[&SRow]) -> usize {
        s.sel_key.as_ref().and_then(|k| sel_rows.iter().position(|r| *r == k)).unwrap_or(s.sel).min(sel_rows.len().saturating_sub(1))
    }

    pub(super) fn sub_button(&mut self, b: Button, repeat: bool, v: &View) {
        let Some(kind) = self.top_sub().map(|s| s.kind) else { return };
        if kind == SubKind::Mixer {
            self.mixer_button(b, repeat, v);
            return;
        }
        let Some(s) = self.top_sub() else { return };
        let rows = self.page_rows(s, v);
        let sel_rows: Vec<&SRow> = rows.iter().filter(|r| r.selectable()).collect();
        let cur = Self::cur_of(s, &sel_rows);
        let row = sel_rows.get(cur).map(|r| (*r).clone());
        let Some(s) = self.top_sub_mut() else { return };
        s.sel = cur;
        if s.adjusting {
            match b {
                Button::Left | Button::Right => {
                    if let Some(SRow::Level(l)) = row {
                        self.adjust_level(l, b == Button::Right, v);
                    }
                    return;
                }
                Button::A | Button::B => {
                    if !repeat {
                        s.adjusting = false;
                        self.fx(Sfx::Tab, Some(Haptic::Tick));
                    }
                    return;
                }
                // anything else leaves adjust mode and does what it does
                _ => s.adjusting = false,
            }
        }
        match b {
            Button::Up | Button::Down | Button::L1 | Button::R1 => {
                let last = sel_rows.len().saturating_sub(1);
                let n = match b {
                    Button::Up => cur.saturating_sub(1),
                    Button::Down => (cur + 1).min(last),
                    Button::L1 => cur.saturating_sub(PAGE_ROWS),
                    _ => (cur + PAGE_ROWS).min(last),
                };
                if n == cur {
                    self.fx(Sfx::Deny, None);
                } else {
                    s.sel = n;
                    s.sel_key = sel_rows.get(n).map(|r| (*r).clone());
                    s.confirm = None;
                    self.fx(Sfx::Tick, Some(Haptic::Tick));
                }
            }
            Button::B | Button::Left if !repeat => self.close_sub(v),
            Button::Y if !repeat && kind == SubKind::Wifi && v.net.is_some() => {
                self.out.push(Out::Setup(Request::WifiScan));
                self.fx(Sfx::Tab, Some(Haptic::Tick));
            }
            Button::X if !repeat => match row {
                Some(SRow::Dev(addr)) => {
                    let name = dev_name(v, &addr);
                    self.out.push(Out::Setup(Request::BtForget { address: addr.clone() }));
                    self.show_toast(Toast { sub: name, sound: None, ..Toast::info("Forgot") });
                    self.fx(Sfx::Close, Some(Haptic::Double));
                }
                _ => self.fx(Sfx::Deny, None),
            },
            // ▶ goes forward: opens a page, picks the next value, adjusts
            Button::Right if !repeat => match row {
                Some(r @ (SRow::Link(_) | SRow::Region(..) | SRow::Opt(_) | SRow::Level(_))) => self.sub_activate(kind, Some(r), v),
                _ => self.fx(Sfx::Deny, None),
            },
            Button::A | Button::Start if !repeat => self.sub_activate(kind, row, v),
            _ => {}
        }
    }

    /// ◀▶ while a page's level row is adjusted.
    fn adjust_level(&mut self, l: Lv, up: bool, v: &View) {
        let d = if up { 10 } else { -10 };
        match l {
            Lv::Brightness if v.sys.brightness.is_some() => self.out.push(Out::Brightness(d)),
            Lv::Volume if v.volume.is_some() => self.out.push(Out::Volume(d)),
            Lv::UiVolume => {
                let nl = (self.prefs.sound_volume as i32 + d).clamp(0, 100) as u8;
                self.prefs.sound_volume = nl;
                self.out.push(Out::Prefs);
                self.fx(Sfx::Vol(nl), Some(Haptic::Tick));
            }
            _ => self.fx(Sfx::Deny, None),
        }
    }

    fn sub_activate(&mut self, kind: SubKind, row: Option<SRow>, v: &View) {
        let net = v.net;
        // a confirming row: the second A does it, any other row forgets it
        if let Some(r) = row.as_ref().filter(|r| r.confirms() || matches!(r, SRow::Act(ActK::Update) if v.update.is_some_and(|u| u.ready))) {
            let Some(s) = self.top_sub_mut() else { return };
            if s.confirm.as_ref() != Some(r) {
                s.confirm = Some(r.clone());
                self.fx(Sfx::Deny, Some(Haptic::Double));
                return;
            }
            s.confirm = None;
        }
        match (kind, row) {
            (_, Some(SRow::Level(l))) => {
                let has = match l {
                    Lv::Brightness => v.sys.brightness.is_some(),
                    Lv::Volume => v.volume.is_some(),
                    Lv::UiVolume => true,
                };
                match self.top_sub_mut() {
                    Some(s) if has => {
                        s.adjusting = true;
                        self.fx(Sfx::Tab, Some(Haptic::Tick));
                    }
                    _ => self.fx(Sfx::Deny, None),
                }
            }
            (_, Some(SRow::Opt(k))) => {
                let p = &mut self.prefs;
                match k {
                    OptK::Palette => p.night = !p.night,
                    OptK::Motion => {
                        let i = Preset::ALL.iter().position(|x| *x == p.motion).unwrap_or(0);
                        p.motion = Preset::ALL[(i + 1) % Preset::ALL.len()];
                    }
                    OptK::ReduceMotion => p.reduce_motion = !p.reduce_motion,
                    OptK::Screensaver => {
                        let i = SAVER_CHOICES.iter().position(|m| *m == p.screensaver).unwrap_or(2);
                        p.screensaver = SAVER_CHOICES[(i + 1) % SAVER_CHOICES.len()];
                    }
                    OptK::UiSounds => p.sounds = !p.sounds,
                    OptK::Haptics => p.haptics = !p.haptics,
                    OptK::SleepMode => p.suspend = !p.suspend,
                    OptK::IdleSleep => {
                        let i = IDLE_CHOICES.iter().position(|m| *m == p.idle_minutes).unwrap_or(0);
                        p.idle_minutes = IDLE_CHOICES[(i + 1) % IDLE_CHOICES.len()];
                    }
                    OptK::AutoUpdate => {
                        if let Some(u) = v.update {
                            self.out.push(Out::UpdateAuto(!u.auto_check));
                        }
                        self.fx(Sfx::Tab, Some(Haptic::Tick));
                        return;
                    }
                }
                self.out.push(Out::Prefs);
                // heard and felt with the new value
                self.fx(Sfx::Tab, Some(Haptic::Tick));
            }
            (_, Some(SRow::Link(k))) => self.open_sub(k, v),
            (_, Some(SRow::Act(a))) => match a {
                ActK::Sleep => {
                    self.close_menu();
                    self.out.push(Out::Sleep);
                }
                ActK::Restart => {
                    self.out.push(Out::Reboot);
                    self.fx(Sfx::Close, Some(Haptic::Long));
                }
                ActK::PowerOff => {
                    self.out.push(Out::PowerOff);
                    self.fx(Sfx::Close, Some(Haptic::Long));
                }
                ActK::Logout => {
                    self.out.push(Out::Logout);
                    self.fx(Sfx::Close, Some(Haptic::Long));
                }
                ActK::Update => match v.update {
                    Some(u) if u.ready => {
                        self.out.push(Out::Reboot);
                        self.fx(Sfx::Close, Some(Haptic::Long));
                    }
                    Some(u) if u.busy => self.show_toast(Toast::info(u.message.clone())),
                    Some(_) => {
                        self.out.push(Out::UpdateCheck);
                        self.show_toast(Toast::info("Checking for updates…"));
                    }
                    None => self.fx(Sfx::Deny, None),
                },
            },
            (_, Some(SRow::Fact(f))) => {
                let (t, s) = fact_text(f, v);
                self.show_toast(Toast { sub: s, sound: None, ..Toast::info(t) });
            }
            (_, Some(SRow::Region(r, _))) if r == "UTC" => self.set_zone("UTC".into()),
            (_, Some(SRow::Region(r, _))) => {
                self.open_sub(SubKind::TzCities, v);
                let cities = tz_cities(&self.tz_zones, &r);
                let cur = cities.iter().position(|c| matches!(c, SRow::Zone(i) if self.tz_zones[*i].id == v.timezone)).unwrap_or(0);
                if let Some(s) = self.top_sub_mut() {
                    s.region = Some(r);
                    s.sel = cur;
                    s.sel_key = cities.get(cur).cloned();
                }
            }
            (_, Some(SRow::Zone(i))) => {
                if let Some(z) = self.tz_zones.get(i) {
                    self.set_zone(z.id.clone());
                }
            }
            (SubKind::Wifi, Some(SRow::Toggle)) => match v.radios.wifi {
                Some(on) => {
                    self.out.push(Out::Radio(crate::radio::Radio::Wifi, !on));
                    self.fx(Sfx::Tab, Some(Haptic::Tick));
                }
                None => self.fx(Sfx::Deny, None),
            },
            (SubKind::Bluetooth, Some(SRow::Toggle)) => {
                let on = bt_on(v);
                if !on {
                    if v.radios.bluetooth == Some(false) {
                        self.out.push(Out::Radio(crate::radio::Radio::Bluetooth, true));
                    }
                    if net.is_some() {
                        self.out.push(Out::Setup(Request::BtPower { on: true }));
                    }
                } else if v.radios.bluetooth.is_some() {
                    self.out.push(Out::Radio(crate::radio::Radio::Bluetooth, false));
                } else {
                    self.out.push(Out::Setup(Request::BtPower { on: false }));
                }
                if v.radios.bluetooth.is_none() && net.is_none() {
                    self.fx(Sfx::Deny, None);
                } else {
                    self.fx(Sfx::Tab, Some(Haptic::Tick));
                }
            }
            (SubKind::Wifi, Some(SRow::Conn | SRow::Details)) => {
                let Some(w) = net.map(|n| &n.wifi) else { return };
                let ssid = w.ssid.clone().unwrap_or_default();
                let bars = w.networks.iter().find(|x| x.ssid == ssid).map(|x| x.bars).unwrap_or(0);
                let mut parts = vec![w.address.clone().unwrap_or_else(|| "no address yet".into())];
                if let Some(f) = w.frequency {
                    parts.push(freq_label(f));
                }
                parts.push(format!("signal {bars}/4"));
                self.show_toast(Toast { sub: parts.join(" · "), ..Toast::info(ssid) });
            }
            (SubKind::Wifi, Some(SRow::Forget)) => {
                let Some(ssid) = net.and_then(|n| n.wifi.ssid.clone()) else { return };
                self.out.push(Out::Setup(Request::WifiForget { ssid: ssid.clone() }));
                if let Some(s) = self.top_sub_mut() {
                    s.sel_key = Some(SRow::Toggle);
                }
                self.show_toast(Toast { sub: ssid, sound: None, ..Toast::info("Forgot") });
                self.fx(Sfx::Close, Some(Haptic::Double));
            }
            (SubKind::Wifi, Some(SRow::Join(ssid))) => {
                let Some(x) = net.and_then(|n| n.wifi.networks.iter().find(|x| x.ssid == ssid)) else { return };
                if x.known || x.security == "open" {
                    self.out.push(Out::Setup(Request::WifiConnect { ssid: ssid.clone(), passphrase: None }));
                    self.show_toast(Toast { sub: ssid, sound: None, ..Toast::info("Joining") });
                    self.fx(Sfx::Open, Some(Haptic::Double));
                } else if x.security == "psk" {
                    // the setup service's password keyboard, then back here
                    self.out.push(Out::WifiPassword(ssid));
                    self.fx(Sfx::Open, Some(Haptic::Tick));
                } else {
                    self.show_toast(Toast { sub: ssid, ..Toast::info("This kind of network is not supported") });
                    self.fx(Sfx::Deny, None);
                }
            }
            (SubKind::Wifi, Some(SRow::Other)) => {
                // WPS and the full list live on the setup service's page
                self.out.push(Out::OpenSetup(SetupPage::Wifi));
                self.fx(Sfx::Open, Some(Haptic::Tick));
            }
            (SubKind::Bluetooth, Some(SRow::Dev(addr))) => {
                let Some(d) = net.and_then(|n| n.bt.devices.iter().find(|d| d.address == addr)) else { return };
                if d.connected {
                    self.out.push(Out::Setup(Request::BtDisconnect { address: addr }));
                } else {
                    self.out.push(Out::Setup(Request::BtConnect { address: addr }));
                }
                self.fx(Sfx::Tab, Some(Haptic::Tick));
            }
            (SubKind::Bluetooth, Some(SRow::Scan)) => {
                let on = !net.is_some_and(|n| n.bt.discovering);
                self.out.push(Out::Setup(Request::BtScan { on }));
                self.fx(Sfx::Tab, Some(Haptic::Tick));
            }
            (SubKind::Bluetooth, Some(SRow::Found(addr))) => {
                let name = dev_name(v, &addr);
                self.out.push(Out::Setup(Request::BtPair { address: addr }));
                self.show_toast(Toast { sub: name, sound: None, ..Toast::info("Pairing") });
                self.fx(Sfx::Open, Some(Haptic::Double));
            }
            _ => self.fx(Sfx::Deny, None),
        }
    }

    /// A zone was picked: set it and go back to Settings (both time zone
    /// pages slide out).
    fn set_zone(&mut self, id: String) {
        self.out.push(Out::SetTimeZone(id));
        let reduce = self.prefs.reduce_motion;
        if let Some(m) = &mut self.menu {
            for s in m.subs.iter_mut().filter(|s| matches!(s.kind, SubKind::TimeZone | SubKind::TzCities)) {
                s.closing = true;
                s.x.set(1.0);
                if reduce {
                    s.x.snap(1.0);
                }
            }
        }
        self.fx(Sfx::Open, Some(Haptic::Double));
    }

    fn mixer_button(&mut self, b: Button, repeat: bool, v: &View) {
        let ch = channels(v);
        let prefs = self.prefs.clone();
        let Some(s) = self.top_sub_mut() else { return };
        s.mix_sel = s.mix_sel.min(ch.len().saturating_sub(1));
        let c = ch[s.mix_sel];
        let (level, muted, live) = chan_state(c, v, &prefs);
        match b {
            // ◀ on the first channel goes back, like every page
            Button::Left if s.mix_sel == 0 && !repeat => self.close_sub(v),
            Button::Left | Button::Right => {
                let n = if b == Button::Left { s.mix_sel.saturating_sub(1) } else { (s.mix_sel + 1).min(ch.len() - 1) };
                if n == s.mix_sel {
                    self.fx(Sfx::Deny, None);
                } else {
                    s.mix_sel = n;
                    self.fx(Sfx::Tick, Some(Haptic::Tick));
                }
            }
            Button::Up | Button::Down => {
                let d: i32 = if b == Button::Up { 5 } else { -5 };
                if !live {
                    self.fx(Sfx::Deny, None);
                    return;
                }
                let nl = (level as i32 + d).clamp(0, 100) as u8;
                match c {
                    Chan::Master => self.out.push(Out::Volume(d)),
                    Chan::App(u) => {
                        self.out.push(Out::MixLevel(u, nl));
                        self.fx(Sfx::Vol(nl), Some(Haptic::Tick));
                    }
                    Chan::Ui => {
                        self.prefs.sound_volume = nl;
                        self.out.push(Out::Prefs);
                        self.fx(Sfx::Vol(nl), Some(Haptic::Tick));
                    }
                }
            }
            Button::A if !repeat => {
                if !live {
                    self.fx(Sfx::Deny, None);
                    return;
                }
                match c {
                    Chan::Master => self.out.push(Out::MasterMute(!muted)),
                    Chan::App(u) => self.out.push(Out::MixMute(u, !muted)),
                    Chan::Ui => {
                        self.prefs.sounds = muted;
                        self.out.push(Out::Prefs);
                    }
                }
                self.fx(Sfx::Tab, Some(Haptic::Tick));
            }
            Button::B if !repeat => self.close_sub(v),
            _ => {}
        }
    }

    pub(super) fn sub_hints(&self, s: &Sub, v: &View) -> Vec<(&'static str, &'static str)> {
        if s.kind == SubKind::Mixer {
            let ch = channels(v);
            let c = ch.get(s.mix_sel).copied().unwrap_or(Chan::Master);
            let muted = chan_state(c, v, &self.prefs).1;
            return vec![("◀▶", "Channel"), ("▲▼", "Level"), ("A", if muted { "Unmute" } else { "Mute" }), ("B", "Back")];
        }
        if s.adjusting {
            return vec![("◀▶", "Adjust"), ("A", "Done"), ("MENU", "Close")];
        }
        let rows = self.page_rows(s, v);
        let sel: Vec<&SRow> = rows.iter().filter(|r| r.selectable()).collect();
        let cur = Self::cur_of(s, &sel);
        let mut h = vec![("▲▼", "Choose")];
        if matches!(s.kind, SubKind::TimeZone | SubKind::TzCities | SubKind::Controls) {
            h.push(("L1/R1", "Page"));
        }
        let a = match sel.get(cur) {
            Some(SRow::Toggle) => Some("Switch"),
            Some(SRow::Dev(addr)) => {
                if v.net.and_then(|n| n.bt.devices.iter().find(|d| d.address == *addr)).is_some_and(|d| d.connected) {
                    Some("Disconnect")
                } else {
                    Some("Connect")
                }
            }
            Some(SRow::Join(_)) => Some("Join"),
            Some(SRow::Found(_)) => Some("Pair"),
            Some(SRow::Forget) => Some("Forget"),
            Some(SRow::Scan) if v.net.is_some_and(|n| n.bt.discovering) => Some("Stop"),
            Some(SRow::Scan) => Some("Search"),
            Some(SRow::Level(_)) => Some("Adjust"),
            Some(SRow::Opt(_)) => Some("Change"),
            Some(SRow::Act(ActK::Sleep)) => Some("Sleep"),
            Some(SRow::Act(ActK::Update)) if v.update.is_some_and(|u| u.ready) => Some("Restart"),
            Some(SRow::Act(ActK::Update)) => Some("Check"),
            Some(r) if s.confirm.as_ref() == Some(*r) => Some("Confirm"),
            Some(SRow::Act(ActK::Restart)) => Some("Restart"),
            Some(SRow::Act(ActK::PowerOff)) => Some("Power off"),
            Some(SRow::Act(ActK::Logout)) => Some("Log out"),
            Some(SRow::Fact(_)) => Some("Show"),
            Some(SRow::Region(n, _)) if n == "UTC" => Some("Set"),
            Some(SRow::Zone(_)) => Some("Set"),
            Some(SRow::Key(_)) => None,
            Some(_) => Some("Open"),
            None => None,
        };
        if let Some(a) = a {
            h.push(("A", a));
        }
        if matches!(sel.get(cur), Some(SRow::Dev(_))) {
            h.push(("X", "Forget"));
        }
        if s.kind == SubKind::Wifi && v.net.is_some() {
            h.push(("Y", "Scan"));
        }
        h.push(("B", "Back"));
        h
    }

    // --------------------------------------------------------- drawing ----

    pub(super) fn draw_sub(&self, b: &mut B, pal: &Pal, v: &View, m: &Cross, s: &Sub, g: &Glass) {
        let dx = s.x.x * W;
        b.push(Xf::translate(dx, 0.0));
        let (y0, h) = (BAR_H, H - BAR_H - HINT_H);
        let accent = m.cat.accent(pal);
        glass(b, pal, g, 0.0, y0, W, h, 0.62);
        // header: serif title, a mono line, an icon block with a hard shadow
        let title = match s.kind {
            SubKind::TzCities => s.region.clone().unwrap_or_default(),
            k => k.title().to_string(),
        };
        let line = self.page_line(s, v);
        let tst = ts(Face::SerifItalic, 64.0, pal.ink).line_height(1.0);
        let top = y0 + 22.0;
        let tw = b.text(40.0, top, &title, &tst);
        let mono = ts(Face::Mono400, 18.0, pal.soft);
        let base = top + 64.0 * 0.82;
        let lw = W - 40.0 - 64.0 - 24.0 - (40.0 + tw + 18.0);
        let line = b.fit(&line, &mono, lw);
        b.text_base(40.0 + tw + 18.0, base, &line, &mono);
        let (bx, by) = (W - 40.0 - 64.0 - 8.0, top);
        b.rect(bx + 8.0, by + 8.0, 64.0, 64.0, pal.dark);
        b.rect(bx, by, 64.0, 64.0, if s.kind == SubKind::Mixer { accent } else { Cat::Settings.accent(pal) });
        b.frame(bx, by, 64.0, 64.0, 2.0, pal.dark);
        draw_icon(b, s.kind.icon(), bx + 32.0, by + 32.0, 40.0, pal, pal.dark);
        if s.kind == SubKind::Mixer {
            self.draw_mixer(b, pal, v, s);
        } else {
            self.draw_sub_rows(b, pal, v, s);
        }
        b.pop();
    }

    /// The header's mono line.
    fn page_line(&self, s: &Sub, v: &View) -> String {
        let p = &self.prefs;
        match s.kind {
            SubKind::Wifi => wifi_line(v),
            SubKind::Bluetooth => bt_line(v),
            SubKind::Mixer => "Each app's level, then the master".into(),
            SubKind::Display => format!("{} · {}{}", if p.night { "Night" } else { "Day" }, p.motion.name(), if p.reduce_motion { " · reduced" } else { "" }),
            SubKind::Sound => match v.volume {
                Some(x) if v.muted => format!("Muted ({x}%) · UI sounds {}", if p.sounds { "on" } else { "off" }),
                Some(x) => format!("Volume {x}% · UI sounds {}", if p.sounds { "on" } else { "off" }),
                None => format!("UI sounds {}", if p.sounds { "on" } else { "off" }),
            },
            SubKind::TimeZone => format!("{} · {}", crate::tz::pretty(v.timezone).0, v.sys.time),
            SubKind::TzCities => format!("{} zones · now {}", tz_cities(&self.tz_zones, s.region.as_deref().unwrap_or("")).len(), crate::tz::pretty(v.timezone).0),
            SubKind::Updates => v.update.map(|u| u.message.clone()).unwrap_or_else(|| "Off".into()),
            SubKind::About => format!("{} · {}", v.host, v.dash.about.and_then(|a| a.version.clone()).unwrap_or_else(|| "Mortar".into())),
            SubKind::Controls => "Every button and chord".into(),
            SubKind::Power => if p.suspend { "Sleep suspends · power key wakes".into() } else { "Sleep turns the screen off".into() },
        }
    }

    fn draw_sub_rows(&self, b: &mut B, pal: &Pal, v: &View, s: &Sub) {
        let rows = self.page_rows(s, v);
        let sel_rows: Vec<&SRow> = rows.iter().filter(|r| r.selectable()).collect();
        let cur = Self::cur_of(s, &sel_rows);
        // where the selection is, to scroll it into view
        let mut y = 0.0;
        let mut sel_y = 0.0;
        let mut idx = 0;
        for r in &rows {
            if r.selectable() {
                if idx == cur {
                    sel_y = y;
                }
                idx += 1;
            }
            y += if matches!(r, SRow::Label(_)) { LBL_H } else { ROW_H };
        }
        let view_h = ROWS_BOTTOM - ROWS_TOP - 8.0;
        // whole rows: never a half-cut row under the header
        let off = (sel_scroll(sel_y, y, view_h) / ROW_H).ceil() * ROW_H;
        // rows scrolled up are cut under the header, not drawn over it
        b.clip(Some([0.0, ROWS_TOP - 4.0, W, ROWS_BOTTOM - ROWS_TOP + 4.0]));
        let mut y = ROWS_TOP - off;
        let mut idx = 0;
        let sel_col = Cat::Settings.accent(pal);
        for r in &rows {
            if let SRow::Label(t) = r {
                let st = ts(Face::Mono400, 16.0, pal.soft).tracking(0.08);
                if y + LBL_H > ROWS_TOP - 8.0 && y < ROWS_BOTTOM {
                    b.text(40.0, y + 16.0, &t.to_uppercase(), &st);
                }
                y += LBL_H;
                continue;
            }
            let on = r.selectable() && idx == cur;
            if r.selectable() {
                idx += 1;
            }
            if y > ROWS_BOTTOM || y + ROW_H < ROWS_TOP - 8.0 {
                y += ROW_H;
                continue;
            }
            let confirm = on && s.confirm.as_ref() == Some(r);
            if on {
                b.rect(0.0, y, W, ROW_H, if confirm { pal.orange } else { sel_col });
            }
            let (title, mut sub) = self.row_text(s.kind, r, v);
            if confirm {
                sub = format!("Press A again to {}", title.to_lowercase());
            } else if on && s.adjusting {
                sub = "◀▶ to change · A when done".into();
            }
            let dis = matches!(r, SRow::Note(..));
            let ink = if on { pal.dark } else { pal.ink };
            b.push_alpha(if dis { 0.4 } else { 1.0 });
            let tst = if on { ts(Face::SerifItalic, 42.0, ink).line_height(1.0) } else { ts(Face::Body600, 30.0, ink).line_height(1.1) };
            let sst = ts(Face::Mono400, 18.0, if on { pal.dark } else { pal.soft });
            let sub_h = if sub.is_empty() { 0.0 } else { sst.line_px() };
            let ttop = y + ROW_H / 2.0 - (tst.line_px() + sub_h) / 2.0;
            let rw = self.draw_row_right(b, pal, v, s, r, on, y);
            let maxw = W - 40.0 - rw - 40.0 - 16.0;
            let t = b.fit(&title, &tst, maxw);
            b.text(40.0, ttop, &t, &tst);
            let sb = b.fit(&sub, &sst, maxw);
            arrow_text(b, 40.0, ttop + tst.line_px(), &sb, &sst, true);
            b.pop_alpha();
            y += ROW_H;
        }
        b.clip(None);
    }

    /// Title and subtitle of a row.
    fn row_text(&self, kind: SubKind, r: &SRow, v: &View) -> (String, String) {
        let w = v.net.map(|n| &n.wifi);
        let bt = v.net.map(|n| &n.bt);
        let p = &self.prefs;
        match r {
            SRow::Toggle if kind == SubKind::Bluetooth => {
                let on = bt_on(v);
                ("Bluetooth".into(), if on { format!("On · visible as “{}”", v.host) } else { "Off".into() })
            }
            SRow::Toggle => ("Wi-Fi".into(), if wifi_on(v) { "On".into() } else { "Off".into() }),
            SRow::Note(t, s) => (t.clone(), s.clone()),
            SRow::Conn => {
                let w = w.cloned().unwrap_or_default();
                let ssid = w.ssid.clone().unwrap_or_default();
                let sub = if w.state == "connecting" {
                    "Joining…".to_string()
                } else {
                    let sec = w.networks.iter().find(|x| x.ssid == ssid).map(|x| if x.security == "open" { "open" } else { "secured" }).unwrap_or("secured");
                    let mut p = vec![w.address.clone().unwrap_or_else(|| "getting an address…".into())];
                    if let Some(f) = w.frequency {
                        p.push(freq_label(f));
                    }
                    p.push(sec.to_string());
                    p.join(" · ")
                };
                (ssid, sub)
            }
            SRow::Details => ("Details".into(), "Address, signal, channel".into()),
            SRow::Forget => ("Forget this network".into(), "You'll need the password again".into()),
            SRow::Join(ssid) => {
                let x = w.and_then(|w| w.networks.iter().find(|x| x.ssid == *ssid));
                let mut sub = match x.map(|x| x.security.as_str()) {
                    Some("open") => "Open".to_string(),
                    Some("psk") | None => "Secured".to_string(),
                    Some(_) => "Enterprise · not supported".to_string(),
                };
                if x.is_some_and(|x| x.known) {
                    sub.push_str(" · saved");
                }
                (ssid.clone(), sub)
            }
            SRow::Other => ("Other network…".into(), "WPS and the full list".into()),
            SRow::Dev(addr) => {
                let d = bt.and_then(|b| b.devices.iter().find(|d| d.address == *addr));
                let busy = bt.is_some_and(|b| b.busy.as_deref() == Some(addr.as_str()));
                let (name, kind, conn) = d.map(|d| (d.name.clone(), d.kind_label(), d.connected)).unwrap_or_default();
                let state = if busy {
                    "connecting…"
                } else if conn {
                    "connected · A to disconnect"
                } else {
                    "not connected · A to connect"
                };
                let mut sub = format!("{kind} · {state}");
                if let Some(bat) = d.and_then(|d| d.battery) {
                    sub = format!("{kind} · {bat}% · {state}");
                }
                (name, sub)
            }
            SRow::Scan => {
                let disc = bt.is_some_and(|b| b.discovering);
                match bt.and_then(|b| b.passkey.clone()) {
                    Some(code) => ("Pairing…".to_string(), format!("Code {code}. If the device shows it too, confirm it there")),
                    None if disc => ("Searching…".to_string(), "Put the device in pairing mode · A to stop".to_string()),
                    None => ("Pair new device".to_string(), "Search for headphones, controllers, keyboards".to_string()),
                }
            }
            SRow::Found(addr) => {
                let d = bt.and_then(|b| b.devices.iter().find(|d| d.address == *addr));
                let busy = bt.is_some_and(|b| b.busy.as_deref() == Some(addr.as_str()));
                let name = d.map(|d| d.name.clone()).unwrap_or_else(|| addr.clone());
                let kind = d.map(|d| d.kind_label()).unwrap_or("Device");
                (name, if busy { format!("{kind} · pairing…") } else { format!("{kind} · A to pair") })
            }
            SRow::Level(Lv::Brightness) => ("Brightness".into(), if v.sys.brightness.is_some() { "A to adjust".into() } else { "No backlight".into() }),
            SRow::Level(Lv::Volume) => ("Volume".into(), if v.muted { "Muted · A to adjust".into() } else { "The speaker or headphones".into() }),
            SRow::Level(Lv::UiVolume) => ("UI sound level".into(), "Clicks and chimes, before the volume".into()),
            SRow::Opt(OptK::Palette) => ("Palette".into(), "Day: light ground · Night: dark".into()),
            SRow::Opt(OptK::Motion) => ("Motion".into(), "How the springs feel".into()),
            SRow::Opt(OptK::ReduceMotion) => ("Reduce motion".into(), "No overshoot; patterns stand still".into()),
            SRow::Opt(OptK::Screensaver) => ("Screensaver".into(), "The cross's pattern fills the screen when idle".into()),
            SRow::Opt(OptK::UiSounds) => ("UI sounds".into(), "Clicks, swooshes, the chime".into()),
            SRow::Opt(OptK::Haptics) => ("Vibration".into(), "A tick on every move".into()),
            SRow::Opt(OptK::SleepMode) => ("Sleep mode".into(), if p.suspend { "Saves the battery · apps pause".into() } else { "Apps keep running · uses more battery".into() }),
            SRow::Opt(OptK::IdleSleep) => ("Sleep when idle".into(), "With no button pressed".into()),
            SRow::Opt(OptK::AutoUpdate) => ("Auto-check daily".into(), "Downloads in the background".into()),
            SRow::Link(SubKind::Mixer) => ("Mixer".into(), "A level for each app".into()),
            SRow::Link(SubKind::Controls) => ("Controls".into(), "Every button and chord".into()),
            SRow::Link(k) => (k.title().into(), String::new()),
            SRow::Act(ActK::Sleep) => ("Sleep".into(), if p.suspend { "Suspend · the power key wakes it".into() } else { "Screen off · any button wakes it".into() }),
            SRow::Act(ActK::Restart) => ("Restart".into(), "Apps close".into()),
            SRow::Act(ActK::PowerOff) => ("Power off".into(), "Apps close".into()),
            SRow::Act(ActK::Logout) => ("Log out".into(), "Apps close; the greeter appears".into()),
            SRow::Act(ActK::Update) => match v.update {
                Some(u) if u.ready => ("Restart to update".into(), u.message.clone()),
                Some(u) if u.busy => (u.message.clone(), "Working in the background".into()),
                Some(u) => ("Check now".into(), u.message.clone()),
                None => ("Updates".into(), "Off".into()),
            },
            SRow::Fact(f) => fact_text(*f, v),
            SRow::Key(i) => {
                let (_, _, what, detail) = CONTROLS[*i];
                (what.into(), detail.into())
            }
            SRow::Region(n, _) if n == "UTC" => ("UTC".into(), "Coordinated Universal Time".into()),
            SRow::Region(n, c) => (n.clone(), format!("{c} zones")),
            SRow::Zone(i) => {
                let z = &self.tz_zones[*i];
                let (city, sub) = crate::tz::pretty(&z.id);
                (city, sub.unwrap_or_default())
            }
            SRow::Label(_) => (String::new(), String::new()),
        }
    }

    /// The right-hand column: switches, values, signal, tags. Returns its width.
    #[allow(clippy::too_many_arguments)]
    fn draw_row_right(&self, b: &mut B, pal: &Pal, v: &View, s: &Sub, r: &SRow, on: bool, y: f32) -> f32 {
        let cy = y + ROW_H / 2.0;
        let right = W - 40.0;
        let mono = ts(Face::Mono400, 18.0, if on { pal.dark } else { pal.ink });
        let ink = if on { pal.dark } else { pal.ink };
        let wifi = v.net.map(|n| &n.wifi);
        let chevron = |b: &mut B| {
            let ch = ts(Face::Mono400, 30.0, ink);
            b.text_right(right, cy - ch.line_px() / 2.0, "›", &ch)
        };
        match r {
            SRow::Toggle => {
                let state = if s.kind == SubKind::Bluetooth { bt_on(v) } else { wifi_on(v) };
                pill(b, pal, right - 64.0, cy - 17.0, state);
                64.0
            }
            SRow::Conn => {
                let ssid = wifi.and_then(|w| w.ssid.clone()).unwrap_or_default();
                let bars = wifi.and_then(|w| w.networks.iter().find(|x| x.ssid == ssid)).map(|x| x.bars).unwrap_or(0);
                sig(b, pal, right, cy, bars, ink)
            }
            SRow::Join(ssid) => {
                let bars = wifi.and_then(|w| w.networks.iter().find(|x| x.ssid == *ssid)).map(|x| x.bars).unwrap_or(0);
                sig(b, pal, right, cy, bars, ink)
            }
            SRow::Details | SRow::Other | SRow::Link(_) => chevron(b),
            SRow::Dev(addr) => {
                let conn = v.net.and_then(|n| n.bt.devices.iter().find(|d| d.address == *addr)).is_some_and(|d| d.connected);
                let mut x = right;
                if on {
                    let lab = ts(Face::Mono400, 18.0, pal.dark);
                    x -= b.text_right(x, cy - lab.line_px() / 2.0, "Forget", &lab) + 8.0;
                    keycap(b, x, cy, "X");
                    x -= 38.0 + 14.0;
                }
                let t = if conn { "On" } else { "Off" };
                let tw = b.measure(t, &mono) + 16.0;
                let th = mono.line_px() + 4.0;
                b.rect(x - tw, cy - th / 2.0, tw, th, DAY.ground);
                b.frame(x - tw, cy - th / 2.0, tw, th, 2.0, pal.dark);
                b.text(x - tw + 8.0, cy - mono.line_px() / 2.0, t, &ts(Face::Mono400, 18.0, pal.dark));
                right - (x - tw)
            }
            SRow::Scan => {
                if v.net.is_some_and(|n| n.bt.discovering) {
                    // a slow turning ring while it searches
                    let a = (self.t as f32 * 4.0) % std::f32::consts::TAU;
                    let pts: Vec<P> = (0..=20)
                        .map(|i| {
                            let t = a + i as f32 / 20.0 * 4.4;
                            (right - 12.0 + 11.0 * t.cos(), cy + 11.0 * t.sin())
                        })
                        .collect();
                    b.stroke(&pts, false, 2.0, ink);
                    26.0
                } else {
                    chevron(b)
                }
            }
            SRow::Level(l) => {
                let lv = match l {
                    Lv::Brightness => v.sys.brightness,
                    Lv::Volume => v.volume.map(|x| if v.muted { 0 } else { x }),
                    Lv::UiVolume => Some(self.prefs.sound_volume),
                };
                let Some(lv) = lv else { return 0.0 };
                let num = ts(Face::Mono400, 22.0, ink);
                let nw = b.measure("100%", &num);
                b.text_right(right, cy - num.line_px() / 2.0, &format!("{lv}%"), &num);
                let (bw, bh) = (150.0, 18.0);
                let bx = right - nw - 12.0 - bw;
                b.rect(bx, cy - bh / 2.0, bw, bh, if on && s.adjusting { pal.yellow } else { DAY.ground });
                b.rect(bx, cy - bh / 2.0, bw * lv.min(100) as f32 / 100.0, bh, pal.dark);
                b.frame(bx, cy - bh / 2.0, bw, bh, 2.0, pal.dark);
                right - bx
            }
            SRow::Opt(k) => {
                let (vals, chosen) = opt_values(*k, &self.prefs, v);
                let st_on = ts(Face::Mono600, 19.0, ink);
                let st_off = ts(Face::Mono400, 19.0, with_a(ink, 0.5));
                let mut segs: Vec<(String, TextStyle)> = Vec::new();
                for (j, o) in vals.iter().enumerate() {
                    if j > 0 {
                        segs.push((" · ".into(), st_off));
                    }
                    segs.push((o.to_string(), if j == chosen { st_on } else { st_off }));
                }
                let total: f32 = segs.iter().map(|(s, st)| b.measure(s, st)).sum();
                let mut x = right - total;
                for (s, st) in &segs {
                    x += b.text(x, cy - st.line_px() / 2.0, s, st);
                }
                total
            }
            SRow::Region(n, _) => {
                let mut w = 0.0;
                if n != "UTC" {
                    w = chevron(b) + 14.0;
                }
                let cur = crate::tz::region(v.timezone);
                if n == cur || (n == "UTC" && v.timezone == "UTC") {
                    w += chip(b, right - w, cy, "CURRENT", 15.0, (8.0, 2.0), pal.lavender, pal.lavender_ink);
                }
                w
            }
            SRow::Zone(i) => {
                let z = &self.tz_zones[*i];
                let txt = format!("{}  {}", crate::tz::hhmm_at(z.offset), crate::tz::fmt_offset(z.offset));
                let mut w = b.text_right(right, cy - mono.line_px() / 2.0, &txt, &mono);
                if z.id == v.timezone {
                    w += 14.0 + chip(b, right - w - 14.0, cy, "CURRENT", 15.0, (8.0, 2.0), pal.lavender, pal.lavender_ink);
                }
                w
            }
            SRow::Key(i) => {
                // the keys as caps, right-aligned
                // "+", "/", "then", "hold" are words between the caps
                let caps: Vec<&str> = CONTROLS[*i].1.split(' ').filter(|c| !c.is_empty()).collect();
                let word = ts(Face::Mono400, 18.0, ink);
                let mut x = right;
                for c in caps.iter().rev() {
                    if matches!(*c, "+" | "/" | "then" | "hold") {
                        x -= b.text_right(x, cy - word.line_px() / 2.0, c, &word) + 8.0;
                    } else {
                        x -= keycap(b, x, cy, c) + 8.0;
                    }
                }
                right - x
            }
            _ => 0.0,
        }
    }

    fn draw_mixer(&self, b: &mut B, pal: &Pal, v: &View, s: &Sub) {
        let ch = channels(v);
        let sel = s.mix_sel.min(ch.len().saturating_sub(1));
        // scroll sideways to keep the selected channel in view
        let total = ch.len() as f32 * (CH_W + CH_GAP) - CH_GAP;
        let avail = W - 80.0;
        let sx = if total <= avail { 0.0 } else { ((sel as f32 + 0.5) * (CH_W + CH_GAP) - avail / 2.0).clamp(0.0, total - avail) };
        let y = ROWS_TOP + 18.0;
        let hgt = 372.0;
        b.clip(Some([0.0, ROWS_TOP, W, ROWS_BOTTOM - ROWS_TOP]));
        for (i, c) in ch.iter().enumerate() {
            let x = 40.0 + i as f32 * (CH_W + CH_GAP) - sx;
            if x > W || x + CH_W < 0.0 {
                continue;
            }
            let on = i == sel;
            let (level, muted, live) = chan_state(*c, v, &self.prefs);
            let (name, col) = match c {
                Chan::Master => ("Master".to_string(), pal.ink),
                Chan::App(u) => v.app(*u).map(|a| (a.name.clone(), a.art.accent(pal))).unwrap_or((String::new(), pal.lavender)),
                Chan::Ui => ("UI sounds".to_string(), pal.lavender),
            };
            if on {
                // a hard shadow outside the box only (the box is see-through)
                b.rect(x + CH_W, y + 8.0, 8.0, hgt, pal.dark);
                b.rect(x + 8.0, y + hgt, CH_W - 8.0, 8.0, pal.dark);
                b.rect(x, y, CH_W, hgt, if pal.night { with_a(pal.ground_2, 0.6) } else { Color(255, 255, 255, 102) });
                b.frame(x, y, CH_W, hgt, 2.0, pal.dark);
            }
            b.push_alpha(if live { 1.0 } else { 0.4 });
            let cx = x + CH_W / 2.0;
            let nst = if on { ts(Face::SerifItalic, 32.0, pal.ink).line_height(1.2) } else { ts(Face::Body600, 22.0, pal.ink).line_height(1.2) };
            let nm = b.fit(&name, &nst, CH_W - 16.0);
            b.text_center(cx, y + 14.0 + (40.0 - nst.line_px()) / 2.0, &nm, &nst);
            // the fader: a track, the level in the channel's colour, a knob
            let (fx, fy, fh) = (cx - 35.0, y + 14.0 + 40.0 + 10.0, 220.0);
            let lv = level.min(100) as f32 * fh / 100.0;
            b.rect(fx + 22.0, fy, 14.0, fh, with_a(DAY.ground, 0.6));
            b.frame(fx + 22.0, fy, 14.0, fh, 2.0, pal.dark);
            if lv > 0.0 {
                b.push_alpha(if muted { 0.35 } else { 1.0 });
                b.rect(fx + 22.0, fy + fh - lv, 14.0, lv, col);
                b.frame(fx + 22.0, fy + fh - lv, 14.0, lv, 2.0, pal.dark);
                b.pop_alpha();
            }
            let ky = fy + fh - lv - 10.0;
            b.rect(fx + 10.0, ky + 4.0, 46.0, 20.0, pal.dark);
            b.rect(fx + 6.0, ky, 46.0, 20.0, DAY.ground);
            b.frame(fx + 6.0, ky, 46.0, 20.0, 2.0, pal.dark);
            let vst = ts(Face::Mono400, 20.0, pal.ink);
            let vy = fy + fh + 10.0;
            let vt = if !live {
                "no sound".to_string()
            } else if muted {
                "muted".to_string()
            } else {
                format!("{level}%")
            };
            b.text_center(cx, vy, &vt, &vst);
            let mst = ts(Face::Mono400, 17.0, pal.dark);
            let (mw, mh) = (44.0, 30.0);
            let my = vy + vst.line_px() + 10.0;
            let pts = crate::scene::rrect_pts(cx - mw / 2.0, my, mw, mh, 15.0);
            b.poly(&pts, if muted { pal.orange } else { DAY.ground });
            b.stroke(&pts, true, 2.0, pal.dark);
            b.text_center(cx, my + mh / 2.0 - mst.line_px() / 2.0, "M", &mst);
            b.pop_alpha();
        }
        b.clip(None);
    }
}

/// About's facts: (title, the whole line).
fn fact_text(f: FactK, v: &View) -> (String, String) {
    let a = v.dash.about;
    match f {
        FactK::Version => {
            let ver = a.and_then(|a| a.version.clone()).unwrap_or_else(|| "unknown".into());
            let sub = match a.and_then(|a| a.generation) {
                Some(g) => format!("NixOS {ver} · generation {g}"),
                None => format!("NixOS {ver}"),
            };
            ("Mortar".into(), sub)
        }
        FactK::Storage => ("Storage".into(), a.and_then(|a| a.storage_line()).unwrap_or_else(|| "Not known".into())),
        FactK::Battery => {
            let mut parts = Vec::new();
            if let Some(b) = v.sys.battery {
                parts.push(format!("{b}%{}", if v.sys.charging { " · charging" } else { "" }));
            }
            if let Some(bt) = a.map(|a| &a.battery) {
                if let Some(h) = &bt.health {
                    parts.push(format!("health {}", h.to_lowercase()));
                }
                if let Some(c) = bt.capacity_pct {
                    parts.push(format!("{c}% of new"));
                }
                if let Some(c) = bt.cycles {
                    parts.push(format!("{c} cycles"));
                }
            }
            ("Battery".into(), if parts.is_empty() { "No battery".into() } else { parts.join(" · ") })
        }
        FactK::Temp => ("Temperature".into(), a.and_then(|a| a.temp_c).map(|t| format!("SoC {t:.0} °C")).unwrap_or_else(|| "Not known".into())),
        FactK::Device => ("Device".into(), format!("{} · Linux {}", v.host, a.and_then(|a| a.kernel.clone()).unwrap_or_else(|| "?".into()))),
    }
}

/// A dark key cap ending at `right`, centred on `cy`. Returns its width.
fn keycap(b: &mut B, right: f32, cy: f32, label: &str) -> f32 {
    let cap = ts(Face::Mono600, 17.0, DAY.ground);
    let tw = arrow_text(b, 0.0, 0.0, label, &cap, false);
    let cw = (tw + 22.0).max(38.0);
    b.rrect(right - cw, cy - 19.0, cw, 38.0, 19.0, Color(29, 29, 29, 225));
    arrow_text(b, right - cw + (cw - tw) / 2.0, cy - cap.line_px() / 2.0, label, &cap, true);
    cw
}

/// Scroll so the selected row stays in view (rows above it shown when
/// they fit).
fn sel_scroll(sel_y: f32, total: f32, view_h: f32) -> f32 {
    (sel_y + ROW_H * 1.5 - view_h).max(0.0).min((total - view_h).max(0.0))
}

fn dev_name(v: &View, addr: &str) -> String {
    v.net.and_then(|n| n.bt.devices.iter().find(|d| d.address == addr)).map(|d| d.name.clone()).unwrap_or_else(|| addr.to_string())
}

fn freq_label(mhz: u32) -> String {
    if mhz >= 5900 {
        "6 GHz".into()
    } else if mhz >= 4900 {
        "5 GHz".into()
    } else {
        "2.4 GHz".into()
    }
}

fn wifi_line(v: &View) -> String {
    if !wifi_on(v) {
        return "Off".into();
    }
    match v.net.map(|n| &n.wifi) {
        Some(w) if w.state == "connected" => format!("Connected to {}", w.ssid.clone().unwrap_or_default()),
        Some(w) if w.state == "connecting" => format!("Joining {}…", w.ssid.clone().unwrap_or_default()),
        Some(w) if w.scanning => "Looking for networks…".into(),
        Some(w) => w.last_error.clone().unwrap_or_else(|| "Not connected".into()),
        None => "Not connected".into(),
    }
}

fn bt_line(v: &View) -> String {
    match v.net.map(|n| &n.bt) {
        Some(b) if bt_on(v) => {
            if let Some(e) = &b.last_error {
                return e.clone();
            }
            format!("{} connected", b.devices.iter().filter(|d| d.connected).count())
        }
        _ => "Off".into(),
    }
}

/// An on/off switch, 64 x 34.
fn pill(b: &mut B, pal: &Pal, x: f32, y: f32, on: bool) {
    let pts = crate::scene::rrect_pts(x, y, 64.0, 34.0, 17.0);
    b.poly(&pts, if on { pal.dark } else { DAY.ground });
    b.stroke(&pts, true, 2.0, pal.dark);
    let kx = if on { x + 64.0 - 4.0 - 11.0 } else { x + 4.0 + 11.0 };
    b.ellipse(kx, y + 17.0, 11.0, 11.0, if on { pal.lime } else { pal.dark });
}

/// Signal bars (0..4), right-aligned at `right`. Returns the width.
fn sig(b: &mut B, pal: &Pal, right: f32, cy: f32, bars: u8, ink: Color) -> f32 {
    let w = 4.0 * 7.0 + 3.0 * 3.0;
    for i in 0..4 {
        let h = 7.0 + i as f32 * 5.0;
        let x = right - w + i as f32 * 10.0;
        b.rect(x, cy + 11.0 - h, 7.0, h, if (i as u8) < bars { ink } else { pal.faint });
    }
    w
}
