// SPDX-License-Identifier: GPL-2.0-or-later
//! Home (no app in front): a calm dashboard, not a launcher (apps start
//! from the MENU). The status bar, then the widgets (widgets.rs) on a 4 x 3
//! grid of Pond tiles: the big clock and date, Continue (the last app or
//! game; A resumes it), Needs you (attention.rs), Now playing, battery,
//! Wi-Fi, storage and the user's command widgets. The D-pad moves between
//! the tiles that do something (a lavender block behind the focused one),
//! A acts, L1/SELECT open the MENU on Apps, R1 on Settings. Nothing drifts:
//! only the charging line moves, and only while it charges.
//!
//! Also here: the first-time tips (a toast the first time a feature
//! matters, remembered in prefs).
//!
//! A child module of `shellui`.

use super::*;
use crate::widgets::{Kind, Slot, Widget};

#[derive(Default)]
pub(super) struct Home {
    /// The focused widget (index into the widget list).
    focus: Option<usize>,
}

impl Home {
    pub fn springs_ref(&self) -> Vec<&Spring> {
        Vec::new()
    }
    pub fn springs_mut(&mut self) -> Vec<&mut Spring> {
        Vec::new()
    }
}

// the grid: from near the top edge to the hint bar
const GX: f32 = 40.0;
const GY: f32 = 36.0;
const GAP: f32 = 20.0;
const TW: f32 = (W - 2.0 * GX - 3.0 * GAP) / 4.0;
const TH: f32 = 200.0;

fn slot_rect(s: &Slot) -> R4 {
    R4 { x: GX + s.col as f32 * (TW + GAP), y: GY + s.row as f32 * (TH + GAP), w: s.w as f32 * TW + (s.w as f32 - 1.0) * GAP, h: TH }
}

/// What A on a tile does.
#[derive(Clone, Debug, PartialEq)]
enum Act {
    Switch(u32),
    Launch(String),
}

/// The tips: (id, title, subtitle).
const TIPS: [(&str, &str, &str); 4] = [
    ("menu", "MENU switches apps", "TIP · MENU, THEN A GOES BACK TO THE LAST ONE"),
    ("keyboard", "START opens the keyboard", "TIP · ▲ FROM THE TOP ROW: ZSH SUGGESTIONS"),
    ("game", "SELECT + MENU: the game's menu", "TIP · MENU ALONE OPENS MORTAR'S"),
    ("controls", "Every button, in one place", "TIP · SETTINGS › ABOUT › CONTROLS"),
];

impl ShellUi {
    fn widgets<'v>(&self, v: &View<'v>) -> &'v [Widget] {
        v.dash.widgets
    }

    /// The app Continue resumes: the most recent running one, else the
    /// last entry on screen (started again).
    fn continue_act(&self, v: &View) -> Option<Act> {
        if let Some(a) = v.running.iter().find(|a| !a.closing) {
            return Some(Act::Switch(a.uid));
        }
        let id = self.prefs.last_app.as_ref()?;
        v.launcher.iter().any(|l| l.id == *id).then(|| Act::Launch(id.clone()))
    }

    fn open_act(v: &View, id: &str) -> Option<Act> {
        let l = v.launcher.iter().find(|l| l.id == id)?;
        match v.by_entry(id) {
            Some(a) if l.single_instance => Some(Act::Switch(a.uid)),
            _ => Some(Act::Launch(id.to_string())),
        }
    }

    /// The app making sound (the loudest playing stream's app).
    fn playing<'v>(&self, v: &View<'v>) -> Option<(&'v RunningInfo, u8)> {
        v.mixer.iter().filter(|m| m.playing && !m.muted).max_by_key(|m| m.level).and_then(|m| v.running.iter().find(|a| a.uid == m.uid).map(|a| (a, m.level)))
    }

    fn act_of(&self, w: &Widget, v: &View) -> Option<Act> {
        if let Some(id) = &w.open {
            return Self::open_act(v, id);
        }
        match w.kind {
            Kind::Continue => self.continue_act(v),
            Kind::NeedsYou => {
                let it = v.dash.attention.first()?;
                Self::open_act(v, &it.app)
            }
            Kind::NowPlaying => self.playing(v).map(|(a, _)| Act::Switch(a.uid)),
            _ => None,
        }
    }

    /// Keep the focus on a tile that does something (Continue first), and
    /// remember the app on screen for Continue.
    pub(super) fn sync_home(&mut self, v: &View) {
        let ws = self.widgets(v);
        let slots = crate::widgets::layout(ws);
        let ok = |i: usize| self.act_of(&ws[i], v).is_some();
        if !self.home.focus.is_some_and(|f| slots.iter().any(|s| s.i == f) && ok(f)) {
            self.home.focus = slots.iter().find(|s| ws[s.i].kind == Kind::Continue && ok(s.i)).or_else(|| slots.iter().find(|s| ok(s.i))).map(|s| s.i);
        }
        if let Some(e) = v.current.and_then(|u| v.app(u)).and_then(|a| a.entry.clone())
            && self.prefs.last_app.as_ref() != Some(&e)
        {
            self.prefs.last_app = Some(e);
            self.out.push(Out::Remember);
        }
    }

    /// Where the Continue card is (launches grow from it, Home shrinks the
    /// app into it).
    pub(super) fn continue_rect(&self, v: &View) -> Option<R4> {
        let ws = self.widgets(v);
        crate::widgets::layout(ws).iter().find(|s| ws[s.i].kind == Kind::Continue).map(slot_rect)
    }

    pub(super) fn home_button(&mut self, b: Button, _repeat: bool, v: &View) {
        let ws = self.widgets(v);
        let slots = crate::widgets::layout(ws);
        match b {
            Button::Up | Button::Down | Button::Left | Button::Right => {
                let Some(cur) = self.home.focus.and_then(|f| slots.iter().find(|s| s.i == f)) else {
                    self.fx(Sfx::Deny, None);
                    return;
                };
                let r = slot_rect(cur);
                let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
                // the nearest tile that does something, that way
                let best = slots
                    .iter()
                    .filter(|s| s.i != cur.i && self.act_of(&ws[s.i], v).is_some())
                    .filter_map(|s| {
                        let o = slot_rect(s);
                        let (dx, dy) = (o.x + o.w / 2.0 - cx, o.y + o.h / 2.0 - cy);
                        let ahead = match b {
                            Button::Up => dy < -1.0,
                            Button::Down => dy > 1.0,
                            Button::Left => dx < -1.0,
                            _ => dx > 1.0,
                        };
                        let (along, across) = if matches!(b, Button::Up | Button::Down) { (dy.abs(), dx.abs()) } else { (dx.abs(), dy.abs()) };
                        ahead.then_some((s.i, along + 2.0 * across))
                    })
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                match best {
                    Some((i, _)) => {
                        self.home.focus = Some(i);
                        self.fx(Sfx::Move, Some(Haptic::Tick));
                    }
                    None => self.fx(Sfx::Deny, None),
                }
            }
            Button::A | Button::Start => {
                let Some((w, slot)) = self.home.focus.and_then(|f| slots.iter().find(|s| s.i == f)).map(|s| (&ws[s.i], *s)) else {
                    self.fx(Sfx::Deny, None);
                    return;
                };
                let from = slot_rect(&slot);
                match self.act_of(w, v) {
                    Some(Act::Switch(uid)) => {
                        let (name, art) = v.app(uid).map(|a| (a.name.clone(), a.art)).unwrap_or_default();
                        self.trans = Some(Trans::Launch { target: Target::Uid(uid), name, art, from, prev: None, t0: self.t });
                        self.lp.snap(0.0);
                        self.lp.set(1.0);
                        self.out.push(Out::Focus(uid));
                        self.fx(Sfx::Open, Some(Haptic::Double));
                    }
                    Some(Act::Launch(id)) => {
                        let Some(l) = v.launcher.iter().find(|l| l.id == id) else { return };
                        self.trans = Some(Trans::Launch { target: Target::Entry(id.clone()), name: l.name.clone(), art: l.art, from, prev: None, t0: self.t });
                        self.lp.snap(0.0);
                        self.lp.set(1.0);
                        self.out.push(Out::Launch(id));
                        self.fx(Sfx::Open, Some(Haptic::Double));
                    }
                    None => self.fx(Sfx::Deny, None),
                }
            }
            // shortcuts: the MENU on Apps (L1, SELECT) or Settings (R1)
            Button::L1 | Button::R1 | Button::Select => self.open_menu_on(if b == Button::R1 { Cat::Settings } else { Cat::Apps }, v),
            Button::B => self.fx(Sfx::Deny, None),
            _ => {}
        }
    }

    /// First-time tips: one toast, once ever, when a feature first matters
    /// (nothing else on screen, no toast up).
    pub(super) fn tips(&mut self, v: &View) {
        if self.toast.is_some() || self.boot.is_some() || self.trans.is_some() || self.menu.is_some() || self.page.is_some() || self.sleep.is_some() || self.power.is_some() {
            return;
        }
        let front = v.current.and_then(|u| v.app(u)).filter(|a| a.has_buffer);
        let game = front.is_some_and(|a| a.game) || front.and_then(|a| a.entry.as_deref()).and_then(|e| v.launcher.iter().find(|l| l.id == e)).is_some_and(|l| l.category == Category::Games);
        let due = |id: &str| match id {
            "menu" => front.is_some(),
            "keyboard" => front.is_some() && v.keyboard_app,
            "game" => game,
            // after a few apps were used: where the full list is
            "controls" => v.current.is_none() && v.running.len() >= 2,
            _ => false,
        };
        let Some(tip) = TIPS.iter().find(|(id, ..)| !self.prefs.tips_seen.iter().any(|s| s == id) && due(id)) else { return };
        self.prefs.tips_seen.push(tip.0.to_string());
        self.out.push(Out::Remember);
        let mut t = Toast::info(tip.1);
        t.sub = tip.2.to_string();
        t.secs = 4.0;
        self.show_toast(t);
    }

    // --------------------------------------------------------- drawing ----

    pub(super) fn draw_home(&self, b: &mut B, pal: &Pal, v: &View) {
        b.rect(0.0, 0.0, W, H, pal.ground);
        let ws = self.widgets(v);
        let slots = crate::widgets::layout(ws);
        // the focused tile last: its offset block goes over its neighbours' gap
        let order = slots.iter().filter(|s| Some(s.i) != self.home.focus).chain(slots.iter().filter(|s| Some(s.i) == self.home.focus));
        for s in order {
            let on = Some(s.i) == self.home.focus;
            self.draw_widget(b, pal, v, &ws[s.i], s, slot_rect(s), on);
        }
        if ws.is_empty() {
            let st = ts(Face::Body400, 24.0, pal.soft);
            b.text(56.0, 420.0, "No widgets (mortar.shell.home.widgets).", &st);
        }
        // No status bar: the clock, Needs you, battery and Wi-Fi tiles already
        // say all of it (the MENU's glass bar still has SSH and the rest).
        let verb = match self.home.focus.and_then(|f| ws.get(f)).and_then(|w| self.act_of(w, v)) {
            Some(Act::Switch(_)) => Some("Resume"),
            Some(Act::Launch(_)) => Some("Open"),
            None => None,
        };
        let mut h = vec![("MENU", "Apps"), ("R1", "Settings")];
        if let Some(verb) = verb {
            h.insert(0, ("◀▶▲▼", "Choose"));
            h.push(("A", verb));
        }
        hints(b, pal, H - 64.0, &h);
    }

    /// The tile's block (accent or plain) with its outline; the focused one
    /// sits on a lavender offset block.
    fn tile(&self, b: &mut B, pal: &Pal, r: R4, fill: Color, on: bool) {
        if on {
            b.block(r.x + 10.0, r.y + 10.0, r.w, r.h, pal.lavender, pal.ink);
        }
        b.block(r.x, r.y, r.w, r.h, fill, pal.ink);
        if on {
            b.frame(r.x, r.y, r.w, r.h, 2.0, pal.ink);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_widget(&self, b: &mut B, pal: &Pal, v: &View, w: &Widget, s: &Slot, r: R4, on: bool) {
        let pad = 18.0;
        let label = |b: &mut B, ink: Color, txt: &str| {
            let st = ts(Face::Mono600, 15.0, ink).tracking(0.08);
            b.text(r.x + pad, r.y + 14.0, &txt.to_uppercase(), &st);
        };
        let soft_on = |fill_is_accent: bool| if fill_is_accent { pal.dark } else { pal.soft };
        match w.kind {
            Kind::Clock => {
                let clock = ts(Face::SerifItalic, 132.0, pal.ink).line_height(0.9).tracking(-0.02);
                b.text(r.x + 4.0, r.y + 4.0, &v.sys.time, &clock);
                let date = ts(Face::Mono400, 18.0, pal.soft).tracking(0.04);
                b.text(r.x + 10.0, r.y + 136.0, &date_line(v.host), &date);
                if v.sys.plugged {
                    self.draw_charge_line(b, pal, v, r.x + 10.0, r.y + 172.0);
                }
            }
            Kind::Continue => {
                let act = self.continue_act(v);
                let app = match &act {
                    Some(Act::Switch(u)) => v.app(*u).map(|a| (a.name.clone(), a.art, AppState::of(a, v).label().to_string(), Some(a))),
                    Some(Act::Launch(id)) => v.launcher.iter().find(|l| l.id == *id).map(|l| (l.name.clone(), l.art, "closed · A starts it again".to_string(), None)),
                    None => None,
                };
                let Some((name, art, state, running)) = app else {
                    self.tile(b, pal, r, pal.tile, on);
                    label(b, pal.soft, &w.label());
                    let t = ts(Face::SerifItalic, 44.0, pal.soft).line_height(1.0);
                    b.text(r.x + pad, r.y + 60.0, "Nothing yet", &t);
                    let st = ts(Face::Mono400, 17.0, pal.soft);
                    b.text(r.x + pad, r.y + 120.0, "MENU opens your apps", &st);
                    return;
                };
                let accent = art.accent(pal);
                self.tile(b, pal, r, pal.tile, on);
                // the app's colour down the left edge, like its window in Running
                b.rect(r.x + 1.0, r.y + 1.0, 10.0, r.h - 2.0, accent);
                b.rect(r.x + 11.0, r.y + 1.0, 1.5, r.h - 2.0, pal.ink);
                // a picture of where it was: its window, else its illustration
                let (pw, ph) = (176.0, 132.0);
                let (px, py) = (r.x + r.w - pad - pw, r.y + (r.h - ph) / 2.0);
                match running.filter(|a| a.has_buffer) {
                    Some(a) => {
                        b.rect(px, py, pw, ph, Color(0, 0, 0, 255));
                        b.clip(Some([px, py, pw, ph]));
                        let g = a.geom;
                        b.image(Src::App(a.uid), px + g[0] * pw / W, py + g[1] * ph / H, g[2] * pw / W, g[3] * ph / H, [0.0, 0.0, 1.0, 1.0]);
                        b.clip(None);
                    }
                    // a game asleep: its last frame
                    None if running.and_then(|a| a.pic).is_some() => {
                        b.rect(px, py, pw, ph, Color(0, 0, 0, 255));
                        b.image(Src::Pic(running.and_then(|a| a.pic).unwrap_or(0)), px, py, pw, ph, [0.0, 0.0, 1.0, 1.0]);
                    }
                    None => {
                        b.rect(px, py, pw, ph, accent);
                        art::draw(b, art, px + pw / 2.0, py + ph / 2.0, 0.6, *pal, pal.dark);
                    }
                }
                b.frame(px, py, pw, ph, 1.5, pal.ink);
                let x = r.x + pad + 12.0;
                let lab = ts(Face::Mono600, 15.0, pal.soft).tracking(0.08);
                b.text(x, r.y + 14.0, &w.label().to_uppercase(), &lab);
                let nm = ts(Face::SerifItalic, 48.0, pal.ink).line_height(1.0);
                let n = b.fit(&name, &nm, px - x - 12.0);
                b.text(x, r.y + 56.0, &n, &nm);
                let st = ts(Face::Mono400, 17.0, pal.soft);
                let s1 = b.fit(&state, &st, px - x - 12.0);
                b.text(x, r.y + 116.0, &s1, &st);
                if matches!(act, Some(Act::Switch(_))) {
                    let s2 = b.fit("A resumes it", &st, px - x - 12.0);
                    b.text(x, r.y + 142.0, &s2, &st);
                }
            }
            Kind::NeedsYou => {
                let items = v.dash.attention;
                let fill = if items.is_empty() { pal.tile } else { pal.yellow };
                self.tile(b, pal, r, fill, on);
                let ink = if items.is_empty() { pal.ink } else { pal.dark };
                label(b, soft_on(!items.is_empty()), &w.label());
                if items.is_empty() {
                    let t = ts(Face::SerifItalic, 44.0, pal.soft).line_height(1.0);
                    b.text(r.x + pad, r.y + 60.0, "All quiet", &t);
                    let st = ts(Face::Mono400, 17.0, pal.soft);
                    b.text(r.x + pad, r.y + 120.0, "Apps that wait for you show here", &st);
                    return;
                }
                if items.len() > 1 {
                    chip(b, r.x + r.w - pad, r.y + 24.0, &format!("{}", items.len()), 15.0, (8.0, 2.0), pal.dark, pal.yellow);
                }
                let name_st = ts(Face::SerifItalic, 40.0, ink).line_height(1.0);
                let txt = ts(Face::Body500, 20.0, ink);
                let mut y = r.y + 50.0;
                for it in items.iter().take(2) {
                    let name = v.launcher.iter().find(|l| l.id == it.app).map(|l| l.name.clone()).unwrap_or_else(|| it.app.clone());
                    let n = b.fit(&name, &name_st, r.w - 2.0 * pad);
                    b.text(r.x + pad, y, &n, &name_st);
                    let t = b.fit(&it.text, &txt, r.w - 2.0 * pad);
                    b.text(r.x + pad, y + 42.0, &t, &txt);
                    y += 70.0;
                }
            }
            Kind::NowPlaying => {
                let p = self.playing(v);
                self.tile(b, pal, r, if p.is_some() { pal.orange } else { pal.tile }, on);
                label(b, soft_on(p.is_some()), &w.label());
                match p {
                    Some((a, level)) => {
                        let nm = ts(Face::SerifItalic, 48.0, pal.dark).line_height(1.0);
                        let n = b.fit(&a.name, &nm, r.w - 2.0 * pad - 120.0);
                        b.text(r.x + pad, r.y + 56.0, &n, &nm);
                        let st = ts(Face::Mono400, 17.0, pal.dark);
                        b.text(r.x + pad, r.y + 116.0, &format!("level {level}% · A switches to it"), &st);
                        // bars, still (no level meters: see the Mixer)
                        for (k, hh) in [40.0, 64.0, 28.0, 52.0].iter().enumerate() {
                            let bx = r.x + r.w - pad - 4.0 * 22.0 + k as f32 * 22.0;
                            b.block(bx, r.y + 132.0 - hh, 14.0, *hh, pal.dark, pal.dark);
                        }
                    }
                    None => {
                        let t = ts(Face::SerifItalic, 44.0, pal.soft).line_height(1.0);
                        b.text(r.x + pad, r.y + 60.0, "Quiet", &t);
                        let st = ts(Face::Mono400, 17.0, pal.soft);
                        b.text(r.x + pad, r.y + 120.0, "Nothing is making sound", &st);
                    }
                }
            }
            Kind::Battery => {
                self.tile(b, pal, r, pal.tile, on);
                label(b, pal.soft, &w.label());
                let val = ts(Face::SerifItalic, 60.0, pal.ink).line_height(1.0);
                let st = ts(Face::Mono400, 17.0, pal.soft);
                match v.sys.battery {
                    Some(p) => {
                        b.text(r.x + pad, r.y + 48.0, &format!("{p}%"), &val);
                        let (bx, by, bw) = (r.x + pad, r.y + 122.0, r.w - 2.0 * pad);
                        let fill = if v.sys.charging || p > 15 { pal.lime } else if p > 5 { pal.yellow } else { pal.orange };
                        b.rect(bx, by, bw, 14.0, pal.ground_2);
                        b.rect(bx, by, bw * p.min(100) as f32 / 100.0, 14.0, fill);
                        b.frame(bx, by, bw, 14.0, 1.5, pal.ink);
                        let sub = if v.sys.charging {
                            "charging".to_string()
                        } else if v.sys.plugged {
                            "on power".to_string()
                        } else {
                            v.dash.about.and_then(|a| a.battery.health.clone()).map(|h| format!("health {}", h.to_lowercase())).unwrap_or_else(|| "on battery".into())
                        };
                        b.text(r.x + pad, r.y + 146.0, &sub, &st);
                    }
                    None => {
                        b.text(r.x + pad, r.y + 48.0, "—", &val);
                        b.text(r.x + pad, r.y + 146.0, "no battery", &st);
                    }
                }
            }
            Kind::Wifi => {
                self.tile(b, pal, r, pal.tile, on);
                label(b, pal.soft, &w.label());
                let val = ts(Face::SerifItalic, 48.0, pal.ink).line_height(1.0);
                let st = ts(Face::Mono400, 17.0, pal.soft);
                let ssid = v.net.and_then(|n| n.wifi.ssid.clone()).filter(|_| v.sys.wifi);
                let (t, sub) = match (v.sys.wifi, v.radios.wifi) {
                    (true, _) => ("On", ssid.unwrap_or_else(|| "connected".into())),
                    (false, Some(false)) => ("Off", "radio off".to_string()),
                    _ => ("Offline", "not connected".to_string()),
                };
                b.text(r.x + pad, r.y + 56.0, t, &val);
                let sb = b.fit(&sub, &st, r.w - 2.0 * pad);
                b.text(r.x + pad, r.y + 146.0, &sb, &st);
                if v.sys.usb_net {
                    chip(b, r.x + r.w - pad, r.y + 24.0, "USB", 14.0, (7.0, 1.0), pal.lavender, pal.lavender_ink);
                }
            }
            Kind::Storage => {
                self.tile(b, pal, r, pal.tile, on);
                label(b, pal.soft, &w.label());
                let val = ts(Face::SerifItalic, 48.0, pal.ink).line_height(1.0);
                let st = ts(Face::Mono400, 17.0, pal.soft);
                match v.dash.about.and_then(|a| a.storage) {
                    Some((free, total)) => {
                        b.text(r.x + pad, r.y + 56.0, &crate::about::gb(free), &val);
                        let (bx, by, bw) = (r.x + pad, r.y + 122.0, r.w - 2.0 * pad);
                        let used = 1.0 - free as f32 / total.max(1) as f32;
                        b.rect(bx, by, bw, 14.0, pal.ground_2);
                        b.rect(bx, by, bw * used.clamp(0.0, 1.0), 14.0, if used > 0.9 { pal.orange } else { pal.lavender });
                        b.frame(bx, by, bw, 14.0, 1.5, pal.ink);
                        b.text(r.x + pad, r.y + 146.0, &format!("free of {}", crate::about::gb(total)), &st);
                    }
                    None => {
                        b.text(r.x + pad, r.y + 56.0, "—", &val);
                        b.text(r.x + pad, r.y + 146.0, "not known", &st);
                    }
                }
            }
            Kind::Command => {
                let out = v.dash.outputs.get(s.i).and_then(|o| o.as_ref());
                let accent = out.and_then(|o| o.accent).or(w.accent);
                let fill = accent_color(accent, pal);
                let on_accent = !matches!(accent, None | Some(crate::widgets::Accent::Ground));
                self.tile(b, pal, r, fill, on);
                let title = out.and_then(|o| o.title.clone()).unwrap_or_else(|| w.label());
                label(b, soft_on(on_accent), &title);
                let ink = if on_accent { pal.dark } else { pal.ink };
                let st = ts(Face::Mono400, 17.0, if on_accent { pal.dark } else { pal.soft });
                match out {
                    Some(o) => {
                        let mut lines = o.value.lines();
                        let first = lines.next().unwrap_or("");
                        let val = ts(Face::SerifItalic, if first.chars().count() <= 8 { 56.0 } else { 34.0 }, ink).line_height(1.0);
                        let f = b.fit(first, &val, r.w - 2.0 * pad);
                        b.text(r.x + pad, r.y + 56.0, &f, &val);
                        let rest = lines.next().unwrap_or(if o.error { "the script failed" } else { "" });
                        let rs = b.fit(rest, &st, r.w - 2.0 * pad);
                        b.text(r.x + pad, r.y + 146.0, &rs, &st);
                        if o.error {
                            chip(b, r.x + r.w - pad, r.y + 24.0, "!", 15.0, (8.0, 2.0), pal.orange, pal.dark);
                        }
                    }
                    None => {
                        let val = ts(Face::SerifItalic, 44.0, pal.soft).line_height(1.0);
                        b.text(r.x + pad, r.y + 60.0, "…", &val);
                        b.text(r.x + pad, r.y + 146.0, "waiting for the script", &st);
                    }
                }
            }
        }
        let _ = s;
    }

    /// On the charger: a thin level line, with a short highlight drifting
    /// along the filled part while it charges.
    fn draw_charge_line(&self, b: &mut B, pal: &Pal, v: &View, x: f32, y: f32) {
        let pct = v.sys.battery.unwrap_or(0).min(100);
        let w = 200.0;
        let fill = (w * pct as f32 / 100.0).max(2.0);
        b.rect(x, y, w, 2.0, pal.ground_2);
        b.rect(x, y, fill, 2.0, pal.lime);
        if v.sys.charging && fill > 30.0 {
            let ph = (self.drift_t / 2.6).fract() as f32;
            let ease = ph * ph * (3.0 - 2.0 * ph);
            b.rect(x + (fill - 28.0) * ease, y - 1.0, 28.0, 4.0, with_a(pal.ink, 0.28 * (1.0 - ph)));
        }
        let lab = ts(Face::Mono400, 13.0, pal.soft).tracking(0.08);
        let txt = match (v.sys.charging, v.sys.battery) {
            (true, Some(p)) => format!("CHARGING · {p}%"),
            (true, None) => "CHARGING".to_string(),
            (false, Some(100)) => "CHARGED".to_string(),
            (false, _) => "ON POWER".to_string(),
        };
        b.text(x + w + 14.0, y + 1.0 - lab.line_px() / 2.0, &txt, &lab);
    }
}

fn accent_color(a: Option<crate::widgets::Accent>, pal: &Pal) -> Color {
    use crate::widgets::Accent::*;
    match a {
        Some(Lime) => pal.lime,
        Some(Lavender) => pal.lavender,
        Some(Yellow) => pal.yellow,
        Some(Orange) => pal.orange,
        Some(Pink) => pal.pink,
        Some(Ground) | None => pal.tile,
    }
}
