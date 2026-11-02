// SPDX-License-Identifier: GPL-2.0-or-later
//! The dashboard: the top card of the Running stack (cross.rs). Being home
//! is the stack scrolled to it with no app in front; the running apps are
//! the cards below it. Not a launcher (apps start from the cross): the
//! widgets (widgets.rs) on a 4 x 3 grid of Pond tiles inside the card: the
//! big clock and date (the clock while the card is selected; the status
//! bar's fades), Recently closed (what you closed last that no longer
//! runs; A starts it again), Needs you (attention.rs), Now playing,
//! battery, Wi-Fi, storage and the user's command widgets.
//!
//! The stack's ▲▼ scroll past the card; A on it (at home) enters its
//! tiles: then the D-pad moves between the tiles that do something (a
//! lavender block behind the focused one), A acts, B leaves. Nothing
//! drifts: only the charging line moves, and only while it charges.
//!
//! Also here: the first-time tips (a toast the first time a feature
//! matters, remembered in prefs).
//!
//! A child module of `shellui`.

use super::cross::{RUN_H, RUN_Y};
use super::*;
use crate::games::GameRef;
use crate::widgets::{Kind, Slot, Widget};
use std::path::{Path, PathBuf};

#[derive(Default)]
pub(super) struct Home {
    /// The focused widget (index into the widget list).
    focus: Option<usize>,
    /// In the tiles: the D-pad moves between them (A on the card).
    pub tiles: bool,
}

// the grid, inside the card (card coordinates: 0..W x 0..RUN_H)
const GX: f32 = 40.0;
const GY: f32 = 20.0;
const GAP: f32 = 16.0;
const TW: f32 = (W - 2.0 * GX - 3.0 * GAP) / 4.0;
const TH: f32 = (RUN_H - 2.0 * GY - 2.0 * GAP) / 3.0;

fn slot_rect(s: &Slot) -> R4 {
    R4 { x: GX + s.col as f32 * (TW + GAP), y: GY + s.row as f32 * (TH + GAP), w: s.w as f32 * TW + (s.w as f32 - 1.0) * GAP, h: TH }
}

/// What A on a tile does.
#[derive(Clone, Debug, PartialEq)]
enum Act {
    Switch(u32),
    Launch(String),
    Game(GameRef),
}

/// Recently closed: an app or game that no longer runs.
struct Recent {
    name: String,
    art: Art,
    act: Act,
    sub: &'static str,
    /// A game's resume picture.
    pic: Option<PathBuf>,
}

/// The tips: (id, title, subtitle).
const TIPS: [(&str, &str, &str); 4] = [
    ("menu", "MENU switches apps", "TIP · MENU, THEN A GOES BACK TO THE LAST ONE"),
    ("keyboard", "START opens the keyboard", "TIP · ▲ FROM THE TOP ROW: ZSH SUGGESTIONS"),
    ("game", "SELECT + MENU: the game's menu", "TIP · MENU ALONE OPENS MORTAR'S"),
    ("controls", "Every button, in one place", "TIP · SETTINGS › ABOUT › CONTROLS"),
];

/// How many entries Recently closed remembers.
const RECENT_MAX: usize = 8;

impl ShellUi {
    fn widgets<'v>(&self, v: &View<'v>) -> &'v [Widget] {
        v.dash.widgets
    }

    /// Recently closed: the newest entry on screen lately that is not
    /// running now (an app, or a game from the library), then the
    /// library's own recents.
    fn recent(&self, v: &View) -> Option<Recent> {
        let lib = v.games.and_then(|g| g.lib.as_ref());
        let resolve = |id: &str| -> Option<Recent> {
            if v.by_entry(id).is_some() {
                return None;
            }
            if let Some(l) = v.launcher.iter().find(|l| l.id == id) {
                return Some(Recent { name: l.name.clone(), art: l.art, act: Act::Launch(id.to_string()), sub: "A opens it again", pic: None });
            }
            let lib = lib?;
            let rest = id.strip_prefix("game:")?;
            let gr = match rest.split_once(':') {
                Some((sys, path)) => lib.game_ref(sys, Path::new(path))?,
                None => lib.system_ref(lib.system(rest)?),
            };
            // its resume state and picture (the recents list, else the system's)
            let (resume, pic) = match &gr.path {
                Some(p) => match lib.recent.iter().find(|r| r.path == *p) {
                    Some(r) => (r.resume, r.thumb.clone()),
                    None => lib.system(&gr.system).and_then(|s| s.games.iter().find(|g| g.path == *p)).map(|g| (g.resume, g.thumb.clone())).unwrap_or((false, None)),
                },
                None => (false, None),
            };
            Some(Recent { name: gr.title(), art: Art::Games, act: Act::Game(gr), sub: if resume { "saved · A resumes it" } else { "A starts it again" }, pic: pic.filter(|_| resume) })
        };
        self.prefs.recent.iter().find_map(|id| resolve(id)).or_else(|| {
            let lib = lib?;
            lib.recent.iter().find_map(|r| resolve(&lib.recent_ref(r).key()))
        })
    }

    fn open_act(v: &View, id: &str) -> Option<Act> {
        match (v.launcher.iter().find(|l| l.id == id), v.by_entry(id)) {
            (Some(l), Some(a)) if l.single_instance => Some(Act::Switch(a.uid)),
            (Some(_), _) => Some(Act::Launch(id.to_string())),
            (None, Some(a)) => Some(Act::Switch(a.uid)),
            (None, None) => None,
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
            Kind::Recent => self.recent(v).map(|r| r.act),
            Kind::NeedsYou => {
                let it = v.dash.attention.first()?;
                Self::open_act(v, &it.app)
            }
            Kind::NowPlaying => self.playing(v).map(|(a, _)| Act::Switch(a.uid)),
            _ => None,
        }
    }

    /// A tile does something (A on the card enters them).
    pub(super) fn dash_acts(&self, v: &View) -> bool {
        let ws = self.widgets(v);
        crate::widgets::layout(ws).iter().any(|s| self.act_of(&ws[s.i], v).is_some())
    }

    /// Keep the focus on a tile that does something (Needs you first),
    /// leave the tiles when none does, and remember what is on screen for
    /// Recently closed.
    pub(super) fn sync_home(&mut self, v: &View) {
        let ws = self.widgets(v);
        let slots = crate::widgets::layout(ws);
        let ok = |i: usize| self.act_of(&ws[i], v).is_some();
        if !self.home.focus.is_some_and(|f| slots.iter().any(|s| s.i == f) && ok(f)) {
            self.home.focus = slots.iter().find(|s| ws[s.i].kind == Kind::NeedsYou && ok(s.i)).or_else(|| slots.iter().find(|s| ok(s.i))).map(|s| s.i);
        }
        if self.home.tiles && self.home.focus.is_none() {
            self.home.tiles = false;
            self.dirty = true;
        }
        if let Some(e) = v.current.and_then(|u| v.app(u)).and_then(|a| a.entry.clone())
            && self.prefs.recent.first() != Some(&e)
        {
            self.prefs.recent.retain(|x| *x != e);
            self.prefs.recent.insert(0, e);
            self.prefs.recent.truncate(RECENT_MAX);
            self.out.push(Out::Remember);
        }
    }

    /// A on the dashboard card at home: into its tiles.
    pub(super) fn enter_tiles(&mut self, v: &View) {
        self.sync_home(v);
        if self.home.focus.is_none() {
            self.fx(Sfx::Deny, None);
            return;
        }
        self.home.tiles = true;
        self.fx(Sfx::Tab, Some(Haptic::Tick));
    }

    /// In the tiles: the D-pad moves, A acts, B leaves.
    pub(super) fn tiles_button(&mut self, b: Button, repeat: bool, v: &View) {
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
            Button::A | Button::Start if !repeat => {
                let Some((w, slot)) = self.home.focus.and_then(|f| slots.iter().find(|s| s.i == f)).map(|s| (&ws[s.i], *s)) else {
                    self.fx(Sfx::Deny, None);
                    return;
                };
                // the app grows out of the tile (the card is the selected one)
                let r = slot_rect(&slot);
                let from = R4 { y: r.y + RUN_Y, ..r };
                match self.act_of(w, v) {
                    Some(Act::Switch(uid)) => self.switch_from_menu_at(uid, v, from),
                    Some(Act::Launch(id)) => {
                        let Some(l) = v.launcher.iter().find(|l| l.id == id) else { return };
                        let (name, art) = (l.name.clone(), l.art);
                        self.close_menu();
                        self.start_trans(Target::Entry(id.clone()), name, art, from, v.current);
                        self.out.push(Out::Launch(id));
                    }
                    Some(Act::Game(g)) => self.launch_game(g, from, v),
                    None => self.fx(Sfx::Deny, None),
                }
            }
            Button::B if !repeat => {
                self.home.tiles = false;
                self.fx(Sfx::Tab, Some(Haptic::Tick));
            }
            _ => {}
        }
    }

    /// The hint bar in the tiles.
    pub(super) fn tiles_hints(&self, v: &View) -> Vec<(&'static str, &'static str)> {
        let ws = self.widgets(v);
        let verb = match self.home.focus.and_then(|f| ws.get(f)).and_then(|w| self.act_of(w, v)) {
            Some(Act::Switch(_)) => Some("Switch"),
            Some(Act::Launch(_)) => Some("Open"),
            Some(Act::Game(_)) => Some("Play"),
            None => None,
        };
        let mut h = vec![("◀▶▲▼", "Choose")];
        if let Some(verb) = verb {
            h.push(("A", verb));
        }
        h.push(("B", "Back"));
        h
    }

    /// The pictures the dashboard shows (a game's in Recently closed).
    pub(super) fn dash_pics(&self, v: &View) -> Vec<PathBuf> {
        let has = self.widgets(v).iter().any(|w| w.kind == Kind::Recent);
        if !has {
            return Vec::new();
        }
        self.recent(v).and_then(|r| r.pic).into_iter().collect()
    }

    /// First-time tips: one toast, once ever, when a feature first matters
    /// (nothing else on screen but home at rest, no toast up).
    pub(super) fn tips(&mut self, v: &View) {
        let resting_home = v.current.is_none() && self.dash_selected() && !self.home.tiles;
        if self.toast.is_some() || self.boot.is_some() || self.trans.is_some() || (self.menu.is_some() && !resting_home) || self.page.is_some() || self.sleep.is_some() || self.power.is_some() {
            return;
        }
        let front = v.current.and_then(|u| v.app(u)).filter(|a| a.has_buffer);
        let game = front.is_some_and(|a| a.game) || front.and_then(|a| a.entry.as_deref()).and_then(|e| v.launcher.iter().find(|l| l.id == e)).is_some_and(|l| l.category == Category::Games);
        let due = |id: &str| match id {
            "menu" => front.is_some(),
            "keyboard" => front.is_some() && v.keyboard_app,
            "game" => game,
            // after a few apps were used: where the full list is
            "controls" => resting_home && v.running.len() >= 2,
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

    /// The dashboard in card coordinates (W x RUN_H); `focus`: in the
    /// tiles (the focused one shows).
    pub(super) fn draw_dash(&self, b: &mut B, pal: &Pal, v: &View, focus: bool) {
        b.rect(0.0, 0.0, W, RUN_H, pal.ground);
        let ws = self.widgets(v);
        let slots = crate::widgets::layout(ws);
        let on = |s: &Slot| focus && Some(s.i) == self.home.focus;
        // the focused tile last: its offset block goes over its neighbours' gap
        for s in slots.iter().filter(|s| !on(s)).chain(slots.iter().filter(|s| on(s))) {
            self.draw_widget(b, pal, v, &ws[s.i], s, slot_rect(s), on(s));
        }
        if ws.is_empty() {
            let st = ts(Face::Body400, 24.0, pal.soft);
            b.text(56.0, RUN_H / 2.0 - 12.0, "No widgets (mortar.shell.home.widgets).", &st);
        }
    }

    /// The tile's block (accent or plain) with its outline; the focused one
    /// sits on a lavender offset block.
    fn tile(&self, b: &mut B, pal: &Pal, r: R4, fill: Color, on: bool) {
        if on {
            b.block(r.x + 8.0, r.y + 8.0, r.w, r.h, pal.lavender, pal.ink);
        }
        b.block(r.x, r.y, r.w, r.h, fill, pal.ink);
        if on {
            b.frame(r.x, r.y, r.w, r.h, 2.0, pal.ink);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_widget(&self, b: &mut B, pal: &Pal, v: &View, w: &Widget, s: &Slot, r: R4, on: bool) {
        let pad = 16.0;
        // rows inside a tile: label, value, bar, small text
        let (y_label, y_val, y_bar, y_sub) = (r.y + 12.0, r.y + 38.0, r.y + r.h - 48.0, r.y + r.h - 30.0);
        let label = |b: &mut B, ink: Color, txt: &str| {
            let st = ts(Face::Mono600, 14.0, ink).tracking(0.08);
            b.text(r.x + pad, y_label, &txt.to_uppercase(), &st);
        };
        let soft_on = |fill_is_accent: bool| if fill_is_accent { pal.dark } else { pal.soft };
        let quiet = |b: &mut B, big: &str, small: &str| {
            let t = ts(Face::SerifItalic, 40.0, pal.soft).line_height(1.0);
            b.text(r.x + pad, y_val + 4.0, big, &t);
            let st = ts(Face::Mono400, 16.0, pal.soft);
            let sm = b.fit(small, &st, r.w - 2.0 * pad);
            b.text(r.x + pad, y_sub, &sm, &st);
        };
        match w.kind {
            Kind::Clock => {
                let clock = ts(Face::SerifItalic, 112.0, pal.ink).line_height(0.9).tracking(-0.02);
                b.text(r.x + 2.0, r.y - 2.0, &v.sys.time, &clock);
                let date = ts(Face::Mono400, 17.0, pal.soft).tracking(0.04);
                b.text(r.x + 8.0, r.y + 104.0, &date_line(v.host), &date);
                if v.sys.plugged {
                    self.draw_charge_line(b, pal, v, r.x + 8.0, r.y + 138.0);
                }
            }
            Kind::Recent => {
                let Some(rc) = self.recent(v) else {
                    self.tile(b, pal, r, pal.tile, on);
                    label(b, pal.soft, &w.label());
                    quiet(b, "Nothing yet", "What you close shows here");
                    return;
                };
                let accent = rc.art.accent(pal);
                self.tile(b, pal, r, pal.tile, on);
                b.rect(r.x + 1.0, r.y + 1.0, 10.0, r.h - 2.0, accent);
                b.rect(r.x + 11.0, r.y + 1.0, 1.5, r.h - 2.0, pal.ink);
                // its resume picture, else its illustration
                let (pw, ph) = (150.0, r.h - 2.0 * 14.0);
                let (px, py) = (r.x + r.w - pad - pw, r.y + 14.0);
                match rc.pic.as_deref().and_then(|p| v.games.and_then(|g| g.pic(Some(p)))) {
                    Some(p) => {
                        b.rect(px, py, pw, ph, Color(0, 0, 0, 255));
                        let f = p.fit(px, py, pw, ph);
                        b.image(Src::Pic(p.id), f[0], f[1], f[2], f[3], [0.0, 0.0, 1.0, 1.0]);
                    }
                    None => {
                        b.rect(px, py, pw, ph, accent);
                        art::draw(b, rc.art, px + pw / 2.0, py + ph / 2.0, 0.52, *pal, pal.dark);
                    }
                }
                b.frame(px, py, pw, ph, 1.5, pal.ink);
                let x = r.x + pad + 12.0;
                let lab = ts(Face::Mono600, 14.0, pal.soft).tracking(0.08);
                b.text(x, y_label, &w.label().to_uppercase(), &lab);
                let nm = ts(Face::SerifItalic, 40.0, pal.ink).line_height(1.0);
                let n = b.fit(&rc.name, &nm, px - x - 12.0);
                b.text(x, y_val + 2.0, &n, &nm);
                let st = ts(Face::Mono400, 16.0, pal.soft);
                let s1 = b.fit(rc.sub, &st, px - x - 12.0);
                b.text(x, y_sub, &s1, &st);
            }
            Kind::NeedsYou => {
                let items = v.dash.attention;
                let fill = if items.is_empty() { pal.tile } else { pal.yellow };
                self.tile(b, pal, r, fill, on);
                label(b, soft_on(!items.is_empty()), &w.label());
                let Some(it) = items.first() else {
                    quiet(b, "All quiet", "Apps that wait for you show here");
                    return;
                };
                if items.len() > 1 {
                    chip(b, r.x + r.w - pad, y_label + 9.0, &format!("+{}", items.len() - 1), 14.0, (8.0, 2.0), pal.dark, pal.yellow);
                }
                let name_st = ts(Face::SerifItalic, 40.0, pal.dark).line_height(1.0);
                let txt = ts(Face::Body500, 19.0, pal.dark);
                let name = v.launcher.iter().find(|l| l.id == it.app).map(|l| l.name.clone()).unwrap_or_else(|| it.app.clone());
                let n = b.fit(&name, &name_st, r.w - 2.0 * pad);
                b.text(r.x + pad, y_val + 2.0, &n, &name_st);
                let t = b.fit(&it.text, &txt, r.w - 2.0 * pad);
                b.text(r.x + pad, y_sub - 4.0, &t, &txt);
            }
            Kind::NowPlaying => {
                let p = self.playing(v);
                self.tile(b, pal, r, if p.is_some() { pal.orange } else { pal.tile }, on);
                label(b, soft_on(p.is_some()), &w.label());
                match p {
                    Some((a, level)) => {
                        let nm = ts(Face::SerifItalic, 44.0, pal.dark).line_height(1.0);
                        let n = b.fit(&a.name, &nm, r.w - 2.0 * pad - 120.0);
                        b.text(r.x + pad, y_val, &n, &nm);
                        let st = ts(Face::Mono400, 16.0, pal.dark);
                        b.text(r.x + pad, y_sub, &format!("level {level}%"), &st);
                        // bars, still (no level meters: see the Mixer)
                        for (k, hh) in [36.0, 58.0, 24.0, 46.0].iter().enumerate() {
                            let bx = r.x + r.w - pad - 4.0 * 22.0 + k as f32 * 22.0;
                            b.block(bx, r.y + r.h - 24.0 - hh, 14.0, *hh, pal.dark, pal.dark);
                        }
                    }
                    None => quiet(b, "Quiet", "Nothing is making sound"),
                }
            }
            Kind::Battery => {
                self.tile(b, pal, r, pal.tile, on);
                label(b, pal.soft, &w.label());
                let val = ts(Face::SerifItalic, 50.0, pal.ink).line_height(1.0);
                let st = ts(Face::Mono400, 16.0, pal.soft);
                match v.sys.battery {
                    Some(p) => {
                        b.text(r.x + pad, y_val - 4.0, &format!("{p}%"), &val);
                        let (bx, bw) = (r.x + pad, r.w - 2.0 * pad);
                        let fill = if v.sys.charging || p > 15 { pal.lime } else if p > 5 { pal.yellow } else { pal.orange };
                        b.rect(bx, y_bar, bw, 12.0, pal.ground_2);
                        b.rect(bx, y_bar, bw * p.min(100) as f32 / 100.0, 12.0, fill);
                        b.frame(bx, y_bar, bw, 12.0, 1.5, pal.ink);
                        let sub = if v.sys.charging {
                            "charging".to_string()
                        } else if v.sys.plugged {
                            "on power".to_string()
                        } else {
                            v.dash.about.and_then(|a| a.battery.health.clone()).map(|h| format!("health {}", h.to_lowercase())).unwrap_or_else(|| "on battery".into())
                        };
                        b.text(r.x + pad, y_sub + 4.0, &sub, &st);
                    }
                    None => {
                        b.text(r.x + pad, y_val - 4.0, "—", &val);
                        b.text(r.x + pad, y_sub + 4.0, "no battery", &st);
                    }
                }
            }
            Kind::Wifi => {
                self.tile(b, pal, r, pal.tile, on);
                label(b, pal.soft, &w.label());
                let val = ts(Face::SerifItalic, 44.0, pal.ink).line_height(1.0);
                let st = ts(Face::Mono400, 16.0, pal.soft);
                let ssid = v.net.and_then(|n| n.wifi.ssid.clone()).filter(|_| v.sys.wifi);
                let (t, sub) = match (v.sys.wifi, v.radios.wifi) {
                    (true, _) => ("On", ssid.unwrap_or_else(|| "connected".into())),
                    (false, Some(false)) => ("Off", "radio off".to_string()),
                    _ => ("Offline", "not connected".to_string()),
                };
                b.text(r.x + pad, y_val, t, &val);
                let sb = b.fit(&sub, &st, r.w - 2.0 * pad);
                b.text(r.x + pad, y_sub + 4.0, &sb, &st);
                if v.sys.usb_net {
                    chip(b, r.x + r.w - pad, y_label + 9.0, "USB", 13.0, (7.0, 1.0), pal.lavender, pal.lavender_ink);
                }
            }
            Kind::Storage => {
                self.tile(b, pal, r, pal.tile, on);
                label(b, pal.soft, &w.label());
                let val = ts(Face::SerifItalic, 44.0, pal.ink).line_height(1.0);
                let st = ts(Face::Mono400, 16.0, pal.soft);
                match v.dash.about.and_then(|a| a.storage) {
                    Some((free, total)) => {
                        b.text(r.x + pad, y_val, &crate::about::gb(free), &val);
                        let (bx, bw) = (r.x + pad, r.w - 2.0 * pad);
                        let used = 1.0 - free as f32 / total.max(1) as f32;
                        b.rect(bx, y_bar, bw, 12.0, pal.ground_2);
                        b.rect(bx, y_bar, bw * used.clamp(0.0, 1.0), 12.0, if used > 0.9 { pal.orange } else { pal.lavender });
                        b.frame(bx, y_bar, bw, 12.0, 1.5, pal.ink);
                        b.text(r.x + pad, y_sub + 4.0, &format!("free of {}", crate::about::gb(total)), &st);
                    }
                    None => {
                        b.text(r.x + pad, y_val, "—", &val);
                        b.text(r.x + pad, y_sub + 4.0, "not known", &st);
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
                let st = ts(Face::Mono400, 16.0, if on_accent { pal.dark } else { pal.soft });
                match out {
                    Some(o) => {
                        let mut lines = o.value.lines();
                        let first = lines.next().unwrap_or("");
                        let val = ts(Face::SerifItalic, if first.chars().count() <= 8 { 50.0 } else { 32.0 }, ink).line_height(1.0);
                        let f = b.fit(first, &val, r.w - 2.0 * pad);
                        b.text(r.x + pad, y_val - 2.0, &f, &val);
                        let rest = lines.next().unwrap_or(if o.error { "the script failed" } else { "" });
                        let rs = b.fit(rest, &st, r.w - 2.0 * pad);
                        b.text(r.x + pad, y_sub + 4.0, &rs, &st);
                        if o.error {
                            chip(b, r.x + r.w - pad, y_label + 9.0, "!", 14.0, (8.0, 2.0), pal.orange, pal.dark);
                        }
                    }
                    None => {
                        let val = ts(Face::SerifItalic, 40.0, pal.soft).line_height(1.0);
                        b.text(r.x + pad, y_val, "…", &val);
                        b.text(r.x + pad, y_sub + 4.0, "waiting for the script", &st);
                    }
                }
            }
        }
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
