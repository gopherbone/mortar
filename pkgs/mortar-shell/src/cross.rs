// SPDX-License-Identifier: GPL-2.0-or-later
//! The MENU, "Cross Menu 2" (the approved plan/mortar-cross-2.html).
//!
//! Two pages side by side, on one spring:
//!
//! - Running, the stack: the dashboard (home.rs; selected: full screen, the
//!   apps a strip at the bottom; scrolled past: a card), then every running
//!   app, most recent first, each a live window edge to edge (1024 x 528,
//!   the app's middle rows), stacked vertically like a scrolling window
//!   manager; the neighbours peek 36 px above and below as tabs (name and
//!   state on a plain card: only the selected card is live). The selected
//!   app has an accent strip and a label. MENU opens here with the
//!   previous app selected (MENU, A flips back). Home is this stack on the
//!   dashboard with no app in front: it is always there then, and B and
//!   MENU come back to the dashboard instead of closing.
//! - The cross: Apps, Games, Music and Settings across the middle, the
//!   selected category lifted in its accent; the selected item right under
//!   it as a bold band from the left edge, the items below continuing down
//!   and the one above dimmed above the category row. A quiet animated
//!   pattern per category runs in a strip at the right edge (patterns.rs).
//!
//! ▶ slides Running off to the left and the cross in; ◀ from Apps (or L1)
//! goes back. The clock and hint bars float over both as frosted glass.
//! Settings is the one place for settings: Brightness and Volume as quick
//! rows, then glass pages (menupages.rs) for Display, Sound, Wi-Fi,
//! Bluetooth, Time zone, Remote access, Updates, About and Power.
//!
//! B goes back one level (adjusting, the options popup, a page) and closes
//! the MENU at the top (Running or the cross); MENU closes everything. At
//! home "closing" lands on the dashboard.
//!
//! A child module of `shellui`: it works on the `ShellUi` state directly.

use super::gamepages::{GamePage, GamePanel, GRow};
use super::menupages::{Sub, SubKind};
use super::*;
use crate::config::Category;
use crate::patterns::{self, Patterns};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cat {
    Running,
    Apps,
    Games,
    Music,
    Settings,
}

/// The cross's categories (Running is its own page, left of them).
pub const CROSS_CATS: [Cat; 4] = [Cat::Apps, Cat::Games, Cat::Music, Cat::Settings];

impl Cat {
    pub fn name(self) -> &'static str {
        match self {
            Cat::Running => "Running",
            Cat::Apps => "Apps",
            Cat::Games => "Games",
            Cat::Music => "Music",
            Cat::Settings => "Settings",
        }
    }
    pub(super) fn i(self) -> usize {
        self as usize
    }
    fn icon(self) -> Ic {
        match self {
            Cat::Running => Ic::Running,
            Cat::Apps => Ic::Apps,
            Cat::Games => Ic::Games,
            Cat::Music => Ic::Music,
            Cat::Settings => Ic::Quick,
        }
    }
    /// The category's accent (the band, its icon, its pattern).
    pub(super) fn accent(self, pal: &Pal) -> Color {
        match self {
            Cat::Running => pal.lavender,
            Cat::Apps => pal.yellow,
            Cat::Games => pal.lime,
            Cat::Music => pal.orange,
            Cat::Settings => pal.pink,
        }
    }
    fn pattern(self) -> patterns::Kind {
        match self {
            Cat::Games => patterns::Kind::Life,
            Cat::Music => patterns::Kind::Scope,
            Cat::Settings => patterns::Kind::Truchet,
            _ => patterns::Kind::Zigzag,
        }
    }
}

/// One item: a window on the Running page or a row of the cross.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Row {
    Home,
    App(u32),
    /// A launcher entry (by id).
    Launch(String),
    /// A game, a recently played one, a system (gamepages.rs).
    Game(GRow),
    Brightness,
    Volume,
    /// Music: the Mixer page.
    Mixer,
    Keyboard,
    /// Settings: a glass page.
    Page(SubKind),
    /// Settings > Remote access (mortar-setup's page).
    Remote,
}

impl Row {
    /// A level: A enters an adjust mode where ◀▶ change it.
    fn level(&self) -> bool {
        matches!(self, Row::Brightness | Row::Volume)
    }
    /// Opens a glass page.
    fn page(&self) -> Option<SubKind> {
        match self {
            Row::Mixer => Some(SubKind::Mixer),
            Row::Page(k) => Some(*k),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CtxItem {
    Switch,
    Restart,
    Close,
}

impl CtxItem {
    fn name(self) -> &'static str {
        match self {
            CtxItem::Switch => "Switch to it",
            CtxItem::Restart => "Restart app",
            CtxItem::Close => "Close",
        }
    }
}

/// Y on a running app: Switch, Restart, Close.
pub(super) struct Ctx {
    uid: u32,
    sel: usize,
    items: Vec<CtxItem>,
    op: Spring,
}

struct RowAnim {
    key: Row,
    y: Spring,
    op: Spring,
    /// The selection band's reach (0..1).
    bw: Spring,
}

pub(super) struct Cross {
    /// Open (1) or closed (0): the frost and the menu fade with it.
    op: Spring,
    /// 0: the Running page; 1: the cross.
    pub(super) page_x: Spring,
    on_cross: bool,
    // Running
    run_keys: Vec<Row>,
    pub(super) run_sel: usize,
    pub(super) run_scroll: Spring,
    pub(super) ctx: Option<Ctx>,
    /// Apps closed with X, hidden until they are gone.
    hidden: Vec<u32>,
    // the cross
    pub cat: Cat,
    cat_x: Spring,
    lift: [Spring; 4],
    rows: Vec<RowAnim>,
    /// A level row is being adjusted (◀▶ change it instead of category).
    pub(super) adjusting: bool,
    /// The glass pages, bottom to top (closing ones slide out on top).
    pub(super) subs: Vec<Sub>,
    /// A system's games (MENU > Games > a system).
    pub(super) gpage: Option<GamePage>,
    /// The game panel (Y on a game in Running).
    pub(super) gpanel: Option<GamePanel>,
    /// No app in front: this is home (B and MENU come back to the
    /// dashboard instead of closing).
    pub(super) home: bool,
    pub(super) patterns: Patterns,
    /// The pattern a category change leaves (it slides out as the new one
    /// slides in, `pat_x` 0 -> 1, `pat_dir` the way the categories move).
    pat_from: Option<Cat>,
    pat_x: Spring,
    pat_dir: f32,
    pub closing: bool,
}

// layout: the mockup's 1024x768 stage, scaled up about 1.4x for the
// Brick's 3.2" panel (about 400 ppi, held at arm's length)
/// The stack: the selected card between the bars, its neighbours showing
/// PEEK px above and below it (as tabs once they rest there).
pub(super) const RUN_H: f32 = 528.0;
const RUN_GAP: f32 = 12.0;
pub(super) const RUN_Y: f32 = BAR_H + (H - BAR_H - HINT_H - RUN_H) / 2.0;
const RUN_STEP: f32 = RUN_H + RUN_GAP;
const PEEK: f32 = RUN_Y - RUN_GAP - BAR_H;
/// The selected category's column, and the step between categories.
const COLX: f32 = 100.0;
const CATW: f32 = 180.0;
/// Category icons: ICON square at (x + 16, CATS_TOP + 10), names below.
const CATS_TOP: f32 = 164.0;
const ICON: f32 = 116.0;
/// The selected row (taller, a serif title) right under the categories,
/// the others below it; one dimmed row above the categories.
pub(super) const ROW_TOP: f32 = 344.0;
pub(super) const SEL_H: f32 = 96.0;
const ROWH: f32 = 80.0;
/// Unselected rows' icon tiles (the selected row's are TILE).
const TILE_S: f32 = 60.0;
/// A row: the index, the icon tile, the text, the value column ending at
/// ITEM_W (all from LISTX).
const LISTX: f32 = 68.0;
const IX_W: f32 = 48.0;
const TILE: f32 = 72.0;
const ITEM_W: f32 = 620.0;
pub(super) const BAR_H: f32 = 72.0;
pub(super) const HINT_H: f32 = 72.0;
const CTX_X: f32 = 560.0;
const CTX_Y: f32 = 290.0;

fn slot_y(j: usize, s: usize) -> f32 {
    let d = j as f32 - s as f32;
    if d == 0.0 {
        ROW_TOP
    } else if d > 0.0 {
        ROW_TOP + SEL_H + 8.0 + (d - 1.0) * ROWH
    } else {
        // under the status bar, clear of the lifted icon and its shadow
        BAR_H + 4.0 + (d + 1.0) * ROWH
    }
}

/// The item above the selection stays, dimmed, above the categories;
/// three show below.
fn row_op(j: usize, s: usize) -> f32 {
    if j < s {
        if s - j <= 1 { 0.62 } else { 0.0 }
    } else if j > s + 3 {
        0.0
    } else {
        1.0
    }
}

/// The cross's categories (Games only when something is in it; Music
/// always has the Mixer).
pub(super) fn cats(v: &View) -> Vec<Cat> {
    CROSS_CATS
        .into_iter()
        .filter(|c| *c != Cat::Games || v.launcher.iter().any(|l| l.category == Category::Games) || super::gamepages::has_games(v))
        .collect()
}

pub(super) fn running_rows(v: &View, hidden: &[u32]) -> Vec<Row> {
    std::iter::once(Row::Home).chain(v.running.iter().filter(|r| !hidden.contains(&r.uid)).map(|r| Row::App(r.uid))).collect()
}

pub(super) fn rows(cat: Cat, v: &View) -> Vec<Row> {
    let launch = |f: &dyn Fn(&LauncherInfo) -> bool| v.launcher.iter().filter(|l| f(l)).map(|l| Row::Launch(l.id.clone())).collect::<Vec<_>>();
    match cat {
        Cat::Running => running_rows(v, &[]),
        // tools only: games and music apps live in their own categories
        Cat::Apps => launch(&|l| l.category == Category::Apps),
        Cat::Games => super::gamepages::games_rows(v),
        Cat::Music => std::iter::once(Row::Mixer).chain(launch(&|l| l.category == Category::Music)).collect(),
        Cat::Settings => settings_rows(v),
    }
}

/// Settings: the quick levels, then a page per group.
pub(super) fn settings_rows(v: &View) -> Vec<Row> {
    let mut r = vec![Row::Brightness, Row::Volume];
    if v.keyboard_app {
        r.push(Row::Keyboard);
    }
    r.extend([Row::Page(SubKind::Display), Row::Page(SubKind::Sound), Row::Page(SubKind::Wifi), Row::Page(SubKind::Bluetooth), Row::Page(SubKind::TimeZone), Row::Remote]);
    if v.update.is_some() {
        r.push(Row::Page(SubKind::Updates));
    }
    r.extend([Row::Page(SubKind::About), Row::Page(SubKind::Power)]);
    r
}

impl Cross {
    fn sel_row(&self, sel: usize) -> Option<&Row> {
        self.rows.get(sel).map(|r| &r.key)
    }

    /// The cross's row at `sel`.
    pub(super) fn sel_key_now(&self, sel: usize) -> Option<Row> {
        self.sel_row(sel).cloned()
    }

    #[cfg(test)]
    pub(super) fn sel_key(&self, sel: usize) -> Option<Row> {
        if self.on_cross { self.sel_row(sel).cloned() } else { self.run_keys.get(self.run_sel).cloned() }
    }

    /// Point every row's springs at its place for selection `s`.
    fn layout(&mut self, s: usize) {
        self.adjusting = false;
        for (j, r) in self.rows.iter_mut().enumerate() {
            r.y.set(slot_y(j, s));
            r.op.set(row_op(j, s));
            r.bw.set(if j == s { 1.0 } else { 0.0 });
        }
    }

    /// New rows: `enter` slides them in (a category change), otherwise
    /// rows that stay keep their motion and new ones appear in place.
    fn set_rows(&mut self, keys: Vec<Row>, s: usize, enter: bool) {
        let mut old = std::mem::take(&mut self.rows);
        self.rows = keys
            .into_iter()
            .enumerate()
            .map(|(j, key)| {
                if !enter && let Some(i) = old.iter().position(|r| r.key == key) {
                    return old.swap_remove(i);
                }
                let (y, o) = (slot_y(j, s), row_op(j, s));
                let mut bw = Spring::new(0.0);
                bw.k_mul = 1.3;
                if enter {
                    RowAnim { key, y: Spring::new(y + 40.0 + j as f32 * 10.0), op: Spring::new(0.0), bw }
                } else {
                    RowAnim { key, y: Spring::new(y), op: Spring::new(o), bw }
                }
            })
            .collect();
        self.layout(s);
    }

    fn springs_mut(&mut self) -> Vec<&mut Spring> {
        let mut v: Vec<&mut Spring> = vec![&mut self.op, &mut self.page_x, &mut self.run_scroll, &mut self.cat_x, &mut self.patterns.nudge, &mut self.pat_x];
        v.extend(self.lift.iter_mut());
        for r in &mut self.rows {
            v.extend([&mut r.y, &mut r.op, &mut r.bw]);
        }
        if let Some(c) = &mut self.ctx {
            v.push(&mut c.op);
        }
        for s in &mut self.subs {
            v.extend(s.springs());
        }
        if let Some(p) = &mut self.gpage {
            v.extend(p.springs());
        }
        if let Some(p) = &mut self.gpanel {
            v.extend(p.springs());
        }
        v
    }

    pub(super) fn springs(&mut self) -> Vec<&mut Spring> {
        self.springs_mut()
    }

    /// The pattern shows (the cross is at least partly on screen).
    pub(super) fn pattern_visible(&self) -> bool {
        !self.closing && self.page_x.x > 0.01
    }

    pub(super) fn moving(&self) -> bool {
        self.closing
            || ![&self.op, &self.page_x, &self.run_scroll, &self.cat_x].iter().all(|s| s.rest())
            || !self.lift.iter().all(|s| s.rest())
            || !self.rows.iter().all(|r| r.y.rest() && r.op.rest() && r.bw.rest())
            || self.ctx.as_ref().is_some_and(|c| !c.op.rest())
            || self.subs.iter().any(|s| s.moving())
            || self.gpage.as_ref().is_some_and(|p| p.moving())
            || self.gpanel.as_ref().is_some_and(|p| p.moving())
            || !(self.patterns.nudge.rest() && self.pat_x.rest())
    }

    /// How far open (0..1), for the backdrop.
    pub(super) fn open(&self) -> f32 {
        self.op.x.clamp(0.0, 1.0)
    }

    /// Fully faded out after closing.
    pub(super) fn gone(&self) -> bool {
        self.closing && self.op.x < 0.02
    }

    /// The screensaver may grow here: the cross alone, settled (not
    /// Running, a glass page or the options popup).
    pub(super) fn saver_ok(&self) -> bool {
        !self.closing && self.on_cross && self.top().is_none() && self.ctx.is_none() && self.gpage.is_none() && self.gpanel.is_none() && self.page_x.x > 0.99
    }

    pub(super) fn on_cross(&self) -> bool {
        self.on_cross
    }

    /// The page in front (not one sliding out).
    pub(super) fn top(&self) -> Option<&Sub> {
        self.subs.iter().rev().find(|s| !s.closing)
    }
}

impl ShellUi {
    fn sel_idx(&self, cat: Cat) -> usize {
        self.cross_sel[cat.i()]
    }

    /// MENU: open on Running with the previous app selected (the stack
    /// comes up on the app in front and scrolls on to it); with no app in
    /// front, on the dashboard.
    pub(super) fn open_menu(&mut self, v: &View) {
        let keys = running_rows(v, &[]);
        let (run_sel, from) = match v.current {
            Some(cur) => {
                let here = keys.iter().position(|r| *r == Row::App(cur));
                let pre = keys.iter().position(|r| matches!(r, Row::App(u) if *u != cur)).or(here).unwrap_or(0);
                (pre, here.unwrap_or(pre))
            }
            None => (0, 0),
        };
        self.build_menu(v, keys, run_sel, from, false);
        self.fx(Sfx::Drawer, Some(Haptic::Tick));
    }

    /// Home: the stack on the dashboard, with no app in front (boot, the
    /// last app gone, back from a start that went to the background).
    /// `snap`: already there (under the boot screen).
    pub(super) fn open_home(&mut self, v: &View, snap: bool) {
        self.build_menu(v, running_rows(v, &[]), 0, 0, snap);
        self.home.tiles = false;
        self.dirty = true;
    }

    fn build_menu(&mut self, v: &View, keys: Vec<Row>, run_sel: usize, from: usize, snap: bool) {
        let mut op = Spring::new(0.0);
        op.set(1.0);
        if snap || self.prefs.reduce_motion {
            op.snap(1.0);
        }
        let mut page_x = Spring::new(0.0);
        page_x.k_mul = 0.9;
        let mut run_scroll = Spring::new(from as f32);
        run_scroll.k_mul = 0.9;
        run_scroll.set(run_sel as f32);
        if snap || self.prefs.reduce_motion {
            run_scroll.snap(run_sel as f32);
        }
        let cat = cats(v).first().copied().unwrap_or(Cat::Apps);
        let mut m = Cross {
            op,
            page_x,
            on_cross: false,
            run_keys: keys,
            run_sel,
            run_scroll,
            ctx: None,
            hidden: Vec::new(),
            cat,
            cat_x: Spring::new(0.0),
            lift: std::array::from_fn(|_| Spring::new(0.0)),
            rows: Vec::new(),
            adjusting: false,
            subs: Vec::new(),
            gpage: None,
            gpanel: None,
            home: v.current.is_none(),
            patterns: Patterns::new(),
            pat_from: None,
            pat_x: Spring { k_mul: 0.7, ..Spring::new(1.0) },
            pat_dir: 1.0,
            closing: false,
        };
        let s = self.sel_idx(cat);
        m.set_rows(rows(cat, v), s, false);
        self.menu = Some(m);
        self.set_cat(cat, v, true);
    }

    /// B at the top, or MENU: close the MENU back to the app in front; with
    /// none (home), come back to the dashboard instead (everything opened
    /// from it slides away; on the dashboard already, nothing happens).
    pub(super) fn leave_menu(&mut self, v: &View) {
        if v.current.is_some() || self.trans.is_some() {
            self.close_menu();
        } else {
            self.back_home(v);
        }
    }

    /// Home: back to the dashboard card (pages, popups and the cross slide
    /// away; the stack scrolls up).
    pub(super) fn back_home(&mut self, v: &View) {
        let reduce = self.prefs.reduce_motion;
        let tiles = std::mem::take(&mut self.home.tiles);
        let Some(m) = &self.menu else { return };
        if m.closing {
            return;
        }
        let there = !tiles && !m.on_cross && m.run_sel == 0 && m.ctx.is_none() && m.top().is_none() && !self.games_take();
        if there {
            return;
        }
        let n0 = self.out.len();
        while self.top_sub().is_some() {
            self.close_sub(v);
        }
        if self.menu.as_ref().is_some_and(|m| m.gpanel.as_ref().is_some_and(|p| !p.closing)) {
            self.close_gpanel();
        }
        if self.menu.as_ref().is_some_and(|m| m.gpage.as_ref().is_some_and(|p| !p.closing)) {
            self.close_gpage();
        }
        self.go_running();
        let Some(m) = &mut self.menu else { return };
        m.ctx = None;
        m.run_sel = 0;
        m.run_scroll.set(0.0);
        if reduce {
            m.run_scroll.snap(0.0);
        }
        // one sound for all of it
        let rest: Vec<Out> = self.out.split_off(n0).into_iter().filter(|o| !matches!(o, Out::Sfx(_) | Out::Haptic(_))).collect();
        self.out.extend(rest);
        self.fx(Sfx::DrawerUp, Some(Haptic::Tick));
    }

    pub(super) fn close_menu(&mut self) {
        if let Some(m) = &mut self.menu {
            if m.closing {
                return;
            }
            m.closing = true;
            m.op.set(0.0);
            m.ctx = None;
            m.adjusting = false;
        }
        self.home.tiles = false;
        self.fx(Sfx::DrawerUp, Some(Haptic::Tick));
    }

    /// ▶ (or R1): the cross slides in.
    fn go_cross(&mut self, snap: bool) {
        let Some(m) = &mut self.menu else { return };
        m.on_cross = true;
        m.ctx = None;
        m.page_x.set(1.0);
        if snap || self.prefs.reduce_motion {
            m.page_x.snap(1.0);
        }
    }

    /// ◀ from the first category (or L1): back to Running.
    fn go_running(&mut self) {
        let Some(m) = &mut self.menu else { return };
        m.on_cross = false;
        m.adjusting = false;
        m.page_x.set(0.0);
        if self.prefs.reduce_motion {
            m.page_x.snap(0.0);
        }
    }

    fn set_cat(&mut self, cat: Cat, v: &View, snap: bool) {
        let s = self.sel_idx(cat);
        let reduce = self.prefs.reduce_motion;
        let Some(m) = &mut self.menu else { return };
        let list = cats(v);
        let idx = list.iter().position(|c| *c == cat).unwrap_or(0);
        let old = m.cat;
        if !snap && old != cat && !reduce {
            // the old pattern slides out the way the categories go, the
            // new one in after it
            let oi = list.iter().position(|c| *c == old).unwrap_or(0);
            m.pat_dir = if idx > oi { 1.0 } else { -1.0 };
            m.pat_from = Some(old);
            m.pat_x.snap(0.0);
            m.pat_x.set(1.0);
        }
        m.cat = cat;
        m.adjusting = false;
        let keys = rows(cat, v);
        let s = s.min(keys.len().saturating_sub(1));
        self.cross_sel[cat.i()] = s;
        m.set_rows(keys, s, !snap);
        m.cat_x.set(-(idx as f32) * CATW);
        for (i, c) in list.iter().enumerate() {
            if let Some(l) = m.lift.get_mut(i) {
                l.set(if *c == cat { 1.0 } else { 0.0 });
            }
        }
        if snap {
            m.cat_x.snap(m.cat_x.t);
            for l in &mut m.lift {
                l.snap(l.t);
            }
            for r in &mut m.rows {
                r.y.snap(r.y.t);
                r.op.snap(r.op.t);
                r.bw.snap(r.bw.t);
            }
            m.pat_from = None;
            m.pat_x.snap(1.0);
        }
    }

    /// Keep the pages in step with the apps (one closed, one started).
    pub(super) fn sync_menu(&mut self, v: &View) {
        let Some(m) = &mut self.menu else { return };
        if m.gone() {
            self.menu = None;
            self.dirty = true;
            return;
        }
        if m.closing {
            return;
        }
        // an app came to the front by itself (started over SSH, `ctl
        // focus`) while home rested on the dashboard: show it
        let home = v.current.is_none();
        if m.home && !home && self.trans.is_none() && !m.on_cross && m.run_sel == 0 && m.ctx.is_none() && m.top().is_none() && !self.home.tiles {
            self.close_menu();
            return;
        }
        m.home = home;
        m.hidden.retain(|u| v.app(*u).is_some());
        let keys = running_rows(v, &m.hidden);
        if keys != m.run_keys {
            let old = m.run_keys.get(m.run_sel).cloned();
            m.run_sel = old.and_then(|o| keys.iter().position(|k| *k == o)).unwrap_or(m.run_sel).min(keys.len().saturating_sub(1));
            m.run_keys = keys;
            m.run_scroll.set(m.run_sel as f32);
            self.dirty = true;
        }
        if !cats(v).contains(&m.cat) {
            m.cat = Cat::Apps;
        }
        let keys = rows(m.cat, v);
        if keys.len() != m.rows.len() || keys.iter().zip(&m.rows).any(|(k, r)| *k != r.key) {
            let ci = m.cat.i();
            let old = m.sel_row(self.cross_sel[ci]).cloned();
            // keep the selection on the same item when it is still there
            let s = old.and_then(|o| keys.iter().position(|k| *k == o)).unwrap_or(self.cross_sel[ci]).min(keys.len().saturating_sub(1));
            self.cross_sel[ci] = s;
            m.set_rows(keys, s, false);
            self.dirty = true;
        }
        if let Some(c) = &m.ctx
            && v.app(c.uid).is_none()
        {
            m.ctx = None;
        }
        self.sync_games(v);
    }

    /// A button with the MENU up; the pattern feels ▲▼.
    pub(super) fn menu_button(&mut self, b: Button, repeat: bool, v: &View) {
        self.sync_menu(v);
        let before = self.menu.as_ref().map(|m| (m.on_cross, m.cat, self.cross_sel[m.cat.i()], m.top().is_some(), m.ctx.is_some(), m.adjusting));
        self.menu_button_inner(b, repeat, v);
        let Some((cross0, cat0, s0, sub0, ctx0, adj0)) = before else { return };
        if self.prefs.reduce_motion {
            return;
        }
        let Some(m) = &mut self.menu else { return };
        if m.closing || sub0 || ctx0 || m.top().is_some() {
            return;
        }
        let s = self.cross_sel[m.cat.i()];
        let _ = (cross0, adj0);
        if m.on_cross && m.cat == cat0 && matches!(b, Button::Up | Button::Down) {
            let dir = if s != s0 { if s > s0 { 1.0 } else { -1.0 } } else if b == Button::Down { 0.5 } else { -0.5 };
            m.patterns.nudge(m.cat.pattern(), dir);
        }
    }

    fn menu_button_inner(&mut self, b: Button, repeat: bool, v: &View) {
        let games = self.games_take();
        let Some(m) = &mut self.menu else { return };
        if m.closing {
            return;
        }
        if m.ctx.is_some() {
            self.ctx_button(b, repeat, v);
            return;
        }
        if games {
            self.games_button(b, repeat, v);
            return;
        }
        // a page sliding out no longer takes buttons
        if m.top().is_some() {
            self.sub_button(b, repeat, v);
            return;
        }
        if !m.on_cross {
            self.running_button(b, repeat, v);
            return;
        }
        let cat = m.cat;
        let s = self.cross_sel[cat.i()];
        let row = m.sel_row(s).cloned();
        if m.adjusting {
            match b {
                Button::Left | Button::Right => {
                    if let Some(r) = row {
                        self.adjust(r, b == Button::Right, v);
                    }
                    return;
                }
                Button::A | Button::B => {
                    if !repeat {
                        m.adjusting = false;
                        self.fx(Sfx::Tab, Some(Haptic::Tick));
                    }
                    return;
                }
                // anything else leaves adjust mode and does what it does
                _ => m.adjusting = false,
            }
        }
        let list = cats(v);
        let i = list.iter().position(|c| *c == cat).unwrap_or(0);
        match b {
            Button::B if !repeat => self.leave_menu(v),
            Button::L1 if !repeat => {
                self.go_running();
                self.fx(Sfx::Move, Some(Haptic::Tick));
            }
            Button::Left if i == 0 => {
                self.go_running();
                self.fx(Sfx::Move, Some(Haptic::Tick));
            }
            Button::Left | Button::Right | Button::R1 => {
                if repeat && b == Button::R1 {
                    return;
                }
                let n = match b {
                    Button::R1 => list.len() - 1,
                    Button::Left => i.saturating_sub(1),
                    _ => (i + 1).min(list.len() - 1),
                };
                if n == i {
                    self.fx(Sfx::Deny, None);
                } else {
                    self.set_cat(list[n], v, false);
                    self.fx(Sfx::Move, Some(Haptic::Tick));
                }
            }
            Button::Up | Button::Down => {
                let n = m.rows.len();
                let ns = if b == Button::Up { s.saturating_sub(1) } else { (s + 1).min(n.saturating_sub(1)) };
                if ns == s {
                    self.fx(Sfx::Deny, None);
                } else {
                    self.cross_sel[cat.i()] = ns;
                    m.layout(ns);
                    self.fx(Sfx::Tick, Some(Haptic::Tick));
                }
            }
            Button::A | Button::Start if !repeat => {
                if let Some(r) = row {
                    self.activate(r, v);
                }
            }
            _ => {}
        }
    }

    fn running_button(&mut self, b: Button, repeat: bool, v: &View) {
        // inside the dashboard: the D-pad moves between its tiles
        if self.home.tiles {
            self.tiles_button(b, repeat, v);
            return;
        }
        let Some(m) = &mut self.menu else { return };
        let n = m.run_keys.len();
        let row = m.run_keys.get(m.run_sel).cloned();
        match b {
            Button::Up | Button::Down => {
                let s = m.run_sel;
                let ns = if b == Button::Up { s.saturating_sub(1) } else { (s + 1).min(n.saturating_sub(1)) };
                if ns == s {
                    self.fx(Sfx::Deny, None);
                } else {
                    m.run_sel = ns;
                    m.run_scroll.set(ns as f32);
                    if self.prefs.reduce_motion {
                        m.run_scroll.snap(ns as f32);
                    }
                    self.fx(Sfx::Tick, Some(Haptic::Tick));
                }
            }
            Button::Right | Button::R1 if !(repeat && b == Button::R1) => {
                let list = cats(v);
                let target = if b == Button::R1 { list.last().copied() } else { list.first().copied() };
                if let Some(c) = target {
                    self.set_cat(c, v, true);
                }
                self.go_cross(false);
                self.fx(Sfx::Tab, Some(Haptic::Tick));
            }
            Button::Left | Button::L1 => self.fx(Sfx::Deny, None),
            Button::B if !repeat => self.leave_menu(v),
            Button::X if !repeat => match row {
                Some(Row::App(uid)) => self.close_from_menu(uid, v),
                _ => self.fx(Sfx::Deny, None),
            },
            Button::Y if !repeat => match row {
                Some(Row::App(uid)) if v.app(uid).is_some_and(|a| a.game) => self.open_gpanel(uid, v),
                Some(Row::App(uid)) => {
                    let mut items = vec![CtxItem::Switch];
                    if v.app(uid).is_some_and(|a| a.entry.is_some()) {
                        items.push(CtxItem::Restart);
                    }
                    items.push(CtxItem::Close);
                    let mut op = Spring::new(0.0);
                    op.set(1.0);
                    m.ctx = Some(Ctx { uid, sel: 0, items, op });
                    self.fx(Sfx::Drawer, Some(Haptic::Tick));
                }
                _ => self.fx(Sfx::Deny, None),
            },
            Button::A | Button::Start if !repeat => match row {
                // with an app in front: go home (it keeps running, its
                // card stays in the stack); at home: into the tiles
                Some(Row::Home) if v.current.is_some() => self.go_home(v),
                Some(Row::Home) => self.enter_tiles(v),
                Some(Row::App(uid)) => self.switch_from_menu(uid, v),
                _ => {}
            },
            _ => {}
        }
    }

    fn ctx_button(&mut self, b: Button, repeat: bool, v: &View) {
        let Some(m) = &mut self.menu else { return };
        let Some(c) = &mut m.ctx else { return };
        match b {
            Button::Up | Button::Down => {
                let n = c.items.len();
                c.sel = if b == Button::Down { (c.sel + 1) % n } else { (c.sel + n - 1) % n };
                self.fx(Sfx::Tick, Some(Haptic::Tick));
            }
            Button::B | Button::Y if !repeat => {
                m.ctx = None;
                self.fx(Sfx::DrawerUp, None);
            }
            Button::A if !repeat => {
                let (uid, item) = (c.uid, c.items[c.sel]);
                m.ctx = None;
                match item {
                    CtxItem::Switch => self.switch_from_menu(uid, v),
                    CtxItem::Close => self.close_from_menu(uid, v),
                    CtxItem::Restart => {
                        self.out.push(Out::Restart(uid));
                        let name = v.app(uid).map(|a| a.name.clone()).unwrap_or_default();
                        self.show_toast(Toast { sub: name, sound: None, ..Toast::info("Restarting") });
                        self.fx(Sfx::Open, Some(Haptic::Double));
                    }
                }
            }
            _ => {}
        }
    }

    /// ◀▶ while adjusting a level.
    fn adjust(&mut self, row: Row, up: bool, v: &View) {
        let d = if up { 10 } else { -10 };
        match row {
            Row::Brightness if v.sys.brightness.is_some() => self.out.push(Out::Brightness(d)),
            Row::Volume if v.volume.is_some() => self.out.push(Out::Volume(d)),
            _ => self.fx(Sfx::Deny, None),
        }
    }

    fn activate(&mut self, row: Row, v: &View) {
        if row == Row::Page(SubKind::TimeZone) {
            // the compositor loads the zones, then opens the page (open_tz)
            self.out.push(Out::OpenTimeZone);
            return;
        }
        if let Some(kind) = row.page() {
            self.open_sub(kind, v);
            return;
        }
        match row {
            Row::Game(g) => self.activate_game(g, v),
            Row::Launch(id) => {
                let Some(l) = v.launcher.iter().find(|l| l.id == id) else { return };
                match v.by_entry(&id) {
                    Some(a) if l.single_instance => {
                        let from = self.sel_rect();
                        self.switch_from_menu_at(a.uid, v, from);
                    }
                    _ => {
                        let from = self.sel_rect();
                        self.close_menu();
                        self.start_trans(Target::Entry(id.clone()), l.name.clone(), l.art, from, v.current);
                        self.out.push(Out::Launch(id));
                    }
                }
            }
            Row::Brightness | Row::Volume => {
                let has = if row == Row::Brightness { v.sys.brightness.is_some() } else { v.volume.is_some() };
                match &mut self.menu {
                    Some(m) if has => {
                        m.adjusting = true;
                        self.fx(Sfx::Tab, Some(Haptic::Tick));
                    }
                    _ => self.fx(Sfx::Deny, None),
                }
            }
            Row::Keyboard => {
                self.close_menu();
                self.out.push(Out::ShowKeyboard);
            }
            Row::Remote => {
                self.out.push(Out::OpenSetup(SetupPage::Remote));
                self.fx(Sfx::Tab, Some(Haptic::Tick));
            }
            _ => {}
        }
    }

    /// Where the selected row's icon is (the zoom starts there).
    pub(super) fn sel_rect(&self) -> R4 {
        let Some(m) = &self.menu else { return R4 { x: LISTX + IX_W, y: ROW_TOP, w: TILE, h: TILE } };
        let s = self.cross_sel[m.cat.i()];
        let y = m.rows.get(s).map(|r| r.y.x).unwrap_or(ROW_TOP);
        R4 { x: LISTX + IX_W, y: y + (SEL_H - TILE) / 2.0, w: TILE, h: TILE }
    }

    pub(super) fn start_trans(&mut self, target: Target, name: String, art: Art, from: R4, prev: Option<u32>) {
        self.trans = Some(Trans::Launch { target, name, art, from, prev, t0: self.t });
        self.lp.snap(0.0);
        self.lp.set(1.0);
        self.fx(Sfx::Open, Some(Haptic::Double));
    }

    /// A on a window: it zooms from its place to the full screen.
    pub(super) fn switch_from_menu(&mut self, uid: u32, v: &View) {
        self.switch_from_menu_at(uid, v, R4 { x: 0.0, y: RUN_Y, w: W, h: RUN_H });
    }

    pub(super) fn switch_from_menu_at(&mut self, uid: u32, v: &View, from: R4) {
        if v.current == Some(uid) {
            self.close_menu();
            return;
        }
        let (name, art) = v.app(uid).map(|a| (a.name.clone(), a.art)).unwrap_or_default();
        self.close_menu();
        self.start_trans(Target::Uid(uid), name, art, from, v.current);
        self.out.push(Out::Focus(uid));
    }

    /// X: close the app; its window goes at once.
    pub(super) fn close_from_menu(&mut self, uid: u32, v: &View) {
        let Some(m) = &mut self.menu else { return };
        m.hidden.push(uid);
        self.out.push(Out::Close(uid));
        self.fx(Sfx::Close, Some(Haptic::Double));
        let name = v.app(uid).map(|a| a.name.clone()).unwrap_or_default();
        // a game that runs says "Saved" itself once its state is written
        if !v.app(uid).is_some_and(|a| a.game && !a.asleep) {
            self.show_toast(Toast { sub: name, sound: None, ..Toast::info("Closed") });
        }
        self.sync_menu(v);
    }

    /// The apps the Running page shows live (they get frame callbacks):
    /// the selected window, and a neighbour while the stack scrolls (at
    /// rest the neighbours are tabs).
    pub fn live_previews(&self) -> Vec<u32> {
        if self.page.is_some() {
            return Vec::new();
        }
        let Some(m) = self.menu.as_ref().filter(|m| !m.closing && m.page_x.x < 0.99) else { return Vec::new() };
        let (s, at) = (m.run_sel, m.run_scroll.x);
        m.run_keys
            .iter()
            .enumerate()
            .filter(|(j, _)| *j == s || (*j as f32 - at).abs() < 0.95)
            .filter_map(|(_, r)| match r {
                Row::App(u) => Some(*u),
                _ => None,
            })
            .collect()
    }

    /// The pattern: stepped only while the cross shows and motion is
    /// not reduced (then it stands still).
    pub(super) fn step_pattern(&mut self, dt: f32) {
        if self.prefs.reduce_motion {
            return;
        }
        if let Some(m) = &mut self.menu
            && m.pattern_visible()
        {
            m.patterns.step(dt);
        }
    }

    // --------------------------------------------------------- drawing ----

    /// The MENU over the frosted backdrop (`g`; home behind gets the flat
    /// veil instead).
    pub(super) fn draw_menu(&self, b: &mut B, pal: &Pal, v: &View, m: &Cross, g: &Glass) {
        let o = m.op.x.clamp(0.0, 1.0);
        if g.src.is_none() && v.current.is_some() {
            b.rect(0.0, 0.0, W, H, with_a(pal.ground, o * 0.9));
        }
        if o < 0.01 {
            return;
        }
        b.push_alpha(o);
        let px = m.page_x.x;
        if px < 0.99 {
            let s = 1.04 - 0.04 * o;
            b.push(Xf::translate(-px * W, 0.0).mul(&Xf::about(W / 2.0, H / 2.0, Xf::scale(s, s))));
            self.draw_running(b, pal, v, m, g);
            b.pop();
        }
        if px > 0.01 {
            b.push(Xf::translate((1.0 - px) * W, 0.0));
            self.draw_cross(b, pal, v, m, g);
            b.pop();
        }
        if let Some(c) = &m.ctx {
            draw_ctx(b, pal, c, g);
        }
        if let Some(p) = &m.gpanel {
            self.draw_gpanel(b, pal, v, p, g);
        }
        // something in Running needs you: a dot on the cross's edge
        // towards it
        if px > 0.01 && !v.dash.attention.is_empty() && m.top().is_none() {
            let x = (1.0 - px) * W + 22.0;
            let cy = CATS_TOP + 10.0 + ICON / 2.0;
            b.ellipse(x, cy, 12.0, 12.0, pal.dark);
            b.ellipse(x, cy, 9.0, 9.0, pal.yellow);
        }
        // the bars fade with the menu under the screensaver
        let (e, _) = self.saver();
        b.push_alpha(1.0 - e);
        self.glass_status(b, pal, v, m, g);
        let hints = self.menu_hints(m, v);
        glass_hints(b, pal, g, &hints);
        b.pop_alpha();
        b.pop_alpha();
    }

    fn draw_running(&self, b: &mut B, pal: &Pal, v: &View, m: &Cross, g: &Glass) {
        // the dashboard selected is full screen (f 0); scrolled to the
        // first app (f 1) it is a card above it like any other. In between
        // it shrinks into its card and the apps rise from below the screen.
        let f = m.run_scroll.x.clamp(0.0, 1.0);
        let push = (1.0 - f) * (H - (RUN_Y + RUN_STEP));
        for (j, r) in m.run_keys.iter().enumerate() {
            let d = j as f32 - m.run_scroll.x;
            let card_y = RUN_Y + d * RUN_STEP + if j > 0 { push } else { 0.0 };
            // the rectangle this card fills (the dashboard: the screen,
            // shrinking to its card)
            let (y, h, frame) = match r {
                Row::Home => (lerp(0.0, card_y, f), lerp(H, RUN_H, f), smooth01(f * 1.6)),
                _ => (card_y, RUN_H, 1.0),
            };
            if y >= H || y + h <= 0.0 {
                continue;
            }
            let on = j == m.run_sel;
            // a card turns into a tab (its name on a plain card) as it
            // settles next to the selected one: at rest only the selected
            // card is live, so an idle stack draws no frames
            let tab = smooth01((d.abs() - 0.55) / 0.4);
            if tab < 1.0 {
                b.clip(Some([0.0, y, W, h]));
                match r {
                    Row::Home => {
                        // the full-screen picture, shrunk to fit the card
                        let k = h / H;
                        b.rect(0.0, y, W, h, pal.ground);
                        b.push(Xf::translate((W - W * k) / 2.0, y).mul(&Xf::scale(k, k)));
                        self.draw_dash(b, pal, v, on && self.home.tiles);
                        b.pop();
                    }
                    Row::App(uid) => {
                        // the app's middle rows (where they are on screen)
                        b.push(Xf::translate(0.0, y - RUN_Y));
                        match v.app(*uid) {
                            Some(a) if a.has_buffer => {
                                b.rect(0.0, 0.0, W, H, Color(0, 0, 0, 255));
                                let gg = a.geom;
                                b.image(Src::App(*uid), gg[0], gg[1], gg[2], gg[3], [0.0, 0.0, 1.0, 1.0]);
                            }
                            // a game asleep (or waking): its last frame
                            Some(a) if a.pic.is_some() => {
                                b.rect(0.0, 0.0, W, H, Color(0, 0, 0, 255));
                                b.image(Src::Pic(a.pic.unwrap_or(0)), 0.0, 0.0, W, H, [0.0, 0.0, 1.0, 1.0]);
                            }
                            Some(a) => self.splash(b, pal, a.art, &a.name, 0.0),
                            None => {}
                        }
                        b.pop();
                    }
                    _ => {}
                }
                b.clip(None);
                let dim = d.abs().min(1.0) * 0.35;
                if dim > 0.0 {
                    b.rect(0.0, y, W, h, with_a(pal.ground, dim));
                }
                // an app that waits for you: a yellow badge at its top right
                if let Some(it) = card_attention(r, v) {
                    needs_you_badge(b, pal, W - 28.0, y + 24.0, &it.text);
                }
            }
            if tab > 0.0 {
                b.push_alpha(tab);
                self.draw_tab(b, pal, v, r, y, h, d < 0.0);
                b.pop_alpha();
            }
            // the card's frame (the full-screen dashboard has none)
            if frame > 0.0 {
                b.push_alpha(frame);
                b.rect(0.0, y, W, 3.0, pal.dark);
                b.rect(0.0, y + h - 3.0, W, 3.0, pal.dark);
                if on {
                    b.rect(0.0, y, 10.0, h, run_accent(r, v, pal));
                    b.rect(10.0, y, 3.0, h, pal.dark);
                }
                b.pop_alpha();
            }
            if on && *r != Row::Home {
                self.draw_run_label(b, pal, v, r, j, y, g);
            }
        }
        // the dashboard full screen: the apps below as a slim, quiet strip
        // at the bottom (▼ leads to them), gone as the stack comes
        let a = 1.0 - smooth01(f / 0.45);
        if a > 0.0 {
            b.push_alpha(a);
            self.draw_app_strip(b, pal, v, m, H - HINT_H - 24.0 + f * 30.0);
            b.pop_alpha();
        }
    }

    /// The running apps as small tabs in a row centred on `cy` (on the
    /// full-screen dashboard): "▼", then each app's accent and name, NEEDS
    /// YOU in yellow; with nothing running, the way to the apps.
    fn draw_app_strip(&self, b: &mut B, pal: &Pal, v: &View, m: &Cross, cy: f32) {
        let st = ts(Face::Mono600, 15.0, pal.soft).tracking(0.08);
        let apps: Vec<&RunningInfo> = m.run_keys.iter().filter_map(|r| if let Row::App(u) = r { v.app(*u) } else { None }).collect();
        if apps.is_empty() {
            let s = "NOTHING RUNNING  ·  ▶ APPS";
            let w = arrow_text(b, 0.0, 0.0, s, &st, false);
            arrow_text(b, (W - w) / 2.0, cy - st.line_px() / 2.0, s, &st, true);
            return;
        }
        let nm = ts(Face::Body500, 17.0, pal.ink);
        let h = 30.0;
        let mut x = 56.0;
        x += arrow_text(b, x, cy - st.line_px() / 2.0, "▼", &st, true) + 14.0;
        for (n, a) in apps.iter().enumerate() {
            let waits = card_attention(&Row::App(a.uid), v).is_some();
            let name = b.fit(&a.name, &nm, 220.0);
            let w = 12.0 + 8.0 + 8.0 + b.measure(&name, &nm) + 12.0;
            if x + w > W - 56.0 - 60.0 {
                b.text(x, cy - st.line_px() / 2.0, &format!("+{}", apps.len() - n), &st);
                break;
            }
            if waits {
                b.rect(x, cy - h / 2.0, w, h, pal.yellow);
            }
            b.frame(x, cy - h / 2.0, w, h, 1.0, if waits { pal.ink } else { with_a(pal.ink, 0.35) });
            b.rect(x + 12.0, cy - 4.0, 8.0, 8.0, a.art.accent(pal));
            b.text(x + 28.0, cy - nm.line_px() / 2.0, &name, &nm);
            x += w + 10.0;
        }
    }

    /// A card at rest next to the selected one: a plain card with its
    /// accent, name and state along the edge that shows (`above`: its
    /// bottom edge).
    #[allow(clippy::too_many_arguments)]
    fn draw_tab(&self, b: &mut B, pal: &Pal, v: &View, r: &Row, y: f32, h: f32, above: bool) {
        b.rect(0.0, y, W, h, pal.ground_2);
        let top = if above { y + h - PEEK } else { y };
        b.rect(0.0, top, 10.0, PEEK, run_accent(r, v, pal));
        b.rect(10.0, top, 3.0, PEEK, pal.dark);
        let cy = top + PEEK / 2.0 + if above { -1.0 } else { 1.0 };
        let (name, state) = match r {
            Row::Home => ("Home".to_string(), date_line(v.host)),
            Row::App(u) => match v.app(*u) {
                Some(a) => (a.name.clone(), AppState::of(a, v).label().to_uppercase()),
                None => (String::new(), String::new()),
            },
            _ => (String::new(), String::new()),
        };
        let nm = ts(Face::SerifItalic, 28.0, pal.ink).line_height(1.0);
        let name = b.fit(&name, &nm, 520.0);
        let nw = b.text(32.0, cy - nm.line_px() / 2.0, &name, &nm);
        let st = ts(Face::Mono400, 15.0, pal.soft).tracking(0.06);
        b.text(32.0 + nw + 16.0, cy - st.line_px() / 2.0, &state, &st);
        let waiting = match r {
            Row::Home => match v.dash.attention.len() {
                0 => None,
                1 => Some("NEEDS YOU".to_string()),
                n => Some(format!("{n} NEED YOU")),
            },
            _ => card_attention(r, v).map(|_| "NEEDS YOU".to_string()),
        };
        if let Some(w) = waiting {
            chip(b, W - 28.0, cy, &w, 14.0, (8.0, 2.0), pal.yellow, pal.dark);
        }
    }

    /// The selected window's label: index, serif name, status.
    #[allow(clippy::too_many_arguments)]
    fn draw_run_label(&self, b: &mut B, pal: &Pal, v: &View, r: &Row, j: usize, y: f32, g: &Glass) {
        let (name, status) = match r {
            Row::Home => ("Home".to_string(), String::new()),
            Row::App(u) => match v.app(*u) {
                Some(a) => (a.name.clone(), AppState::of(a, v).label().to_string()),
                None => (String::new(), String::new()),
            },
            _ => (String::new(), String::new()),
        };
        let h = 58.0;
        let (x, top) = (28.0, y + RUN_H - 18.0 - h);
        let ix = ts(Face::Mono600, 18.0, pal.ground);
        let ix_s = format!("{:02}", j + 1);
        let iw = b.measure(&ix_s, &ix) + 24.0;
        b.rect(x, top, iw, h, pal.dark);
        b.text(x + 12.0, top + h / 2.0 - ix.line_px() / 2.0, &ix_s, &ts(Face::Mono600, 18.0, DAY.ground));
        let nm = ts(Face::SerifItalic, 40.0, pal.ink).line_height(1.1);
        let name = b.fit(&name, &nm, 600.0);
        let nw = b.measure(&name, &nm) + 28.0;
        let nx = x + iw;
        glass(b, pal, g, nx, top, nw, h, 0.6);
        b.text(nx + 14.0, top + h / 2.0 - nm.line_px() / 2.0, &name, &nm);
        open_box(b, nx, top, nw, h, 3.0, pal.dark);
        if !status.is_empty() {
            let st = ts(Face::Mono400, 17.0, pal.ink);
            let sw = b.measure(&status, &st) + 24.0;
            let (sx, sh) = (nx + nw, 56.0);
            let sy = top + (h - sh) / 2.0;
            glass(b, pal, g, sx, sy, sw, sh, 0.6);
            b.text(sx + 12.0, sy + sh / 2.0 - st.line_px() / 2.0, &status, &st);
            open_box(b, sx, sy, sw, sh, 3.0, pal.dark);
        }
    }

    fn draw_cross(&self, b: &mut B, pal: &Pal, v: &View, m: &Cross, g: &Glass) {
        let accent = m.cat.accent(pal);
        // the screensaver: the pattern grows from its strip over the screen
        // and the menu fades out
        let (e, dim) = self.saver();
        if m.pattern_visible() {
            let x0 = patterns::STRIP_X * (1.0 - e);
            // the new pattern slides in as the old one slides out
            let p = m.pat_x.x;
            let off = patterns::STRIP_W * m.pat_dir;
            if let Some(old) = m.pat_from.filter(|_| p < 0.999) {
                m.patterns.draw(b, pal, old.pattern(), -p * off, x0, e);
            }
            m.patterns.draw(b, pal, m.cat.pattern(), (1.0 - p) * off, x0, e);
            if dim > 0.0 {
                b.rect(0.0, 0.0, W, H, with_a(pal.ground, dim));
            }
        }
        if e >= 0.999 {
            return;
        }
        b.push_alpha(1.0 - e);
        let s = self.cross_sel[m.cat.i()];
        // rows first, the categories over them (a row moving past never
        // covers an icon or its shadow)
        for (j, r) in m.rows.iter().enumerate() {
            let a = r.op.x.clamp(0.0, 1.0);
            if a < 0.01 || r.y.x > H {
                continue;
            }
            b.push(Xf::translate(LISTX, r.y.x));
            b.push_alpha(a);
            self.draw_item(b, pal, v, m, r, j == s, accent, g);
            b.pop_alpha();
            b.pop();
        }
        self.draw_cats(b, pal, v, m, g);
        if let Some(Row::Game(gr)) = m.sel_row(s).filter(|_| m.cat == Cat::Games && m.gpage.is_none())
            && matches!(gr, GRow::Continue | GRow::Recent(_))
        {
            self.draw_games_pic(b, pal, v, gr, g);
        }
        for sub in &m.subs {
            self.draw_sub(b, pal, v, m, sub, g);
        }
        if let Some(p) = &m.gpage {
            self.draw_gpage(b, pal, v, p, g);
        }
        b.pop_alpha();
    }

    fn draw_cats(&self, b: &mut B, pal: &Pal, v: &View, m: &Cross, g: &Glass) {
        let list = cats(v);
        let cur = list.iter().position(|c| *c == m.cat).unwrap_or(0) as f32;
        let name_st = ts(Face::SerifItalic, 34.0, pal.ink).line_height(1.0);
        let cw = ICON + 32.0;
        for (i, c) in list.iter().enumerate() {
            let x = COLX + i as f32 * CATW + m.cat_x.x;
            if x > W + 40.0 || x + cw < -40.0 {
                continue;
            }
            let dist = (i as f32 - cur).abs();
            let l = m.lift.get(i).map(|s| s.x).unwrap_or(0.0);
            b.push_alpha((1.0 - dist * 0.16).max(0.0));
            let (ix, iy) = (x + 16.0, CATS_TOP + 10.0);
            let sc = 0.82 + 0.18 * l;
            // lifted: up and left, with a hard ink shadow; it scales from
            // its bottom centre
            b.push(Xf::translate(-7.0 * l, -10.0 * l).mul(&Xf::about(ix + ICON / 2.0, iy + ICON, Xf::scale(sc, sc))));
            if l > 0.01 {
                b.push_alpha(l.clamp(0.0, 1.0));
                b.rect(ix + 9.0, iy + 9.0, ICON, ICON, pal.dark);
                b.pop_alpha();
            }
            let glyph = ICON * 0.6;
            if *c == m.cat {
                b.rect(ix, iy, ICON, ICON, c.accent(pal));
                b.frame(ix, iy, ICON, ICON, 2.0, pal.dark);
                draw_icon(b, c.icon(), ix + ICON / 2.0, iy + ICON / 2.0, glyph, pal, pal.dark);
            } else {
                glass(b, pal, g, ix, iy, ICON, ICON, 0.4);
                b.frame(ix, iy, ICON, ICON, 1.0, Color(255, 255, 255, 190));
                draw_icon(b, c.icon(), ix + ICON / 2.0, iy + ICON / 2.0, glyph, pal, pal.ink);
            }
            b.pop();
            b.text_center(x + cw / 2.0, iy + ICON + 12.0, c.name(), &name_st);
            b.pop_alpha();
        }
    }

    /// One item in its own space (0, 0 is its top left; ITEM_W wide, SEL_H
    /// tall when selected, else ROWH).
    #[allow(clippy::too_many_arguments)]
    fn draw_item(&self, b: &mut B, pal: &Pal, v: &View, m: &Cross, r: &RowAnim, on: bool, accent: Color, g: &Glass) {
        let row = &r.key;
        let info = row_info(row, v, &self.prefs);
        let h = if on { SEL_H } else { ROWH };
        let cy = h / 2.0;
        let bw = r.bw.x;
        if bw > 0.01 {
            // the band, from the left screen edge, with its offset shadow
            // (a bouncy spring overshoots: the band never reaches the pattern)
            let x1 = bw.min(1.0) * (ITEM_W + 24.0);
            let (by, bh) = (4.0, SEL_H - 8.0);
            b.rect(-LISTX - 8.0, by + 10.0, x1 + LISTX + 8.0 + 10.0, bh, pal.dark);
            b.rect(-LISTX - 8.0, by, x1 + LISTX + 8.0, bh, accent);
            b.rect(-LISTX - 8.0, by, x1 + LISTX + 8.0, 2.0, pal.dark);
            b.rect(-LISTX - 8.0, by + bh - 2.0, x1 + LISTX + 8.0, 2.0, pal.dark);
            b.rect(x1 - 2.0, by, 2.0, bh, pal.dark);
        }
        // on the band: dark ink; elsewhere ink and ink-soft (at least
        // 4.5:1 on the frosted ground)
        let (ink, soft) = if on { (pal.dark, pal.dark) } else { (pal.ink, pal.soft) };
        let ix = ts(Face::Mono400, 18.0, soft);
        b.text(0.0, cy - ix.line_px() / 2.0, &info.index_of(m, row), &ix);
        // the icon tile: glass, or solid with an ink outline when selected
        let tile = if on { TILE } else { TILE_S };
        let (sx, sy) = (IX_W, cy - tile / 2.0);
        if on {
            b.rect(sx, sy, tile, tile, DAY.ground);
            b.frame(sx, sy, tile, tile, 2.0, pal.dark);
        } else {
            glass(b, pal, g, sx, sy, tile, tile, 0.45);
            b.frame(sx, sy, tile, tile, 1.0, Color(255, 255, 255, 190));
        }
        draw_icon(b, info.icon, sx + tile / 2.0, sy + tile / 2.0, tile * 0.62, pal, if on { pal.dark } else { pal.ink });
        // the value column, right-aligned at the item's right edge
        let adjusting = on && m.adjusting;
        let mut right = ITEM_W;
        let num_st = ts(Face::Mono400, 22.0, ink);
        if let Some(lv) = info.level {
            let txt = format!("{lv}%");
            let nw = b.measure("100%", &num_st);
            b.text_right(ITEM_W, cy - num_st.line_px() / 2.0, &txt, &num_st);
            let (bw2, bh) = (130.0, 18.0);
            let (bx, by) = (ITEM_W - nw - 12.0 - bw2, cy - bh / 2.0);
            b.rect(bx, by, bw2, bh, if adjusting { pal.yellow } else { DAY.ground });
            b.rect(bx, by, bw2 * (lv.min(100) as f32 / 100.0), bh, pal.dark);
            b.frame(bx, by, bw2, bh, 2.0, pal.dark);
            right = bx - 14.0;
        } else if info.chevron {
            let ch = ts(Face::Mono400, 30.0, ink);
            let cw = b.text_right(ITEM_W, cy - ch.line_px() / 2.0, "›", &ch);
            right = ITEM_W - cw - 16.0;
            if let Some(n) = &info.value {
                let w = b.text_right(right - 6.0, cy - num_st.line_px() / 2.0, n, &num_st);
                right -= 6.0 + w + 14.0;
            }
        } else if let Some(n) = &info.value {
            let w = b.text_right(ITEM_W, cy - num_st.line_px() / 2.0, n, &num_st);
            right = ITEM_W - w - 14.0;
        }
        let title = if on { ts(Face::SerifItalic, 56.0, ink).line_height(0.95) } else { ts(Face::Body600, 32.0, ink).line_height(1.05) };
        let sub = ts(Face::Mono400, 20.0, soft);
        let tx = IX_W + TILE + 20.0;
        let top = cy - (title.line_px() + 2.0 + sub.line_px()) / 2.0;
        let t = b.fit(&info.title, &title, right - tx);
        b.text(tx, top, &t, &title);
        let s = if adjusting {
            "◀▶ to change · A when done".to_string()
        } else {
            info.sub
        };
        let s = b.fit(&s, &sub, right - tx);
        arrow_text(b, tx, top + title.line_px() + 2.0, &s, &sub, true);
    }

    /// The hint bar. The same words everywhere: ▲▼ Choose, ◀▶ Category /
    /// Adjust / Channel, A Open / Switch / Adjust / Change / Done, B Back
    /// (one level), MENU Close (everything).
    pub(super) fn menu_hints(&self, m: &Cross, v: &View) -> Vec<(&'static str, &'static str)> {
        let mut h = self.menu_hints_inner(m, v);
        // home: MENU comes back to the dashboard rather than closing
        if v.current.is_none() {
            let rest = !m.on_cross && m.run_sel == 0 && !self.home.tiles && m.ctx.is_none() && m.top().is_none() && !self.games_take();
            h.retain(|x| !(rest && x.0 == "MENU"));
            for x in &mut h {
                if *x == ("MENU", "Close") {
                    x.1 = "Home";
                }
            }
        }
        h
    }

    fn menu_hints_inner(&self, m: &Cross, v: &View) -> Vec<(&'static str, &'static str)> {
        if let Some(sub) = m.top() {
            return self.sub_hints(sub, v);
        }
        if let Some(h) = self.games_hints(v) {
            return h;
        }
        if m.ctx.is_some() {
            return vec![("▲▼", "Choose"), ("A", "Select"), ("B", "Back")];
        }
        if !m.on_cross && self.home.tiles {
            return self.tiles_hints(v);
        }
        if !m.on_cross {
            let mut h = if m.run_keys.len() > 1 { vec![("▲▼", "Choose")] } else { Vec::new() };
            h.push(("▶", "Apps"));
            match m.run_keys.get(m.run_sel) {
                Some(Row::Home) if v.current.is_some() => h.push(("A", "Go home")),
                Some(Row::Home) => {
                    if self.dash_acts(v) {
                        h.push(("A", "Widgets"));
                    }
                }
                Some(Row::App(u)) if v.app(*u).is_some_and(|a| a.game) => {
                    let wake = v.app(*u).is_some_and(|a| a.asleep);
                    h.extend([("X", "Close app"), ("Y", "Game menu"), ("A", if wake { "Wake" } else { "Switch" })]);
                }
                Some(_) => h.extend([("X", "Close app"), ("Y", "Options"), ("A", "Switch")]),
                None => {}
            }
            h.push(("MENU", "Close"));
            return h;
        }
        if m.adjusting {
            return vec![("◀▶", "Adjust"), ("A", "Done"), ("MENU", "Close")];
        }
        let first = cats(v).first() == Some(&m.cat);
        let mut h = vec![("◀▶", if first { "Running / Category" } else { "Category" }), ("▲▼", "Choose")];
        match m.sel_row(self.cross_sel[m.cat.i()]) {
            Some(r) if r.level() => h.push(("A", "Adjust")),
            Some(Row::Game(g)) => h.push(("A", super::gamepages::game_hint(g, v))),
            Some(Row::Launch(id)) => {
                let running = v.launcher.iter().find(|l| l.id == *id).is_some_and(|l| l.running && l.single_instance);
                h.push(("A", if running { "Switch" } else { "Open" }));
            }
            Some(Row::Keyboard) => h.push(("A", "Show")),
            Some(_) => h.push(("A", "Open")),
            None => {}
        }
        h.push(("MENU", "Close"));
        h
    }

    /// The status bar: frosted glass with the clock and the device's state
    /// (no hairline, no category tabs).
    fn glass_status(&self, b: &mut B, pal: &Pal, v: &View, m: &Cross, g: &Glass) {
        glass(b, pal, g, 0.0, 0.0, W, BAR_H, 0.42);
        // the dashboard's big clock is the clock while it is selected:
        // this one fades as the stack scrolls onto it
        let dash = m.run_keys.iter().position(|r| *r == Row::Home).map(|i| i as f32);
        let a = dash.map(|i| (m.run_scroll.x - i).abs().min(1.0).max(m.page_x.x.clamp(0.0, 1.0))).unwrap_or(1.0);
        if a > 0.01 {
            let clock = ts(Face::SerifItalic, 48.0, with_a(pal.ink, a)).line_height(1.0);
            b.text(28.0, BAR_H / 2.0 - clock.line_px() / 2.0, &v.sys.time, &clock);
        }
        let mono = ts(Face::Mono400, 19.0, pal.ink);
        let mut right = W - 28.0;
        if let Some(bat) = v.sys.battery {
            right -= b.text_right(right, BAR_H / 2.0 - mono.line_px() / 2.0, &format!("{bat}%"), &mono) + 8.0;
            // a small battery: body, level, cap
            let (bw, bh) = (42.0, 20.0);
            let (bx, by) = (right - 3.0 - bw, BAR_H / 2.0 - bh / 2.0);
            b.frame(bx, by, bw, bh, 1.5, pal.ink);
            let fill = if v.sys.charging || bat > 15 {
                pal.lime
            } else if bat > 5 {
                pal.yellow
            } else {
                pal.orange
            };
            b.rect(bx + 3.5, by + 3.5, (bw - 7.0) * bat.min(100) as f32 / 100.0, bh - 7.0, fill);
            b.rect(bx + bw + 1.0, by + 6.0, 4.0, 8.0, pal.ink);
            if v.sys.charging {
                b.poly_concave(&bolt_pts(bx - 14.0, BAR_H / 2.0, 22.0), pal.ink);
                right = bx - 30.0;
            } else {
                right = bx - 14.0;
            }
        }
        let mut net = Vec::new();
        if v.sys.wifi {
            net.push("Wi-Fi");
        }
        if v.sys.usb_net {
            net.push("USB");
        }
        if !net.is_empty() {
            right -= b.text_right(right, BAR_H / 2.0 - mono.line_px() / 2.0, &net.join(" · "), &mono) + 12.0;
        }
        if v.sys.ssh_sessions > 0 {
            chip(b, right, BAR_H / 2.0, "SSH", 18.0, (9.0, 3.0), pal.lime, pal.lime_ink);
        }
    }
}

/// A frosted pane over the backdrop the MENU sits on: the blurred app (cut
/// to the pane) under its veil, then a
/// light ground at `tint` and a light top edge. With nothing blurred (an
/// app still starting) a flat ground.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Glass {
    /// The blurred backdrop (the app or home) and where it is on screen.
    pub src: Option<(Src, [f32; 4])>,
    /// The veil over the blurred app (frost x 0.62).
    pub veil: f32,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn glass(b: &mut B, pal: &Pal, g: &Glass, x: f32, y: f32, w: f32, h: f32, tint: f32) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    match g.src {
        Some((src, gg)) if gg[2] > 0.0 && gg[3] > 0.0 => {
            // the blur under the pane, in screen space (the pane may move
            // or scale; the backdrop does not)
            let m = b.xf();
            let (p0, p1) = (m.apply((x, y)), m.apply((x + w, y + h)));
            let uv = [(p0.0 - gg[0]) / gg[2], (p0.1 - gg[1]) / gg[3], (p1.0 - gg[0]) / gg[2], (p1.1 - gg[1]) / gg[3]];
            b.image(src, x, y, w, h, uv.map(|c| c.clamp(0.0, 1.0)));
            b.rect(x, y, w, h, with_a(pal.ground, g.veil));
        }
        _ => b.rect(x, y, w, h, with_a(pal.ground, 0.9)),
    }
    b.rect(x, y, w, h, with_a(pal.ground, tint));
    b.rect(x, y, w, 1.0, Color(255, 255, 255, if pal.night { 60 } else { 190 }));
}

/// A box border without its left side (the label's segments).
fn open_box(b: &mut B, x: f32, y: f32, w: f32, h: f32, bw: f32, c: Color) {
    b.rect(x, y, w, bw, c);
    b.rect(x, y + h - bw, w, bw, c);
    b.rect(x + w - bw, y, bw, h, c);
}

/// The hint bar: frosted glass, right-aligned (cap, label) pairs.
pub(super) fn glass_hints(b: &mut B, pal: &Pal, g: &Glass, items: &[(&str, &str)]) {
    let y = H - HINT_H;
    glass(b, pal, g, 0.0, y, W, HINT_H, 0.42);
    let cap_st = ts(Face::Mono600, 17.0, DAY.ground);
    let lab = ts(Face::Body400, 22.0, pal.soft);
    let mut right = W - 28.0;
    for (cap, label) in items.iter().rev() {
        let lw = b.measure(label, &lab);
        b.text(right - lw, y + HINT_H / 2.0 - lab.line_px() / 2.0, label, &lab);
        right -= lw + 10.0;
        let tw = arrow_text(b, 0.0, 0.0, cap, &cap_st, false);
        let cw = (tw + 22.0).max(38.0);
        b.rrect(right - cw, y + HINT_H / 2.0 - 19.0, cw, 38.0, 19.0, Color(29, 29, 29, 209));
        arrow_text(b, right - cw + (cw - tw) / 2.0, y + HINT_H / 2.0 - cap_st.line_px() / 2.0, cap, &cap_st, true);
        right -= cw + 26.0;
    }
}

pub(super) struct RowInfo {
    pub title: String,
    pub sub: String,
    pub icon: Ic,
    /// A level bar and its percentage.
    pub level: Option<u8>,
    /// A plain value in the value column.
    pub value: Option<String>,
    /// Opens a page (›).
    pub chevron: bool,
}

impl RowInfo {
    fn index_of(&self, m: &Cross, row: &Row) -> String {
        format!("{:02}", m.rows.iter().position(|r| r.key == *row).map(|i| i + 1).unwrap_or(0))
    }
}

fn row_info(row: &Row, v: &View, prefs: &Prefs) -> RowInfo {
    let ri = |title: &str, sub: String, icon: Ic| RowInfo { title: title.to_string(), sub, icon, level: None, value: None, chevron: false };
    let page = |k: SubKind, sub: String| RowInfo { chevron: true, ..ri(k.title(), sub, k.icon()) };
    match row {
        Row::Home => ri("Home", "Clock, Needs you, Now playing".into(), Ic::Home),
        Row::App(uid) => match v.app(*uid) {
            Some(a) => ri(&a.name, String::new(), Ic::art(a.art)),
            None => ri("", String::new(), Ic::Apps),
        },
        Row::Game(g) => super::gamepages::game_row_info(g, v),
        Row::Launch(id) if super::gamepages::launch_label(id, v).is_some() => {
            let (t, s) = super::gamepages::launch_label(id, v).unwrap_or_default();
            ri(&t, s, Ic::Games)
        }
        Row::Launch(id) => match v.launcher.iter().find(|l| l.id == *id) {
            Some(l) => {
                let state = v.by_entry(id).filter(|_| l.single_instance).map(|a| AppState::of(a, v).label());
                let sub = match state {
                    Some(st) if l.description.is_empty() => st.to_string(),
                    Some(st) => format!("{} · {st}", l.description),
                    None => l.description.clone(),
                };
                ri(&l.name, sub, Ic::art(l.art))
            }
            None => ri(id, String::new(), Ic::Apps),
        },
        Row::Brightness => RowInfo { level: v.sys.brightness, ..ri("Brightness", if v.sys.brightness.is_some() { "A to adjust".into() } else { "No backlight".into() }, Ic::Sun) },
        Row::Volume => RowInfo { level: v.volume.map(|x| if v.muted { 0 } else { x }), ..ri("Volume", if v.muted { "Muted · A to adjust".into() } else { "A to adjust".into() }, Ic::Vol) },
        Row::Mixer => RowInfo { value: v.volume.map(|x| format!("{x}%")), chevron: true, ..ri("Mixer", "A level for each app".into(), Ic::Music) },
        Row::Keyboard => ri("Keyboard", "On-screen keys for this app".into(), Ic::Keys),
        Row::Remote => {
            let sub = if v.sys.sshd_listening {
                match v.sys.ssh_sessions {
                    0 => "SSH on · pair a computer".to_string(),
                    1 => "SSH on · 1 session".to_string(),
                    n => format!("SSH on · {n} sessions"),
                }
            } else {
                "SSH off · pair a computer".to_string()
            };
            RowInfo { chevron: true, ..ri("Remote access", sub, Ic::Link) }
        }
        Row::Page(k) => {
            let sub = match k {
                SubKind::Display => format!("{} · {}{}", if prefs.night { "Night" } else { "Day" }, prefs.motion.name(), if prefs.reduce_motion { " · reduced" } else { "" }),
                SubKind::Sound => format!("Mixer, UI sounds {}, vibration {}", if prefs.sounds { "on" } else { "off" }, if prefs.haptics { "on" } else { "off" }),
                SubKind::Wifi => super::menupages::wifi_summary(v),
                SubKind::Bluetooth => super::menupages::bt_summary(v),
                SubKind::TimeZone => format!("{} · {}", crate::tz::pretty(v.timezone).0, v.sys.time),
                SubKind::Updates => v.update.map(|u| u.message.clone()).unwrap_or_default(),
                SubKind::About => "Version, storage, battery, controls".into(),
                SubKind::Power => "Sleep, restart, power off, log out".into(),
                _ => String::new(),
            };
            page(*k, sub)
        }
    }
}

/// Smoothstep of `x` clamped to 0..1.
fn smooth01(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// A card's colour down its left edge (the dashboard's is lime).
fn run_accent(r: &Row, v: &View, pal: &Pal) -> Color {
    match r {
        Row::Home => pal.lime,
        Row::App(u) => v.app(*u).map(|a| a.art.accent(pal)).unwrap_or(pal.lavender),
        _ => pal.lavender,
    }
}

/// What a running app's card says it waits for (Needs you).
fn card_attention<'v>(r: &Row, v: &View<'v>) -> Option<&'v crate::attention::Item> {
    let Row::App(u) = r else { return None };
    let e = v.app(*u)?.entry.as_deref()?;
    v.dash.attention.iter().find(|i| i.app == e)
}

/// "NEEDS YOU" on a window in Running, right-aligned at `right`.
fn needs_you_badge(b: &mut B, pal: &Pal, right: f32, top: f32, text: &str) {
    let lab = ts(Face::Mono600, 17.0, pal.dark).tracking(0.06);
    let txt = ts(Face::Body500, 20.0, pal.dark);
    let t = b.fit(text, &txt, 380.0);
    let (lw, tw) = (b.measure("NEEDS YOU", &lab), b.measure(&t, &txt));
    let h = 44.0;
    let w = 16.0 + lw + 14.0 + tw + 16.0;
    let x = right - w;
    b.rect(x + 6.0, top + 6.0, w, h, pal.dark);
    b.rect(x, top, w, h, pal.yellow);
    b.frame(x, top, w, h, 2.0, pal.dark);
    b.text(x + 16.0, top + h / 2.0 - lab.line_px() / 2.0, "NEEDS YOU", &lab);
    b.text(x + 16.0 + lw + 14.0, top + h / 2.0 - txt.line_px() / 2.0, &t, &txt);
}

/// The options popup (Y on a window): frosted, ink outline and shadow.
fn draw_ctx(b: &mut B, pal: &Pal, c: &Ctx, g: &Glass) {
    let a = c.op.x.clamp(0.0, 1.0);
    let rh = 64.0;
    let (x, y, w) = (CTX_X - (1.0 - c.op.x) * 20.0, CTX_Y, 340.0);
    b.push_alpha(a);
    let h = rh * c.items.len() as f32;
    b.rect(x + 8.0, y + 8.0, w, h, pal.dark);
    glass(b, pal, g, x, y, w, h, 0.7);
    for (i, it) in c.items.iter().enumerate() {
        let ry = y + i as f32 * rh;
        let on = i == c.sel;
        let warn = *it == CtxItem::Close;
        if on {
            b.rect(x, ry, w, rh, if warn { pal.orange } else { pal.lime });
        }
        let st = ts(Face::Body400, 28.0, if on { pal.dark } else { pal.ink });
        b.text(x + 20.0, ry + rh / 2.0 - st.line_px() / 2.0, it.name(), &st);
    }
    b.frame(x, y, w, h, 2.0, pal.dark);
    b.pop_alpha();
}

// ------------------------------------------------------------- icons ----

/// Flat line icons (after the mockup's SVGs on a 60-unit grid centred on
/// 0,0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Ic {
    Running,
    Apps,
    Games,
    Music,
    Quick,
    Power,
    Home,
    M8,
    Pico,
    Claude,
    Term,
    Puzzle,
    Sun,
    Vol,
    Wifi,
    Bt,
    Keys,
    Clock,
    Screen,
    Link,
    Update,
    Info,
    Pad,
}

impl Ic {
    fn art(a: Art) -> Ic {
        match a {
            Art::M8 => Ic::M8,
            Art::Pico8 => Ic::Pico,
            Art::Games => Ic::Games,
            Art::Claude => Ic::Claude,
            Art::Puzzle => Ic::Puzzle,
            Art::Term => Ic::Term,
            Art::Generic => Ic::Apps,
        }
    }
}

/// Points along a circle from `a0` to `a1` degrees (y down).
fn arc(cx: f32, cy: f32, r: f32, a0: f32, a1: f32) -> Vec<P> {
    let n = ((a1 - a0).abs() / 8.0).ceil().max(2.0) as usize;
    (0..=n)
        .map(|i| {
            let a = (a0 + (a1 - a0) * i as f32 / n as f32).to_radians();
            (cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}

pub(super) fn draw_icon(b: &mut B, ic: Ic, cx: f32, cy: f32, size: f32, pal: &Pal, ink: Color) {
    if ic == Ic::Claude {
        art::draw_spark(b, cx, cy, size);
        return;
    }
    let s = size / 60.0;
    b.push(Xf::translate(cx, cy).mul(&Xf::scale(s, s)));
    let lw = 2.6;
    let rect = |b: &mut B, x: f32, y: f32, w: f32, h: f32, fill: Option<Color>| {
        let pts = [(x, y), (x + w, y), (x + w, y + h), (x, y + h)];
        if let Some(f) = fill {
            b.poly(&pts, f);
        }
        b.stroke(&pts, true, lw, ink);
    };
    let line = |b: &mut B, pts: &[P]| b.stroke(pts, false, lw, ink);
    let dot = |b: &mut B, x: f32, y: f32, r: f32, fill: Color, stroke: bool| {
        let pts = crate::scene::ellipse_pts(x, y, r, r, 28);
        b.poly(&pts, fill);
        if stroke {
            b.stroke(&pts, true, lw, ink);
        }
    };
    match ic {
        Ic::Running => {
            rect(b, -22.0, -16.0, 30.0, 24.0, Some(pal.lavender));
            rect(b, -8.0, -6.0, 30.0, 24.0, Some(DAY.ground));
        }
        Ic::Apps => {
            rect(b, -22.0, -22.0, 18.0, 18.0, Some(pal.yellow));
            rect(b, 4.0, -22.0, 18.0, 18.0, None);
            rect(b, -22.0, 4.0, 18.0, 18.0, None);
            rect(b, 4.0, 4.0, 18.0, 18.0, Some(pal.lavender));
        }
        Ic::Games => {
            let pts = crate::scene::rrect_pts(-26.0, -12.0, 52.0, 26.0, 13.0);
            b.poly(&pts, pal.lime);
            b.stroke(&pts, true, lw, ink);
            line(b, &[(-14.0, -2.0), (-14.0, 6.0)]);
            line(b, &[(-18.0, 2.0), (-10.0, 2.0)]);
            dot(b, 12.0, 0.0, 2.6, ink, false);
            dot(b, 17.0, 5.0, 2.6, ink, false);
        }
        Ic::Music => {
            rect(b, -24.0, -18.0, 48.0, 36.0, Some(pal.orange));
            for (x, h) in [(-14.0, 14.0), (-4.0, 8.0), (6.0, 18.0), (16.0, 4.0)] {
                line(b, &[(x, 6.0), (x, 6.0 - h)]);
            }
        }
        Ic::Quick => {
            line(b, &[(-22.0, -10.0), (22.0, -10.0)]);
            line(b, &[(-22.0, 10.0), (22.0, 10.0)]);
            dot(b, -8.0, -10.0, 6.0, pal.pink, true);
            dot(b, 10.0, 10.0, 6.0, pal.lime, true);
        }
        Ic::Power => {
            line(b, &[(0.0, -24.0), (0.0, -4.0)]);
            line(b, &arc(0.0, -1.72, 20.0, -134.4, -405.6));
        }
        Ic::Home => {
            let pts = [(-22.0, 4.0), (0.0, -16.0), (22.0, 4.0)];
            b.poly(&[(-14.0, -2.0), (-14.0, 20.0), (14.0, 20.0), (14.0, -2.0)], pal.lime);
            line(b, &pts);
            line(b, &[(-14.0, -2.0), (-14.0, 20.0), (14.0, 20.0), (14.0, -2.0)]);
        }
        Ic::M8 => {
            rect(b, -24.0, -16.0, 48.0, 32.0, Some(pal.yellow));
            rect(b, -18.0, -10.0, 22.0, 16.0, Some(pal.dark));
            for y in [-8.0, 0.0, 8.0] {
                line(b, &[(10.0, y), (16.0, y)]);
            }
        }
        Ic::Pico => {
            rect(b, -16.0, -24.0, 32.0, 48.0, Some(pal.pink));
            rect(b, -10.0, -14.0, 20.0, 16.0, Some(pal.dark));
        }
        Ic::Term => {
            rect(b, -24.0, -18.0, 48.0, 36.0, Some(pal.dark));
            b.stroke(&[(-14.0, -6.0), (-7.0, 0.0), (-14.0, 6.0)], false, lw, pal.lime);
            b.stroke(&[(0.0, 8.0), (12.0, 8.0)], false, lw, pal.lime);
        }
        Ic::Puzzle => {
            rect(b, -22.0, -22.0, 44.0, 44.0, None);
            rect(b, -6.0, -6.0, 12.0, 12.0, Some(pal.orange));
            line(b, &[(-22.0, -6.0), (-10.0, -6.0)]);
            line(b, &[(10.0, 6.0), (22.0, 6.0)]);
        }
        Ic::Sun => {
            dot(b, 0.0, 0.0, 9.0, pal.yellow, true);
            for k in 0..8 {
                let a = (k as f32 * 45.0).to_radians();
                let (c, s) = (a.cos(), a.sin());
                line(b, &[(c * 18.0, s * 18.0), (c * 24.0, s * 24.0)]);
            }
        }
        Ic::Vol => {
            let body = [(-20.0, -8.0), (-10.0, -8.0), (2.0, -18.0), (2.0, 18.0), (-10.0, 8.0), (-20.0, 8.0)];
            b.poly_concave(&body, pal.lime);
            b.stroke(&body, true, lw, ink);
            line(b, &arc(1.06, 0.0, 12.0, -41.8, 41.8));
            line(b, &arc(1.72, 0.0, 20.0, -44.4, 44.4));
        }
        Ic::Wifi => {
            line(b, &arc(0.0, 17.24, 32.0, -133.4, -46.6));
            line(b, &arc(0.0, 17.28, 20.0, -134.4, -45.6));
            line(b, &arc(0.0, 17.29, 8.0, -131.4, -48.6));
        }
        Ic::Bt => line(b, &[(-8.0, -12.0), (10.0, 4.0), (0.0, 14.0), (0.0, -18.0), (10.0, -8.0), (-8.0, 8.0)]),
        Ic::Keys => {
            rect(b, -24.0, -14.0, 48.0, 28.0, Some(DAY.ground));
            for (x, y) in [(-16.0, -6.0), (-6.0, -6.0), (4.0, -6.0), (14.0, -6.0)] {
                line(b, &[(x, y), (x + 3.0, y)]);
            }
            line(b, &[(-12.0, 6.0), (12.0, 6.0)]);
        }
        Ic::Clock => {
            dot(b, 0.0, 0.0, 22.0, pal.lavender, true);
            line(b, &[(0.0, -13.0), (0.0, 0.0), (10.0, 6.0)]);
        }
        Ic::Screen => {
            rect(b, -24.0, -18.0, 48.0, 30.0, Some(pal.lavender));
            dot(b, -6.0, -4.0, 6.0, pal.yellow, true);
            line(b, &[(-10.0, 22.0), (10.0, 22.0)]);
            line(b, &[(0.0, 12.0), (0.0, 22.0)]);
        }
        Ic::Link => {
            // two chain links
            let a = crate::scene::rrect_pts(-24.0, -9.0, 28.0, 18.0, 9.0);
            let c = crate::scene::rrect_pts(-4.0, -9.0, 28.0, 18.0, 9.0);
            b.poly(&a, pal.lime);
            b.stroke(&a, true, lw, ink);
            b.stroke(&c, true, lw, ink);
        }
        Ic::Update => {
            rect(b, -20.0, 10.0, 40.0, 10.0, Some(pal.lime));
            line(b, &[(0.0, -22.0), (0.0, 4.0)]);
            line(b, &[(-10.0, -6.0), (0.0, 4.0), (10.0, -6.0)]);
        }
        Ic::Info => {
            dot(b, 0.0, 0.0, 22.0, pal.yellow, true);
            line(b, &[(0.0, -2.0), (0.0, 12.0)]);
            dot(b, 0.0, -10.0, 2.6, ink, false);
        }
        Ic::Pad => {
            let pts = crate::scene::rrect_pts(-26.0, -14.0, 52.0, 28.0, 8.0);
            b.poly(&pts, pal.pink);
            b.stroke(&pts, true, lw, ink);
            line(b, &[(-15.0, -6.0), (-15.0, 6.0)]);
            line(b, &[(-21.0, 0.0), (-9.0, 0.0)]);
            dot(b, 10.0, -3.0, 3.0, ink, false);
            dot(b, 17.0, 4.0, 3.0, ink, false);
        }
        Ic::Claude => {}
    }
    b.pop();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What an unselected row draws (its tile and two text lines, with a
    /// little room), as (x0, y0, x1, y1).
    fn row_content(y: f32) -> [f32; 4] {
        [LISTX, y + ROWH / 2.0 - 32.0, LISTX + ITEM_W, y + ROWH / 2.0 + 32.0]
    }

    /// The selected category's icon, lifted (up 10, left 7, full size),
    /// with its hard shadow (9 px down and right).
    fn icon_with_shadow() -> [f32; 4] {
        let (x, y) = (COLX + 16.0 - 7.0, CATS_TOP + 10.0 - 10.0);
        [x, y, x + ICON + 9.0, y + ICON + 9.0]
    }

    #[test]
    fn row_above_clears_the_category_icon() {
        let icon = icon_with_shadow();
        for n in [2usize, 6, 10] {
            for s in [1, n / 2, n - 1] {
                // the row above the selection, the only one shown up there
                let above = row_content(slot_y(s - 1, s));
                assert!(above[3] + 16.0 <= icon[1], "row above (n {n}, s {s}) ends at {}, icon starts at {}", above[3], icon[1]);
                assert!(above[1] >= BAR_H, "under the status bar");
                assert_eq!(row_op(s - 1, s), 0.62);
                if s >= 2 {
                    assert_eq!(row_op(s - 2, s), 0.0, "only one row above");
                }
                // the selected band starts below the icon, its shadow and
                // the category names
                let names_bottom = CATS_TOP + 10.0 + ICON + 12.0 + 34.0;
                assert!(slot_y(s, s) + 4.0 >= icon[3].max(names_bottom), "band at {}", slot_y(s, s));
                // the third row below still ends above the hint bar
                assert!(slot_y(s + 3, s) + ROWH <= H - HINT_H, "row 3 below at {}", slot_y(s + 3, s));
            }
        }
    }
}
