// SPDX-License-Identifier: GPL-2.0-or-later
//! The MENU's games (mortar-play, one app per game; games.rs):
//!
//! - The Games category: Continue (the last game, its resume picture beside
//!   the row), up to five more recently played games, one row per system
//!   that has games (with the count), then the other game entries (PICO-8,
//!   PuzzleScript) and "All systems" (mortar-play's own library).
//! - A system's glass page: its games (cleaned names, a RESUME marker where
//!   a resume state exists), the selected one's picture on the right; L1/R1
//!   page through, A plays.
//! - The game panel (Y on a game in Running): Save state and Load state
//!   (◀▶ pick one of four slots, each with its picture and age), Reset,
//!   Fast-forward, Change disc (multi-disc games), Quit. A sleeping game has
//!   Wake and Close.
//!
//! A child module of `shellui` next to `cross`.

use super::cross::{draw_icon, glass, Glass, Ic, Row, RowInfo, BAR_H, HINT_H};
use super::*;
use crate::games::{GameCmd, GameRef, GameStatus, Pic, SLOTS};
use std::path::PathBuf;

/// A Games row that is not a launcher entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GRow {
    /// The last game played.
    Continue,
    /// Another recently played game (by file).
    Recent(PathBuf),
    /// A system with games (by id): opens its page; a no-content system
    /// (2048) starts.
    System(String),
}

pub(super) struct GamePage {
    pub sys: String,
    sel: usize,
    scroll: Spring,
    /// 0: in place; 1: off to the right.
    x: Spring,
    pub closing: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PItem {
    Save,
    Load,
    Reset,
    Ff,
    Disc,
    Quit,
    Wake,
    Close,
}

impl PItem {
    fn name(self) -> &'static str {
        match self {
            PItem::Save => "Save state",
            PItem::Load => "Load state",
            PItem::Reset => "Reset",
            PItem::Ff => "Fast-forward",
            PItem::Disc => "Change disc",
            PItem::Quit => "Quit",
            PItem::Wake => "Wake",
            PItem::Close => "Close",
        }
    }
    fn slots(self) -> bool {
        matches!(self, PItem::Save | PItem::Load)
    }
}

pub(super) struct GamePanel {
    pub uid: u32,
    sel: usize,
    slot: u8,
    op: Spring,
    pub closing: bool,
}

impl GamePage {
    pub fn springs(&mut self) -> Vec<&mut Spring> {
        vec![&mut self.scroll, &mut self.x]
    }
    pub fn moving(&self) -> bool {
        self.closing || !(self.scroll.rest() && self.x.rest())
    }
    pub fn gone(&self) -> bool {
        self.closing && self.x.x > 0.98
    }
}

impl GamePanel {
    pub fn springs(&mut self) -> Vec<&mut Spring> {
        vec![&mut self.op]
    }
    pub fn moving(&self) -> bool {
        self.closing || !self.op.rest()
    }
    pub fn gone(&self) -> bool {
        self.closing && self.op.x < 0.02
    }
}

fn lib<'a>(v: &View<'a>) -> Option<&'a crate::games::Library> {
    v.games.and_then(|g| g.lib.as_ref())
}

/// The Games category has something besides launcher entries.
pub(super) fn has_games(v: &View) -> bool {
    lib(v).is_some_and(|l| !l.shelves().is_empty() || !l.recent.is_empty())
}

/// The Games category's rows.
pub(super) fn games_rows(v: &View) -> Vec<Row> {
    let mut r = Vec::new();
    if let Some(l) = lib(v) {
        if !l.recent.is_empty() {
            r.push(Row::Game(GRow::Continue));
        }
        r.extend(l.recent.iter().skip(1).take(5).map(|x| Row::Game(GRow::Recent(x.path.clone()))));
        r.extend(l.shelves().iter().map(|s| Row::Game(GRow::System(s.id.clone()))));
    }
    let library = v.games.and_then(|g| g.library_app.as_deref());
    // the other game entries, mortar-play's own library ("All systems") last
    let mut entries: Vec<&LauncherInfo> = v.launcher.iter().filter(|l| l.category == Category::Games).collect();
    entries.sort_by_key(|l| Some(l.id.as_str()) == library);
    r.extend(entries.into_iter().map(|l| Row::Launch(l.id.clone())));
    r
}

/// The game a row starts.
fn row_game(g: &GRow, v: &View) -> Option<GameRef> {
    let l = lib(v)?;
    match g {
        GRow::Continue => l.recent.first().map(|r| l.recent_ref(r)),
        GRow::Recent(p) => l.recent.iter().find(|r| r.path == *p).map(|r| l.recent_ref(r)),
        GRow::System(id) => l.system(id).filter(|s| s.no_content).map(|s| l.system_ref(s)),
    }
}

/// The resume picture of a Continue / recent row.
fn row_thumb(g: &GRow, v: &View) -> Option<PathBuf> {
    let l = lib(v)?;
    match g {
        GRow::Continue => l.recent.first()?.thumb.clone(),
        GRow::Recent(p) => l.recent.iter().find(|r| r.path == *p)?.thumb.clone(),
        GRow::System(_) => None,
    }
}

fn running_key(key: &str, v: &View) -> bool {
    v.by_entry(key).is_some()
}

pub(super) fn game_row_info(g: &GRow, v: &View) -> RowInfo {
    let ri = |title: String, sub: String| RowInfo { title, sub, icon: Ic::Games, level: None, value: None, chevron: false };
    let Some(l) = lib(v) else { return ri(String::new(), String::new()) };
    let state = |g: &GameRef, resume: bool| {
        if running_key(&g.key(), v) {
            "running"
        } else if resume {
            "resumes"
        } else {
            "starts fresh"
        }
    };
    match g {
        GRow::Continue => match l.recent.first() {
            Some(r) => {
                let gr = l.recent_ref(r);
                ri(r.name.clone(), format!("Continue · {} · {}", gr.system_name, state(&gr, r.resume)))
            }
            None => ri(String::new(), String::new()),
        },
        GRow::Recent(p) => match l.recent.iter().find(|r| r.path == *p) {
            Some(r) => {
                let gr = l.recent_ref(r);
                ri(r.name.clone(), format!("{} · recently played · {}", r.tag, state(&gr, r.resume)))
            }
            None => ri(String::new(), String::new()),
        },
        GRow::System(id) => match l.system(id) {
            Some(s) if s.no_content => ri(s.name.clone(), format!("{} · no game needed", s.tag)),
            Some(s) => {
                let n = if s.count == 1 { "1 game".to_string() } else { format!("{} games", s.count) };
                RowInfo { value: Some(n), chevron: true, ..ri(s.name.clone(), s.tag.clone()) }
            }
            None => ri(id.clone(), String::new()),
        },
    }
}

/// "All systems": the launcher entry that opens mortar-play's own library.
pub(super) fn launch_label(id: &str, v: &View) -> Option<(String, String)> {
    let g = v.games?;
    (g.library_app.as_deref() == Some(id)).then(|| ("All systems".to_string(), "mortar-play's library · every system".to_string()))
}

pub(super) fn game_hint(g: &GRow, v: &View) -> &'static str {
    match g {
        GRow::System(id) if lib(v).and_then(|l| l.system(id)).is_some_and(|s| !s.no_content) => "Open",
        _ => match row_game(g, v) {
            Some(gr) if running_key(&gr.key(), v) => "Switch",
            _ => "Play",
        },
    }
}

fn panel_items(uid: u32, v: &View) -> Vec<PItem> {
    if v.app(uid).is_some_and(|a| a.asleep) {
        return vec![PItem::Wake, PItem::Close];
    }
    let st = v.games.and_then(|g| g.status.get(&uid));
    let mut it = vec![PItem::Save, PItem::Load, PItem::Reset, PItem::Ff];
    if st.is_some_and(|s| s.multi_disc()) {
        it.push(PItem::Disc);
    }
    it.push(PItem::Quit);
    it
}

// the system page: the list on the left, the picture on the right
const PAGE_TOP: f32 = 176.0;
const PROW_H: f32 = 64.0;
const PER_PAGE: usize = 8;
const LIST_W: f32 = 600.0;
// the game panel
const PX: f32 = 64.0;
const PY: f32 = 104.0;
const PW: f32 = 896.0;
const PH: f32 = 560.0;
const PROW: f32 = 64.0;

impl ShellUi {
    /// A game row: play it (or switch to it), or open its system.
    pub(super) fn activate_game(&mut self, g: GRow, v: &View) {
        if let GRow::System(id) = &g
            && lib(v).and_then(|l| l.system(id)).is_some_and(|s| !s.no_content)
        {
            self.open_gpage(id.clone());
            return;
        }
        match row_game(&g, v) {
            Some(gr) => {
                let from = self.sel_rect();
                self.launch_game(gr, from, v);
            }
            None => self.fx(Sfx::Deny, None),
        }
    }

    /// Start a game as its own app, zooming from `from`; a game that runs
    /// is switched to.
    fn launch_game(&mut self, g: GameRef, from: R4, v: &View) {
        if let Some(a) = v.by_entry(&g.key()) {
            let uid = a.uid;
            self.switch_from_menu_at(uid, v, from);
            return;
        }
        self.close_menu();
        self.start_trans(Target::Entry(g.key()), g.title(), Art::Games, from, v.current);
        self.out.push(Out::LaunchGame(g));
    }

    fn open_gpage(&mut self, sys: String) {
        let reduce = self.prefs.reduce_motion;
        let Some(m) = &mut self.menu else { return };
        let mut x = Spring::new(1.0);
        x.k_mul = 0.9;
        x.set(0.0);
        if reduce {
            x.snap(0.0);
        }
        m.gpage = Some(GamePage { sys, sel: 0, scroll: Spring::new(0.0), x, closing: false });
        self.fx(Sfx::Tab, Some(Haptic::Tick));
    }

    fn close_gpage(&mut self) {
        let reduce = self.prefs.reduce_motion;
        let Some(p) = self.menu.as_mut().and_then(|m| m.gpage.as_mut()) else { return };
        p.closing = true;
        p.x.set(1.0);
        if reduce {
            p.x.snap(1.0);
        }
        self.fx(Sfx::DrawerUp, Some(Haptic::Tick));
    }

    /// Y on a game in Running.
    pub(super) fn open_gpanel(&mut self, uid: u32, v: &View) {
        let reduce = self.prefs.reduce_motion;
        let Some(m) = &mut self.menu else { return };
        let mut op = Spring::new(0.0);
        op.set(1.0);
        if reduce {
            op.snap(1.0);
        }
        m.gpanel = Some(GamePanel { uid, sel: 0, slot: 0, op, closing: false });
        if !v.app(uid).is_some_and(|a| a.asleep) {
            self.out.push(Out::GameCtl(uid, GameCmd::ListStates));
        }
        self.fx(Sfx::Drawer, Some(Haptic::Tick));
    }

    fn close_gpanel(&mut self) {
        let reduce = self.prefs.reduce_motion;
        let Some(p) = self.menu.as_mut().and_then(|m| m.gpanel.as_mut()) else { return };
        p.closing = true;
        p.op.set(0.0);
        if reduce {
            p.op.snap(0.0);
        }
        self.fx(Sfx::DrawerUp, None);
    }

    /// Drop pages that went, and a panel whose game is gone.
    pub(super) fn sync_games(&mut self, v: &View) {
        let Some(m) = &mut self.menu else { return };
        if m.gpage.as_ref().is_some_and(|p| p.gone() || lib(v).and_then(|l| l.system(&p.sys)).is_none()) {
            m.gpage = None;
            self.dirty = true;
        }
        if m.gpanel.as_ref().is_some_and(|p| p.gone() || v.app(p.uid).is_none()) {
            m.gpanel = None;
            self.dirty = true;
        }
    }

    /// A game page or panel takes the buttons.
    pub(super) fn games_take(&self) -> bool {
        self.menu.as_ref().is_some_and(|m| m.gpanel.as_ref().is_some_and(|p| !p.closing) || m.gpage.as_ref().is_some_and(|p| !p.closing))
    }

    pub(super) fn games_button(&mut self, b: Button, repeat: bool, v: &View) {
        let Some(m) = &self.menu else { return };
        if m.gpanel.as_ref().is_some_and(|p| !p.closing) {
            self.gpanel_button(b, repeat, v);
        } else {
            self.gpage_button(b, repeat, v);
        }
    }

    fn gpage_button(&mut self, b: Button, repeat: bool, v: &View) {
        let Some(sys) = self.menu.as_ref().and_then(|m| m.gpage.as_ref()).map(|p| p.sys.clone()) else { return };
        let Some(s) = lib(v).and_then(|l| l.system(&sys)) else { return };
        let n = s.games.len();
        let reduce = self.prefs.reduce_motion;
        let Some(p) = self.menu.as_mut().and_then(|m| m.gpage.as_mut()) else { return };
        let cur = p.sel.min(n.saturating_sub(1));
        let new = match b {
            Button::Up => cur.saturating_sub(1),
            Button::Down => (cur + 1).min(n.saturating_sub(1)),
            Button::L1 => cur.saturating_sub(PER_PAGE),
            Button::R1 => (cur + PER_PAGE).min(n.saturating_sub(1)),
            Button::B | Button::Left if !repeat => {
                self.close_gpage();
                return;
            }
            Button::A | Button::Start if !repeat => {
                let Some(g) = s.games.get(cur) else { return };
                let Some(gr) = lib(v).and_then(|l| l.game_ref(&sys, &g.path)) else { return };
                let top = (cur / PER_PAGE * PER_PAGE) as f32;
                let y = PAGE_TOP + (cur as f32 - top) * PROW_H;
                self.launch_game(gr, R4 { x: 40.0, y, w: LIST_W, h: PROW_H }, v);
                return;
            }
            _ => return,
        };
        if new == cur {
            self.fx(Sfx::Deny, None);
            return;
        }
        p.sel = new;
        let top = (new / PER_PAGE * PER_PAGE) as f32 * PROW_H;
        p.scroll.set(top);
        if reduce {
            p.scroll.snap(top);
        }
        let paged = matches!(b, Button::L1 | Button::R1);
        self.fx(if paged { Sfx::Move } else { Sfx::Tick }, Some(Haptic::Tick));
    }

    fn gpanel_button(&mut self, b: Button, repeat: bool, v: &View) {
        let Some(uid) = self.menu.as_ref().and_then(|m| m.gpanel.as_ref()).map(|p| p.uid) else { return };
        let items = panel_items(uid, v);
        let st = v.games.and_then(|g| g.status.get(&uid)).cloned();
        let Some(p) = self.menu.as_mut().and_then(|m| m.gpanel.as_mut()) else { return };
        p.sel = p.sel.min(items.len() - 1);
        let it = items[p.sel];
        match b {
            Button::Up | Button::Down => {
                let n = items.len();
                p.sel = if b == Button::Down { (p.sel + 1) % n } else { (p.sel + n - 1) % n };
                self.fx(Sfx::Tick, Some(Haptic::Tick));
            }
            Button::Left | Button::Right if it.slots() => {
                let n = SLOTS as u8;
                p.slot = if b == Button::Right { (p.slot + 1) % n } else { (p.slot + n - 1) % n };
                self.fx(Sfx::Tick, Some(Haptic::Tick));
            }
            Button::B | Button::Y if !repeat => self.close_gpanel(),
            Button::A if !repeat => {
                let slot = p.slot;
                self.panel_activate(uid, it, slot, st.as_ref(), v);
            }
            _ => {}
        }
    }

    fn panel_activate(&mut self, uid: u32, it: PItem, slot: u8, st: Option<&GameStatus>, v: &View) {
        match it {
            PItem::Save => {
                if st.is_some_and(|s| !s.can_save) {
                    self.show_toast(Toast::info("This game can't save states"));
                    self.fx(Sfx::Deny, None);
                    return;
                }
                self.out.push(Out::GameCtl(uid, GameCmd::Save(slot)));
                self.fx(Sfx::Tab, Some(Haptic::Double));
            }
            PItem::Load => {
                if st.is_some_and(|s| s.slots[slot as usize].is_none()) {
                    self.show_toast(Toast::info(format!("Slot {} is empty", slot + 1)));
                    self.fx(Sfx::Deny, None);
                    return;
                }
                self.out.push(Out::GameCtl(uid, GameCmd::Load(slot)));
                // straight into the game, where the state was saved
                self.switch_from_menu(uid, v);
            }
            PItem::Reset => {
                self.out.push(Out::GameCtl(uid, GameCmd::Reset));
                self.switch_from_menu(uid, v);
            }
            PItem::Ff => {
                self.out.push(Out::GameCtl(uid, GameCmd::FastForward(None)));
                self.fx(Sfx::Tab, Some(Haptic::Tick));
            }
            PItem::Disc => {
                self.out.push(Out::GameCtl(uid, GameCmd::NextDisc));
                self.fx(Sfx::Tab, Some(Haptic::Tick));
            }
            PItem::Quit | PItem::Close => {
                self.close_gpanel();
                self.close_from_menu(uid, v);
            }
            PItem::Wake => self.switch_from_menu(uid, v),
        }
    }

    pub(super) fn games_hints(&self, v: &View) -> Option<Vec<(&'static str, &'static str)>> {
        let m = self.menu.as_ref()?;
        if let Some(p) = m.gpanel.as_ref().filter(|p| !p.closing) {
            let items = panel_items(p.uid, v);
            let it = items[p.sel.min(items.len() - 1)];
            let mut h = vec![("▲▼", "Choose")];
            if it.slots() {
                h.push(("◀▶", "Slot"));
            }
            h.extend([("A", "Select"), ("B", "Back")]);
            return Some(h);
        }
        let p = m.gpage.as_ref().filter(|p| !p.closing)?;
        let s = lib(v)?.system(&p.sys)?;
        let running = s.games.get(p.sel).and_then(|g| lib(v)?.game_ref(&p.sys, &g.path)).is_some_and(|g| running_key(&g.key(), v));
        let mut h = vec![("▲▼", "Choose")];
        if s.games.len() > PER_PAGE {
            h.push(("L1 R1", "Page"));
        }
        h.extend([("A", if running { "Switch" } else { "Play" }), ("B", "Back")]);
        Some(h)
    }

    /// Files the screens want decoded (the compositor's game worker loads
    /// them): the selected game's resume picture, the panel's slots.
    pub fn wanted_pics(&self, v: &View) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let Some(m) = self.menu.as_ref().filter(|m| !m.closing) else { return out };
        if let Some(p) = &m.gpanel {
            if let Some(st) = v.games.and_then(|g| g.status.get(&p.uid)) {
                out.extend(st.slots.iter().flatten().filter_map(|s| s.thumb.clone()));
            }
            return out;
        }
        if let (Some(p), Some(l)) = (&m.gpage, lib(v)) {
            if let Some(s) = l.system(&p.sys) {
                // the selected game first, then its neighbours
                let sel = p.sel.min(s.games.len().saturating_sub(1));
                for i in [sel, sel + 1, sel.wrapping_sub(1)] {
                    if let Some(t) = s.games.get(i).and_then(|g| g.thumb.clone()) {
                        out.push(t);
                    }
                }
            }
            return out;
        }
        if m.on_cross() && m.cat == Cat::Games {
            if let Some(Row::Game(g)) = m.sel_key_now(self.cross_sel[Cat::Games.i()]) {
                out.extend(row_thumb(&g, v));
            }
            // the Continue picture is wanted first anyway
            out.extend(row_thumb(&GRow::Continue, v));
        }
        out.dedup();
        out
    }

    /// The MENU wants a fresh library (it shows, or a games page does).
    pub fn wants_games(&self) -> bool {
        self.menu.as_ref().is_some_and(|m| !m.closing)
    }

    /// The game panel's game (for `ctl state`).
    pub fn game_panel(&self) -> Option<u32> {
        self.menu.as_ref()?.gpanel.as_ref().filter(|p| !p.closing).map(|p| p.uid)
    }

    /// The system page that shows (for `ctl state`).
    pub fn game_page(&self) -> Option<String> {
        self.menu.as_ref()?.gpage.as_ref().filter(|p| !p.closing).map(|p| p.sys.clone())
    }

    // --------------------------------------------------------- drawing ----

    /// Beside the selected Continue / recent row: its resume picture.
    pub(super) fn draw_games_pic(&self, b: &mut B, pal: &Pal, v: &View, g: &GRow, gl: &Glass) {
        let Some(path) = row_thumb(g, v) else { return };
        let pic = v.games.and_then(|x| x.pic(Some(&path)));
        let (w, h) = (240.0, 180.0);
        let (x, y) = (W - 24.0 - w, super::cross::ROW_TOP + super::cross::SEL_H + 8.0);
        picture(b, pal, gl, pic, x, y, w, h, "");
    }

    pub(super) fn draw_gpage(&self, b: &mut B, pal: &Pal, v: &View, p: &GamePage, g: &Glass) {
        let Some(l) = lib(v) else { return };
        let Some(s) = l.system(&p.sys) else { return };
        b.push(Xf::translate(p.x.x * W, 0.0));
        let (y0, h) = (BAR_H, H - BAR_H - HINT_H);
        let accent = Cat::Games.accent(pal);
        glass(b, pal, g, 0.0, y0, W, h, 0.62);
        // header: serif title, a mono line, the icon block with its shadow
        let tst = ts(Face::SerifItalic, 64.0, pal.ink).line_height(1.0);
        let top = y0 + 22.0;
        let tw = b.text(40.0, top, &s.name, &tst);
        let mono = ts(Face::Mono400, 18.0, pal.soft);
        let pages = s.games.len().div_ceil(PER_PAGE).max(1);
        let page = p.sel / PER_PAGE + 1;
        let mut line = if s.count == 1 { "1 game".to_string() } else { format!("{} games", s.count) };
        if pages > 1 {
            line = format!("{line} · page {page} of {pages}");
        }
        let line = b.fit(&line, &mono, W - 40.0 - 64.0 - 24.0 - (40.0 + tw + 18.0));
        b.text_base(40.0 + tw + 18.0, top + 64.0 * 0.82, &line, &mono);
        let (bx, by) = (W - 40.0 - 64.0 - 8.0, top);
        b.rect(bx + 8.0, by + 8.0, 64.0, 64.0, pal.dark);
        b.rect(bx, by, 64.0, 64.0, accent);
        b.frame(bx, by, 64.0, 64.0, 2.0, pal.dark);
        draw_icon(b, Ic::Games, bx + 32.0, by + 32.0, 40.0, pal, pal.dark);
        // the list, a page at a time
        let sel = p.sel.min(s.games.len().saturating_sub(1));
        let bottom = H - HINT_H;
        b.clip(Some([0.0, PAGE_TOP, W, bottom - PAGE_TOP]));
        for (i, gm) in s.games.iter().enumerate() {
            let y = PAGE_TOP + i as f32 * PROW_H - p.scroll.x;
            if y + PROW_H < PAGE_TOP || y > bottom {
                continue;
            }
            let on = i == sel;
            if on {
                b.rect(0.0, y, 40.0 + LIST_W, PROW_H, accent);
                b.rect(40.0 + LIST_W, y, 2.0, PROW_H, pal.dark);
            }
            let ink = if on { pal.dark } else { pal.ink };
            let mut right = 40.0 + LIST_W - 16.0;
            if gm.resume {
                // the resume marker: a small block tag
                let st = ts(Face::Mono600, 14.0, pal.dark).tracking(0.06);
                let tw = b.measure("RESUME", &st) + 14.0;
                let (tx, ty, th) = (right - tw, y + PROW_H / 2.0 - 13.0, 26.0);
                b.rect(tx, ty, tw, th, if on { DAY.ground } else { accent });
                b.frame(tx, ty, tw, th, 1.5, pal.dark);
                b.text(tx + 7.0, ty + th / 2.0 - st.line_px() / 2.0, "RESUME", &st);
                right = tx - 12.0;
            }
            if let Some(r) = &gm.region {
                let st = ts(Face::Mono400, 16.0, if on { pal.dark } else { pal.soft });
                right -= b.text_right(right, y + PROW_H / 2.0 - st.line_px() / 2.0, r, &st) + 12.0;
            }
            let nst = if on { ts(Face::SerifItalic, 38.0, ink).line_height(1.0) } else { ts(Face::Body600, 26.0, ink).line_height(1.1) };
            let name = b.fit(&gm.name, &nst, right - 40.0);
            b.text(40.0, y + PROW_H / 2.0 - nst.line_px() / 2.0, &name, &nst);
        }
        b.clip(None);
        // the selected game's picture and what A does
        if let Some(gm) = s.games.get(sel) {
            let (x, y, w, hh) = (40.0 + LIST_W + 32.0, PAGE_TOP + 8.0, W - (40.0 + LIST_W + 32.0) - 40.0, 0.0);
            let ph = w * 0.75;
            let pic = v.games.and_then(|x| x.pic(gm.thumb.as_deref()));
            picture(b, pal, g, pic, x, y, w, ph, &s.tag);
            let _ = hh;
            let st = ts(Face::Mono400, 17.0, pal.soft);
            let running = l.game_ref(&p.sys, &gm.path).is_some_and(|gr| running_key(&gr.key(), v));
            let what = if running {
                "Running · A switches to it"
            } else if gm.resume {
                "Resumes where you left off"
            } else {
                "Starts from the beginning"
            };
            let ty = y + ph + 22.0;
            let t = b.fit(what, &st, w);
            b.text(x, ty, &t, &st);
            let fname = gm.path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            let f = b.fit(&fname, &st, w);
            b.text(x, ty + st.line_px() + 4.0, &f, &st);
        }
        b.pop();
    }

    pub(super) fn draw_gpanel(&self, b: &mut B, pal: &Pal, v: &View, p: &GamePanel, g: &Glass) {
        let a = p.op.x.clamp(0.0, 1.0);
        if a < 0.01 {
            return;
        }
        let Some(app) = v.app(p.uid) else { return };
        let items = panel_items(p.uid, v);
        let sel = p.sel.min(items.len() - 1);
        let st = v.games.and_then(|x| x.status.get(&p.uid));
        let accent = Cat::Games.accent(pal);
        b.push_alpha(a);
        b.push(Xf::translate(0.0, (1.0 - p.op.x) * 24.0));
        b.rect(PX + 10.0, PY + 10.0, PW, PH, pal.dark);
        glass(b, pal, g, PX, PY, PW, PH, 0.7);
        b.frame(PX, PY, PW, PH, 2.0, pal.dark);
        // header
        let tst = ts(Face::SerifItalic, 48.0, pal.ink).line_height(1.0);
        let name = b.fit(&app.name, &tst, PW - 48.0 - 80.0);
        b.text(PX + 24.0, PY + 20.0, &name, &tst);
        let mono = ts(Face::Mono400, 17.0, pal.soft);
        let status = if app.asleep {
            "Asleep · saved where you left it".to_string()
        } else {
            match st {
                Some(s) if s.fast_forward => "Game menu · fast-forward on".to_string(),
                Some(_) => "Game menu".to_string(),
                None => "Game menu · reading saves…".to_string(),
            }
        };
        b.text(PX + 24.0, PY + 20.0 + tst.line_px() + 6.0, &status, &mono);
        let (bx, by) = (PX + PW - 24.0 - 56.0, PY + 20.0);
        b.rect(bx + 6.0, by + 6.0, 56.0, 56.0, pal.dark);
        b.rect(bx, by, 56.0, 56.0, accent);
        b.frame(bx, by, 56.0, 56.0, 2.0, pal.dark);
        draw_icon(b, Ic::Games, bx + 28.0, by + 28.0, 36.0, pal, pal.dark);
        // rows on the left
        let (lx, ly, lw) = (PX, PY + 112.0, 420.0);
        for (i, it) in items.iter().enumerate() {
            let y = ly + i as f32 * PROW;
            let on = i == sel;
            if on {
                b.rect(lx, y, lw, PROW, if *it == PItem::Quit || *it == PItem::Close { pal.orange } else { accent });
                b.rect(lx, y, lw, 2.0, pal.dark);
                b.rect(lx, y + PROW - 2.0, lw, 2.0, pal.dark);
                b.rect(lx + lw - 2.0, y, 2.0, PROW, pal.dark);
            }
            let ink = if on { pal.dark } else { pal.ink };
            let tst = if on { ts(Face::SerifItalic, 36.0, ink).line_height(1.0) } else { ts(Face::Body600, 26.0, ink).line_height(1.1) };
            b.text(lx + 24.0, y + PROW / 2.0 - tst.line_px() / 2.0, it.name(), &tst);
            let vst = ts(Face::Mono400, 17.0, if on { pal.dark } else { pal.soft });
            let val = match it {
                PItem::Save | PItem::Load => format!("◀ Slot {} ▶", p.slot + 1),
                PItem::Ff => (if st.is_some_and(|s| s.fast_forward) { "On" } else { "Off" }).to_string(),
                PItem::Disc => st.and_then(|s| s.discs).map(|(i, n)| format!("Disc {i} of {n}")).unwrap_or_default(),
                _ => String::new(),
            };
            if !val.is_empty() {
                let vw = arrow_text(b, 0.0, 0.0, &val, &vst, false);
                arrow_text(b, lx + lw - 20.0 - vw, y + PROW / 2.0 - vst.line_px() / 2.0, &val, &vst, true);
            }
        }
        // the four slots on the right: picture, number, age
        if !app.asleep {
            let on_slots = items[sel].slots();
            let (gx, gy) = (PX + 448.0, PY + 112.0);
            let (cw, ch) = (196.0, 147.0);
            for i in 0..SLOTS {
                let (cx, cy) = (gx + (i % 2) as f32 * (cw + 24.0), gy + (i / 2) as f32 * (ch + 52.0));
                let slot = st.and_then(|s| s.slots[i].as_ref());
                let chosen = i == p.slot as usize;
                b.push_alpha(if on_slots || chosen { 1.0 } else { 0.55 });
                let pic = slot.and_then(|s| v.games.and_then(|x| x.pic(s.thumb.as_deref())));
                if chosen && on_slots {
                    b.rect(cx + 8.0, cy + 8.0, cw, ch, pal.dark);
                }
                match slot {
                    Some(_) => picture_plain(b, pal, pic, cx, cy, cw, ch),
                    None => {
                        b.rect(cx, cy, cw, ch, with_a(pal.ground_2, 0.8));
                        let es = ts(Face::Mono400, 16.0, pal.soft);
                        b.text_center(cx + cw / 2.0, cy + ch / 2.0 - es.line_px() / 2.0, "Empty", &es);
                    }
                }
                b.frame(cx, cy, cw, ch, if chosen && on_slots { 3.0 } else { 1.5 }, pal.dark);
                let cap = ts(Face::Mono600, 16.0, pal.ink);
                let age = ts(Face::Mono400, 15.0, pal.soft);
                let ty = cy + ch + 8.0;
                let w1 = b.text(cx, ty, &format!("SLOT {}", i + 1), &cap);
                let when = match (slot, st) {
                    (Some(s), _) => crate::games::ago(s.age_s),
                    (None, Some(_)) => "empty".to_string(),
                    (None, None) => String::new(),
                };
                b.text(cx + w1 + 10.0, ty + 1.0, &when, &age);
                b.pop_alpha();
            }
        } else {
            let pic = app.pic.map(|id| Pic { id, w: W as u32, h: H as u32 });
            picture(b, pal, g, pic, PX + 448.0, PY + 112.0, 416.0, 312.0, "");
        }
        b.pop();
        b.pop_alpha();
    }
}

/// A picture in an ink frame with a hard shadow; a flat block with `label`
/// while it loads or when there is none.
#[allow(clippy::too_many_arguments)]
fn picture(b: &mut B, pal: &Pal, g: &Glass, pic: Option<Pic>, x: f32, y: f32, w: f32, h: f32, label: &str) {
    b.rect(x + 8.0, y + 8.0, w, h, pal.dark);
    glass(b, pal, g, x, y, w, h, 0.5);
    picture_plain(b, pal, pic, x, y, w, h);
    if pic.is_none() && !label.is_empty() {
        let st = ts(Face::SerifItalic, 48.0, pal.soft).line_height(1.0);
        b.text_center(x + w / 2.0, y + h / 2.0 - st.line_px() / 2.0, label, &st);
    }
    b.frame(x, y, w, h, 2.0, pal.dark);
}

/// The picture fitted into the box, black around it (as the game shows it).
fn picture_plain(b: &mut B, _pal: &Pal, pic: Option<Pic>, x: f32, y: f32, w: f32, h: f32) {
    let Some(p) = pic else { return };
    b.rect(x, y, w, h, Color(0, 0, 0, 255));
    let r = p.fit(x, y, w, h);
    b.image(Src::Pic(p.id), r[0], r[1], r[2], r[3], [0.0, 0.0, 1.0, 1.0]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::games::{GamesView, Library, SlotInfo};
    use mortar_ui::status::Sys;

    const LIST: &str = r#"{"systems":[
      {"id":"gbc","name":"Game Boy Color","tag":"GBC","order":11,"count":10,"games":[
        {"path":"/r/a.gbc","name":"A"},{"path":"/r/b.gbc","name":"B"},{"path":"/r/c.gbc","name":"C"},{"path":"/r/d.gbc","name":"D"},
        {"path":"/r/e.gbc","name":"E"},{"path":"/r/f.gbc","name":"F"},{"path":"/r/g.gbc","name":"G"},{"path":"/r/h.gbc","name":"H"},
        {"path":"/r/i.gbc","name":"I","resume":true,"thumb":"/s/i.png"},{"path":"/r/j.gbc","name":"J"}]},
      {"id":"nes","name":"NES","tag":"NES","order":20,"count":0,"games":[]},
      {"id":"2048","name":"2048","tag":"2048","order":90,"noContent":true}],
     "recent":[{"system":"gbc","path":"/r/i.gbc","name":"I","tag":"GBC","resume":true,"thumb":"/s/i.png"},
               {"system":"gbc","path":"/r/a.gbc","name":"A","tag":"GBC"}]}"#;

    fn games() -> GamesView {
        GamesView { lib: Library::parse(LIST).ok(), library_app: Some("play".into()), ..Default::default() }
    }
    fn launcher() -> Vec<LauncherInfo> {
        [("pico8", "PICO-8"), ("play", "Games"), ("puzzle", "PuzzleScript")]
            .iter()
            .map(|(id, n)| LauncherInfo { id: id.to_string(), name: n.to_string(), description: String::new(), art: Art::Games, running: false, category: Category::Games, single_instance: true })
            .collect()
    }
    fn game(uid: u32, key: &str, asleep: bool) -> RunningInfo {
        RunningInfo { uid, entry: Some(key.into()), name: "I · GBC".into(), art: Art::Games, has_buffer: !asleep, closing: false, geom: [0.0, 0.0, W, H], game: true, asleep, pic: asleep.then_some(5) }
    }
    fn view<'a>(running: &'a [RunningInfo], l: &'a [LauncherInfo], sys: &'a Sys, g: &'a GamesView, current: Option<u32>) -> View<'a> {
        View {
            running,
            launcher: l,
            sys,
            volume: Some(50),
            current,
            keyboard_app: false,
            osk_h: 0.0,
            osk_drop: 0.0,
            host: "brick",
            timezone: "UTC",
            radios: Radios::default(),
            update: None,
            muted: false,
            net: None,
            mixer: &[],
            games: Some(g),
        }
    }
    fn acts(o: Vec<Out>) -> Vec<Out> {
        o.into_iter().filter(|o| !matches!(o, Out::Sfx(_) | Out::Haptic(_))).collect()
    }
    fn sel(ui: &ShellUi) -> Option<Row> {
        let m = ui.menu.as_ref()?;
        m.sel_key(ui.cross_sel[m.cat.i()])
    }
    fn to_games(ui: &mut ShellUi, v: &View) {
        ui.menu_key(v);
        ui.button(Button::Right, false, v);
        ui.button(Button::Right, false, v);
        assert_eq!(ui.menu_category(), Some(Cat::Games));
    }

    #[test]
    fn games_category_rows() {
        let (sys, g, l) = (Sys::demo(None), games(), launcher());
        let v = view(&[], &l, &sys, &g, None);
        let rows = games_rows(&v);
        assert_eq!(
            rows,
            vec![
                Row::Game(GRow::Continue),
                Row::Game(GRow::Recent("/r/a.gbc".into())),
                Row::Game(GRow::System("gbc".into())),
                Row::Game(GRow::System("2048".into())),
                Row::Launch("pico8".into()),
                Row::Launch("puzzle".into()),
                Row::Launch("play".into()),
            ],
            "Continue, recents, systems with games, the other entries, All systems last"
        );
        assert_eq!(launch_label("play", &v).unwrap().0, "All systems");
        let info = game_row_info(&GRow::System("gbc".into()), &v);
        assert_eq!((info.title.as_str(), info.value.as_deref()), ("Game Boy Color", Some("10 games")));
        assert!(game_row_info(&GRow::Continue, &v).sub.starts_with("Continue · Game Boy Color"));
        // with the cross on Continue its resume picture is wanted
        let mut ui = ShellUi::new(Prefs::default(), false);
        to_games(&mut ui, &v);
        assert_eq!(sel(&ui), Some(Row::Game(GRow::Continue)));
        assert_eq!(ui.wanted_pics(&v), vec![PathBuf::from("/s/i.png")]);
    }

    #[test]
    fn a_launches_and_again_switches() {
        let (sys, g, l) = (Sys::demo(None), games(), launcher());
        let v = view(&[], &l, &sys, &g, None);
        let mut ui = ShellUi::new(Prefs::default(), false);
        to_games(&mut ui, &v);
        let out = acts(ui.button(Button::A, false, &v));
        let Some(Out::LaunchGame(gr)) = out.first() else { panic!("{out:?}") };
        assert_eq!((gr.title(), gr.key()), ("I · GBC".to_string(), "game:gbc:/r/i.gbc".to_string()));
        assert!(matches!(&ui.trans, Some(Trans::Launch { target: Target::Entry(k), .. }) if k == "game:gbc:/r/i.gbc"));
        // it runs: Continue now switches to it
        let r = [game(7, "game:gbc:/r/i.gbc", false)];
        let v2 = view(&r, &l, &sys, &g, None);
        ui.trans = None;
        let mut ui = ShellUi::new(Prefs::default(), false);
        to_games(&mut ui, &v2);
        assert_eq!(acts(ui.button(Button::A, false, &v2)), vec![Out::Focus(7)]);
        // 2048 needs no game: its row starts it
        let mut ui = ShellUi::new(Prefs::default(), false);
        to_games(&mut ui, &v);
        for _ in 0..3 {
            ui.button(Button::Down, false, &v);
        }
        let out = acts(ui.button(Button::A, false, &v));
        assert!(matches!(out.first(), Some(Out::LaunchGame(g)) if g.path.is_none() && g.system == "2048"));
    }

    #[test]
    fn system_page_pages_and_plays() {
        let (sys, g, l) = (Sys::demo(None), games(), launcher());
        let v = view(&[], &l, &sys, &g, None);
        let mut ui = ShellUi::new(Prefs::default(), false);
        to_games(&mut ui, &v);
        ui.button(Button::Down, false, &v);
        ui.button(Button::Down, false, &v);
        assert!(acts(ui.button(Button::A, false, &v)).is_empty());
        assert_eq!(ui.game_page().as_deref(), Some("gbc"));
        // R1: a page down (8 games a page), to the resumable one
        ui.button(Button::R1, false, &v);
        assert_eq!(ui.menu.as_ref().unwrap().gpage.as_ref().unwrap().sel, 8);
        assert_eq!(ui.wanted_pics(&v).first(), Some(&PathBuf::from("/s/i.png")));
        ui.button(Button::R1, false, &v);
        assert_eq!(ui.menu.as_ref().unwrap().gpage.as_ref().unwrap().sel, 9);
        ui.button(Button::L1, false, &v);
        assert_eq!(ui.menu.as_ref().unwrap().gpage.as_ref().unwrap().sel, 1);
        let out = acts(ui.button(Button::A, false, &v));
        assert!(matches!(out.first(), Some(Out::LaunchGame(g)) if g.key() == "game:gbc:/r/b.gbc"));
        // B goes back to the cross
        let mut ui = ShellUi::new(Prefs::default(), false);
        to_games(&mut ui, &v);
        ui.button(Button::Down, false, &v);
        ui.button(Button::Down, false, &v);
        ui.button(Button::A, false, &v);
        ui.button(Button::B, false, &v);
        assert!(ui.game_page().is_none());
        assert!(ui.menu_open());
    }

    #[test]
    fn game_panel() {
        let (sys, mut g, l) = (Sys::demo(None), games(), launcher());
        let r = [game(7, "game:gbc:/r/i.gbc", false), game(8, "game:gbc:/r/a.gbc", true)];
        let v = view(&r, &l, &sys, &g, Some(7));
        let mut ui = ShellUi::new(Prefs::default(), false);
        ui.menu_key(&v);
        // the previous app (the sleeping game) is selected; up: the one in front
        ui.button(Button::Up, false, &v);
        assert_eq!(sel(&ui), Some(Row::App(7)));
        assert!(ui.menu_hints(ui.menu.as_ref().unwrap(), &v).contains(&("Y", "Game menu")));
        assert_eq!(acts(ui.button(Button::Y, false, &v)), vec![Out::GameCtl(7, GameCmd::ListStates)]);
        assert_eq!(ui.game_panel(), Some(7));
        // ◀▶ pick the slot; A saves to it
        ui.button(Button::Right, false, &v);
        assert_eq!(acts(ui.button(Button::A, false, &v)), vec![Out::GameCtl(7, GameCmd::Save(1))]);
        // load: an empty slot is refused, a saved one loads and switches
        let mut st = GameStatus { can_save: true, ..Default::default() };
        st.slots[1] = Some(SlotInfo { age_s: 3, thumb: Some("/s/1.png".into()) });
        g.status.insert(7, st);
        let v = view(&r, &l, &sys, &g, Some(7));
        assert_eq!(ui.wanted_pics(&v), vec![PathBuf::from("/s/1.png")]);
        ui.button(Button::Down, false, &v);
        ui.button(Button::Left, false, &v);
        assert!(acts(ui.button(Button::A, false, &v)).is_empty(), "slot 1 is empty");
        ui.button(Button::Right, false, &v);
        assert_eq!(acts(ui.button(Button::A, false, &v)), vec![Out::GameCtl(7, GameCmd::Load(1))]);
        assert!(!ui.menu_open());
        // no Change disc for a one-disc game; Quit closes it
        assert_eq!(panel_items(7, &v), vec![PItem::Save, PItem::Load, PItem::Reset, PItem::Ff, PItem::Quit]);
        g.status.get_mut(&7).unwrap().discs = Some((1, 2));
        let v = view(&r, &l, &sys, &g, Some(7));
        assert!(panel_items(7, &v).contains(&PItem::Disc));
        // a sleeping game: Wake and Close; A on it in Running wakes it
        assert_eq!(panel_items(8, &v), vec![PItem::Wake, PItem::Close]);
        let mut ui = ShellUi::new(Prefs::default(), false);
        ui.menu_key(&v);
        assert_eq!(sel(&ui), Some(Row::App(8)));
        assert!(ui.menu_hints(ui.menu.as_ref().unwrap(), &v).contains(&("A", "Wake")));
        assert_eq!(acts(ui.button(Button::Y, false, &v)), vec![], "nothing to ask a sleeping game");
        ui.button(Button::Down, false, &v);
        assert_eq!(acts(ui.button(Button::A, false, &v)), vec![Out::Close(8)]);
    }
}
