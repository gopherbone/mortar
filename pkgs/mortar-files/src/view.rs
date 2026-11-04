// SPDX-License-Identifier: GPL-2.0-or-later
//! Drawing. One function per screen, all from `App`'s state; nothing here
//! changes what the app is doing. Solid blocks on the ground, a band for what's
//! selected, the panel beside it in the same colour.

use crate::app::*;
use crate::listing::{Entry, Kind, human_size, plural};
use crate::mosaic::{self, DARK, Glyph, Motif, PINK, glyph, kind_colors};
use crate::ops::{self, Choice, JobKind};
use crate::places::{Place, PlaceKind};
use crate::preview::Preview;
use crate::storage::TileKind;
use mortar_ui::gfx::{Color, Face, Rect, TextStyle, Ui};
use mortar_ui::theme;
use resvg::tiny_skia::{Pixmap, PixmapPaint, Transform};
use std::os::unix::ffi::OsStrExt;

/// The panel: its left edge, and the square of art in it.
pub const PANEL_X: f32 = 664.0;
pub const ART: u32 = 320;
const ART_X: f32 = 684.0;
const ART_Y: f32 = 20.0;
/// The band, fixed: the list rolls under it.
const BAND_Y: f32 = 228.0;
const BAND_H: f32 = 96.0;
const ROW_H: f32 = 60.0;
/// The band's hard shadow, and the gap under it.
const BAND_SHADOW: f32 = 10.0;
const BAND_GAP: f32 = 14.0;
const TITLE_H: f32 = 92.0;
const LIST_BOTTOM: f32 = H - HINT_H;
/// The text viewer.
pub const TEXT_COLS: usize = 58;
pub const TEXT_ROWS: usize = 15;
const TEXT_LINE: f32 = 38.0;

fn serif(size: f32, c: Color) -> TextStyle {
    TextStyle::new(Face::SerifItalic, size, c).line_height(1.1)
}
fn body(size: f32, c: Color) -> TextStyle {
    TextStyle::new(Face::Body600, size, c)
}
fn regular(size: f32, c: Color) -> TextStyle {
    TextStyle::new(Face::Body500, size, c)
}
fn mono(size: f32, c: Color) -> TextStyle {
    TextStyle::new(Face::Mono400, size, c)
}

// --------------------------------------------------------------- helpers ----

/// Shorten from the middle, keeping an extension: game titles differ at the
/// end as often as at the start.
pub fn fit_name(ui: &Ui, s: &str, st: &TextStyle, max_w: f32) -> String {
    if ui.measure(s, st) <= max_w {
        return s.to_string();
    }
    let c: Vec<char> = s.chars().collect();
    let tail = c.len().min(match s.rfind('.') {
        Some(i) if s.len() - i <= 8 => s[i..].chars().count() + 3,
        _ => 6,
    });
    let suffix: String = c[c.len() - tail..].iter().collect();
    let (mut lo, mut hi) = (0usize, c.len() - tail);
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let cand: String = c[..mid].iter().collect::<String>() + "…" + &suffix;
        if ui.measure(&cand, st) <= max_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let out: String = c[..lo].iter().collect::<String>() + "…" + &suffix;
    if ui.measure(&out, st) <= max_w { out } else { "…".into() }
}

/// Word-wrap to lines, breaking inside a word only if it must.
pub fn wrap(ui: &Ui, s: &str, st: &TextStyle, max_w: f32, max_lines: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    let push_word = |cur: &mut String, lines: &mut Vec<String>, word: &str| {
        let cand = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
        if ui.measure(&cand, st) <= max_w {
            *cur = cand;
            return;
        }
        if !cur.is_empty() {
            lines.push(std::mem::take(cur));
        }
        let mut w = String::new();
        for ch in word.chars() {
            w.push(ch);
            if ui.measure(&w, st) > max_w {
                w.pop();
                lines.push(std::mem::take(&mut w));
                w.push(ch);
            }
        }
        *cur = w;
    };
    for word in s.split(' ') {
        push_word(&mut cur, &mut lines, word);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        if let Some(l) = lines.last_mut() {
            *l = fit_name(ui, &format!("{l}…"), st, max_w);
            if !l.ends_with('…') {
                l.push('…');
            }
        }
    }
    lines
}

fn triangle(ui: &mut Ui, cx: f32, cy: f32, s: f32, dir: char, c: Color) {
    let pts = match dir {
        '◀' => [(cx - s, cy), (cx + s * 0.7, cy - s), (cx + s * 0.7, cy + s)],
        '▶' => [(cx + s, cy), (cx - s * 0.7, cy - s), (cx - s * 0.7, cy + s)],
        '▲' => [(cx, cy - s), (cx - s, cy + s * 0.7), (cx + s, cy + s * 0.7)],
        _ => [(cx, cy + s), (cx - s, cy - s * 0.7), (cx + s, cy - s * 0.7)],
    };
    ui.fill_poly(&pts, c);
}

/// The shell's button cap: a circle for one letter, a pill for more. Arrows
/// are drawn, since the fonts don't all have them.
fn cap(ui: &mut Ui, x: f32, cy: f32, label: &str, ink: Color, on: Color) -> f32 {
    let d = 38.0;
    let arrows = label.chars().all(|c| "◀▶▲▼".contains(c));
    let st = TextStyle::new(Face::Mono600, 19.0, on);
    let w = if arrows {
        let n = label.chars().count() as f32;
        if n <= 1.0 { d } else { d + (n - 1.0) * 17.0 }
    } else if label.chars().count() <= 1 {
        d
    } else {
        ui.measure(label, &st) + 26.0
    };
    ui.fill_rrect(Rect::new(x, cy - d / 2.0, w, d), d / 2.0, ink);
    if arrows {
        let n = label.chars().count() as f32;
        for (i, ch) in label.chars().enumerate() {
            triangle(ui, x + w / 2.0 + (i as f32 - (n - 1.0) / 2.0) * 16.0, cy, 6.5, ch, on);
        }
    } else {
        let tw = ui.measure(label, &st);
        ui.text_vcenter(x + (w - tw) / 2.0, cy, label, &st);
    }
    w
}

fn cap_ink() -> Color {
    theme::get().line_soft
}

fn hints_w(ui: &Ui, hints: &[(&str, &str)]) -> f32 {
    let st = regular(23.0, cap_ink());
    let mut w = 0.0;
    for (b, l) in hints {
        let arrows = b.chars().all(|c| "◀▶▲▼".contains(c));
        let n = b.chars().count() as f32;
        w += if arrows || n <= 1.0 {
            38.0 + if arrows { (n - 1.0) * 17.0 } else { 0.0 }
        } else {
            ui.measure(b, &TextStyle::new(Face::Mono600, 19.0, cap_ink())) + 26.0
        };
        w += 10.0 + ui.measure(l, &st) + 26.0;
    }
    w - 26.0
}

/// Button hints from x, left to right.
fn hints(ui: &mut Ui, mut x: f32, hs: &[(&str, &str)], ink: Color, on: Color) {
    let cy = H - HINT_H / 2.0;
    let st = regular(23.0, ink);
    for (b, l) in hs {
        x += cap(ui, x, cy, b, ink, on) + 10.0;
        x += ui.text_vcenter(x, cy, l, &st) + 26.0;
    }
}

fn hints_right(ui: &mut Ui, hs: &[(&str, &str)], ink: Color, on: Color) {
    let w = hints_w(ui, hs);
    hints(ui, W - 28.0 - w, hs, ink, on);
}

/// A block with a hard offset shadow and a thick ink edge: "this one".
fn lifted(ui: &mut Ui, r: Rect, fill: Color) {
    let t = theme::get();
    ui.fill_block(Rect::new(r.x + 10.0, r.y + 10.0, r.w, r.h), t.line);
    ui.fill_block(r, fill);
    ui.outline(r, 3.0, t.line);
}

/// A big dark block: ink by day, a step up from the ground at night.
fn block_dark() -> Color {
    let t = theme::get();
    if t.name == "dark" { t.ground_2 } else { DARK }
}

fn glyph_for(e: &Entry) -> Glyph {
    if e.broken {
        Glyph::Broken
    } else if e.is_link {
        Glyph::Link
    } else {
        Glyph::Kind(e.kind)
    }
}

/// A small tile with a glyph on it.
fn tile(ui: &mut Ui, x: f32, y: f32, s: f32, bg: Color, ink: Color, g: Glyph) {
    ui.fill_block(Rect::new(x, y, s, s), bg);
    let cell = ((s - 12.0) / 6.0).floor();
    let w = cell * 6.0;
    mosaic::glyph(ui, g, x + (s - w) / 2.0, y + (s - w) / 2.0, cell, 0.0, ink);
}

// ------------------------------------------------------------------ top ----

pub fn draw(app: &mut App, ui: &mut Ui) {
    if let Some(dir) = app.slide_req.take() {
        let from = ui.pm.clone();
        render(app, ui);
        let to = ui.pm.clone();
        app.slide = Some(Slide { dir, start: app.now, from, to });
    } else if app.slide_done() {
        app.slide = None;
        render(app, ui);
        return;
    } else if app.slide.is_none() {
        render(app, ui);
        return;
    }
    let p = app.slide_progress().unwrap_or(1.0);
    if let Some(s) = &app.slide {
        compose(ui, s, p);
    }
}

/// A frame of a slide: two finished pictures, moved. A few memcpys, far
/// cheaper than drawing a screen.
fn compose(ui: &mut Ui, s: &Slide, p: f32) {
    let (w, h) = (ui.pm.width() as usize, ui.pm.height() as usize);
    if s.from.width() as usize != w || s.to.width() as usize != w || s.from.height() as usize != h || s.to.height() as usize != h {
        ui.pm.data_mut().copy_from_slice(s.to.data());
        return;
    }
    let (a, b) = (s.from.data(), s.to.data());
    let d = ui.pm.data_mut();
    let px = (p * w as f32).round() as usize;
    let py = (p * h as f32).round() as usize;
    let row = w * 4;
    let panel = PANEL_X as usize;
    let pw = w.saturating_sub(panel);
    let pp = (p * pw as f32).round() as usize;
    match s.dir {
        SlideDir::Forward | SlideDir::Back => {
            for y in 0..h {
                let (o, sa, sb) = (&mut d[y * row..(y + 1) * row], &a[y * row..(y + 1) * row], &b[y * row..(y + 1) * row]);
                if s.dir == SlideDir::Forward {
                    o[..(w - px) * 4].copy_from_slice(&sa[px * 4..]);
                    o[(w - px) * 4..].copy_from_slice(&sb[..px * 4]);
                } else {
                    o[px * 4..].copy_from_slice(&sa[..(w - px) * 4]);
                    o[..px * 4].copy_from_slice(&sb[(w - px) * 4..]);
                }
            }
        }
        SlideDir::Up => {
            d[..(h - py) * row].copy_from_slice(&a[..(h - py) * row]);
            d[(h - py) * row..].copy_from_slice(&b[..py * row]);
        }
        SlideDir::Down => {
            d[..py * row].copy_from_slice(&b[(h - py) * row..]);
            d[py * row..].copy_from_slice(&a[..(h - py) * row]);
        }
        SlideDir::PanelIn | SlideDir::PanelOut => {
            let opening = s.dir == SlideDir::PanelIn;
            let (under, over) = if opening { (a, b) } else { (b, a) };
            let shown = if opening { pp } else { pw - pp };
            d.copy_from_slice(under);
            for y in 0..h {
                let o = &mut d[y * row..(y + 1) * row];
                let src = &over[y * row..(y + 1) * row];
                o[(w - shown) * 4..].copy_from_slice(&src[panel * 4..(panel + shown) * 4]);
            }
        }
    }
}

fn render(app: &App, ui: &mut Ui) {
    let t = theme::get();
    ui.clear(t.ground);
    if app.job_visible() {
        draw_job(app, ui);
        return;
    }
    match &app.overlay {
        Some(Overlay::Confirm(c)) => return draw_confirm(app, ui, c),
        Some(Overlay::Naming(n)) => return draw_naming(app, ui, n),
        Some(Overlay::Find(f)) => return draw_find(app, ui, f),
        Some(Overlay::Report(r)) => return draw_report(ui, r),
        _ => {}
    }
    match &app.screen {
        Screen::Places => draw_places(app, ui),
        Screen::Browse => draw_browse(app, ui),
        Screen::Image { index, pic } => draw_image(app, ui, *index, pic),
        Screen::Text { name, lines, cut, top } => draw_text(ui, name, lines, *cut, *top),
        Screen::Storage => draw_storage(app, ui),
    }
    if let Some((s, _)) = &app.toast {
        draw_toast(ui, s);
    }
}

fn draw_toast(ui: &mut Ui, s: &str) {
    let t = theme::get();
    let st = body(26.0, DARK);
    let lines = wrap(ui, s, &st, 760.0, 2);
    let w = lines.iter().map(|l| ui.measure(l, &st)).fold(0.0, f32::max) + 52.0;
    let h = lines.len() as f32 * 36.0 + 36.0;
    let r = Rect::new(24.0, 20.0, w, h);
    ui.fill_block(Rect::new(r.x + 10.0, r.y + 10.0, r.w, r.h), DARK);
    ui.fill_block(r, t.yellow);
    ui.outline(r, 3.0, DARK);
    for (i, l) in lines.iter().enumerate() {
        ui.text(r.x + 26.0, r.y + 16.0 + i as f32 * 36.0, l, &st);
    }
}

// --------------------------------------------------------------- places ----

/// The mosaic of places: Home large, the others in a grid, and the last one
/// taking what's left of the bottom row.
pub fn place_rects(n: usize) -> Vec<Rect> {
    let (cw, _) = PLACE_CELL;
    let rows = if n <= 5 { 2 } else { PLACE_ROWS };
    let ch = (H - HINT_H) / rows as f32;
    let mut out = vec![Rect::new(0.0, 0.0, cw * 2.0, ch * 2.0)];
    let mut cells = Vec::new();
    for r in 0..rows {
        for c in 0..PLACE_COLS {
            if !(r < 2 && c < 2) {
                cells.push((c, r));
            }
        }
    }
    for i in 0..n.saturating_sub(1) {
        let (c, r) = cells[i.min(cells.len() - 1)];
        let mut w = cw;
        if i == n - 2 {
            // to the end of its row
            w = (PLACE_COLS - c) as f32 * cw;
        }
        out.push(Rect::new(c as f32 * cw, r as f32 * ch, w, ch));
    }
    out
}

fn place_look(p: &Place) -> (Color, Color, Motif, Color, Color) {
    let t = theme::get();
    match p.kind {
        PlaceKind::Home => (t.yellow, DARK, Motif::Sun, t.orange, PINK),
        PlaceKind::Games => (t.lime, DARK, Motif::Zigzag, DARK, t.lavender),
        PlaceKind::Saves => (PINK, DARK, Motif::Truchet, t.lavender, PINK),
        PlaceKind::Bios => (t.orange, DARK, Motif::Chips, DARK, t.yellow),
        PlaceKind::Removable => (t.lavender, DARK, Motif::Card, DARK, t.yellow),
        PlaceKind::Trash => (t.ground_2, t.line, Motif::Crumble, t.line, t.orange),
        PlaceKind::Root => (t.lavender, DARK, Motif::Stairs, DARK, t.yellow),
        PlaceKind::Storage => (block_dark(), mosaic::LIGHT, Motif::Blocks, t.lime, PINK),
    }
}

fn draw_places(app: &App, ui: &mut Ui) {
    let rects = place_rects(app.places.len());
    // the selected one last: it lifts over its neighbours
    let order = (0..app.places.len()).filter(|i| *i != app.place_sel).chain(std::iter::once(app.place_sel));
    for i in order {
        let p = &app.places[i];
        let r = rects[i];
        let (bg, ink, m, c1, c2) = place_look(p);
        let big = i == 0;
        let sel = i == app.place_sel;
        let shown = if sel { Rect::new(r.x + 1.0, r.y + 1.0, r.w - 8.0, r.h - 8.0) } else { Rect::new(r.x + 4.0, r.y + 4.0, r.w - 8.0, r.h - 8.0) };
        if sel {
            lifted(ui, shown, bg);
        } else {
            ui.fill_block(shown, bg);
        }
        let inner = shown.inset(16.0, 14.0);
        let name_st = serif(if big { 100.0 } else { 50.0 }, ink);
        let nh = name_st.line_px();
        let fact = app.place_facts.get(&p.name).cloned().unwrap_or_default();
        let fact_st = regular(if big { 28.0 } else { 22.0 }, ink);
        let top = inner.y + nh + fact_st.line_px() + 2.0;
        let art = Rect::new(inner.x, top, inner.w, (inner.bottom() - top).max(0.0));
        mosaic::motif(ui, m, art, c1, c2, p.name.as_bytes());
        ui.text_fit(inner.x, inner.y, &p.name, &name_st, inner.w);
        ui.text_fit(inner.x + 2.0, inner.y + nh, &fact, &fact_st, inner.w);
    }
    let ink = cap_ink();
    let g = theme::get().ground;
    hints_right(ui, &[("◀▶▲▼", "Choose"), ("A", "Open")], ink, g);
}

// --------------------------------------------------------------- browse ----

/// A path as a person says it: from the place it's in.
fn path_parts(app: &App, dir: &std::path::Path) -> Vec<String> {
    let place = app.places.iter().filter(|p| p.kind != PlaceKind::Storage && dir.starts_with(&p.path)).max_by_key(|p| p.path.as_os_str().len());
    let mut parts = Vec::new();
    let rest = match place {
        Some(p) => {
            parts.push(p.name.clone());
            dir.strip_prefix(&p.path).ok().map(|r| r.to_path_buf()).unwrap_or_default()
        }
        None => dir.to_path_buf(),
    };
    parts.extend(rest.components().map(|c| crate::listing::display(c.as_os_str())));
    parts
}

fn friendly_path(app: &App, dir: &std::path::Path) -> String {
    path_parts(app, dir).join(" / ")
}

fn crumbs(app: &App) -> (Vec<String>, String) {
    let mut parts = path_parts(app, &app.dir);
    let last = parts.pop().unwrap_or_default();
    (parts, last)
}

fn draw_title(app: &App, ui: &mut Ui, max_right: f32) {
    let t = theme::get();
    let (parents, name) = crumbs(app);
    let name_st = serif(60.0, t.line);
    let crumb_st = regular(26.0, t.line_soft);
    let base_cy = TITLE_H / 2.0 + 4.0;
    let mut x = 28.0;
    let name_w = ui.measure(&name, &name_st).min(max_right - x - 40.0);
    // parents that fit, nearest first
    let mut shown: Vec<String> = Vec::new();
    let mut used = name_w;
    for p in parents.iter().rev() {
        let s = fit_name(ui, p, &crumb_st, 170.0);
        let w = ui.measure(&s, &crumb_st) + 30.0;
        if x + used + w + 34.0 > max_right {
            shown.push("…".into());
            break;
        }
        used += w;
        shown.push(s);
    }
    shown.reverse();
    for s in &shown {
        x += ui.text_vcenter(x, base_cy, s, &crumb_st);
        triangle(ui, x + 13.0, base_cy, 6.0, '▶', t.line_soft);
        x += 28.0;
    }
    let fit = fit_name(ui, &name, &name_st, max_right - x);
    ui.text_vcenter(x, base_cy - 2.0, &fit, &name_st);
}

/// The row's right-hand value: whatever it's sorted by.
fn row_value(app: &App, e: &Entry) -> String {
    match app.sort {
        crate::listing::Sort::Date => app.clock.short(e.mtime),
        _ if e.is_dir || e.broken => String::new(),
        _ => human_size(e.size),
    }
}

fn row_y(d: f32) -> f32 {
    // above the band, rows stack up from its top edge; below, from its bottom
    if d <= 0.0 {
        BAND_Y + d * ROW_H
    } else {
        BAND_Y + (BAND_H + BAND_GAP - ROW_H) * d.min(1.0) + d * ROW_H
    }
}

fn draw_browse(app: &App, ui: &mut Ui) {
    let t = theme::get();
    let selected = app.selected().cloned();
    let (bg, ink) = selected.as_ref().map(|e| entry_colors(e)).unwrap_or((t.ground_2, t.line));
    let pos = app.scroll_pos();
    let n = app.entries.len();
    let first = (pos - 4.0).floor().max(0.0) as usize;
    let last = ((pos + 7.0).ceil() as usize).min(n);
    let right = PANEL_X - 20.0;
    for i in first..last {
        let d = i as f32 - pos;
        let y = row_y(d);
        if y + ROW_H < TITLE_H - 8.0 || y > LIST_BOTTOM {
            continue;
        }
        draw_row(app, ui, &app.entries[i], y, right);
    }
    // the title and the hint row sit over rows on their way out
    ui.fill_block(Rect::new(0.0, 0.0, PANEL_X, BAND_Y - 2.0 * ROW_H - 2.0), t.ground);
    ui.fill_block(Rect::new(0.0, LIST_BOTTOM, PANEL_X, HINT_H), t.ground);
    let mark_w = draw_status_block(app, ui);
    draw_title(app, ui, PANEL_X - 24.0 - mark_w);

    if let Some(e) = &selected {
        draw_band(app, ui, e, bg, ink);
    } else {
        draw_empty_body(app, ui);
    }
    if matches!(app.overlay, Some(Overlay::Actions { .. })) {
        draw_actions(app, ui);
    } else {
        draw_panel(app, ui, selected.as_ref(), bg, ink);
    }
    draw_browse_hints(app, ui);
}

fn entry_colors(e: &Entry) -> (Color, Color) {
    if e.broken || e.special {
        let t = theme::get();
        return (t.ground_2, t.line);
    }
    kind_colors(e.kind)
}

fn draw_row(app: &App, ui: &mut Ui, e: &Entry, y: f32, right: f32) {
    let t = theme::get();
    let marked = app.marks.contains(&e.name);
    let cy = y + ROW_H / 2.0;
    let s = 48.0;
    let (bg, ink) = entry_colors(e);
    if marked {
        tile(ui, 28.0, cy - s / 2.0, s, t.line, t.lime, Glyph::Check);
    } else {
        tile(ui, 28.0, cy - s / 2.0, s, bg, ink, glyph_for(e));
    }
    let name_c = if e.hidden { t.line_soft } else { t.line };
    let st = body(29.0, name_c);
    let val = row_value(app, e);
    let val_st = mono(22.0, t.line_soft);
    let vw = ui.measure(&val, &val_st);
    ui.text_vcenter(right - vw, cy, &val, &val_st);
    let name = if app.in_trash() { app.trash_name(e) } else { e.display.clone() };
    let max = right - vw - 26.0 - 96.0;
    let fit = fit_name(ui, &name, &st, max);
    ui.text_vcenter(96.0, cy, &fit, &st);
    if e.is_dir && !app.in_trash() && app.sort != crate::listing::Sort::Date {
        triangle(ui, right - 8.0, cy, 7.0, '▶', t.line_soft);
    }
}

/// The selected row: the shell's band, from the screen's edge into the panel.
fn draw_band(app: &App, ui: &mut Ui, e: &Entry, bg: Color, ink: Color) {
    let t = theme::get();
    // the shadow only shows below it; the panel covers the rest
    ui.fill_block(Rect::new(0.0, BAND_Y + BAND_H, PANEL_X, BAND_SHADOW), t.line);
    ui.fill_block(Rect::new(0.0, BAND_Y, PANEL_X, BAND_H), bg);
    let marked = app.marks.contains(&e.name);
    let s = 64.0;
    let (tx, ty) = (24.0, BAND_Y + (BAND_H - s) / 2.0);
    if marked {
        tile(ui, tx, ty, s, t.line, t.lime, Glyph::Check);
    } else {
        tile(ui, tx, ty, s, t.ground, t.line, glyph_for(e));
    }
    let st = serif(58.0, ink);
    let name = if app.in_trash() { app.trash_name(e) } else { e.display.clone() };
    let fit = fit_name(ui, &name, &st, PANEL_X - 112.0 - 18.0);
    ui.text_vcenter(112.0, BAND_Y + BAND_H / 2.0 - 2.0, &fit, &st);
}

/// How many are marked, or what's in hand: a yellow block by the title.
fn draw_status_block(app: &App, ui: &mut Ui) -> f32 {
    let t = theme::get();
    let (label, fill) = if let Some(c) = &app.carry {
        (format!("{} {}", if c.moving { "Moving" } else { "Copying" }, c.sources.len()), t.lime)
    } else if !app.marks.is_empty() {
        (format!("{} marked", app.marks.len()), t.yellow)
    } else if app.loading || !app.dir_read_only() {
        return 0.0;
    } else {
        return draw_read_only_tag(ui);
    };
    let st = body(28.0, DARK);
    let w = ui.measure(&label, &st) + 36.0;
    let r = Rect::new(PANEL_X - 30.0 - w, 20.0, w, 54.0);
    ui.fill_block(Rect::new(r.x + 8.0, r.y + 8.0, r.w, r.h), t.line);
    ui.fill_block(r, fill);
    ui.outline(r, 3.0, t.line);
    ui.text_vcenter(r.x + 18.0, r.y + r.h / 2.0, &label, &st);
    w + 30.0
}

fn draw_read_only_tag(ui: &mut Ui) -> f32 {
    let t = theme::get();
    let st = body(23.0, t.line);
    let label = "Read-only";
    let w = ui.measure(label, &st) + 56.0;
    let x = PANEL_X - 26.0 - w;
    ui.fill_block(Rect::new(x, 26.0, w, 42.0), t.ground_2);
    glyph(ui, Glyph::Lock, x + 10.0, 35.0, 4.0, 0.0, t.line);
    ui.text_vcenter(x + 44.0, 47.0, label, &st);
    w + 26.0
}

fn draw_browse_hints(app: &App, ui: &mut Ui) {
    let (ink, on) = (cap_ink(), theme::get().ground);
    let mut hs: Vec<(&str, &str)> = Vec::new();
    if app.overlay.is_some() {
        hs = vec![("▲▼", "Choose"), ("A", "OK"), ("B", "Back")];
        if matches!(app.overlay, Some(Overlay::Actions { sel, ref rows }) if matches!(rows.get(sel), Some(Act::Sort))) {
            hs = vec![("▲▼", "Choose"), ("◀▶", "Change"), ("B", "Back")];
        }
    } else if app.carry.is_some() {
        hs = vec![("A", "Open"), ("Y", "Put here"), ("X", "Actions"), ("B", "Back")];
    } else if app.in_trash() {
        hs = vec![("A", "Restore…"), ("X", "Actions"), ("L2", "Places")];
    } else if !app.entries.is_empty() {
        let open = match app.selected() {
            Some(e) if e.is_dir => "Open",
            _ => "Open",
        };
        hs.push(("A", open));
        hs.push(("Y", "Mark"));
        hs.push(("X", "Actions"));
        hs.push(("L2", "Places"));
    } else {
        hs.push(("B", "Back"));
        hs.push(("X", "Actions"));
        hs.push(("L2", "Places"));
    }
    hints(ui, 28.0, &hs, ink, on);
}

fn draw_empty_body(app: &App, ui: &mut Ui) {
    let t = theme::get();
    let (title, sub) = if app.loading {
        ("Reading…".to_string(), String::new())
    } else if app.hidden_count > 0 {
        ("Nothing to see.".to_string(), format!("{} hidden. X shows them.", plural(app.hidden_count, "thing")))
    } else if app.in_trash() {
        ("The Trash is empty.".to_string(), String::new())
    } else {
        ("Empty folder.".to_string(), String::new())
    };
    ui.text(28.0, BAND_Y - 16.0, &title, &serif(64.0, t.line));
    if !sub.is_empty() {
        ui.text(30.0, BAND_Y + 64.0, &sub, &regular(26.0, t.line_soft));
    }
}

// ---------------------------------------------------------------- panel ----

fn draw_panel(app: &App, ui: &mut Ui, e: Option<&Entry>, bg: Color, ink: Color) {
    let t = theme::get();
    let panel = Rect::new(PANEL_X, 0.0, W - PANEL_X, H);
    ui.fill_block(panel, bg);
    let art = Rect::new(ART_X, ART_Y, ART as f32, ART as f32);
    let Some(e) = e else {
        let (x, y) = (ART_X + 64.0, ART_Y + 64.0);
        glyph(ui, Glyph::Kind(Kind::Folder), x, y, 28.0, 4.0, ink);
        return;
    };
    let path = app.dir.join(&e.name);
    let preview = app.panel.as_ref().filter(|p| p.0 == path).map(|p| &p.1);
    let paper = t.ground;
    match preview {
        None => ui.fill_block(art, DARK),
        Some(Preview::Image(pm)) => draw_framed(ui, pm, art, DARK),
        Some(Preview::Text(lines)) => {
            ui.fill_block(art, paper);
            let st = mono(20.0, t.line);
            for (i, l) in lines.iter().take(12).enumerate() {
                let clip: String = l.chars().take(24).collect();
                ui.text(art.x + 12.0, art.y + 8.0 + i as f32 * 25.0, &clip, &st);
            }
        }
        Some(Preview::Bytes(b)) => mosaic::byte_mosaic(ui, art, b),
        Some(Preview::Folder(f)) => mosaic::folder_mosaic(ui, art, &f.by_kind, e.name.as_bytes()),
        Some(Preview::Note(_)) => {
            ui.fill_block(art, DARK);
            let g = if e.broken { Glyph::Broken } else { Glyph::Lock };
            let cell = 28.0;
            let w = mosaic::glyph_w(cell, 4.0);
            glyph(ui, g, art.x + (art.w - w) / 2.0, art.y + (art.h - w) / 2.0, cell, 4.0, mosaic::LIGHT);
        }
    }
    // the facts, in big type
    let x = ART_X;
    let w = ART as f32;
    let mut y = ART_Y + ART as f32 + 14.0;
    let title = match preview {
        Some(Preview::Folder(f)) => {
            if f.count == 0 { "Empty".to_string() } else { plural(f.count, "item") }
        }
        Some(Preview::Note(n)) => n.clone(),
        _ if e.is_dir => "Folder".into(),
        _ if e.is_link => format!("Link, {}", e.kind.word().to_lowercase()),
        _ => e.kind.word().to_string(),
    };
    let tst = serif(if title.len() > 16 { 42.0 } else { 62.0 }, ink);
    for l in wrap(ui, &title, &tst, w, 2) {
        ui.text(x, y, &l, &tst);
        y += tst.line_px();
    }
    y += 2.0;
    let big = body(38.0, ink);
    let size = match preview {
        _ if e.broken || e.special => None,
        Some(Preview::Folder(f)) => Some(format!("{}{}", human_size(f.bytes), if f.more { "+" } else { "" })),
        _ if e.is_dir => None,
        _ => Some(human_size(e.size)),
    };
    if let Some(s) = size {
        ui.text(x, y, &s, &big);
        y += big.line_px();
    }
    let mst = mono(24.0, ink);
    if !e.broken {
        ui.text(x, y, &app.clock.long(e.mtime), &mst);
    }
    y += mst.line_px() + 6.0;
    // the whole name, when the band had to shorten it
    let name_st = regular(24.0, ink);
    let shown_name = if app.in_trash() { app.trash_name(e) } else { e.display.clone() };
    let band_w = ui.measure(&shown_name, &serif(58.0, ink));
    if band_w > PANEL_X - 130.0 {
        let room = ((H - 58.0 - y) / name_st.line_px()).floor().max(1.0) as usize;
        for l in wrap(ui, &shown_name, &name_st, w, room) {
            ui.text(x, y, &l, &name_st);
            y += name_st.line_px();
        }
    }
    if app.in_trash()
        && let Some(i) = app.trash.read_info(&e.name)
    {
        let from = i.origin.parent().map(|p| friendly_path(app, p)).unwrap_or_default();
        let st = regular(22.0, ink);
        let room = ((H - 58.0 - y) / st.line_px()).floor().max(1.0) as usize;
        for l in wrap(ui, &format!("From {from}"), &st, w, room) {
            ui.text(x, y, &l, &st);
            y += st.line_px();
        }
    }
    // where you are in a long list
    let pos = format!("{} / {}", app.sel + 1, app.entries.len());
    let sort = app.sort.word();
    let pst = mono(23.0, ink);
    ui.text(x, H - 50.0, &pos, &pst);
    let sw = ui.measure(sort, &pst);
    ui.text(x + w - sw, H - 50.0, sort, &pst);
}

/// A picture centred in `r` on `back`.
fn draw_framed(ui: &mut Ui, pm: &Pixmap, r: Rect, back: Color) {
    ui.fill_block(r, back);
    let x = r.x + (r.w - pm.width() as f32) / 2.0;
    let y = r.y + (r.h - pm.height() as f32) / 2.0;
    ui.pm.draw_pixmap(x.round() as i32, y.round() as i32, pm.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
}

fn act_word(app: &App, a: Act) -> String {
    match a {
        Act::Copy => "Copy".into(),
        Act::Move => "Move".into(),
        Act::Rename => "Rename".into(),
        Act::Delete => "Delete".into(),
        Act::NewFolder => "New folder".into(),
        Act::MarkAll => "Mark all".into(),
        Act::ClearMarks => "Clear marks".into(),
        Act::Paste => {
            let c = app.carry.as_ref();
            format!("{} here", if c.is_some_and(|c| c.moving) { "Move" } else { "Copy" })
        }
        Act::Forget => "Never mind".into(),
        Act::StorageHere => "What's using space".into(),
        Act::Sort => format!("Sort: {}", app.sort.word()),
        Act::Hidden => format!("Hidden: {}", if app.show_hidden { "shown" } else { "not shown" }),
        Act::Restore => "Restore".into(),
        Act::DeleteForever => "Delete forever".into(),
        Act::EmptyTrash => "Empty the Trash".into(),
    }
}

fn draw_actions(app: &App, ui: &mut Ui) {
    let t = theme::get();
    let Some(Overlay::Actions { sel, rows }) = &app.overlay else { return };
    let light = Color::hex(0xe1ebf7);
    ui.fill_block(Rect::new(PANEL_X, 0.0, W - PANEL_X, H), block_dark());
    // what it acts on
    let targets = app.targets();
    let head = if let Some(c) = &app.carry {
        plural(c.sources.len(), "item")
    } else if targets.len() == 1 {
        if app.in_trash() { app.trash_name(targets[0]) } else { targets[0].display.clone() }
    } else if targets.is_empty() {
        "This folder".to_string()
    } else {
        plural(targets.len(), "item")
    };
    let st = serif(46.0, light);
    let fit = fit_name(ui, &head, &st, W - PANEL_X - 48.0);
    ui.text(PANEL_X + 24.0, 22.0, &fit, &st);
    let row_h = 66.0;
    let top = 96.0;
    for (i, a) in rows.iter().enumerate() {
        let y = top + i as f32 * row_h;
        if i == *sel {
            let fill = if a.is_danger() { t.orange } else { t.yellow };
            ui.fill_block(Rect::new(PANEL_X - 18.0, y + 6.0, W - PANEL_X + 18.0, row_h - 2.0), Color::hex(0x000000));
            ui.fill_block(Rect::new(PANEL_X - 18.0, y, W - PANEL_X + 18.0, row_h - 4.0), fill);
            ui.outline(Rect::new(PANEL_X - 18.0, y, W - PANEL_X + 22.0, row_h - 4.0), 3.0, DARK);
            ui.text_vcenter(PANEL_X + 24.0, y + (row_h - 4.0) / 2.0 - 1.0, &act_word(app, *a), &serif(42.0, DARK));
        } else {
            let c = if a.is_danger() { t.orange } else { light };
            ui.text_vcenter(PANEL_X + 24.0, y + (row_h - 4.0) / 2.0, &act_word(app, *a), &body(28.0, c));
        }
    }
}

// ------------------------------------------------------------- viewers ----

fn draw_image(app: &App, ui: &mut Ui, index: usize, pic: &Option<Result<std::sync::Arc<Pixmap>, String>>) {
    let t = theme::get();
    ui.clear(DARK);
    let light = Color::hex(0xe1ebf7);
    match pic {
        Some(Ok(pm)) => {
            let x = (W - pm.width() as f32) / 2.0;
            let y = (H - pm.height() as f32) / 2.0;
            ui.pm.draw_pixmap(x.round() as i32, y.round() as i32, pm.as_ref().as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        }
        Some(Err(why)) => {
            ui.text(60.0, 300.0, why, &serif(56.0, light));
        }
        None => {}
    }
    if let Some(e) = app.entries.get(index) {
        let st = body(24.0, light);
        let n = app.entries.iter().filter(|x| x.kind == Kind::Image).count();
        let i = app.entries[..index].iter().filter(|x| x.kind == Kind::Image).count() + 1;
        let label = format!("{}   {i} / {n}", fit_name(ui, &e.display, &st, 720.0));
        let w = ui.measure(&label, &st) + 36.0;
        ui.fill_block(Rect::new(0.0, H - 58.0, w, 58.0), DARK);
        ui.text_vcenter(18.0, H - 29.0, &label, &st);
        let _ = t;
    }
}

fn draw_text(ui: &mut Ui, name: &str, lines: &Option<Result<Vec<String>, String>>, cut: bool, top: usize) {
    let t = theme::get();
    let title = serif(52.0, t.line);
    let fit = fit_name(ui, name, &title, W - 60.0);
    ui.text(28.0, 14.0, &fit, &title);
    let y0 = 86.0;
    match lines {
        Some(Ok(l)) => {
            let st = mono(27.0, t.line);
            for (i, s) in l.iter().skip(top).take(TEXT_ROWS).enumerate() {
                ui.text(28.0, y0 + i as f32 * TEXT_LINE, s, &st);
            }
            let (ink, on) = (cap_ink(), t.ground);
            let pos = if l.is_empty() { "Empty".to_string() } else { format!("Line {} of {}{}", top + 1, l.len(), if cut { "+" } else { "" }) };
            ui.fill_block(Rect::new(0.0, H - HINT_H, W, HINT_H), t.ground);
            ui.text_vcenter(28.0, H - HINT_H / 2.0, &pos, &regular(23.0, ink));
            hints_right(ui, &[("▲▼", "Scroll"), ("L1", "Page"), ("R1", "Page"), ("B", "Back")], ink, on);
        }
        Some(Err(e)) => {
            ui.text(28.0, 200.0, e, &serif(56.0, t.line));
        }
        None => {}
    }
}

// -------------------------------------------------------------- storage ----

fn tile_colors(tl: &crate::storage::Tile) -> (Color, Color) {
    let t = theme::get();
    match tl.kind {
        TileKind::Folder => match tl.tint {
            Some(k) if k != Kind::Text && k != Kind::Other => kind_colors(k),
            _ => (t.lavender, DARK),
        },
        TileKind::File(kind) => kind_colors(kind),
        TileKind::Free => (t.ground, t.line),
        TileKind::System => (block_dark(), mosaic::LIGHT),
        TileKind::Others(_) => (t.ground_2, t.line),
    }
}

/// Free room: empty sockets, nothing filled in.
fn free_sockets(ui: &mut Ui, r: Rect) {
    let t = theme::get();
    let (g, s) = (28.0, 24.0);
    let (cols, rows) = (((r.w - 8.0) / g).floor() as i32, ((r.h - 8.0) / g).floor() as i32);
    for y in 0..rows {
        for x in 0..cols {
            ui.fill_block(Rect::new(r.right() - (x + 1) as f32 * g, r.bottom() - (y + 1) as f32 * g, s, s), t.ground_2);
        }
    }
}

fn draw_storage(app: &App, ui: &mut Ui) {
    let t = theme::get();
    let Some(l) = app.levels.last() else { return };
    let (ink, on) = (cap_ink(), t.ground);
    // where you are, and how much it comes to
    let title = serif(60.0, t.line);
    let total: u64 = l.tiles.iter().filter(|t| !matches!(t.kind, TileKind::Free)).map(|t| t.size).sum();
    let size_st = body(32.0, t.line);
    let size = if l.tiles.is_empty() { String::new() } else { format!("{} used", human_size(total)) };
    let sw = ui.measure(&size, &size_st);
    ui.text_vcenter(W - 28.0 - sw, TITLE_H / 2.0 + 2.0, &size, &size_st);
    let named = l.dir.as_ref().and_then(|d| path_parts(app, d).pop()).unwrap_or_else(|| l.title.clone());
    let fit = fit_name(ui, &named, &title, W - 96.0 - sw);
    ui.text_vcenter(28.0, TITLE_H / 2.0, &fit, &title);

    if let Some(e) = &l.error {
        ui.text(28.0, 280.0, e, &serif(56.0, t.line));
    }
    let n = l.tiles.len();
    let order = (0..n).filter(|i| *i != l.sel).chain((n > 0).then_some(l.sel));
    for i in order {
        let (tl, r) = (&l.tiles[i], l.rects[i]);
        if r.w < 1.0 || r.h < 1.0 {
            continue;
        }
        let (bg, tink) = tile_colors(tl);
        let sel = i == l.sel;
        let mut b = Rect::new(r.x + 2.0, r.y + 2.0, (r.w - 4.0).max(1.0), (r.h - 4.0).max(1.0));
        if sel {
            b = Rect::new((b.x - 4.0).max(1.0), b.y - 4.0, b.w, b.h);
            lifted(ui, b, bg);
        } else {
            ui.fill_block(b, bg);
        }
        if tl.kind == TileKind::Free {
            free_sockets(ui, b.inset(4.0, 4.0));
        }
        let plate = tl.kind == TileKind::Free;
        if b.w > 120.0 && b.h > 60.0 {
            let big = b.w > 240.0 && b.h > 130.0;
            let st = if big { serif(52.0, tink) } else { body(23.0, tink) };
            let fit = fit_name(ui, &tl.name, &st, b.w - 28.0);
            let show_size = b.h > if big { 104.0 } else { 76.0 };
            if plate {
                // on the sockets, words need ground under them
                ui.fill_block(Rect::new(b.x + 6.0, b.y + 6.0, (ui.measure(&fit, &st) + 24.0).max(150.0), if show_size { 104.0 } else { 62.0 }), t.ground);
            }
            ui.text(b.x + 14.0, b.y + 8.0, &fit, &st);
            if show_size {
                let st2 = regular(if big { 26.0 } else { 20.0 }, tink);
                ui.text(b.x + if big { 16.0 } else { 14.0 }, b.y + 10.0 + st.line_px(), &human_size(tl.size), &st2);
            }
        }
    }
    // the selected one in words, whatever its size on the mosaic
    let cy = H - HINT_H / 2.0;
    let enter = match l.tiles.get(l.sel).map(|s| s.kind) {
        Some(TileKind::Folder) => Some("Inside"),
        Some(TileKind::File(_)) => Some("Find it"),
        Some(TileKind::Others(_)) => Some("List them"),
        _ => None,
    };
    let mut hs = vec![("◀▶▲▼", "Choose")];
    if let Some(e) = enter {
        hs.push(("A", e));
    }
    hs.push(("B", "Back"));
    let hw = hints_w(ui, &hs);
    if l.scanning {
        ui.text_vcenter(28.0, cy, "Adding it up…", &regular(24.0, ink));
    } else if let Some(s) = l.tiles.get(l.sel) {
        let st = body(25.0, t.line);
        let label = format!("{}  {}", s.name, human_size(s.size));
        let fit = fit_name(ui, &label, &st, W - 80.0 - hw);
        ui.text_vcenter(28.0, cy, &fit, &st);
    }
    hints_right(ui, &hs, ink, on);
}

// ------------------------------------------------------------- full ones ----

fn confirm_title(c: &Confirm) -> String {
    let n = c.sources.len();
    let what = if n == 1 { "this".to_string() } else { format!("these {n}") };
    if c.empty_trash {
        format!("Empty the Trash? {} gone for good.", plural(n, "thing"))
    } else if c.forever {
        format!("Delete {what} for good?")
    } else {
        format!("Put {what} in the Trash?")
    }
}

fn draw_confirm(app: &App, ui: &mut Ui, c: &Confirm) {
    let t = theme::get();
    let bg = if c.forever { t.orange } else { t.yellow };
    let one = c.sources.len() == 1;
    ui.clear(bg);
    // what's going, a tile each, in a dark strip like the cross's pattern;
    // the fewer there are, the bigger they are
    ui.fill_block(Rect::new(W - 252.0, 0.0, 252.0, H), DARK);
    let n = c.kinds.len();
    let (grid, cols) = if n <= 5 { (140.0f32, 1usize) } else if n <= 24 { (56.0, 4) } else { (28.0, 8) };
    let tile_s = grid - if grid > 100.0 { 12.0 } else { 4.0 };
    let x0 = W - 252.0 + (252.0 - cols as f32 * grid) / 2.0 + if grid > 100.0 { 6.0 } else { 2.0 };
    let rows = ((H - 16.0) / grid).floor() as usize;
    for (i, k) in c.kinds.iter().take(cols * rows).enumerate() {
        let (kbg, kink) = kind_colors(*k);
        let (cx, cy) = ((i % cols) as f32, (i / cols) as f32);
        let (px, py) = (x0 + cx * grid, 12.0 + cy * grid);
        if tile_s >= 40.0 {
            tile(ui, px, py, tile_s, kbg, kink, Glyph::Kind(*k));
        } else {
            ui.fill_block(Rect::new(px, py, tile_s, tile_s), if *k == Kind::Text || *k == Kind::Other { mosaic::LIGHT } else { kbg });
        }
    }
    let left = 48.0;
    let room = W - 252.0 - left - 24.0;
    let tst = serif(84.0, DARK);
    let mut y = 40.0;
    for l in wrap(ui, &confirm_title(c), &tst, room, 3) {
        ui.text(left, y, &l, &tst);
        y += tst.line_px();
    }
    y += 16.0;
    let st = body(30.0, DARK);
    for s in c.names.iter().take(4) {
        let fit = fit_name(ui, s, &st, room);
        ui.text(left, y, &fit, &st);
        y += 44.0;
    }
    if c.sources.len() > c.names.len() {
        ui.text(left, y, &format!("and {} more", c.sources.len() - c.names.len()), &regular(28.0, DARK));
        y += 44.0;
    }
    if let Some(r) = &c.reason {
        ui.text(left, y + 8.0, r, &regular(28.0, DARK));
    } else if !c.forever {
        ui.text(left, y + 8.0, if one { "You can get it back from there." } else { "You can get them back from there." }, &regular(28.0, DARK));
    } else {
        ui.text(left, y + 8.0, if one { "There's no getting it back." } else { "There's no getting them back." }, &regular(28.0, DARK));
    }
    // the action, large: a different button from the one that got you here
    let btn_y = H - 170.0;
    let on = bg;
    if c.forever {
        let held = c.held_since.map(|s| (app.now.saturating_sub(s) as f32 / HOLD_MS as f32).clamp(0.0, 1.0)).unwrap_or(0.0);
        let (mut x, cy) = (left, btn_y + 30.0);
        x += cap(ui, x, cy, "L1", DARK, on) + 12.0;
        x += cap(ui, x, cy, "R1", DARK, on) + 16.0;
        ui.text_vcenter(x, cy, "Hold both to delete", &body(32.0, DARK));
        // the hold fills a row of tiles
        if c.nudge && held == 0.0 {
            ui.text(left, btn_y - 38.0, "One button won't do it, on purpose.", &regular(26.0, DARK));
        }
        let n = 22;
        for i in 0..n {
            let r = Rect::new(left + i as f32 * 28.0, btn_y + 66.0, 24.0, 24.0);
            let lit = (i as f32 + 0.5) / n as f32 <= held;
            ui.fill_block(r, if lit { DARK } else { bg.mix(DARK, 0.75) });
        }
    } else {
        let cy = btn_y + 30.0;
        let w = cap(ui, left, cy, "Y", DARK, on) + 14.0;
        ui.text_vcenter(left + w, cy, if one { "Put it in the Trash" } else { "Put them in the Trash" }, &body(32.0, DARK));
        if c.nudge {
            ui.text_vcenter(left, btn_y + 80.0, "A won't do it, on purpose. It's Y.", &regular(26.0, DARK));
        }
    }
    let cy = H - HINT_H / 2.0;
    let w = cap(ui, left, cy, "B", DARK, on) + 10.0;
    ui.text_vcenter(left + w, cy, if one { "Keep it" } else { "Keep them" }, &regular(26.0, DARK));
}

/// A text field, big: the text, its cursor as a solid block.
fn draw_field(ui: &mut Ui, r: Rect, f: &Field) {
    let t = theme::get();
    ui.fill_block(Rect::new(r.x + 8.0, r.y + 8.0, r.w, r.h), DARK);
    ui.fill_block(r, Color::hex(0xf4f8fd));
    ui.outline(r, 3.0, DARK);
    let st = body(38.0, DARK);
    let room = r.w - 40.0;
    let s = f.string();
    let chars: Vec<char> = s.chars().collect();
    // keep the cursor in view: scroll by whole characters
    let mut start = 0;
    let before = |a: usize, ui: &Ui| -> f32 { ui.measure(&chars[a..f.cursor.min(chars.len())].iter().collect::<String>(), &st) };
    while before(start, ui) > room - 8.0 && start < f.cursor {
        start += 1;
    }
    let mut end = chars.len();
    while end > f.cursor && ui.measure(&chars[start..end].iter().collect::<String>(), &st) > room {
        end -= 1;
    }
    let visible: String = chars[start..end].iter().collect();
    let cy = r.y + r.h / 2.0;
    let cx = r.x + 20.0 + before(start, ui);
    ui.fill_block(Rect::new(cx - 2.0, r.y + 14.0, 7.0, r.h - 28.0), PINK);
    let _ = t;
    ui.text_vcenter(r.x + 20.0, cy, &visible, &st);
}

fn draw_naming(app: &App, ui: &mut Ui, n: &Naming) {
    let t = theme::get();
    let h = ui.height();
    ui.clear(t.lavender);
    let (title, verb) = match &n.purpose {
        NameFor::Rename { .. } => ("Rename", "Rename"),
        NameFor::NewFolder => ("New folder", "Make it"),
    };
    ui.text(36.0, 16.0, title, &serif(64.0, DARK));
    let (f_y, f_h) = (100.0, 88.0);
    draw_field(ui, Rect::new(36.0, f_y, W - 80.0, f_h), &n.field);
    let y = f_y + f_h + 28.0;
    if let Some(e) = &n.error {
        ui.text(36.0, y, e, &body(28.0, DARK));
    } else if let NameFor::Rename { original, .. } = &n.purpose {
        let st = regular(24.0, DARK);
        let fit = fit_name(ui, &format!("Was {original}"), &st, W - 80.0);
        ui.text(36.0, y, &fit, &st);
    }
    let short = app.short;
    let cy = if short { h - 36.0 } else { H - HINT_H / 2.0 };
    if short {
        let mut x = 36.0;
        x += cap(ui, x, cy, "Y", DARK, t.lavender) + 10.0;
        x += ui.text_vcenter(x, cy, verb, &regular(26.0, DARK)) + 26.0;
        x += cap(ui, x, cy, "L2", DARK, t.lavender) + 10.0;
        ui.text_vcenter(x, cy, "Never mind", &regular(26.0, DARK));
    } else {
        // no keyboard showing: the pattern gets the room, the hints the bottom
        let m = Rect::new(36.0, y + 60.0, W - 72.0, H - HINT_H - y - 80.0);
        mosaic::motif(ui, Motif::Truchet, m, PINK, t.lavender, b"naming");
        let mut x = 36.0;
        x += cap(ui, x, cy, "A", DARK, t.lavender) + 10.0;
        x += ui.text_vcenter(x, cy, verb, &regular(26.0, DARK)) + 26.0;
        x += cap(ui, x, cy, "START", DARK, t.lavender) + 10.0;
        x += ui.text_vcenter(x, cy, "Keyboard", &regular(26.0, DARK)) + 26.0;
        x += cap(ui, x, cy, "L2", DARK, t.lavender) + 10.0;
        ui.text_vcenter(x, cy, "Never mind", &regular(26.0, DARK));
    }
}

fn draw_find(app: &App, ui: &mut Ui, f: &Field) {
    let t = theme::get();
    let h = ui.height();
    ui.clear(t.lime);
    ui.text(36.0, 8.0, "Find", &serif(56.0, DARK));
    draw_field(ui, Rect::new(176.0, 14.0, W - 216.0, 72.0), f);
    let (mut y, step) = (112.0, 56.0);
    let from = app.sel;
    for (i, e) in app.entries.iter().skip(from).take(((h - 150.0) / step).max(1.0) as usize).enumerate() {
        let st = if i == 0 { serif(44.0, DARK) } else { body(27.0, DARK) };
        tile(ui, 36.0, y + 2.0, 44.0, DARK, t.lime, glyph_for(e));
        let fit = fit_name(ui, &e.display, &st, W - 130.0);
        ui.text_vcenter(96.0, y + 24.0, &fit, &st);
        y += step;
    }
    let cy = h - 30.0;
    let mut x = 36.0;
    x += cap(ui, x, cy, "Y", DARK, t.lime) + 10.0;
    ui.text_vcenter(x, cy, "Done", &regular(24.0, DARK));
}

fn draw_report(ui: &mut Ui, r: &ops::Report) {
    let t = theme::get();
    ui.clear(if r.refused.is_some() || !r.failed.is_empty() { t.orange } else { t.yellow });
    let left = 48.0;
    let room = W - 2.0 * left;
    let mut y = 40.0;
    let st = serif(76.0, DARK);
    for l in wrap(ui, &r.headline(), &st, room, 3) {
        ui.text(left, y, &l, &st);
        y += st.line_px();
    }
    y += 12.0;
    let d = regular(30.0, DARK);
    for l in r.details() {
        for w in wrap(ui, &l, &d, room, 2) {
            ui.text(left, y, &w, &d);
            y += d.line_px();
        }
    }
    y += 16.0;
    let name_st = body(28.0, DARK);
    let why_st = regular(25.0, DARK);
    let rows = ((H - HINT_H - y) / 84.0).floor().max(1.0) as usize;
    for f in r.failed.iter().take(rows) {
        let fit = fit_name(ui, &f.name, &name_st, room);
        ui.text(left, y, &fit, &name_st);
        y += 40.0;
        let fit = fit_name(ui, &f.why, &why_st, room);
        ui.text(left, y, &fit, &why_st);
        y += 44.0;
    }
    if r.failed.len() > rows {
        ui.text(left, H - HINT_H - 34.0, &format!("and {} more", r.failed.len() - rows), &why_st);
    }
    let cy = H - HINT_H / 2.0;
    let w = cap(ui, left, cy, "A", DARK, t.ground) + 10.0;
    ui.text_vcenter(left + w, cy, "OK", &regular(26.0, DARK));
}

fn draw_job(app: &App, ui: &mut Ui) {
    let Some(j) = &app.job else { return };
    if let Some(c) = &j.clash {
        return draw_clash(app, ui, c);
    }
    let t = theme::get();
    let p = &j.progress;
    let frac = if p.bytes_total > 0 {
        p.bytes_done as f32 / p.bytes_total as f32
    } else if p.files_total > 0 {
        p.files_done as f32 / p.files_total as f32
    } else {
        0.0
    }
    .clamp(0.0, 1.0);
    // the whole screen is the progress bar: tiles lighting from the top,
    // in the colours of what's moving
    let (g, tile_s) = (28.0f32, 24.0f32);
    let (cols, rows) = (37usize, 27usize);
    let total = cols * rows;
    let lit = (frac * total as f32).round() as usize;
    for i in 0..lit {
        let (cx, cy) = ((i % cols) as f32, (i / cols) as f32);
        let k = j.kinds.get(i * j.kinds.len().max(1) / total).copied().unwrap_or(Kind::Other);
        let (c, ink) = kind_colors(k);
        let c = if k == Kind::Text || k == Kind::Other { ink } else { c };
        ui.fill_block(Rect::new(-12.0 + cx * g + 2.0, 6.0 + cy * g + 2.0, tile_s, tile_s), c);
    }
    // what's happening, on a solid plate
    let verb = if j.cancelling {
        "Stopping…".to_string()
    } else {
        match j.kind {
            JobKind::Copy => "Copying",
            JobKind::Move => "Moving",
            JobKind::Trash => "To the Trash",
            JobKind::Delete => "Deleting",
            JobKind::Restore => "Restoring",
        }
        .to_string()
    };
    let r = Rect::new(112.0, 236.0, 800.0, 296.0);
    ui.fill_block(Rect::new(r.x + 10.0, r.y + 10.0, r.w, r.h), t.line);
    ui.fill_block(r, t.ground);
    ui.outline(r, 3.0, t.line);
    ui.text(r.x + 36.0, r.y + 20.0, &verb, &serif(76.0, t.line));
    let nums = if p.bytes_total > 0 { format!("{} of {}", human_size(p.bytes_done), human_size(p.bytes_total)) } else { format!("{} of {}", p.files_done, p.files_total) };
    ui.text(r.x + 38.0, r.y + 118.0, &nums, &body(38.0, t.line));
    let st = regular(26.0, t.line_soft);
    let fit = fit_name(ui, &p.current, &st, r.w - 72.0);
    ui.text(r.x + 38.0, r.y + 176.0, &fit, &st);
    let cy = r.bottom() - 42.0;
    let w = cap(ui, r.x + 38.0, cy, "B", cap_ink(), t.ground) + 10.0;
    ui.text_vcenter(r.x + 38.0 + w, cy, if j.cancelling { "Finishing this file" } else { "Stop" }, &regular(24.0, cap_ink()));
}

fn draw_clash(app: &App, ui: &mut Ui, c: &ClashUi) {
    let t = theme::get();
    ui.clear(t.yellow);
    let left = 48.0;
    let st = serif(68.0, DARK);
    let mut y = 28.0;
    for l in wrap(ui, &format!("“{}” is already there.", c.clash.name), &st, W - 2.0 * left, 2) {
        ui.text(left, y, &l, &st);
        y += st.line_px();
    }
    y += 16.0;
    // the two, side by side
    let bw = (W - 2.0 * left - 24.0) / 2.0;
    let side = |ui: &mut Ui, x: f32, title: &str, s: &ops::Side, fill: Color| {
        let r = Rect::new(x, y, bw, 178.0);
        ui.fill_block(r, fill);
        ui.text(x + 20.0, y + 10.0, title, &serif(44.0, DARK));
        let size = if s.is_dir { "A folder".to_string() } else { human_size(s.size) };
        ui.text(x + 20.0, y + 68.0, &size, &body(36.0, DARK));
        ui.text(x + 20.0, y + 122.0, &app.clock.long(s.mtime), &mono(24.0, DARK));
    };
    side(ui, left, "There now", &c.clash.existing, t.orange);
    side(ui, left + bw + 24.0, "Coming in", &c.clash.incoming, t.lime);
    y += 178.0 + 28.0;
    // the choices: big blocks; Skip first, so a stray A changes nothing
    let n = c.clash.options.len() as f32;
    let cw = (W - 2.0 * left - (n - 1.0) * 16.0) / n;
    for (i, o) in c.clash.options.iter().enumerate() {
        let r = Rect::new(left + i as f32 * (cw + 16.0), y, cw, 110.0);
        let danger = *o == Choice::Replace;
        if i == c.sel {
            let fill = if danger { t.orange } else { Color::hex(0xf4f8fd) };
            ui.fill_block(Rect::new(r.x + 10.0, r.y + 10.0, r.w, r.h), DARK);
            ui.fill_block(r, fill);
            ui.outline(r, 3.0, DARK);
            let label = serif(46.0, DARK);
            let tw = ui.measure(choice_word(*o), &label);
            ui.text_vcenter(r.x + (r.w - tw) / 2.0, r.y + r.h / 2.0, choice_word(*o), &label);
        } else {
            ui.fill_block(r, t.yellow.mix(DARK, 0.82));
            let label = body(28.0, DARK);
            let tw = ui.measure(choice_word(*o), &label);
            ui.text_vcenter(r.x + (r.w - tw) / 2.0, r.y + r.h / 2.0, choice_word(*o), &label);
        }
    }
    y += 110.0 + 26.0;
    let cy = y + 22.0;
    tile(ui, left, cy - 22.0, 44.0, if c.for_rest { DARK } else { t.yellow.mix(DARK, 0.82) }, t.lime, if c.for_rest { Glyph::Check } else { Glyph::Kind(Kind::Other) });
    if !c.for_rest {
        ui.fill_block(Rect::new(left, cy - 22.0, 44.0, 44.0), t.yellow.mix(DARK, 0.82));
    }
    ui.text_vcenter(left + 60.0, cy, "Do this for the rest", &regular(27.0, DARK));
    let ccy = H - HINT_H / 2.0;
    let mut x = left;
    for (b, l) in [("◀▶", "Choose"), ("A", "OK"), ("Y", "For the rest"), ("B", "Stop")] {
        x += cap(ui, x, ccy, b, DARK, t.yellow) + 10.0;
        x += ui.text_vcenter(x, ccy, l, &regular(24.0, DARK)) + 26.0;
    }
}
