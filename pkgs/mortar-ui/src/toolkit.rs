// SPDX-License-Identifier: GPL-2.0-or-later
//! The Mortar device-UI toolkit: the building blocks of the plan mockups
//! (status bar, button-hint bar, menu items, settings rows, toggles, pills,
//! tags, tiles, panels, titles, toasts, an isometric illustration) plus
//! list-selection state, drawn in the shared theme (`crate::theme`).
//!
//! Sizes are in `cqw` (1% of screen width) like the mockup CSS, so every
//! component matches the drawings at 1024x768 and scales with the screen.
//! Style v2: flat colour blocks, square corners, 1 px ink outlines, lime
//! selection blocks; round only for pills, toggles, dots and button caps.

use crate::gfx::{Color, Face, Rect, TextStyle, Ui};
use crate::icons::Icon;
use crate::theme::{self, Theme};

fn t() -> &'static Theme {
    theme::get()
}

/// The outline width in px for this screen (1 px at 1024 wide).
pub fn hair(ui: &Ui) -> f32 {
    (ui.width() / 1024.0 * theme::HAIRLINE).round().max(1.0)
}

// ------------------------------------------------------------ styles ----

pub fn st_body(ui: &Ui, cq: f32, color: Color) -> TextStyle {
    TextStyle::new(Face::Body400, ui.cq(cq), color)
}
pub fn st_mono(ui: &Ui, cq: f32, color: Color) -> TextStyle {
    TextStyle::new(Face::Mono400, ui.cq(cq), color)
}

/// A block with a 1 px ink outline: the basic shape of style v2.
pub fn block(ui: &mut Ui, r: Rect, fill: Color) {
    ui.fill_block(r, fill);
    let h = hair(ui);
    ui.outline(r, h, t().line);
}

// -------------------------------------------------------- tags, pills ----

/// Accent kinds for tags (square) and pills (round).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PillKind {
    /// Attention (yellow).
    Amber,
    /// Positive / primary (lime).
    Ok,
    /// Inactive (recessed ground).
    Off,
    /// Secondary accent (lavender).
    Lavender,
    /// Destructive (orange).
    Orange,
}

pub fn kind_colors(kind: PillKind) -> (Color, Color) {
    let t = t();
    match kind {
        PillKind::Amber => (t.yellow, t.yellow_ink),
        PillKind::Ok => (t.lime, t.lime_ink),
        PillKind::Off => (t.ground_2, t.line_soft),
        PillKind::Lavender => (t.lavender, t.lavender_ink),
        PillKind::Orange => (t.orange, t.orange_ink),
    }
}

fn tag_style(ui: &Ui, fg: Color) -> TextStyle {
    TextStyle::new(Face::Mono600, ui.cq(1.8), fg).line_height(1.25)
}

pub fn tag_w(ui: &Ui, text: &str) -> f32 {
    ui.measure(text, &tag_style(ui, t().line)) + ui.cq(1.6)
}

/// A tag: a tight solid rectangle with an ink outline and a mono label,
/// right edge at `right`. Returns its width.
pub fn tag(ui: &mut Ui, right: f32, cy: f32, text: &str, kind: PillKind) -> f32 {
    let (bg, fg) = kind_colors(kind);
    let st = tag_style(ui, fg);
    let w = tag_w(ui, text);
    let h = st.line_px() + ui.cq(0.5);
    block(ui, Rect::new(right - w, cy - h / 2.0, w, h), bg);
    ui.text_vcenter((right - w + ui.cq(0.8)).round(), cy, text, &st);
    w
}

pub fn pill_w(ui: &Ui, text: &str) -> f32 {
    ui.measure(text, &TextStyle::new(Face::Mono600, ui.cq(1.8), t().line)) + ui.cq(2.4)
}

/// A pill (fully round), right edge at `right`. Returns its width.
pub fn pill(ui: &mut Ui, right: f32, cy: f32, text: &str, kind: PillKind) -> f32 {
    let (bg, fg) = kind_colors(kind);
    let st = TextStyle::new(Face::Mono600, ui.cq(1.8), fg);
    let w = pill_w(ui, text);
    let h = st.line_px() + ui.cq(0.6);
    let hw = hair(ui);
    ui.pill_shape(Rect::new(right - w, cy - h / 2.0, w, h), bg, t().line, hw);
    ui.text_vcenter(right - w + ui.cq(1.2), cy, text, &st);
    w
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
    /// e.g. "USB 10.55.0.1" or "Hotspot on", drawn as a yellow tag.
    pub note: Option<String>,
}

/// Status bar height (2 x 1.6cqw padding + 2.8cqw icon).
pub fn status_bar_h(ui: &Ui) -> f32 {
    ui.cq(6.0)
}

pub fn status_bar(ui: &mut Ui, s: &StatusInfo) {
    let t = t();
    let h = status_bar_h(ui);
    let cy = h / 2.0;
    let pad_x = ui.cq(3.0);
    let hw = hair(ui);
    ui.fill_block(Rect::new(0.0, h - hw, ui.width(), hw), t.line);
    let clock = TextStyle::new(Face::SerifItalic, ui.cq(3.4), t.line).line_height(1.0);
    ui.text_vcenter(pad_x, cy, &s.left, &clock);

    // right cluster, laid out right-to-left with a 1.4cqw gap
    let st = st_mono(ui, 2.0, t.line);
    let gap = ui.cq(1.4);
    let icon = ui.cq(2.8);
    let mut x = ui.width() - pad_x;
    if let Some(b) = s.battery {
        let txt = format!("{b}%");
        if b <= 20 {
            x -= tag(ui, x, cy, &txt, PillKind::Amber);
        } else {
            x -= ui.measure(&txt, &st);
            ui.text_vcenter(x, cy, &txt, &st);
        }
        x -= ui.cq(0.8) + icon;
        ui.icon(Icon::Bat, x, cy - icon / 2.0, icon, t.line, 1.6, false);
        x -= gap;
    }
    if s.wifi {
        x -= icon;
        ui.icon(Icon::Wifi, x, cy - icon / 2.0, icon, t.line, 1.6, false);
        x -= gap;
    }
    if s.ssh_sessions > 0 || s.ssh_badge_always {
        let txt = if s.ssh_sessions > 0 { format!("SSH · {}", s.ssh_sessions) } else { "SSH".into() };
        x -= tag(ui, x, cy, &txt, PillKind::Lavender) + gap;
    }
    if let Some(n) = &s.note {
        tag(ui, x, cy, n, PillKind::Amber);
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

/// A button cap: an ink circle for single letters, an ink pill for longer
/// names (button glyphs stay round). Returns its width.
pub fn button_cap(ui: &mut Ui, x: f32, cy: f32, label: &str) -> f32 {
    let t = t();
    let d = ui.cq(3.2);
    let st = TextStyle::new(Face::Mono600, ui.cq(1.8), t.ground);
    let tw = ui.measure(label, &st);
    if label.chars().count() <= 1 {
        ui.fill_circle(x + d / 2.0, cy, d / 2.0, t.line);
        ui.text_vcenter(x + (d - tw) / 2.0, cy, label, &st);
        d
    } else {
        let w = tw + ui.cq(2.4);
        ui.fill_rrect(Rect::new(x, cy - d / 2.0, w, d), d / 2.0, t.line);
        ui.text_vcenter(x + ui.cq(1.2), cy, label, &st);
        w
    }
}

fn button_cap_w(ui: &Ui, label: &str) -> f32 {
    if label.chars().count() <= 1 {
        ui.cq(3.2)
    } else {
        ui.measure(label, &TextStyle::new(Face::Mono600, ui.cq(1.8), t().line)) + ui.cq(2.4)
    }
}

pub fn hint_bar(ui: &mut Ui, hints: &[Hint]) {
    let t = t();
    let h = hint_bar_h(ui);
    let top = ui.height() - h;
    let hw = hair(ui);
    ui.fill_block(Rect::new(0.0, top, ui.width(), hw), t.line);
    let cy = top + hw + (h - hw) / 2.0;
    let st = st_body(ui, 2.1, t.line_soft);
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
    ui.clear(t().ground);
    status_bar(ui, status);
    hint_bar(ui, hints);
    let top = status_bar_h(ui) + ui.cq(2.0);
    let bottom = ui.height() - hint_bar_h(ui);
    Rect::new(ui.cq(5.0), top, ui.width() - ui.cq(10.0), bottom - top)
}

// ------------------------------------------------------------ titles ----

/// `.d-h`: big screen title, serif italic. Returns the height used.
pub fn title_h1(ui: &mut Ui, x: f32, y: f32, text: &str) -> f32 {
    let st = TextStyle::new(Face::SerifItalic, ui.cq(7.4), t().line).tracking(-0.01).line_height(1.05);
    ui.text(x, y, text, &st);
    st.line_px()
}

/// `.d-h2`: section title, serif italic.
pub fn title_h2(ui: &mut Ui, x: f32, y: f32, text: &str) -> f32 {
    let st = TextStyle::new(Face::SerifItalic, ui.cq(5.2), t().line).line_height(1.1);
    ui.text(x, y, text, &st);
    st.line_px()
}

/// `.crumb`: breadcrumb above a title.
pub fn crumb(ui: &mut Ui, x: f32, y: f32, text: &str) -> f32 {
    let st = st_mono(ui, 1.9, t().line_soft);
    ui.text(x, y, text, &st);
    st.line_px()
}

/// `.lbl`: small uppercase tracked label.
pub fn label(ui: &mut Ui, x: f32, y: f32, text: &str) -> f32 {
    let st = st_mono(ui, 1.8, t().line_soft).tracking(0.12);
    ui.text(x, y, &text.to_uppercase(), &st);
    st.line_px()
}

/// `.d-sub`: body text, wrapped to `max_w`. Returns the height used.
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

// --------------------------------------------------------- menu item ----

#[derive(Clone, Debug)]
pub enum Glyph {
    None,
    Icon(Icon),
    /// A serif initial (profiles).
    Letter(String),
}

/// `.mi`: a big selectable block with a glyph tile, title, description and
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

/// The square glyph tile of a menu item: lavender next to a lime selection,
/// recessed ground otherwise.
fn glyph_tile(ui: &mut Ui, x: f32, cy: f32, glyph: &Glyph, selected: bool, dimmed: bool) -> f32 {
    let t = t();
    let g = ui.cq(5.2);
    let r = Rect::new(x, cy - g / 2.0, g, g);
    let ink = if dimmed { t.line_faint } else { t.line };
    match glyph {
        Glyph::None => return 0.0,
        Glyph::Icon(icon) => {
            block(ui, r, if selected { t.lavender } else { t.ground_2 });
            let s = ui.cq(3.2);
            let ic = if selected { t.lavender_ink } else { ink };
            ui.icon(*icon, x + (g - s) / 2.0, cy - s / 2.0, s, ic, 1.8, true);
        }
        Glyph::Letter(l) => {
            block(ui, r, if selected { t.lavender } else { t.ground_2 });
            let st = TextStyle::new(Face::SerifItalic, ui.cq(4.2), if selected { t.lavender_ink } else { ink }).line_height(1.0);
            let lw = ui.measure(l, &st);
            ui.text_vcenter(x + (g - lw) / 2.0, cy, l, &st);
        }
    }
    g + ui.cq(2.4)
}

pub fn menu_item(ui: &mut Ui, x: f32, y: f32, w: f32, it: &MenuItem) -> f32 {
    let t = t();
    let h = menu_item_h(ui, it.desc.is_some());
    let r = Rect::new(x, y, w, h);
    block(ui, r, if it.selected { t.lime } else { t.ground });
    let pad = ui.cq(2.4);
    let cy = y + h / 2.0;
    let tx = x + pad + glyph_tile(ui, x + pad, cy, &it.glyph, it.selected, it.dimmed);
    let mut right = x + w - pad;
    if let Some((p, kind)) = &it.pill {
        right -= pill(ui, right, cy, p, *kind) + ui.cq(2.4);
    }
    let (fg, dim) = if it.selected {
        (t.lime_ink, t.lime_ink)
    } else if it.dimmed {
        (t.line_faint, t.line_faint)
    } else {
        (t.line, t.line_soft)
    };
    let tst = TextStyle::new(Face::Body600, ui.cq(2.9), fg);
    let dst = st_body(ui, 2.2, dim);
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

// -------------------------------------------------------------- tile ----

/// `.tile` (home screen): a square-cornered block with a big line icon at
/// the top and a name + description at the bottom.
pub fn tile(ui: &mut Ui, r: Rect, icon: Icon, title: &str, desc: &str, selected: bool) {
    let t = t();
    block(ui, r, if selected { t.lime } else { t.ground });
    let pad = ui.cq(2.2);
    let s = ui.cq(6.0);
    let ink = if selected { t.lime_ink } else { t.line };
    ui.icon(icon, r.x + pad, r.y + pad, s, ink, 1.6, true);
    let tst = TextStyle::new(Face::SerifItalic, ui.cq(3.6), ink).line_height(1.1);
    let dst = st_body(ui, 1.9, if selected { t.lime_ink } else { t.line_soft });
    let by = r.bottom() - pad - dst.line_px();
    ui.text_fit(r.x + pad, by - tst.line_px(), title, &tst, r.w - 2.0 * pad);
    ui.text_fit(r.x + pad, by, desc, &dst, r.w - 2.0 * pad);
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
    let t = t();
    let (w, h) = (ui.cq(7.0), ui.cq(3.6));
    let hw = hair(ui);
    ui.pill_shape(Rect::new(x, cy - h / 2.0, w, h), if on { t.lime } else { t.ground_2 }, t.line, hw);
    let k = ui.cq(2.6) / 2.0;
    let kx = if on { x + w - ui.cq(0.5) - k } else { x + ui.cq(0.5) + k };
    ui.fill_circle(kx, cy, k, t.line);
}

pub fn row(ui: &mut Ui, x: f32, y: f32, w: f32, r: &Row) -> f32 {
    let t = t();
    let h = row_h(ui, r.desc.is_some());
    let rect = Rect::new(x, y, w, h);
    let hw = hair(ui);
    if r.selected {
        block(ui, rect, t.lime);
    } else {
        ui.fill_block(Rect::new(x, y + h - hw, w, hw), t.line_soft);
    }
    let (fg, dim) = if r.selected { (t.lime_ink, t.lime_ink) } else { (t.line, t.line_soft) };
    let pad = ui.cq(2.4);
    let cy = y + h / 2.0;
    let mut right = x + w - pad;
    match &r.value {
        RowValue::None => {}
        RowValue::Text(v) => {
            let st = st_mono(ui, 2.2, dim);
            let tw = ui.measure(v, &st);
            ui.text_vcenter(right - tw, cy, v, &st);
            right -= tw + ui.cq(2.0);
        }
        RowValue::Toggle(on) => {
            toggle(ui, right - ui.cq(7.0), cy, *on);
            right -= ui.cq(9.0);
        }
    }
    let tst = TextStyle::new(Face::Body500, ui.cq(2.7), fg);
    let dst = st_body(ui, 2.0, dim);
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

/// `.k`: an on-screen key, a square block. `small` uses the smaller dim
/// label of `.k.w` (del, OK, shift…).
pub fn key_cap(ui: &mut Ui, r: Rect, label: &str, selected: bool, small: bool) {
    let t = t();
    block(ui, r, if selected { t.lime } else { t.ground });
    let face = if selected { Face::Mono600 } else { Face::Mono400 };
    let st = if small {
        TextStyle::new(face, ui.cq(2.0), if selected { t.lime_ink } else { t.line_soft })
    } else {
        TextStyle::new(face, ui.cq(2.8), if selected { t.lime_ink } else { t.line })
    };
    let w = ui.measure(label, &st);
    ui.text_vcenter(r.x + (r.w - w) / 2.0, r.y + r.h / 2.0, label, &st);
}

/// `.field`: a focused text field (ink outline) with a caret after `text`
/// and an optional dim hint on the right. Returns its height.
pub fn text_field(ui: &mut Ui, x: f32, y: f32, w: f32, text: &str, hint: Option<&str>) -> f32 {
    let t = t();
    let st = st_mono(ui, 3.0, t.line);
    let h = st.line_px() + ui.cq(2.8);
    let r = Rect::new(x, y, w, h);
    block(ui, r, t.ground);
    let tx = x + ui.cq(2.0);
    let tw = ui.text_vcenter(tx, y + h / 2.0, text, &st);
    let ch = ui.cq(3.2);
    ui.fill_block(Rect::new(tx + tw + ui.cq(0.3), y + (h - ch) / 2.0, ui.cq(1.4), ch), t.lime);
    ui.outline(Rect::new(tx + tw + ui.cq(0.3), y + (h - ch) / 2.0, ui.cq(1.4), ch), hair(ui), t.line);
    if let Some(hn) = hint {
        let hs = st_mono(ui, 2.0, t.line_faint);
        let hw = ui.measure(hn, &hs);
        ui.text_vcenter(x + w - ui.cq(2.0) - hw, y + h / 2.0, hn, &hs);
    }
    h
}

// ------------------------------------------------------ panel, toast ----

/// A recessed panel.
pub fn panel(ui: &mut Ui, r: Rect) {
    block(ui, r, t().ground_2);
}

/// A transient message centred above the hint bar: a yellow block.
pub fn toast(ui: &mut Ui, text: &str) {
    let t = t();
    let st = TextStyle::new(Face::Body500, ui.cq(2.4), t.yellow_ink);
    let w = ui.measure(text, &st) + ui.cq(5.0);
    let h = st.line_px() + ui.cq(2.4);
    let x = (ui.width() - w) / 2.0;
    let y = ui.height() - hint_bar_h(ui) - h - ui.cq(2.0);
    block(ui, Rect::new(x, y, w, h), t.yellow);
    ui.text_vcenter(x + ui.cq(2.5), y + h / 2.0, text, &st);
}

// ----------------------------------------------------- illustration ----

/// An isometric line drawing for empty states: a stack of three blocks (1 px
/// ink, lime/lavender/yellow fills), centred on (cx, cy), `size` px wide.
pub fn iso_blocks(ui: &mut Ui, cx: f32, cy: f32, size: f32) {
    let t = t();
    let lw = hair(ui) * 1.5;
    // one isometric unit: x axis goes right-down, y axis left-down
    let u = size / 6.0;
    let (dx, dy) = (u * 0.866, u * 0.5);
    let p = |x: f32, y: f32, z: f32| (cx + (x - y) * dx, cy + (x + y) * dy - z * u);
    let cube = |ui: &mut Ui, x: f32, y: f32, z: f32, w: f32, d: f32, h: f32, top: Color| {
        let top_f = [p(x, y, z + h), p(x + w, y, z + h), p(x + w, y + d, z + h), p(x, y + d, z + h)];
        let left_f = [p(x, y + d, z + h), p(x + w, y + d, z + h), p(x + w, y + d, z), p(x, y + d, z)];
        let right_f = [p(x + w, y, z + h), p(x + w, y + d, z + h), p(x + w, y + d, z), p(x + w, y, z)];
        ui.fill_poly(&left_f, t.ground);
        ui.fill_poly(&right_f, t.ground_2);
        ui.fill_poly(&top_f, top);
        for f in [&top_f, &left_f, &right_f] {
            ui.stroke_poly(f, true, lw, t.line);
        }
    };
    cube(ui, -1.5, -1.5, -1.0, 3.0, 3.0, 1.0, t.lime);
    cube(ui, -1.0, -1.0, 0.0, 2.0, 2.0, 0.8, t.lavender);
    cube(ui, -0.6, -0.6, 0.8, 1.2, 1.2, 0.6, t.yellow);
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
