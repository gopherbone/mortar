// SPDX-License-Identifier: GPL-2.0-or-later
//! The dashboard: the top of the Running stack (cross.rs). Being home is
//! the stack on it with no app in front: then it is full screen, the apps
//! a slim strip at the bottom; ▼ shrinks it into a card above them. Not a
//! launcher (apps start from the cross), and quiet: the big clock (the
//! clock while it is selected; the status bar's fades), the date and an
//! inline battery, then only the tiles (widgets.rs, 4 x 2) with something
//! to say, thin outlines on the ground: Needs you (attention.rs, the one
//! yellow tile), Now playing, Recently closed (what you closed last that
//! no longer runs; A starts it again), and Wi-Fi, storage or scripts if
//! configured.
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

// full-screen coordinates (the card is the same picture, shrunk): the
// clock and the line under it, then up to two rows of tiles, clear of the
// bars and of the strip of apps at the bottom
const GX: f32 = 56.0;
const GAP: f32 = 16.0;
const TW: f32 = (W - 2.0 * GX - 3.0 * GAP) / 4.0;
const TH: f32 = 126.0;
const CLOCK_Y: f32 = 84.0;
/// The tiles' top: under the clock block, or high up without a clock.
fn tiles_top(ws: &[Widget]) -> f32 {
    if ws.iter().any(|w| w.kind == Kind::Clock) { 352.0 } else { 104.0 }
}

fn slot_rect(ws: &[Widget], s: &Slot) -> R4 {
    R4 { x: GX + s.col as f32 * (TW + GAP), y: tiles_top(ws) + s.row as f32 * (TH + GAP), w: s.w as f32 * TW + (s.w as f32 - 1.0) * GAP, h: TH }
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

    /// A tile has something to say (an empty one is left out and the
    /// others move up: no placeholders).
    fn shown(&self, i: usize, v: &View) -> bool {
        let Some(w) = self.widgets(v).get(i) else { return false };
        match w.kind {
            Kind::NeedsYou => !v.dash.attention.is_empty(),
            Kind::NowPlaying => self.playing(v).is_some(),
            Kind::Recent => self.recent(v).is_some(),
            Kind::Command => v.dash.outputs.get(i).is_some_and(|o| o.is_some()),
            _ => true,
        }
    }

    /// The tiles that show, placed.
    fn dash_slots(&self, v: &View) -> Vec<Slot> {
        crate::widgets::layout(self.widgets(v), |i| self.shown(i, v))
    }

    /// Where tile `i` is on screen (tests).
    #[cfg(test)]
    pub(super) fn dash_tile_rect(&self, v: &View, i: usize) -> Option<[f32; 4]> {
        let ws = self.widgets(v);
        self.dash_slots(v).iter().find(|s| s.i == i).map(|s| slot_rect(ws, s)).map(|r| [r.x, r.y, r.w, r.h])
    }

    /// A tile does something (A on the card enters them).
    pub(super) fn dash_acts(&self, v: &View) -> bool {
        let ws = self.widgets(v);
        self.dash_slots(v).iter().any(|s| self.act_of(&ws[s.i], v).is_some())
    }

    /// Keep the focus on a tile that does something (Needs you first),
    /// leave the tiles when none does, and remember what is on screen for
    /// Recently closed.
    pub(super) fn sync_home(&mut self, v: &View) {
        let ws = self.widgets(v);
        let slots = self.dash_slots(v);
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
        let slots = self.dash_slots(v);
        match b {
            Button::Up | Button::Down | Button::Left | Button::Right => {
                let Some(cur) = self.home.focus.and_then(|f| slots.iter().find(|s| s.i == f)) else {
                    self.fx(Sfx::Deny, None);
                    return;
                };
                let r = slot_rect(ws, cur);
                let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
                // the nearest tile that does something, that way
                let best = slots
                    .iter()
                    .filter(|s| s.i != cur.i && self.act_of(&ws[s.i], v).is_some())
                    .filter_map(|s| {
                        let o = slot_rect(ws, s);
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
                // the app grows out of the tile
                let from = slot_rect(ws, &slot);
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

    /// The dashboard, full screen (W x H; as a card it is the same picture
    /// shrunk): the big clock, the line under it (date, host, battery),
    /// then the tiles that have something to say. Plain ground, ink text,
    /// thin outlines; only Needs you is yellow. `focus`: in the tiles (the
    /// focused one shows).
    pub(super) fn draw_dash(&self, b: &mut B, pal: &Pal, v: &View, focus: bool) {
        let ws = self.widgets(v);
        // the pattern, right of the clock (under everything else)
        if self.prefs.home_art != crate::homeart::Kind::Off {
            self.art.draw(b, pal, crate::homeart::hour_of(&v.sys.time));
        }
        if ws.iter().any(|w| w.kind == Kind::Clock) {
            let clock = ts(Face::SerifItalic, 216.0, pal.ink).line_height(0.86).tracking(-0.025);
            b.text(GX - 8.0, CLOCK_Y, &v.sys.time, &clock);
            // the line under it: date and host, then the battery (inline)
            let st = ts(Face::Mono400, 19.0, pal.soft).tracking(0.06);
            let y = CLOCK_Y + 196.0;
            b.text(GX, y, &date_line(v.host), &st);
            if v.sys.plugged {
                self.draw_charge_line(b, pal, v, GX, y + 40.0);
            }
        }
        let slots = self.dash_slots(v);
        let on = |s: &Slot| focus && Some(s.i) == self.home.focus;
        // the focused tile last: its offset block goes over its neighbours' gap
        for s in slots.iter().filter(|s| !on(s)).chain(slots.iter().filter(|s| on(s))) {
            self.draw_widget(b, pal, v, &ws[s.i], s, slot_rect(ws, s), on(s));
        }
        if ws.is_empty() {
            let st = ts(Face::Body400, 24.0, pal.soft);
            b.text(GX, H / 2.0 - 12.0, "No widgets (mortar.shell.home.widgets).", &st);
        }
    }

    /// A tile: a thin ink outline on the ground (Needs you: yellow); the
    /// focused one sits on a lavender offset block.
    fn tile(&self, b: &mut B, pal: &Pal, r: R4, fill: Option<Color>, on: bool) {
        if on {
            b.block(r.x + 7.0, r.y + 7.0, r.w, r.h, pal.lavender, pal.ink);
            b.rect(r.x, r.y, r.w, r.h, pal.ground);
        }
        if let Some(f) = fill {
            b.rect(r.x, r.y, r.w, r.h, f);
        }
        b.frame(r.x, r.y, r.w, r.h, if on { 2.0 } else { 1.0 }, if on || fill.is_some() { pal.ink } else { with_a(pal.ink, 0.45) });
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_widget(&self, b: &mut B, pal: &Pal, v: &View, w: &Widget, s: &Slot, r: R4, on: bool) {
        let pad = 16.0;
        // rows inside a tile: label, value, bar, small text
        let (y_label, y_val, y_bar, y_sub) = (r.y + 12.0, r.y + 36.0, r.y + r.h - 44.0, r.y + r.h - 28.0);
        let label = |b: &mut B, ink: Color, txt: &str| {
            let st = ts(Face::Mono600, 13.0, ink).tracking(0.1);
            b.text(r.x + pad, y_label, &txt.to_uppercase(), &st);
        };
        let small = ts(Face::Mono400, 15.0, pal.soft);
        match w.kind {
            // inline (draw_dash)
            Kind::Clock => {}
            Kind::Battery => {
                self.tile(b, pal, r, None, on);
                label(b, pal.soft, &w.label());
                let val = ts(Face::SerifItalic, 38.0, pal.ink).line_height(1.0);
                match v.sys.battery {
                    Some(p) => {
                        b.text(r.x + pad, y_val, &format!("{p}%"), &val);
                        let (bx, bw) = (r.x + pad, r.w - 2.0 * pad);
                        b.rect(bx, y_bar, bw, 3.0, with_a(pal.ink, 0.15));
                        b.rect(bx, y_bar, bw * p.min(100) as f32 / 100.0, 3.0, if v.sys.charging || p > 15 { pal.ink } else { pal.orange });
                        let sub = if v.sys.charging { "charging" } else if v.sys.plugged { "on power" } else { "on battery" };
                        b.text(r.x + pad, y_sub, sub, &small);
                    }
                    None => {
                        b.text(r.x + pad, y_val, "—", &val);
                        b.text(r.x + pad, y_sub, "no battery", &small);
                    }
                }
            }
            Kind::Recent => {
                let Some(rc) = self.recent(v) else { return };
                self.tile(b, pal, r, None, on);
                // a game's resume picture, small, on the right
                let pic = rc.pic.as_deref().and_then(|p| v.games.and_then(|g| g.pic(Some(p))));
                let right = match pic {
                    Some(p) => {
                        let (pw, ph) = (r.h * 4.0 / 3.0 - 32.0, r.h - 32.0);
                        let (px, py) = (r.x + r.w - pad - pw, r.y + 16.0);
                        let f = p.fit(px, py, pw, ph);
                        b.image(Src::Pic(p.id), f[0], f[1], f[2], f[3], [0.0, 0.0, 1.0, 1.0]);
                        b.frame(f[0], f[1], f[2], f[3], 1.0, with_a(pal.ink, 0.45));
                        px - 12.0
                    }
                    None => r.x + r.w - pad,
                };
                label(b, pal.soft, &w.label());
                let nm = ts(Face::SerifItalic, 38.0, pal.ink).line_height(1.0);
                let n = b.fit(&rc.name, &nm, right - r.x - pad);
                b.text(r.x + pad, y_val, &n, &nm);
                let s1 = b.fit(rc.sub, &small, right - r.x - pad);
                b.text(r.x + pad, y_sub, &s1, &small);
            }
            Kind::NeedsYou => {
                let items = v.dash.attention;
                let Some(it) = items.first() else { return };
                self.tile(b, pal, r, Some(pal.yellow), on);
                label(b, pal.dark, &w.label());
                if items.len() > 1 {
                    chip(b, r.x + r.w - pad, y_label + 8.0, &format!("+{}", items.len() - 1), 13.0, (7.0, 1.0), pal.dark, pal.yellow);
                }
                let name_st = ts(Face::SerifItalic, 38.0, pal.dark).line_height(1.0);
                let txt = ts(Face::Body500, 18.0, pal.dark);
                let name = v.launcher.iter().find(|l| l.id == it.app).map(|l| l.name.clone()).unwrap_or_else(|| it.app.clone());
                let n = b.fit(&name, &name_st, r.w - 2.0 * pad);
                b.text(r.x + pad, y_val, &n, &name_st);
                let t = b.fit(&it.text, &txt, r.w - 2.0 * pad);
                b.text(r.x + pad, y_sub - 4.0, &t, &txt);
            }
            Kind::NowPlaying => {
                let Some((a, level)) = self.playing(v) else { return };
                self.tile(b, pal, r, None, on);
                label(b, pal.soft, &w.label());
                let nm = ts(Face::SerifItalic, 38.0, pal.ink).line_height(1.0);
                let n = b.fit(&a.name, &nm, r.w - 2.0 * pad - 80.0);
                b.text(r.x + pad, y_val, &n, &nm);
                b.text(r.x + pad, y_sub, &format!("level {level}%"), &small);
                // bars, still and thin (no level meters: see the Mixer)
                for (k, hh) in [26.0, 42.0, 18.0, 34.0].iter().enumerate() {
                    let bx = r.x + r.w - pad - 4.0 * 14.0 + k as f32 * 14.0;
                    b.rect(bx, r.y + r.h - 22.0 - hh, 6.0, *hh, pal.ink);
                }
            }
            Kind::Wifi => {
                self.tile(b, pal, r, None, on);
                label(b, pal.soft, &w.label());
                let val = ts(Face::SerifItalic, 38.0, pal.ink).line_height(1.0);
                let ssid = v.net.and_then(|n| n.wifi.ssid.clone()).filter(|_| v.sys.wifi);
                let (t, sub) = match (v.sys.wifi, v.radios.wifi) {
                    (true, _) => ("On", ssid.unwrap_or_else(|| "connected".into())),
                    (false, Some(false)) => ("Off", "radio off".to_string()),
                    _ => ("Offline", "not connected".to_string()),
                };
                b.text(r.x + pad, y_val, t, &val);
                let sb = b.fit(&sub, &small, r.w - 2.0 * pad);
                b.text(r.x + pad, y_sub, &sb, &small);
                if v.sys.usb_net {
                    chip(b, r.x + r.w - pad, y_label + 8.0, "USB", 12.0, (6.0, 1.0), pal.ground, pal.ink);
                }
            }
            Kind::Storage => {
                self.tile(b, pal, r, None, on);
                label(b, pal.soft, &w.label());
                let val = ts(Face::SerifItalic, 38.0, pal.ink).line_height(1.0);
                match v.dash.about.and_then(|a| a.storage) {
                    Some((free, total)) => {
                        b.text(r.x + pad, y_val, &crate::about::gb(free), &val);
                        let (bx, bw) = (r.x + pad, r.w - 2.0 * pad);
                        let used = (1.0 - free as f32 / total.max(1) as f32).clamp(0.0, 1.0);
                        b.rect(bx, y_bar, bw, 3.0, with_a(pal.ink, 0.15));
                        b.rect(bx, y_bar, bw * used, 3.0, if used > 0.9 { pal.orange } else { pal.ink });
                        b.text(r.x + pad, y_sub, &format!("free of {}", crate::about::gb(total)), &small);
                    }
                    None => {
                        b.text(r.x + pad, y_val, "—", &val);
                        b.text(r.x + pad, y_sub, "not known", &small);
                    }
                }
            }
            Kind::Command => {
                let Some(o) = v.dash.outputs.get(s.i).and_then(|o| o.as_ref()) else { return };
                self.tile(b, pal, r, None, on);
                let title = o.title.clone().unwrap_or_else(|| w.label());
                label(b, pal.soft, &title);
                // its accent: a small square by the label, not a fill
                if let Some(a) = o.accent.or(w.accent).filter(|a| *a != crate::widgets::Accent::Ground) {
                    b.rect(r.x + r.w - pad - 10.0, y_label + 2.0, 10.0, 10.0, accent_color(Some(a), pal));
                }
                let mut lines = o.value.lines();
                let first = lines.next().unwrap_or("");
                let val = ts(Face::SerifItalic, if first.chars().count() <= 8 { 44.0 } else { 30.0 }, pal.ink).line_height(1.0);
                let f = b.fit(first, &val, r.w - 2.0 * pad);
                b.text(r.x + pad, y_val - 2.0, &f, &val);
                let rest = lines.next().unwrap_or(if o.error { "the script failed" } else { "" });
                let rs = b.fit(rest, &small, r.w - 2.0 * pad);
                b.text(r.x + pad, y_sub, &rs, &small);
                if o.error {
                    chip(b, r.x + r.w - pad, y_label + 8.0, "!", 13.0, (7.0, 1.0), pal.orange, pal.dark);
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
