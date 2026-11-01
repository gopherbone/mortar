// SPDX-License-Identifier: GPL-2.0-or-later
//! The MENU: a cross (XMB-style) over the running app, after the approved
//! mockup (plan/mortar-cross.html). Categories run across (Running, Apps,
//! Games, Music, Quick, Power); the items of the selected one run down
//! beneath it. The app keeps running underneath, dimmed by a veil.
//!
//! A child module of `shellui`: it works on the `ShellUi` state directly.

use super::*;
use crate::config::Category;
use crate::radio::Radio;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cat {
    Running,
    Apps,
    Games,
    Music,
    Quick,
    Power,
}

pub const ALL_CATS: [Cat; 6] = [Cat::Running, Cat::Apps, Cat::Games, Cat::Music, Cat::Quick, Cat::Power];

impl Cat {
    pub fn name(self) -> &'static str {
        match self {
            Cat::Running => "Running",
            Cat::Apps => "Apps",
            Cat::Games => "Games",
            Cat::Music => "Music",
            Cat::Quick => "Quick",
            Cat::Power => "Power",
        }
    }
    fn i(self) -> usize {
        self as usize
    }
    fn icon(self) -> Ic {
        match self {
            Cat::Running => Ic::Running,
            Cat::Apps => Ic::Apps,
            Cat::Games => Ic::Games,
            Cat::Music => Ic::Music,
            Cat::Quick => Ic::Quick,
            Cat::Power => Ic::Power,
        }
    }
}

/// One item under a category.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Row {
    Home,
    App(u32),
    /// A launcher entry (by id).
    Launch(String),
    Brightness,
    Volume,
    Wifi,
    Bluetooth,
    Sleep,
    Keyboard,
    Settings,
    Restart,
    PowerOff,
}

impl Row {
    /// A level: A enters an adjust mode where ◀▶ change it.
    fn level(&self) -> bool {
        matches!(self, Row::Brightness | Row::Volume)
    }
    /// On or off: A switches it.
    fn toggle(&self) -> bool {
        matches!(self, Row::Wifi | Row::Bluetooth)
    }
    /// Needs a second A.
    fn warn(&self) -> bool {
        matches!(self, Row::Restart | Row::PowerOff)
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
struct Ctx {
    uid: u32,
    sel: usize,
    items: Vec<CtxItem>,
    op: Spring,
}

struct RowAnim {
    key: Row,
    y: Spring,
    op: Spring,
    sc: Spring,
}

pub(super) struct Cross {
    pub cat: Cat,
    /// Open (1) or closed (0): the veil and the menu fade with it.
    op: Spring,
    cat_x: Spring,
    cat_sc: [Spring; 6],
    rows: Vec<RowAnim>,
    /// The live preview (running apps, Home).
    pv: Spring,
    ctx: Option<Ctx>,
    /// A Power row waiting for its confirming A.
    confirm: Option<Row>,
    /// Apps closed with X, hidden until they are gone.
    hidden: Vec<u32>,
    /// A level row is being adjusted (◀▶ change it instead of category).
    adjusting: bool,
    pub closing: bool,
}

// layout (the mockup's 1024x768 stage)
const COLX: f32 = 200.0;
const CATW: f32 = 150.0;
const CATS_TOP: f32 = 150.0;
const ROW_X: f32 = COLX - 40.0;
const ROW_TOP: f32 = 330.0;
const ROWH: f32 = 84.0;
const ROW_W: f32 = 470.0;
const PV: [f32; 4] = [W - 44.0 - 380.0, 330.0, 380.0, 285.0];
const CTX_X: f32 = COLX + 420.0;
const CTX_Y: f32 = ROW_TOP - 10.0;

fn slot_y(j: usize, s: usize) -> f32 {
    let d = j as f32 - s as f32;
    if d == 0.0 {
        ROW_TOP
    } else if d > 0.0 {
        ROW_TOP + 40.0 + d * ROWH
    } else {
        ROW_TOP - 190.0 + d * 64.0
    }
}

fn row_op(j: usize, s: usize) -> f32 {
    if j < s || j > s + 4 { 0.0 } else { 1.0 }
}

/// The categories shown (Games and Music only when something is in them).
pub(super) fn cats(v: &View) -> Vec<Cat> {
    ALL_CATS
        .into_iter()
        .filter(|c| match c {
            Cat::Games => v.launcher.iter().any(|l| l.category == Category::Games),
            Cat::Music => v.launcher.iter().any(|l| l.category == Category::Music),
            _ => true,
        })
        .collect()
}

pub(super) fn rows(cat: Cat, v: &View, hidden: &[u32]) -> Vec<Row> {
    let launch = |f: &dyn Fn(&LauncherInfo) -> bool| v.launcher.iter().filter(|l| f(l)).map(|l| Row::Launch(l.id.clone())).collect();
    match cat {
        Cat::Running => std::iter::once(Row::Home).chain(v.running.iter().filter(|r| !hidden.contains(&r.uid)).map(|r| Row::App(r.uid))).collect(),
        Cat::Apps => launch(&|_| true),
        Cat::Games => launch(&|l| l.category == Category::Games),
        Cat::Music => launch(&|l| l.category == Category::Music),
        Cat::Quick => {
            let mut r = vec![Row::Brightness, Row::Volume, Row::Wifi, Row::Bluetooth, Row::Sleep];
            if v.keyboard_app {
                r.push(Row::Keyboard);
            }
            r.push(Row::Settings);
            r
        }
        Cat::Power => vec![Row::Sleep, Row::Restart, Row::PowerOff],
    }
}

/// Rows whose selection changes the live preview.
fn previews(r: Option<&Row>) -> bool {
    matches!(r, Some(Row::App(_) | Row::Home))
}

impl Cross {
    fn sel_row(&self, sel: usize) -> Option<&Row> {
        self.rows.get(sel).map(|r| &r.key)
    }

    #[cfg(test)]
    pub(super) fn sel_key(&self, sel: usize) -> Option<Row> {
        self.sel_row(sel).cloned()
    }

    /// Point every row's springs at its place for selection `s`.
    fn layout(&mut self, s: usize) {
        self.adjusting = false;
        for (j, r) in self.rows.iter_mut().enumerate() {
            r.y.set(slot_y(j, s));
            r.sc.set(if j == s { 1.0 } else { 0.86 });
            r.op.set(row_op(j, s));
        }
        let show = previews(self.rows.get(s).map(|r| &r.key));
        self.pv.set(if show { 1.0 } else { 0.0 });
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
                let sc = if j == s { 1.0 } else { 0.86 };
                if enter {
                    RowAnim { key, y: Spring::new(y + 30.0), op: Spring::new(0.0), sc: Spring::new(sc) }
                } else {
                    RowAnim { key, y: Spring::new(y), op: Spring::new(o), sc: Spring::new(sc) }
                }
            })
            .collect();
        self.layout(s);
    }

    pub(super) fn springs(&mut self) -> Vec<&mut Spring> {
        let mut v: Vec<&mut Spring> = vec![&mut self.op, &mut self.cat_x, &mut self.pv];
        v.extend(self.cat_sc.iter_mut());
        for r in &mut self.rows {
            v.extend([&mut r.y, &mut r.op, &mut r.sc]);
        }
        if let Some(c) = &mut self.ctx {
            v.push(&mut c.op);
        }
        v
    }

    pub(super) fn moving(&self) -> bool {
        self.closing
            || ![&self.op, &self.cat_x, &self.pv].iter().all(|s| s.rest())
            || !self.cat_sc.iter().all(|s| s.rest())
            || !self.rows.iter().all(|r| r.y.rest() && r.op.rest() && r.sc.rest())
            || self.ctx.as_ref().is_some_and(|c| !c.op.rest())
    }

    /// Fully faded out after closing.
    pub(super) fn gone(&self) -> bool {
        self.closing && self.op.x < 0.02
    }
}

impl ShellUi {
    fn sel_idx(&self, cat: Cat) -> usize {
        self.cross_sel[cat.i()]
    }

    /// MENU: open on Running with the previous app selected.
    pub(super) fn open_menu(&mut self, v: &View) {
        let keys = rows(Cat::Running, v, &[]);
        let pre = match v.current {
            Some(cur) => keys.iter().position(|r| matches!(r, Row::App(u) if *u != cur)).or_else(|| keys.iter().position(|r| *r == Row::App(cur))),
            None => keys.iter().position(|r| matches!(r, Row::App(_))),
        };
        self.cross_sel[Cat::Running.i()] = pre.unwrap_or(0);
        let mut op = Spring::new(0.0);
        op.set(1.0);
        let mut cat_x = Spring::new(60.0);
        cat_x.set(0.0);
        let cat_sc = std::array::from_fn(|i| Spring::new(if i == 0 { 1.0 } else { 0.72 }));
        let mut m = Cross { cat: Cat::Running, op, cat_x, cat_sc, rows: Vec::new(), pv: Spring::new(0.0), ctx: None, confirm: None, hidden: Vec::new(), adjusting: false, closing: false };
        let s = self.cross_sel[0];
        m.set_rows(keys, s, false);
        m.pv.snap(m.pv.t);
        self.menu = Some(m);
        self.confirm = None;
        self.fx(Sfx::Drawer, Some(Haptic::Tick));
    }

    /// Open the menu straight on a category (home shortcuts).
    pub(super) fn open_menu_on(&mut self, cat: Cat, v: &View) {
        self.open_menu(v);
        if cats(v).contains(&cat) {
            self.set_cat(cat, v, true);
        }
    }

    pub(super) fn close_menu(&mut self) {
        if let Some(m) = &mut self.menu {
            if m.closing {
                return;
            }
            m.closing = true;
            m.op.set(0.0);
            m.ctx = None;
        }
        self.fx(Sfx::DrawerUp, Some(Haptic::Tick));
    }

    fn set_cat(&mut self, cat: Cat, v: &View, snap: bool) {
        let s = self.sel_idx(cat);
        let Some(m) = &mut self.menu else { return };
        let list = cats(v);
        let idx = list.iter().position(|c| *c == cat).unwrap_or(0);
        m.cat = cat;
        m.confirm = None;
        m.ctx = None;
        let keys = rows(cat, v, &m.hidden);
        let s = s.min(keys.len().saturating_sub(1));
        self.cross_sel[cat.i()] = s;
        m.set_rows(keys, s, !snap);
        m.cat_x.set(-(idx as f32) * CATW);
        for c in ALL_CATS {
            m.cat_sc[c.i()].set(if c == cat { 1.0 } else { 0.72 });
        }
        if snap {
            m.cat_x.snap(m.cat_x.t);
            for s in m.springs() {
                s.snap(s.t);
            }
        }
    }

    /// Keep the rows in step with the apps (one closed, one started).
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
        m.hidden.retain(|u| v.app(*u).is_some());
        if !cats(v).contains(&m.cat) {
            m.cat = Cat::Apps;
        }
        let keys = rows(m.cat, v, &m.hidden);
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
    }

    pub(super) fn menu_button(&mut self, b: Button, repeat: bool, v: &View) {
        self.sync_menu(v);
        let Some(m) = &mut self.menu else { return };
        if m.closing {
            return;
        }
        if m.ctx.is_some() {
            self.ctx_button(b, repeat, v);
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
        match b {
            Button::B if !repeat => self.close_menu(),
            Button::Left | Button::Right | Button::L1 | Button::R1 => {
                if repeat && matches!(b, Button::L1 | Button::R1) {
                    return;
                }
                let list = cats(v);
                let i = list.iter().position(|c| *c == cat).unwrap_or(0);
                let n = match b {
                    Button::L1 => 0,
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
                    m.confirm = None;
                    m.layout(ns);
                    self.fx(Sfx::Tick, Some(Haptic::Tick));
                }
            }
            Button::X if !repeat => match row {
                Some(Row::App(uid)) => self.close_from_menu(uid, v),
                _ => self.fx(Sfx::Deny, None),
            },
            Button::Y if !repeat => match row {
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
                // the radio rows: their pages (networks, paired devices)
                Some(Row::Wifi | Row::Bluetooth) => {
                    self.out.push(Out::OpenSetup(if row == Some(Row::Wifi) { SetupPage::Wifi } else { SetupPage::Bluetooth }));
                    self.fx(Sfx::Open, Some(Haptic::Tick));
                }
                _ => self.fx(Sfx::Deny, None),
            },
            Button::A | Button::Start if !repeat => {
                if let Some(r) = row {
                    self.activate(r, v);
                }
            }
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

    /// ◀▶ while adjusting a level, A on a toggle.
    fn adjust(&mut self, row: Row, up: bool, v: &View) {
        let d = if up { 10 } else { -10 };
        match row {
            Row::Brightness if v.sys.brightness.is_some() => self.out.push(Out::Brightness(d)),
            Row::Volume if v.volume.is_some() => self.out.push(Out::Volume(d)),
            Row::Wifi | Row::Bluetooth => {
                let (r, on) = if row == Row::Wifi { (Radio::Wifi, v.radios.wifi) } else { (Radio::Bluetooth, v.radios.bluetooth) };
                match on {
                    Some(on) => {
                        self.out.push(Out::Radio(r, !on));
                        self.fx(Sfx::Move, Some(Haptic::Tick));
                    }
                    None => self.fx(Sfx::Deny, None),
                }
            }
            _ => self.fx(Sfx::Deny, None),
        }
    }

    fn activate(&mut self, row: Row, v: &View) {
        match row {
            Row::Home => {
                self.close_menu();
                if v.current.is_some() {
                    self.go_home(v);
                }
            }
            Row::App(uid) => self.switch_from_menu(uid, v),
            Row::Launch(id) => {
                let Some(l) = v.launcher.iter().find(|l| l.id == id) else { return };
                match v.by_entry(&id) {
                    Some(a) if l.single_instance => self.switch_from_menu(a.uid, v),
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
            Row::Wifi | Row::Bluetooth => self.adjust(row, true, v),
            Row::Sleep => {
                self.close_menu();
                self.out.push(Out::Sleep);
            }
            Row::Keyboard => {
                self.close_menu();
                self.out.push(Out::ShowKeyboard);
            }
            Row::Settings => self.open_settings(),
            Row::Restart | Row::PowerOff => {
                let Some(m) = &mut self.menu else { return };
                if m.confirm.as_ref() == Some(&row) {
                    m.confirm = None;
                    self.out.push(if row == Row::Restart { Out::Reboot } else { Out::PowerOff });
                    self.fx(Sfx::Close, Some(Haptic::Long));
                } else {
                    m.confirm = Some(row);
                    self.fx(Sfx::Deny, Some(Haptic::Double));
                }
            }
        }
    }

    /// Where the selected row's swatch is (the zoom starts there).
    fn sel_rect(&self) -> R4 {
        let Some(m) = &self.menu else { return R4 { x: ROW_X, y: ROW_TOP, w: 64.0, h: 64.0 } };
        let s = self.cross_sel[m.cat.i()];
        let (y, sc) = m.rows.get(s).map(|r| (r.y.x, r.sc.x)).unwrap_or((ROW_TOP, 1.0));
        let w = 64.0 * sc;
        R4 { x: ROW_X, y: y + (64.0 - w) / 2.0, w, h: w }
    }

    fn start_trans(&mut self, target: Target, name: String, art: Art, from: R4, prev: Option<u32>) {
        self.trans = Some(Trans::Launch { target, name, art, from, prev, t0: self.t });
        self.lp.snap(0.0);
        self.lp.set(1.0);
        self.fx(Sfx::Open, Some(Haptic::Double));
    }

    /// A on a running app: it zooms from its row to the full screen.
    fn switch_from_menu(&mut self, uid: u32, v: &View) {
        if v.current == Some(uid) {
            self.close_menu();
            return;
        }
        let from = self.sel_rect();
        let (name, art) = v.app(uid).map(|a| (a.name.clone(), a.art)).unwrap_or_default();
        self.close_menu();
        self.start_trans(Target::Uid(uid), name, art, from, v.current);
        self.out.push(Out::Focus(uid));
    }

    /// X: close the app; its row goes at once.
    fn close_from_menu(&mut self, uid: u32, v: &View) {
        let Some(m) = &mut self.menu else { return };
        m.hidden.push(uid);
        self.out.push(Out::Close(uid));
        self.fx(Sfx::Close, Some(Haptic::Double));
        let name = v.app(uid).map(|a| a.name.clone()).unwrap_or_default();
        self.show_toast(Toast { sub: name, sound: None, ..Toast::info("Closed") });
        self.sync_menu(v);
    }

    /// The app the preview shows live (its frame callbacks keep coming).
    pub fn live_preview(&self) -> Option<u32> {
        if self.page.is_some() || self.settings.is_some() || self.tz.is_some() {
            return None;
        }
        let m = self.menu.as_ref().filter(|m| !m.closing && m.cat == Cat::Running)?;
        match m.sel_row(self.cross_sel[Cat::Running.i()]) {
            Some(Row::App(u)) => Some(*u),
            _ => None,
        }
    }

    // --------------------------------------------------------- drawing ----

    pub(super) fn draw_menu(&self, b: &mut B, pal: &Pal, v: &View, m: &Cross) {
        let o = m.op.x.clamp(0.0, 1.0);
        b.rect(0.0, 0.0, W, H, with_a(pal.ground, o * 0.9));
        if o < 0.01 {
            return;
        }
        b.push(Xf::translate(0.0, (1.0 - o) * 18.0));
        b.push_alpha(o);
        let sel = self.cross_sel[m.cat.i()];
        self.draw_cats(b, pal, v, m);
        for (j, r) in m.rows.iter().enumerate() {
            let a = r.op.x.clamp(0.0, 1.0);
            if a < 0.01 || r.y.x > H {
                continue;
            }
            b.push(Xf::translate(ROW_X, r.y.x).mul(&Xf::about(0.0, 32.0, Xf::scale(r.sc.x, r.sc.x))));
            b.push_alpha(a);
            self.draw_row(b, pal, v, m, &r.key, j == sel);
            b.pop_alpha();
            b.pop();
        }
        self.draw_preview(b, pal, v, m);
        if let Some(c) = &m.ctx {
            draw_ctx(b, pal, c);
        }
        draw_status(b, pal, v);
        let row = m.sel_row(sel);
        hints(b, pal, H - 64.0, &menu_hints(row, v, m.adjusting));
        b.pop_alpha();
        b.pop();
    }

    fn draw_cats(&self, b: &mut B, pal: &Pal, v: &View, m: &Cross) {
        let list = cats(v);
        let cur = list.iter().position(|c| *c == m.cat).unwrap_or(0) as i32;
        let name_st = ts(Face::SerifItalic, 30.0, pal.ink).line_height(1.0);
        for (i, c) in list.iter().enumerate() {
            let dist = (i as i32 - cur).unsigned_abs() as f32;
            if dist > 3.0 {
                continue;
            }
            let x = COLX + i as f32 * CATW + m.cat_x.x - 60.0;
            let sc = m.cat_sc[c.i()].x;
            let on = *c == m.cat;
            b.push(Xf::about(x + 60.0, CATS_TOP + 68.0, Xf::scale(sc, sc)));
            b.push_alpha(1.0 - dist * 0.18);
            let (bg, line) = if on { (pal.lime, pal.lime_ink) } else { (pal.ground, pal.ink) };
            b.block(x + 12.0, CATS_TOP, 96.0, 96.0, bg, line);
            draw_icon(b, c.icon(), x + 60.0, CATS_TOP + 48.0, 58.0, pal, pal.ink);
            b.text_center(x + 60.0, CATS_TOP + 106.0, c.name(), &name_st);
            if *c == Cat::Running {
                let n = v.running.iter().filter(|r| !m.hidden.contains(&r.uid)).count();
                chip(b, x + 118.0, CATS_TOP - 10.0 + 9.8, &n.to_string(), 13.0, (6.0, 1.0), pal.lavender, pal.lavender_ink);
            }
            b.pop_alpha();
            b.pop();
        }
    }

    /// One item in its own space: the 64 px swatch at (0, 0), text to its
    /// right, a value chip at the right end.
    fn draw_row(&self, b: &mut B, pal: &Pal, v: &View, m: &Cross, row: &Row, on: bool) {
        let confirm = on && m.confirm.as_ref() == Some(row);
        let info = row_info(row, v);
        let (bg, line) = if confirm {
            (pal.orange, pal.dark)
        } else if on {
            (pal.lime, pal.lime_ink)
        } else {
            (pal.ground, pal.ink)
        };
        b.block(0.0, 0.0, 64.0, 64.0, bg, line);
        let ic_ink = if confirm { pal.dark } else { pal.ink };
        draw_icon(b, info.icon, 32.0, 32.0, 40.0, pal, ic_ink);
        let mut right = ROW_W;
        let adjusting = on && m.adjusting;
        match (&info.value, info.level) {
            (Some(_), Some(lv)) if adjusting => {
                // the chip becomes a short slider: a yellow bar on a track
                let (tw, th) = (120.0, 18.0);
                let x = ROW_W - tw;
                b.block(x, 32.0 - th / 2.0, tw, th, pal.ground, pal.ink);
                let fw = (tw - 2.0) * (lv.min(100) as f32 / 100.0);
                b.rect(x + 1.0, 32.0 - th / 2.0 + 1.0, fw, th - 2.0, pal.yellow);
                b.rect(x + 1.0 + fw, 32.0 - th / 2.0 + 1.0, 1.0, th - 2.0, pal.ink);
                let st = ts(Face::Mono600, 17.0, pal.ink);
                let w = b.text_right(x - 10.0, 32.0 - st.line_px() / 2.0, &format!("{lv}%"), &st);
                right -= tw + 10.0 + w + 12.0;
            }
            (Some(val), _) => {
                let (fill, ink) = if on { (pal.yellow, pal.dark) } else { (pal.ground, pal.ink) };
                right -= chip(b, ROW_W, 32.0, val, 17.0, (10.0, 3.0), fill, ink) + 12.0;
            }
            _ => {}
        }
        if previews(Some(row)) {
            // keep clear of the live preview
            right = right.min(PV[0] - ROW_X - 16.0);
        }
        let title = if on { ts(Face::SerifItalic, 40.0, pal.ink).line_height(1.0) } else { ts(Face::Body600, 26.0, pal.ink) };
        let sub = ts(Face::Mono400, 15.0, pal.soft);
        let top = 32.0 - (title.line_px() + 2.0 + sub.line_px()) / 2.0;
        let live_w = if info.live { b.measure("ON SCREEN", &ts(Face::Mono400, 12.0, pal.ink)) + 14.0 + 6.0 } else { 0.0 };
        let t = b.fit(&info.title, &title, right - 80.0 - live_w);
        let tw = b.text(80.0, top, &t, &title);
        if info.live {
            chip(b, 80.0 + tw + live_w, top + title.line_px() / 2.0 - 1.5, "ON SCREEN", 12.0, (6.0, 1.0), pal.lavender, pal.lavender_ink);
        }
        let s = if confirm {
            format!("Press A again to {}", info.title.to_lowercase())
        } else if adjusting {
            "◀▶ to change".to_string()
        } else {
            info.sub
        };
        let s = b.fit(&s, &sub, right - 80.0);
        arrow_text(b, 80.0, top + title.line_px() + 2.0, &s, &sub, true);
    }

    fn draw_preview(&self, b: &mut B, pal: &Pal, v: &View, m: &Cross) {
        let a = m.pv.x.clamp(0.0, 1.0);
        if a < 0.01 {
            return;
        }
        let row = m.sel_row(self.cross_sel[m.cat.i()]);
        let [x, y, w, h] = PV;
        b.push(Xf::translate((1.0 - m.pv.x) * 30.0, 0.0));
        b.push_alpha(a);
        let (left, right) = match row {
            Some(Row::App(uid)) => match v.app(*uid) {
                Some(app) => {
                    b.rect(x, y, w, h, Color(0, 0, 0, 255));
                    if app.has_buffer {
                        let (sx, sy) = (w / W, h / H);
                        let g = app.geom;
                        b.image(Src::App(*uid), x + g[0] * sx, y + g[1] * sy, g[2] * sx, g[3] * sy, [0.0, 0.0, 1.0, 1.0]);
                    } else {
                        b.rect(x, y, w, h, app.art.accent(pal));
                        art::draw(b, app.art, x + w / 2.0, y + h / 2.0, 0.9, *pal, pal.dark);
                    }
                    let state = if app.closing {
                        "Closing…"
                    } else if !app.has_buffer {
                        "Starting…"
                    } else {
                        ""
                    };
                    let where_ = if v.current == Some(*uid) { "on screen" } else { "in the background" };
                    (format!("{} · {where_}", app.name), state.to_string())
                }
                None => (String::new(), String::new()),
            },
            _ => {
                // Home: a small home screen
                b.rect(x, y, w, h, pal.lime);
                let st = ts(Face::SerifItalic, 90.0, pal.lime_ink);
                b.text_base(x + 32.0, y + 140.0, &v.sys.time, &st);
                for k in 0..4 {
                    let (rx, ry) = (x + 32.0 + k as f32 * 82.0, y + 178.0);
                    b.stroke(&[(rx, ry), (rx + 68.0, ry), (rx + 68.0, ry + 80.0), (rx, ry + 80.0)], true, 1.5, pal.lime_ink);
                }
                ("Home".to_string(), String::new())
            }
        };
        b.frame(x, y, w, h, 1.0, pal.ink);
        let cap = ts(Face::Mono400, 14.0, pal.soft);
        let rw = if right.is_empty() { 0.0 } else { b.text_right(x + w, y + h + 8.0, &right, &cap) + 12.0 };
        let l = b.fit(&left, &cap, w - rw);
        b.text(x, y + h + 8.0, &l, &cap);
        b.pop_alpha();
        b.pop();
    }
}

struct RowInfo {
    title: String,
    sub: String,
    icon: Ic,
    value: Option<String>,
    /// Level rows: the value as a number (the slider).
    level: Option<u8>,
    live: bool,
}

fn row_info(row: &Row, v: &View) -> RowInfo {
    let ri = |title: &str, sub: String, icon: Ic| RowInfo { title: title.to_string(), sub, icon, value: None, level: None, live: false };
    let pct = |x: Option<u8>| Some(x.map(|x| format!("{x}%")).unwrap_or_else(|| "n/a".into()));
    let toggle = |x: Option<bool>| {
        Some(match x {
            Some(true) => "On".to_string(),
            Some(false) => "Off".to_string(),
            None => "n/a".to_string(),
        })
    };
    match row {
        Row::Home => ri("Home", "Tiles and clock".into(), Ic::Home),
        Row::App(uid) => match v.app(*uid) {
            Some(a) => {
                let sub = if a.closing {
                    "Closing…"
                } else if !a.has_buffer {
                    "Starting…"
                } else if v.current == Some(*uid) {
                    "On screen"
                } else {
                    "In the background"
                };
                RowInfo { live: v.current == Some(*uid), ..ri(&a.name, sub.into(), Ic::art(a.art)) }
            }
            None => ri("", String::new(), Ic::Apps),
        },
        Row::Launch(id) => match v.launcher.iter().find(|l| l.id == *id) {
            Some(l) => {
                let sub = if l.running && l.single_instance { format!("{} · running", l.description) } else { l.description.clone() };
                ri(&l.name, sub.trim_start_matches(" · ").to_string(), Ic::art(l.art))
            }
            None => ri(id, String::new(), Ic::Apps),
        },
        Row::Brightness => RowInfo { value: pct(v.sys.brightness), level: v.sys.brightness, ..ri("Brightness", "A to adjust".into(), Ic::Sun) },
        Row::Volume => RowInfo { value: pct(v.volume), level: v.volume, ..ri("Volume", "A to adjust".into(), Ic::Vol) },
        Row::Wifi => {
            let sub = match v.radios.wifi {
                None => "No Wi-Fi switch",
                Some(false) => "Radio off",
                Some(true) if v.sys.wifi => "Connected",
                Some(true) => "Not connected · networks in Settings…",
            };
            RowInfo { value: toggle(v.radios.wifi), ..ri("Wi-Fi", sub.into(), Ic::Wifi) }
        }
        Row::Bluetooth => {
            let sub = match v.radios.bluetooth {
                None => "No Bluetooth switch",
                Some(false) => "Radio off",
                Some(true) => "Y: devices, pairing",
            };
            RowInfo { value: toggle(v.radios.bluetooth), ..ri("Bluetooth", sub.into(), Ic::Bt) }
        }
        Row::Sleep => ri("Sleep", "Screen off, apps keep their place".into(), Ic::Moon),
        Row::Keyboard => ri("Keyboard", "On-screen keys for this app".into(), Ic::Keys),
        Row::Settings => ri("Settings…", "Sounds, motion, palette, time zone".into(), Ic::Quick),
        Row::Restart => ri("Restart", "Apps close".into(), Ic::Restart),
        Row::PowerOff => ri("Power off", "Apps close".into(), Ic::Power),
    }
}

fn menu_hints(row: Option<&Row>, v: &View, adjusting: bool) -> Vec<(&'static str, &'static str)> {
    if adjusting {
        return vec![("◀▶", "Adjust"), ("A/B", "Done"), ("MENU", "Close")];
    }
    let mut h = vec![("◀▶", "Category"), ("▲▼", "Choose")];
    match row {
        Some(Row::App(_)) => h.extend([("X", "Close"), ("Y", "Options"), ("A", "Switch")]),
        Some(Row::Home) => h.push(("A", "Go home")),
        Some(r) if r.level() => h.push(("A", "Adjust")),
        Some(r) if r.toggle() => {
            let on = if *r == Row::Wifi { v.radios.wifi } else { v.radios.bluetooth };
            h.push(("Y", if *r == Row::Wifi { "Networks" } else { "Devices" }));
            h.push(("A", if on == Some(true) { "Turn off" } else { "Turn on" }));
        }
        Some(Row::Launch(id)) => {
            let running = v.launcher.iter().find(|l| l.id == *id).is_some_and(|l| l.running && l.single_instance);
            h.push(("A", if running { "Switch" } else { "Open" }));
        }
        Some(Row::Sleep) => h.push(("A", "Sleep")),
        Some(r) if r.warn() => h.push(("A", "Choose")),
        Some(_) => h.push(("A", "Open")),
        None => {}
    }
    h.push(("MENU", "Close"));
    h
}

fn draw_status(b: &mut B, pal: &Pal, v: &View) {
    let clock = ts(Face::SerifItalic, 34.0, pal.ink).line_height(1.0);
    b.text(30.0, 28.0 - clock.line_px() / 2.0, &v.sys.time, &clock);
    let sb = ts(Face::Mono400, 18.0, pal.soft);
    let mut right = W - 30.0;
    let mut net = Vec::new();
    if v.sys.wifi {
        net.push("Wi-Fi".to_string());
    }
    if v.sys.usb_net {
        net.push("USB".to_string());
    }
    right = battery_status(b, pal, v, right, &mut net);
    let net = net.join(" · ");
    if !net.is_empty() {
        right -= b.text_right(right, 28.0 - sb.line_px() / 2.0, &net, &sb) + 10.0;
    }
    if v.sys.ssh_sessions > 0 {
        right -= chip(b, right, 28.0, "SSH", 15.0, (9.0, 2.0), pal.lime, pal.lime_ink) + 10.0;
    }
    chip(b, right, 28.0, &format!("{} running", v.running.len()), 15.0, (9.0, 2.0), pal.lavender, pal.lavender_ink);
}

fn draw_ctx(b: &mut B, pal: &Pal, c: &Ctx) {
    let a = c.op.x.clamp(0.0, 1.0);
    let rh = 56.0;
    let (x, y, w) = (CTX_X - (1.0 - c.op.x) * 20.0, CTX_Y, 300.0);
    b.push_alpha(a);
    let h = rh * c.items.len() as f32;
    b.rect(x, y, w, h, pal.ground);
    let st_fg = |col| ts(Face::Body400, 22.0, col);
    for (i, it) in c.items.iter().enumerate() {
        let ry = y + i as f32 * rh;
        let on = i == c.sel;
        let warn = *it == CtxItem::Close;
        let fg = if on && warn {
            pal.dark
        } else if on {
            pal.lime_ink
        } else {
            pal.ink
        };
        if on {
            b.rect(x, ry, w, rh, if warn { pal.orange } else { pal.lime });
        }
        if i > 0 {
            b.rect(x, ry, w, 1.0, pal.faint);
        }
        let st = st_fg(fg);
        b.text(x + 18.0, ry + rh / 2.0 - st.line_px() / 2.0, it.name(), &st);
    }
    b.frame(x, y, w, h, 1.0, pal.ink);
    b.pop_alpha();
}

// ------------------------------------------------------------- icons ----

/// Flat line icons (1 px ink style, after the mockup's SVGs on a 60-unit
/// grid centred on 0,0).
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
    Moon,
    Restart,
    Keys,
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
    let lw = 2.4;
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
            rect(b, -8.0, -6.0, 30.0, 24.0, Some(pal.ground));
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
            rect(b, -24.0, -18.0, 48.0, 36.0, Some(pal.yellow));
            for (x, h) in [(-14.0, 14.0), (-4.0, 8.0), (6.0, 18.0), (16.0, 4.0)] {
                line(b, &[(x, 6.0), (x, 6.0 - h)]);
            }
        }
        Ic::Quick => {
            line(b, &[(-22.0, -10.0), (22.0, -10.0)]);
            line(b, &[(-22.0, 10.0), (22.0, 10.0)]);
            dot(b, -8.0, -10.0, 6.0, pal.lime, true);
            dot(b, 10.0, 10.0, 6.0, pal.lavender, true);
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
        Ic::Moon => {
            // a crescent: the outer circle minus an inner one
            let mut pts = arc(1.32, 0.85, 20.0, -70.5, -339.1);
            pts.extend(arc(20.48, -7.99, 16.0, 91.7, 218.7));
            b.poly_concave(&pts, pal.lavender);
            b.stroke(&pts, true, lw, ink);
        }
        Ic::Restart => {
            line(b, &arc(0.22, 3.12, 20.0, -27.2, -351.7));
            line(b, &[(20.0, -20.0), (20.0, -6.0), (6.0, -6.0)]);
        }
        Ic::Keys => {
            rect(b, -24.0, -14.0, 48.0, 28.0, Some(pal.ground));
            for (x, y) in [(-16.0, -6.0), (-6.0, -6.0), (4.0, -6.0), (14.0, -6.0)] {
                line(b, &[(x, y), (x + 3.0, y)]);
            }
            line(b, &[(-12.0, 6.0), (12.0, 6.0)]);
        }
        Ic::Claude => {}
    }
    b.pop();
}
