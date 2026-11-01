// SPDX-License-Identifier: GPL-2.0-or-later
//! The Mortar device-UI toolkit: theme tokens and the building blocks of the
//! plan mockups (status bar, button-hint bar, menu items, settings rows,
//! toggles, pills, panels, titles, toasts) plus list-selection state.
//!
//! Sizes are in `cqw` (1% of screen width) like the mockup CSS, so every
//! component matches the drawings at 1024x768 and scales with the screen.

use crate::gfx::{Color, Face, Rect, TextStyle, Ui};
use crate::icons::Icon;

pub mod theme {
    use crate::gfx::Color;
    pub const BG: Color = Color::hex(0x0d1116);
    pub const PANEL: Color = Color::hex(0x19212a);
    pub const PANEL_2: Color = Color::hex(0x232d38);
    pub const LINE: Color = Color::hex(0x2f3b47);
    pub const FG: Color = Color::hex(0xe9eef1);
    pub const DIM: Color = Color::hex(0x8e9ba6);
    pub const AMBER: Color = Color::hex(0xf3b23c);
    pub const OK: Color = Color::hex(0x5fd08f);
}
use theme::*;

// ------------------------------------------------------------ styles ----

pub fn st_body(ui: &Ui, cq: f32, color: Color) -> TextStyle {
    TextStyle::new(Face::Body400, ui.cq(cq), color)
}
pub fn st_mono(ui: &Ui, cq: f32, color: Color) -> TextStyle {
    TextStyle::new(Face::Mono400, ui.cq(cq), color)
}

// -------------------------------------------------------- status bar ----

/// What the status bar shows. `left` is the clock once the time is known, or
/// "mortar" during setup.
#[derive(Clone, Debug, Default)]
pub struct StatusInfo {
    pub left: String,
    pub wifi: bool,
    pub battery: Option<u8>,
    pub ssh_sessions: u32,
    pub ssh_badge_always: bool,
    /// e.g. "USB 10.55.0.1" or "Hotspot on", drawn in amber.
    pub note: Option<String>,
}

/// Status bar height (2 x 1.6cqw padding + 2.8cqw icon).
pub fn status_bar_h(ui: &Ui) -> f32 {
    ui.cq(6.0)
}

pub fn status_bar(ui: &mut Ui, s: &StatusInfo) {
    let h = status_bar_h(ui);
    let cy = h / 2.0;
    let pad_x = ui.cq(3.0);
    let st = st_mono(ui, 2.0, DIM);
    ui.text_vcenter(pad_x, cy, &s.left, &st);

    // right cluster, laid out right-to-left with a 1.6cqw gap
    let gap = ui.cq(1.6);
    let icon = ui.cq(2.8);
    let mut x = ui.width() - pad_x;
    if let Some(b) = s.battery {
        let t = format!("{b}%");
        x -= ui.measure(&t, &st);
        ui.text_vcenter(x, cy, &t, &st);
        x -= gap + icon;
        ui.icon(Icon::Bat, x, cy - icon / 2.0, icon, DIM, 2.0, false);
        x -= gap;
    }
    if s.wifi {
        x -= icon;
        ui.icon(Icon::Wifi, x, cy - icon / 2.0, icon, DIM, 2.0, false);
        x -= gap;
    }
    if s.ssh_sessions > 0 || s.ssh_badge_always {
        let t = if s.ssh_sessions > 0 { format!("SSH · {}", s.ssh_sessions) } else { "SSH".into() };
        let bst = TextStyle::new(Face::Mono600, ui.cq(2.0), OK);
        let bw = ui.measure(&t, &bst) + ui.cq(1.6) + ui.cq(0.4);
        let bh = bst.line_px() + ui.cq(0.4);
        x -= bw;
        ui.stroke_rrect(Rect::new(x, cy - bh / 2.0, bw, bh), ui.cq(0.6), ui.cq(0.2), OK);
        ui.text_vcenter(x + ui.cq(1.0), cy, &t, &bst);
        x -= gap;
    }
    if let Some(n) = &s.note {
        let nst = st.color(AMBER);
        x -= ui.measure(n, &nst);
        ui.text_vcenter(x, cy, n, &nst);
    }
}

// ---------------------------------------------------------- hint bar ----

/// One entry of the bottom bar: a button cap ("A", "L1/R1", "MENU") and
/// what it does.
#[derive(Clone, Debug)]
pub struct Hint {
    pub button: String,
    pub label: String,
}

pub fn hint(button: &str, label: &str) -> Hint {
    Hint { button: button.into(), label: label.into() }
}

pub fn hint_bar_h(ui: &Ui) -> f32 {
    ui.cq(2.0 * 2.0 + 3.2 + 0.15)
}

/// A button cap: a circle for single letters, a pill for longer names.
/// Returns its width.
pub fn button_cap(ui: &mut Ui, x: f32, cy: f32, label: &str) -> f32 {
    let d = ui.cq(3.2);
    let st = TextStyle::new(Face::Mono600, ui.cq(1.8), FG);
    let tw = ui.measure(label, &st);
    if label.chars().count() <= 1 {
        ui.fill_circle(x + d / 2.0, cy, d / 2.0, PANEL_2);
        ui.text_vcenter(x + (d - tw) / 2.0, cy, label, &st);
        d
    } else {
        let w = tw + ui.cq(2.4);
        ui.fill_rrect(Rect::new(x, cy - d / 2.0, w, d), ui.cq(1.6), PANEL_2);
        ui.text_vcenter(x + ui.cq(1.2), cy, label, &st);
        w
    }
}

fn button_cap_w(ui: &Ui, label: &str) -> f32 {
    if label.chars().count() <= 1 {
        ui.cq(3.2)
    } else {
        ui.measure(label, &TextStyle::new(Face::Mono600, ui.cq(1.8), FG)) + ui.cq(2.4)
    }
}

pub fn hint_bar(ui: &mut Ui, hints: &[Hint]) {
    let h = hint_bar_h(ui);
    let top = ui.height() - h;
    ui.fill_rect(Rect::new(0.0, top, ui.width(), ui.cq(0.15)), LINE);
    let cy = top + ui.cq(0.15) + (h - ui.cq(0.15)) / 2.0;
    let st = st_body(ui, 2.1, DIM);
    let (gap_items, gap_in) = (ui.cq(3.5), ui.cq(1.0));
    let total: f32 = hints
        .iter()
        .map(|h| button_cap_w(ui, &h.button) + gap_in + ui.measure(&h.label, &st))
        .sum::<f32>()
        + gap_items * (hints.len().saturating_sub(1)) as f32;
    let mut x = ui.width() - ui.cq(3.5) - total;
    for h in hints {
        x += button_cap(ui, x, cy, &h.button) + gap_in;
        x += ui.text_vcenter(x, cy, &h.label, &st) + gap_items;
    }
}

/// Clear the screen, draw status bar and hint bar, return the body area
/// (already padded 1cqw top, 5cqw sides like `.body`).
pub fn frame(ui: &mut Ui, status: &StatusInfo, hints: &[Hint]) -> Rect {
    ui.clear(BG);
    status_bar(ui, status);
    hint_bar(ui, hints);
    let top = status_bar_h(ui) + ui.cq(1.0);
    let bottom = ui.height() - hint_bar_h(ui);
    Rect::new(ui.cq(5.0), top, ui.width() - ui.cq(10.0), bottom - top)
}

// ------------------------------------------------------------ titles ----

/// `.d-h`: big screen title. Returns the height used.
pub fn title_h1(ui: &mut Ui, x: f32, y: f32, text: &str) -> f32 {
    let st = TextStyle::new(Face::Display800, ui.cq(6.4), FG).tracking(-0.02).line_height(1.05);
    ui.text(x, y, text, &st);
    st.line_px()
}

/// `.d-h2`: section title.
pub fn title_h2(ui: &mut Ui, x: f32, y: f32, text: &str) -> f32 {
    let st = TextStyle::new(Face::Display700, ui.cq(4.2), FG).line_height(1.1);
    ui.text(x, y, text, &st);
    st.line_px()
}

/// `.crumb`: breadcrumb above a title.
pub fn crumb(ui: &mut Ui, x: f32, y: f32, text: &str) -> f32 {
    let st = st_mono(ui, 2.0, DIM);
    ui.text(x, y, text, &st);
    st.line_px()
}

/// `.lbl`: small uppercase tracked label.
pub fn label(ui: &mut Ui, x: f32, y: f32, text: &str) -> f32 {
    let st = st_mono(ui, 1.8, DIM).tracking(0.12);
    ui.text(x, y, &text.to_uppercase(), &st);
    st.line_px()
}

/// `.d-sub`: dim body text, wrapped to `max_w`. Returns the height used.
pub fn paragraph(ui: &mut Ui, x: f32, y: f32, max_w: f32, text: &str, st: &TextStyle) -> f32 {
    let mut lines: Vec<String> = Vec::new();
    for para in text.split('\n') {
        let mut cur = String::new();
        for word in para.split_whitespace() {
            let cand = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
            if ui.measure(&cand, st) > max_w && !cur.is_empty() {
                lines.push(std::mem::replace(&mut cur, word.to_string()));
            } else {
                cur = cand;
            }
        }
        lines.push(cur);
    }
    for (i, l) in lines.iter().enumerate() {
        ui.text(x, y + i as f32 * st.line_px(), l, st);
    }
    lines.len() as f32 * st.line_px()
}

// -------------------------------------------------------------- pill ----

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PillKind {
    Amber,
    Ok,
    Off,
}

pub fn pill_w(ui: &Ui, text: &str) -> f32 {
    ui.measure(text, &TextStyle::new(Face::Mono600, ui.cq(1.8), BG)) + ui.cq(2.4)
}

/// Draw a pill with its right edge at `right`. Returns its width.
pub fn pill(ui: &mut Ui, right: f32, cy: f32, text: &str, kind: PillKind) -> f32 {
    let (bg, fg) = match kind {
        PillKind::Amber => (AMBER, BG),
        PillKind::Ok => (OK, BG),
        PillKind::Off => (PANEL_2, DIM),
    };
    let st = TextStyle::new(Face::Mono600, ui.cq(1.8), fg);
    let w = pill_w(ui, text);
    let h = st.line_px() + ui.cq(0.6);
    ui.fill_rrect(Rect::new(right - w, cy - h / 2.0, w, h), h / 2.0, bg);
    ui.text_vcenter(right - w + ui.cq(1.2), cy, text, &st);
    w
}

// --------------------------------------------------------- menu item ----

#[derive(Clone, Debug)]
pub enum Glyph {
    None,
    Icon(Icon),
    /// A display-font initial (profiles), amber when selected.
    Letter(String),
}

/// `.mi`: a big selectable item with a glyph tile, title, description and
/// optional pill.
#[derive(Clone, Debug)]
pub struct MenuItem {
    pub glyph: Glyph,
    pub title: String,
    pub desc: Option<String>,
    pub pill: Option<(String, PillKind)>,
    pub selected: bool,
    pub dimmed: bool,
}

impl MenuItem {
    pub fn new(title: &str) -> MenuItem {
        MenuItem { glyph: Glyph::None, title: title.into(), desc: None, pill: None, selected: false, dimmed: false }
    }
}

pub fn menu_item_h(ui: &Ui, has_desc: bool) -> f32 {
    let tx = ui.cq(2.9 * 1.35) + if has_desc { ui.cq(2.2 * 1.35) } else { 0.0 };
    tx.max(ui.cq(5.2)) + ui.cq(2.0 * 1.9 + 2.0 * 0.25)
}

pub fn menu_item(ui: &mut Ui, x: f32, y: f32, w: f32, it: &MenuItem) -> f32 {
    let h = menu_item_h(ui, it.desc.is_some());
    let r = Rect::new(x, y, w, h);
    let rad = ui.cq(1.4);
    ui.fill_rrect(r, rad, if it.selected { PANEL_2 } else { PANEL });
    if it.selected {
        ui.stroke_rrect(r, rad, ui.cq(0.25), AMBER);
    }
    let pad = ui.cq(2.4 + 0.25);
    let cy = y + h / 2.0;
    let mut tx = x + pad;
    let g = ui.cq(5.2);
    match &it.glyph {
        Glyph::None => {}
        Glyph::Icon(icon) => {
            ui.fill_rrect(Rect::new(tx, cy - g / 2.0, g, g), ui.cq(1.0), BG);
            let s = ui.cq(3.2);
            ui.icon(*icon, tx + (g - s) / 2.0, cy - s / 2.0, s, if it.dimmed { DIM } else { AMBER }, 2.0, true);
            tx += g + ui.cq(2.4);
        }
        Glyph::Letter(l) => {
            ui.fill_rrect(Rect::new(tx, cy - g / 2.0, g, g), ui.cq(1.0), BG);
            let st = TextStyle::new(Face::Display800Small, ui.cq(3.0), if it.selected { AMBER } else { DIM });
            let lw = ui.measure(l, &st);
            ui.text_vcenter(tx + (g - lw) / 2.0, cy, l, &st);
            tx += g + ui.cq(2.4);
        }
    }
    let mut right = x + w - pad;
    if let Some((p, kind)) = &it.pill {
        right -= pill(ui, right, cy, p, *kind) + ui.cq(2.4);
    }
    let tst = TextStyle::new(Face::Body600, ui.cq(2.9), if it.dimmed { DIM } else { FG });
    let dst = st_body(ui, 2.2, DIM);
    let text_h = tst.line_px() + if it.desc.is_some() { dst.line_px() } else { 0.0 };
    let ty = cy - text_h / 2.0;
    ui.text_fit(tx, ty, &it.title, &tst, right - tx);
    if let Some(d) = &it.desc {
        ui.text_fit(tx, ty + tst.line_px(), d, &dst, right - tx);
    }
    h
}

/// A vertical stack of menu items `gap_cq` apart (`.menu` uses 1.2). Scrolls
/// so the selection stays visible within `max_h`.
pub fn menu(ui: &mut Ui, r: Rect, gap_cq: f32, items: &[MenuItem], list: &mut ListState) -> f32 {
    let (x, y, w, max_h) = (r.x, r.y, r.w, r.h);
    let gap = ui.cq(gap_cq);
    let hs: Vec<f32> = items.iter().map(|i| menu_item_h(ui, i.desc.is_some())).collect();
    list.fit(&hs, gap, max_h);
    let mut cy = y;
    for (i, it) in items.iter().enumerate().skip(list.scroll) {
        if cy + hs[i] > y + max_h + 0.5 {
            break;
        }
        cy += menu_item(ui, x, cy, w, it) + gap;
    }
    cy - y
}

// ------------------------------------------------------ settings row ----

#[derive(Clone, Debug)]
pub enum RowValue {
    None,
    Text(String),
    Toggle(bool),
}

/// `.row`: a settings/list row with title, optional description and a value
/// on the right (text, "›" link or toggle).
#[derive(Clone, Debug)]
pub struct Row {
    pub title: String,
    pub desc: Option<String>,
    pub value: RowValue,
    pub selected: bool,
}

impl Row {
    pub fn new(title: &str, value: RowValue) -> Row {
        Row { title: title.into(), desc: None, value, selected: false }
    }
}

pub fn row_h(ui: &Ui, has_desc: bool) -> f32 {
    let t = ui.cq(2.7 * 1.35) + if has_desc { ui.cq(2.0 * 1.35) } else { 0.0 };
    t.max(ui.cq(3.6)) + ui.cq(2.0 * 1.55 + 0.15)
}

pub fn toggle(ui: &mut Ui, x: f32, cy: f32, on: bool) {
    let (w, h) = (ui.cq(7.0), ui.cq(3.6));
    let bg = if on { OK.mix(PANEL_2, 0.4) } else { PANEL_2 };
    ui.fill_rrect(Rect::new(x, cy - h / 2.0, w, h), h / 2.0, bg);
    let k = ui.cq(2.6) / 2.0;
    let kx = if on { x + w - ui.cq(0.5) - k } else { x + ui.cq(0.5) + k };
    ui.fill_circle(kx, cy, k, if on { OK } else { DIM });
}

pub fn row(ui: &mut Ui, x: f32, y: f32, w: f32, r: &Row) -> f32 {
    let h = row_h(ui, r.desc.is_some());
    let rect = Rect::new(x, y, w, h);
    if r.selected {
        ui.fill_rrect(rect, ui.cq(1.0), PANEL_2);
        ui.stroke_rrect(rect, ui.cq(1.0), ui.cq(0.25), AMBER);
    } else {
        ui.fill_rect(Rect::new(x, y + h - ui.cq(0.15), w, ui.cq(0.15)), LINE);
    }
    let pad = ui.cq(2.4);
    let cy = y + (h - ui.cq(0.15)) / 2.0;
    let mut right = x + w - pad;
    match &r.value {
        RowValue::None => {}
        RowValue::Text(t) => {
            let st = st_mono(ui, 2.2, DIM);
            let tw = ui.measure(t, &st);
            ui.text_vcenter(right - tw, cy, t, &st);
            right -= tw + ui.cq(2.0);
        }
        RowValue::Toggle(on) => {
            toggle(ui, right - ui.cq(7.0), cy, *on);
            right -= ui.cq(9.0);
        }
    }
    let tst = st_body(ui, 2.7, FG);
    let dst = st_body(ui, 2.0, DIM);
    let th = tst.line_px() + if r.desc.is_some() { dst.line_px() } else { 0.0 };
    let ty = cy - th / 2.0;
    ui.text_fit(x + pad, ty, &r.title, &tst, right - x - pad);
    if let Some(d) = &r.desc {
        ui.text_fit(x + pad, ty + tst.line_px(), d, &dst, right - x - pad);
    }
    h
}

/// A `.set` list of rows, scrolled to keep the selection visible.
pub fn rows(ui: &mut Ui, r: Rect, items: &[Row], list: &mut ListState) -> f32 {
    let (x, y, w, max_h) = (r.x, r.y, r.w, r.h);
    let hs: Vec<f32> = items.iter().map(|r| row_h(ui, r.desc.is_some())).collect();
    list.fit(&hs, 0.0, max_h);
    let mut cy = y;
    for (i, r) in items.iter().enumerate().skip(list.scroll) {
        if cy + hs[i] > y + max_h + 0.5 {
            break;
        }
        cy += row(ui, x, cy, w, r);
    }
    cy - y
}

// ------------------------------------------------- keys, text field ----

/// `.k`: an on-screen key. `small` uses the dim 2cqw label of `.k.w`.
pub fn key_cap(ui: &mut Ui, r: Rect, label: &str, selected: bool, small: bool) {
    ui.fill_rrect(r, ui.cq(0.9), if selected { AMBER } else { PANEL });
    let st = if small {
        TextStyle::new(if selected { Face::Mono600 } else { Face::Mono400 }, ui.cq(2.0), if selected { BG } else { DIM })
    } else {
        TextStyle::new(if selected { Face::Mono600 } else { Face::Mono400 }, ui.cq(2.8), if selected { BG } else { FG })
    };
    let w = ui.measure(label, &st);
    ui.text_vcenter(r.x + (r.w - w) / 2.0, r.y + r.h / 2.0, label, &st);
}

/// `.field`: a focused text field (amber border) with a caret after `text`
/// and an optional dim hint on the right. Returns its height.
pub fn text_field(ui: &mut Ui, x: f32, y: f32, w: f32, text: &str, hint: Option<&str>) -> f32 {
    let st = st_mono(ui, 3.0, FG);
    let h = st.line_px() + ui.cq(2.8);
    let r = Rect::new(x, y, w, h);
    ui.fill_rrect(r, ui.cq(1.0), PANEL);
    ui.stroke_rrect(r, ui.cq(1.0), ui.cq(0.25), AMBER);
    let tx = x + ui.cq(2.0);
    let tw = ui.text_vcenter(tx, y + h / 2.0, text, &st);
    let ch = ui.cq(3.2);
    ui.fill_rect(Rect::new(tx + tw + ui.cq(0.3), y + (h - ch) / 2.0, ui.cq(0.3), ch), AMBER);
    if let Some(hn) = hint {
        let hs = st_body(ui, 2.1, DIM);
        let hw = ui.measure(hn, &hs);
        ui.text_vcenter(x + w - ui.cq(2.0) - hw, y + h / 2.0, hn, &hs);
    }
    h
}

// ------------------------------------------------------ panel, toast ----

pub fn panel(ui: &mut Ui, r: Rect) {
    ui.fill_rrect(r, ui.cq(1.4), PANEL);
}

/// A transient message centred above the hint bar.
pub fn toast(ui: &mut Ui, text: &str) {
    let st = st_body(ui, 2.4, FG);
    let w = ui.measure(text, &st) + ui.cq(5.0);
    let h = st.line_px() + ui.cq(2.4);
    let x = (ui.width() - w) / 2.0;
    let y = ui.height() - hint_bar_h(ui) - h - ui.cq(2.0);
    let r = Rect::new(x, y, w, h);
    ui.fill_rrect(r, ui.cq(1.2), PANEL_2);
    ui.stroke_rrect(r, ui.cq(1.2), ui.cq(0.2), AMBER);
    ui.text_vcenter(x + ui.cq(2.5), y + h / 2.0, text, &st);
}

// --------------------------------------------------------- list state ----

/// Selection + scroll offset for a vertical list.
#[derive(Clone, Debug, Default)]
pub struct ListState {
    pub sel: usize,
    pub scroll: usize,
}

impl ListState {
    pub fn up(&mut self, len: usize) {
        if len > 0 {
            self.sel = if self.sel == 0 { len - 1 } else { self.sel - 1 };
        }
    }
    pub fn down(&mut self, len: usize) {
        if len > 0 {
            self.sel = (self.sel + 1) % len;
        }
    }
    pub fn clamp(&mut self, len: usize) {
        if self.sel >= len {
            self.sel = len.saturating_sub(1);
        }
    }
    /// Adjust `scroll` so `sel` fits in `max_h` given item heights.
    fn fit(&mut self, hs: &[f32], gap: f32, max_h: f32) {
        if hs.is_empty() {
            *self = ListState::default();
            return;
        }
        self.clamp(hs.len());
        if self.sel < self.scroll {
            self.scroll = self.sel;
        }
        loop {
            let used: f32 = hs[self.scroll..=self.sel.min(hs.len().saturating_sub(1)).max(self.scroll)]
                .iter()
                .map(|h| h + gap)
                .sum::<f32>()
                - gap;
            if used <= max_h + 0.5 || self.scroll >= self.sel {
                break;
            }
            self.scroll += 1;
        }
    }
}
