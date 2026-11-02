// SPDX-License-Identifier: GPL-2.0-or-later
//! The frontend's screens, drawn with the mortar-ui toolkit in the shared
//! theme (style v2, "Pond"): library home, game list, in-game menu, and
//! message screens (missing BIOS, errors, empty folders). Pure drawing from
//! plain view structs, so every screen renders headless for screenshots.

use crate::video::Frame;
use mortar_ui::gfx::{Face, Rect, TextStyle, Ui};
use mortar_ui::icons::Icon;
use mortar_ui::theme;
use mortar_ui::toolkit::{self, Glyph, Hint, ListState, MenuItem, PillKind, Row, RowValue, StatusInfo, hint};

/// Draw `f` scaled (nearest) into `r` of the canvas, keeping its aspect
/// (`aspect` = display aspect, 0 = square pixels), with an ink outline.
pub fn picture(ui: &mut Ui, f: &Frame, r: Rect, aspect: f32) -> Rect {
    if f.is_empty() || r.w < 2.0 || r.h < 2.0 {
        return r;
    }
    let a = if aspect > 0.0 { aspect } else { f.w as f32 / f.h as f32 };
    let (mut w, mut h) = (r.w, r.w / a);
    if h > r.h {
        h = r.h;
        w = h * a;
    }
    let dst = Rect::new(r.x + (r.w - w) / 2.0, r.y + (r.h - h) / 2.0, w, h).snap();
    let (pw, ph) = (ui.pm.width() as i32, ui.pm.height() as i32);
    let data = ui.pm.data_mut();
    for dy in 0..dst.h as i32 {
        let y = dst.y as i32 + dy;
        if y < 0 || y >= ph {
            continue;
        }
        let sy = ((dy as f32 + 0.5) * f.h as f32 / dst.h) as u32;
        let row = &f.px[(sy.min(f.h - 1) * f.w) as usize..];
        for dx in 0..dst.w as i32 {
            let x = dst.x as i32 + dx;
            if x < 0 || x >= pw {
                continue;
            }
            let sx = ((dx as f32 + 0.5) * f.w as f32 / dst.w) as u32;
            let p = row[sx.min(f.w - 1) as usize];
            let o = ((y * pw + x) * 4) as usize;
            data[o] = (p >> 16) as u8;
            data[o + 1] = (p >> 8) as u8;
            data[o + 2] = p as u8;
            data[o + 3] = 255;
        }
    }
    let hw = toolkit::hair(ui);
    ui.outline(dst.inset(-hw, -hw), hw, theme::get().line);
    dst
}

/// Wrap `text` to `max_w` (words, and long words such as paths broken
/// after a '/' or anywhere). Returns the height used.
pub fn para(ui: &mut Ui, x: f32, y: f32, max_w: f32, text: &str, st: &TextStyle) -> f32 {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let cand = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
        if ui.measure(&cand, st) <= max_w {
            cur = cand;
            continue;
        }
        if !cur.is_empty() {
            lines.push(std::mem::take(&mut cur));
        }
        // the word alone may still be too long: break it
        let mut rest = word.to_string();
        while ui.measure(&rest, st) > max_w {
            let chars: Vec<char> = rest.chars().collect();
            let mut n = chars.len();
            while n > 1 && ui.measure(&chars[..n].iter().collect::<String>(), st) > max_w {
                n -= 1;
            }
            // prefer a break right after a '/'
            if let Some(p) = chars[..n].iter().rposition(|c| *c == '/')
                && p > n / 2
            {
                n = p + 1;
            }
            lines.push(chars[..n].iter().collect());
            rest = chars[n..].iter().collect();
        }
        cur = rest;
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    for (i, l) in lines.iter().enumerate() {
        ui.text(x, y + i as f32 * st.line_px(), l, st);
    }
    lines.len() as f32 * st.line_px()
}

fn soft(ui: &Ui, cq: f32) -> TextStyle {
    toolkit::st_body(ui, cq, theme::get().line_soft)
}

// ------------------------------------------------------------------ home ----

#[derive(Clone, Debug)]
pub struct HomeEntry {
    pub icon: Icon,
    pub title: String,
    pub desc: String,
    pub pill: Option<(String, PillKind)>,
    pub dimmed: bool,
}

pub struct HomeView<'a> {
    pub status: &'a StatusInfo,
    pub entries: &'a [HomeEntry],
    /// Picture for the selected entry (resume thumbnail), with its caption.
    pub preview: Option<(&'a Frame, String, String)>,
    /// Lines for the right column when there is no picture.
    pub info: Vec<String>,
    pub can_quit: bool,
}

pub fn home(ui: &mut Ui, v: &HomeView, list: &mut ListState) {
    let mut hints = vec![hint("A", "Open")];
    if v.can_quit {
        hints.insert(0, hint("B", "Quit"));
    }
    let body = toolkit::frame(ui, v.status, &hints);
    let gap = ui.cq(4.0);
    let lw = (body.w - gap) * 0.56;
    let left = Rect::new(body.x, body.y, lw, body.h);
    let right = Rect::new(body.x + lw + gap, body.y, body.w - lw - gap, body.h);

    let th = toolkit::title_h1(ui, left.x, left.y, "Games");
    let (_, rest) = left.take_top(th + ui.cq(1.6));
    let items: Vec<MenuItem> = v
        .entries
        .iter()
        .enumerate()
        .map(|(i, e)| MenuItem {
            glyph: Glyph::Icon(e.icon),
            title: e.title.clone(),
            desc: Some(e.desc.clone()),
            pill: e.pill.clone(),
            selected: i == list.sel,
            dimmed: e.dimmed,
        })
        .collect();
    toolkit::menu(ui, rest, 1.0, &items, list);

    side_panel(ui, right, v.preview.as_ref().map(|(f, a, b)| (*f, a.as_str(), b.as_str())), &v.info, th);
}

/// The right column: a resume picture with caption, or info lines over the
/// isometric illustration.
fn side_panel(ui: &mut Ui, right: Rect, preview: Option<(&Frame, &str, &str)>, info: &[String], top: f32) {
    let t = theme::get();
    let y0 = right.y + top + ui.cq(1.6);
    match preview {
        Some((f, title, sub)) => {
            let lh = toolkit::label(ui, right.x, y0, "Resume");
            let pr = Rect::new(right.x, y0 + lh + ui.cq(1.0), right.w, right.w * 0.75);
            let shown = picture(ui, f, pr, 0.0);
            let mut y = shown.bottom() + ui.cq(2.0);
            let st = TextStyle::new(Face::SerifItalic, ui.cq(3.6), t.line).line_height(1.1);
            ui.text_fit(right.x, y, title, &st, right.w);
            y += st.line_px();
            ui.text_fit(right.x, y, sub, &soft(ui, 2.0), right.w);
        }
        None => {
            let pr = Rect::new(right.x, y0, right.w, right.h - top);
            toolkit::iso_blocks(ui, pr.x + pr.w / 2.0, pr.y + pr.w * 0.28, pr.w * 0.62);
            let mut y = pr.y + pr.w * 0.62;
            for l in info {
                let st = if l.starts_with('/') { toolkit::st_mono(ui, 1.9, theme::get().line) } else { soft(ui, 2.1) };
                y += para(ui, pr.x, y, pr.w, l, &st) + ui.cq(0.8);
            }
        }
    }
}

// ----------------------------------------------------------------- games ----

pub struct GamesView<'a> {
    pub status: &'a StatusInfo,
    pub system: &'a str,
    pub tag: &'a str,
    pub games: &'a [(String, String, bool)],
    pub preview: Option<&'a Frame>,
    pub info: Vec<String>,
}

pub fn games(ui: &mut Ui, v: &GamesView, list: &mut ListState) {
    let hints = [hint("L1/R1", "Page"), hint("B", "Back"), hint("A", "Play")];
    let body = toolkit::frame(ui, v.status, &hints);
    let gap = ui.cq(4.0);
    let lw = (body.w - gap) * 0.62;
    let left = Rect::new(body.x, body.y, lw, body.h);
    let right = Rect::new(body.x + lw + gap, body.y, body.w - lw - gap, body.h);
    let mut y = left.y;
    let ch = toolkit::crumb(ui, left.x, y, &format!("Games / {}", v.tag));
    let n = v.games.len();
    toolkit::tag(ui, left.right(), y + ch / 2.0, &if n == 1 { "1 game".to_string() } else { format!("{n} games") }, PillKind::Lavender);
    y += ch;
    let st = TextStyle::new(Face::SerifItalic, ui.cq(5.2), theme::get().line).line_height(1.1);
    ui.text_fit(left.x, y, v.system, &st, body.w);
    y += st.line_px() + ui.cq(1.2);
    let rows: Vec<Row> = v
        .games
        .iter()
        .enumerate()
        .map(|(i, (name, tags, resumable))| {
            let mut r = Row::new(name, if tags.is_empty() { RowValue::None } else { RowValue::Text(tags.clone()) });
            if *resumable {
                r.desc = Some("Resume where you left off".into());
            }
            r.selected = i == list.sel;
            r
        })
        .collect();
    toolkit::rows(ui, Rect::new(left.x, y, left.w, left.bottom() - y), &rows, list);
    let top = y - left.y - ui.cq(1.6);
    side_panel(ui, right, v.preview.map(|f| (f, "", "")), &v.info, top);
}

// ------------------------------------------------------------ game menu ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    Resume,
    Save,
    Load,
    Disc,
    Display,
    Reset,
    Quit,
}

pub struct MenuView<'a> {
    pub status: &'a StatusInfo,
    pub system: &'a str,
    pub game: &'a str,
    pub items: &'a [(MenuAction, String, Option<String>)],
    pub slot: u8,
    /// The Display choice's name ("Sharp bilinear").
    pub display: String,
    /// Picture on the right: the slot's thumbnail, or the paused game.
    pub picture: Option<&'a Frame>,
    pub aspect: f32,
    pub caption: String,
    pub toast: Option<&'a str>,
}

pub fn game_menu(ui: &mut Ui, v: &MenuView, list: &mut ListState) {
    let on_display = v.items.get(list.sel).is_some_and(|i| i.0 == MenuAction::Display);
    let hints = if on_display {
        vec![hint("←/→", "Display"), hint("B", "Resume"), hint("A", "Keep for game"), hint("X", "For all")]
    } else {
        vec![hint("←/→", "Slot"), hint("B", "Resume"), hint("A", "Select")]
    };
    let body = toolkit::frame(ui, v.status, &hints);
    let gap = ui.cq(4.0);
    let lw = (body.w - gap) * 0.46;
    let left = Rect::new(body.x, body.y, lw, body.h);
    let right = Rect::new(body.x + lw + gap, body.y, body.w - lw - gap, body.h);
    let mut y = left.y;
    y += toolkit::crumb(ui, left.x, y, v.system);
    let st = TextStyle::new(Face::SerifItalic, ui.cq(5.2), theme::get().line).line_height(1.1);
    ui.text_fit(left.x, y, v.game, &st, body.w);
    y += st.line_px() + ui.cq(1.6);
    let rows: Vec<Row> = v
        .items
        .iter()
        .enumerate()
        .map(|(i, (a, title, desc))| {
            let value = match a {
                MenuAction::Save | MenuAction::Load => RowValue::Text(format!("‹ slot {} ›", v.slot + 1)),
                MenuAction::Display => RowValue::Text(format!("‹ {} ›", v.display)),
                _ => RowValue::None,
            };
            let mut r = Row::new(title, value);
            r.desc = desc.clone();
            r.selected = i == list.sel;
            r
        })
        .collect();
    toolkit::rows(ui, Rect::new(left.x, y, left.w, left.bottom() - y), &rows, list);

    let pr = Rect::new(right.x, y, right.w, right.w * 0.75);
    match v.picture {
        Some(f) => {
            let shown = picture(ui, f, pr, v.aspect);
            ui.text_fit(right.x, shown.bottom() + ui.cq(1.6), &v.caption, &soft(ui, 2.1), right.w);
        }
        None => {
            toolkit::panel(ui, pr);
            let st = toolkit::st_mono(ui, 2.0, theme::get().line_soft);
            let w = ui.measure(&v.caption, &st);
            ui.text_vcenter(pr.x + (pr.w - w) / 2.0, pr.y + pr.h / 2.0, &v.caption, &st);
        }
    }
    if let Some(t) = v.toast {
        toolkit::toast(ui, t);
    }
}

// -------------------------------------------------------------- message ----

pub struct MessageView<'a> {
    pub status: &'a StatusInfo,
    pub crumb: &'a str,
    pub title: &'a str,
    pub lines: &'a [String],
    pub hints: Vec<Hint>,
    /// Attention tag next to the title (e.g. "BIOS").
    pub tag: Option<(&'a str, PillKind)>,
}

pub fn message(ui: &mut Ui, v: &MessageView) {
    let body = toolkit::frame(ui, v.status, &v.hints);
    let mut y = body.y;
    y += toolkit::crumb(ui, body.x, y, v.crumb);
    let th = toolkit::title_h1(ui, body.x, y, v.title);
    if let Some((t, k)) = v.tag {
        toolkit::tag(ui, body.right(), y + th / 2.0, t, k);
    }
    y += th + ui.cq(2.0);
    let w = body.w * 0.66;
    toolkit::iso_blocks(ui, body.x + body.w * 0.87, body.bottom() - body.w * 0.16, body.w * 0.2);
    for l in v.lines {
        let st = if l.starts_with('/') || l.starts_with("~/") || l.starts_with("ssh ") || l.starts_with("scp ") {
            toolkit::st_mono(ui, 2.2, theme::get().line)
        } else {
            soft(ui, 2.5)
        };
        y += para(ui, body.x, y, w, l, &st) + ui.cq(1.2);
    }
}
