// SPDX-License-Identifier: GPL-2.0-or-later
//! The MENU's glass pages (plan/mortar-cross-2.html): Wi-Fi, Bluetooth and
//! the Mixer, frosted panels that slide in from the right over the cross.
//!
//! - Wi-Fi: on/off (rfkill, radio.rs), the connected network with its
//!   signal (Details, Forget), nearby networks to join, "Other network…".
//!   Through mortar-setupd (iwd): scan, connect, forget; secured networks
//!   it does not know open the setup password keyboard.
//! - Bluetooth: on/off (rfkill and BlueZ power), paired devices (A connects
//!   or disconnects, X forgets), "Pair new device" (discovery) and the
//!   devices found, A pairs. Through mortar-setupd (BlueZ, its agent).
//! - Mixer: Master, a fader per running app (its PipeWire streams), UI
//!   sounds. ◀▶ pick a channel, ▲▼ ±5, A mutes.
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
}

impl SubKind {
    pub fn name(self) -> &'static str {
        match self {
            SubKind::Wifi => "wifi",
            SubKind::Bluetooth => "bluetooth",
            SubKind::Mixer => "mixer",
        }
    }
}

/// A row of the Wi-Fi or Bluetooth page.
#[derive(Clone, Debug, PartialEq, Eq)]
enum SRow {
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
}

impl SRow {
    fn selectable(&self) -> bool {
        !matches!(self, SRow::Label(_) | SRow::Note(..))
    }
}

pub(super) struct Sub {
    pub kind: SubKind,
    sel: usize,
    sel_key: Option<SRow>,
    scroll: Spring,
    /// 0: in place; 1: off to the right.
    x: Spring,
    pub closing: bool,
    mix_sel: usize,
    mix_scroll: Spring,
}

impl Sub {
    fn new(kind: SubKind, reduce: bool) -> Sub {
        let mut x = Spring::new(1.0);
        x.k_mul = 0.9;
        x.set(0.0);
        if reduce {
            x.snap(0.0);
        }
        Sub { kind, sel: 0, sel_key: None, scroll: Spring::new(0.0), x, closing: false, mix_sel: 0, mix_scroll: Spring::new(0.0) }
    }

    pub fn springs(&mut self) -> Vec<&mut Spring> {
        vec![&mut self.scroll, &mut self.x, &mut self.mix_scroll]
    }

    pub fn moving(&self) -> bool {
        self.closing || !(self.scroll.rest() && self.x.rest() && self.mix_scroll.rest())
    }

    /// Gone after sliding out.
    pub fn gone(&self) -> bool {
        self.closing && self.x.x > 0.98
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

fn sub_rows(kind: SubKind, v: &View) -> Vec<SRow> {
    match kind {
        SubKind::Wifi => wifi_rows(v),
        SubKind::Bluetooth => bt_rows(v),
        SubKind::Mixer => Vec::new(),
    }
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

const ROW_H: f32 = 84.0;
const LBL_H: f32 = 44.0;
const ROWS_TOP: f32 = 176.0;
const ROWS_BOTTOM: f32 = H - HINT_H;
const CH_W: f32 = 168.0;
const CH_GAP: f32 = 18.0;

impl ShellUi {
    pub(super) fn open_sub(&mut self, kind: SubKind, v: &View) {
        let reduce = self.prefs.reduce_motion;
        let Some(m) = &mut self.menu else { return };
        let mut s = Sub::new(kind, reduce);
        if kind == SubKind::Mixer {
            // start on the app in front, else Master
            let ch = channels(v);
            s.mix_sel = v.current.and_then(|u| ch.iter().position(|c| *c == Chan::App(u))).unwrap_or(0);
        }
        m.sub = Some(s);
        if kind == SubKind::Wifi && wifi_on(v) && v.net.is_some() {
            self.out.push(Out::Setup(Request::WifiScan));
        }
        self.fx(Sfx::Tab, Some(Haptic::Tick));
    }

    fn close_sub(&mut self, v: &View) {
        let Some(m) = &mut self.menu else { return };
        let Some(s) = &mut m.sub else { return };
        s.closing = true;
        s.x.set(1.0);
        if self.prefs.reduce_motion {
            s.x.snap(1.0);
        }
        if s.kind == SubKind::Bluetooth && v.net.is_some_and(|n| n.bt.discovering) {
            self.out.push(Out::Setup(Request::BtScan { on: false }));
        }
        self.fx(Sfx::DrawerUp, Some(Haptic::Tick));
    }

    /// The page in front (for `ctl state`).
    pub fn menu_sub(&self) -> Option<SubKind> {
        self.menu.as_ref()?.sub.as_ref().filter(|s| !s.closing).map(|s| s.kind)
    }

    /// The compositor should keep mortar-setupd's state fresh (Settings or
    /// a Wi-Fi/Bluetooth page shows).
    pub fn wants_net(&self) -> bool {
        self.menu.as_ref().is_some_and(|m| !m.closing && m.on_cross() && (m.cat == Cat::Settings || m.sub.as_ref().is_some_and(|s| s.kind != SubKind::Mixer)))
    }

    /// The Mixer page shows: poll the streams.
    pub fn wants_mixer(&self) -> bool {
        self.menu_sub() == Some(SubKind::Mixer)
    }

    /// Drop a page that slid out.
    pub(super) fn sync_sub(&mut self) {
        if let Some(m) = &mut self.menu
            && m.sub.as_ref().is_some_and(|s| s.gone())
        {
            m.sub = None;
            self.dirty = true;
        }
    }

    pub(super) fn sub_button(&mut self, b: Button, repeat: bool, v: &View) {
        let Some(kind) = self.menu.as_ref().and_then(|m| m.sub.as_ref()).filter(|s| !s.closing).map(|s| s.kind) else { return };
        if kind == SubKind::Mixer {
            self.mixer_button(b, repeat, v);
            return;
        }
        let rows = sub_rows(kind, v);
        let sel_rows: Vec<&SRow> = rows.iter().filter(|r| r.selectable()).collect();
        let Some(s) = self.menu.as_mut().and_then(|m| m.sub.as_mut()) else { return };
        let cur = s.sel_key.as_ref().and_then(|k| sel_rows.iter().position(|r| *r == k)).unwrap_or(s.sel).min(sel_rows.len().saturating_sub(1));
        s.sel = cur;
        match b {
            Button::Up | Button::Down => {
                let n = if b == Button::Up { cur.saturating_sub(1) } else { (cur + 1).min(sel_rows.len().saturating_sub(1)) };
                if n == cur {
                    self.fx(Sfx::Deny, None);
                } else {
                    s.sel = n;
                    s.sel_key = sel_rows.get(n).map(|r| (*r).clone());
                    self.fx(Sfx::Tick, Some(Haptic::Tick));
                }
            }
            Button::B | Button::Left if !repeat => self.close_sub(v),
            Button::Y if !repeat && kind == SubKind::Wifi && v.net.is_some() => {
                self.out.push(Out::Setup(Request::WifiScan));
                self.fx(Sfx::Tab, Some(Haptic::Tick));
            }
            Button::X if !repeat => match sel_rows.get(cur) {
                Some(SRow::Dev(addr)) => {
                    let name = dev_name(v, addr);
                    self.out.push(Out::Setup(Request::BtForget { address: addr.clone() }));
                    self.show_toast(Toast { sub: name, sound: None, ..Toast::info("Forgot") });
                    self.fx(Sfx::Close, Some(Haptic::Double));
                }
                _ => self.fx(Sfx::Deny, None),
            },
            Button::A if !repeat => {
                let row = sel_rows.get(cur).map(|r| (*r).clone());
                self.sub_activate(kind, row, v);
            }
            _ => {}
        }
    }

    fn sub_activate(&mut self, kind: SubKind, row: Option<SRow>, v: &View) {
        let net = v.net;
        match (kind, row) {
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
                if let Some(s) = self.menu.as_mut().and_then(|m| m.sub.as_mut()) {
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

    fn mixer_button(&mut self, b: Button, repeat: bool, v: &View) {
        let ch = channels(v);
        let prefs = self.prefs.clone();
        let Some(s) = self.menu.as_mut().and_then(|m| m.sub.as_mut()) else { return };
        s.mix_sel = s.mix_sel.min(ch.len().saturating_sub(1));
        let c = ch[s.mix_sel];
        let (level, muted, live) = chan_state(c, v, &prefs);
        match b {
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
        let rows = sub_rows(s.kind, v);
        let sel: Vec<&SRow> = rows.iter().filter(|r| r.selectable()).collect();
        let cur = s.sel_key.as_ref().and_then(|k| sel.iter().position(|r| *r == k)).unwrap_or(s.sel).min(sel.len().saturating_sub(1));
        let mut h = vec![("▲▼", "Choose")];
        let a = match sel.get(cur) {
            Some(SRow::Toggle) => "Switch",
            Some(SRow::Dev(addr)) => {
                if v.net.and_then(|n| n.bt.devices.iter().find(|d| d.address == *addr)).is_some_and(|d| d.connected) {
                    "Disconnect"
                } else {
                    "Connect"
                }
            }
            Some(SRow::Join(_)) => "Join",
            Some(SRow::Found(_)) => "Pair",
            Some(SRow::Forget) => "Forget",
            Some(SRow::Scan) if v.net.is_some_and(|n| n.bt.discovering) => "Stop",
            Some(SRow::Scan) => "Search",
            _ => "Open",
        };
        h.push(("A", a));
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
        let (title, line, icon) = match s.kind {
            SubKind::Wifi => ("Wi-Fi", wifi_line(v), Ic::Wifi),
            SubKind::Bluetooth => ("Bluetooth", bt_line(v), Ic::Bt),
            SubKind::Mixer => ("Mixer", "Each app's level, then the master".to_string(), Ic::Music),
        };
        let tst = ts(Face::SerifItalic, 64.0, pal.ink).line_height(1.0);
        let top = y0 + 22.0;
        let tw = b.text(40.0, top, title, &tst);
        let mono = ts(Face::Mono400, 18.0, pal.soft);
        let base = top + 64.0 * 0.82;
        let lw = W - 40.0 - 64.0 - 24.0 - (40.0 + tw + 18.0);
        let line = b.fit(&line, &mono, lw);
        b.text_base(40.0 + tw + 18.0, base, &line, &mono);
        let (bx, by) = (W - 40.0 - 64.0 - 8.0, top);
        b.rect(bx + 8.0, by + 8.0, 64.0, 64.0, pal.dark);
        b.rect(bx, by, 64.0, 64.0, if s.kind == SubKind::Mixer { accent } else { Cat::Settings.accent(pal) });
        b.frame(bx, by, 64.0, 64.0, 2.0, pal.dark);
        draw_icon(b, icon, bx + 32.0, by + 32.0, 40.0, pal, pal.dark);
        if s.kind == SubKind::Mixer {
            self.draw_mixer(b, pal, v, s, accent);
        } else {
            self.draw_sub_rows(b, pal, v, s);
        }
        b.pop();
    }

    fn draw_sub_rows(&self, b: &mut B, pal: &Pal, v: &View, s: &Sub) {
        let rows = sub_rows(s.kind, v);
        let sel_rows: Vec<&SRow> = rows.iter().filter(|r| r.selectable()).collect();
        let cur = s.sel_key.as_ref().and_then(|k| sel_rows.iter().position(|r| *r == k)).unwrap_or(s.sel).min(sel_rows.len().saturating_sub(1));
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
        let off = sel_scroll(sel_y, y, view_h);
        b.clip(Some([0.0, ROWS_TOP - 8.0, W, ROWS_BOTTOM - ROWS_TOP + 8.0]));
        let mut y = ROWS_TOP - off;
        let mut idx = 0;
        let sel_col = Cat::Settings.accent(pal);
        for r in &rows {
            if let SRow::Label(t) = r {
                let st = ts(Face::Mono400, 16.0, pal.soft).tracking(0.08);
                b.text(40.0, y + 16.0, &t.to_uppercase(), &st);
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
            if on {
                b.rect(0.0, y, W, ROW_H, sel_col);
            }
            let (title, sub) = Self::row_text(s.kind, r, v);
            let dis = matches!(r, SRow::Note(..));
            let ink = if on { pal.dark } else { pal.ink };
            b.push_alpha(if dis { 0.4 } else { 1.0 });
            let tst = if on { ts(Face::SerifItalic, 42.0, ink).line_height(1.0) } else { ts(Face::Body600, 30.0, ink).line_height(1.1) };
            let sst = ts(Face::Mono400, 18.0, if on { pal.dark } else { pal.soft });
            let ttop = y + ROW_H / 2.0 - (tst.line_px() + sst.line_px()) / 2.0;
            let rw = self.draw_row_right(b, pal, v, r, on, y);
            let maxw = W - 40.0 - rw - 40.0 - 16.0;
            let t = b.fit(&title, &tst, maxw);
            b.text(40.0, ttop, &t, &tst);
            let sb = b.fit(&sub, &sst, maxw);
            b.text(40.0, ttop + tst.line_px(), &sb, &sst);
            b.pop_alpha();
            y += ROW_H;
        }
        b.clip(None);
    }

    /// Title, subtitle, and a plain right-hand value of a row.
    fn row_text(kind: SubKind, r: &SRow, v: &View) -> (String, String) {
        let w = v.net.map(|n| &n.wifi);
        let bt = v.net.map(|n| &n.bt);
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
                let (t, s) = match bt.and_then(|b| b.passkey.clone()) {
                    Some(code) => ("Pairing…".to_string(), format!("Code {code}. If the device shows it too, confirm it there")),
                    None if disc => ("Searching…".to_string(), "Put the device in pairing mode · A to stop".to_string()),
                    None => ("Pair new device".to_string(), "Search for headphones, controllers, keyboards".to_string()),
                };
                (t, s)
            }
            SRow::Found(addr) => {
                let d = bt.and_then(|b| b.devices.iter().find(|d| d.address == *addr));
                let busy = bt.is_some_and(|b| b.busy.as_deref() == Some(addr.as_str()));
                let name = d.map(|d| d.name.clone()).unwrap_or_else(|| addr.clone());
                let kind = d.map(|d| d.kind_label()).unwrap_or("Device");
                (name, if busy { format!("{kind} · pairing…") } else { format!("{kind} · A to pair") })
            }
            SRow::Label(_) => (String::new(), String::new()),
        }
    }

    /// The right-hand column: switches, signal, tags. Returns its width.
    fn draw_row_right(&self, b: &mut B, pal: &Pal, v: &View, r: &SRow, on: bool, y: f32) -> f32 {
        let cy = y + ROW_H / 2.0;
        let right = W - 40.0;
        let mono = ts(Face::Mono400, 18.0, if on { pal.dark } else { pal.ink });
        let ink = if on { pal.dark } else { pal.ink };
        let wifi = v.net.map(|n| &n.wifi);
        match r {
            SRow::Toggle => {
                let is_bt = self.menu.as_ref().and_then(|m| m.sub.as_ref()).is_some_and(|s| s.kind == SubKind::Bluetooth);
                let state = if is_bt { bt_on(v) } else { wifi_on(v) };
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
            SRow::Details | SRow::Other => {
                let ch = ts(Face::Mono400, 30.0, ink);
                b.text_right(right, cy - ch.line_px() / 2.0, "›", &ch)
            }
            SRow::Dev(addr) => {
                let conn = v.net.and_then(|n| n.bt.devices.iter().find(|d| d.address == *addr)).is_some_and(|d| d.connected);
                let mut x = right;
                if on {
                    let lab = ts(Face::Mono400, 18.0, pal.dark);
                    x -= b.text_right(x, cy - lab.line_px() / 2.0, "Forget", &lab) + 8.0;
                    let cap = ts(Face::Mono600, 17.0, DAY.ground);
                    b.rrect(x - 38.0, cy - 19.0, 38.0, 38.0, 19.0, Color(29, 29, 29, 209));
                    b.text_center(x - 19.0, cy - cap.line_px() / 2.0, "X", &cap);
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
                let disc = v.net.is_some_and(|n| n.bt.discovering);
                let ch = ts(Face::Mono400, 30.0, ink);
                if disc {
                    // a slow turning ring while it searches
                    let a = (self.t as f32 * 4.0) % std::f32::consts::TAU;
                    let pts: Vec<P> = (0..=20).map(|i| {
                        let t = a + i as f32 / 20.0 * 4.4;
                        (right - 12.0 + 11.0 * t.cos(), cy + 11.0 * t.sin())
                    }).collect();
                    b.stroke(&pts, false, 2.0, ink);
                    26.0
                } else {
                    b.text_right(right, cy - ch.line_px() / 2.0, "›", &ch)
                }
            }
            _ => 0.0,
        }
    }

    fn draw_mixer(&self, b: &mut B, pal: &Pal, v: &View, s: &Sub, accent: Color) {
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
        let _ = accent;
    }
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

/// An on/off switch, 52 x 28.
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
