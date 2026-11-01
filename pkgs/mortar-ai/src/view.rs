// SPDX-License-Identifier: GPL-2.0-or-later
//! Drawing: the transcript (wrapped, scrolled from the bottom), the compose
//! field and the permission card, with the mortar-ui toolkit and theme.

use crate::model::{Entry, Model, NoticeKind, ToolState};
use mortar_ui::gfx::{Color, Face, Rect, TextStyle, Ui};
use mortar_ui::theme;
use mortar_ui::toolkit::{self, Hint, PillKind, StatusInfo, hint};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Sty {
    User,
    Assistant,
    Code,
    ToolHead,
    ToolOut,
    Info,
    Error,
    Turn,
}

#[derive(Clone, Debug)]
struct VLine {
    text: String,
    sty: Sty,
    /// Coloured block on the left (tool state dot, user bar).
    mark: Option<Color>,
    /// Extra space above this line (first line of an entry).
    gap: f32,
}

#[derive(Default)]
pub struct View {
    cache: HashMap<usize, (u64, u32, Vec<VLine>)>,
    page: usize,
}

fn style(ui: &Ui, s: Sty) -> TextStyle {
    let t = theme::get();
    match s {
        Sty::User => TextStyle::new(Face::Body500, ui.cq(2.5), t.line),
        Sty::Assistant => TextStyle::new(Face::Body400, ui.cq(2.4), t.line).line_height(1.3),
        Sty::Code => TextStyle::new(Face::Mono400, ui.cq(2.1), t.line),
        Sty::ToolHead => TextStyle::new(Face::Mono600, ui.cq(2.1), t.line),
        Sty::ToolOut => TextStyle::new(Face::Mono400, ui.cq(1.9), t.line_soft),
        Sty::Info => TextStyle::new(Face::Body400, ui.cq(2.1), t.line_soft),
        Sty::Error => TextStyle::new(Face::Body500, ui.cq(2.2), t.line),
        Sty::Turn => TextStyle::new(Face::Mono400, ui.cq(1.7), t.line_faint),
    }
}

/// Left indent of the text (marks sit in the gutter).
fn indent(ui: &Ui, s: Sty) -> f32 {
    match s {
        Sty::ToolOut => ui.cq(4.6),
        Sty::Code => ui.cq(3.0),
        _ => ui.cq(2.6),
    }
}

/// Word-wrap `text` to `max_w`, hard-breaking words that don't fit.
pub fn wrap(ui: &Ui, text: &str, st: &TextStyle, max_w: f32) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut cur = String::new();
        // keep leading indentation (code, lists)
        let lead: String = para.chars().take_while(|c| *c == ' ').collect();
        cur.push_str(&lead);
        for word in para.split(' ').filter(|w| !w.is_empty()) {
            let cand = if cur.trim().is_empty() { format!("{cur}{word}") } else { format!("{cur} {word}") };
            if ui.measure(&cand, st) <= max_w {
                cur = cand;
                continue;
            }
            if !cur.trim().is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            // the word alone may still be too wide
            let mut piece = String::new();
            for ch in word.chars() {
                piece.push(ch);
                if ui.measure(&piece, st) > max_w && piece.chars().count() > 1 {
                    piece.pop();
                    out.push(std::mem::take(&mut piece));
                    piece.push(ch);
                }
            }
            cur = piece;
        }
        out.push(cur);
    }
    out
}

/// Wrap without touching spaces (command output, code): hard breaks only.
pub fn wrap_raw(ui: &Ui, text: &str, st: &TextStyle, max_w: f32) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        let para = para.replace('\t', "    ");
        let mut cur = String::new();
        for ch in para.chars() {
            cur.push(ch);
            if ui.measure(&cur, st) > max_w && cur.chars().count() > 1 {
                cur.pop();
                out.push(std::mem::take(&mut cur));
                cur.push(ch);
            }
        }
        out.push(cur);
    }
    out
}

fn entry_hash(e: &Entry) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    match e {
        Entry::User(s) => (0, s).hash(&mut h),
        Entry::Assistant { text, .. } => (1, text).hash(&mut h),
        Entry::Tool { name, summary, state, output, .. } => {
            (2, name, summary, output).hash(&mut h);
            format!("{state:?}").hash(&mut h);
        }
        Entry::Notice(k, s) => (3, format!("{k:?}"), s).hash(&mut h),
        Entry::Turn(s) => (4, s).hash(&mut h),
    }
    h.finish()
}

fn state_color(s: &ToolState) -> Color {
    let t = theme::get();
    match s {
        ToolState::Running => t.yellow,
        ToolState::Waiting => t.lavender,
        ToolState::Ok => t.lime,
        ToolState::Denied | ToolState::Error => t.orange,
    }
}

fn layout(ui: &Ui, e: &Entry, w: f32) -> Vec<VLine> {
    let t = theme::get();
    let gap = ui.cq(1.6);
    let mut out: Vec<VLine> = Vec::new();
    let add = |out: &mut Vec<VLine>, text: &str, sty: Sty, mark: Option<Color>| {
        let st = style(ui, sty);
        let lines = if matches!(sty, Sty::Code | Sty::ToolOut) {
            wrap_raw(ui, text, &st, w - indent(ui, sty))
        } else {
            wrap(ui, text, &st, w - indent(ui, sty))
        };
        for (i, l) in lines.into_iter().enumerate() {
            out.push(VLine { text: l, sty, mark: if i == 0 || sty == Sty::User { mark } else { None }, gap: 0.0 });
        }
    };
    match e {
        Entry::User(s) => add(&mut out, s, Sty::User, Some(t.lavender)),
        Entry::Assistant { text, .. } => {
            let mut code = false;
            for line in text.trim_end().split('\n') {
                if line.trim_start().starts_with("```") {
                    code = !code;
                    continue;
                }
                let sty = if code { Sty::Code } else { Sty::Assistant };
                let l = if code { line.to_string() } else { line.trim_start_matches('#').trim_start().replace("**", "") };
                add(&mut out, &l, sty, None);
            }
        }
        Entry::Tool { name, summary, state, output, .. } => {
            let tag = match state {
                ToolState::Waiting => "  · needs you",
                ToolState::Denied => "  · denied",
                _ => "",
            };
            add(&mut out, &format!("{name}  {summary}{tag}"), Sty::ToolHead, Some(state_color(state)));
            if !output.is_empty() && *state != ToolState::Denied {
                add(&mut out, output, Sty::ToolOut, None);
            }
        }
        Entry::Notice(NoticeKind::Info, s) => add(&mut out, s, Sty::Info, None),
        Entry::Notice(NoticeKind::Error, s) => add(&mut out, s, Sty::Error, Some(t.orange)),
        Entry::Turn(s) => add(&mut out, s, Sty::Turn, None),
    }
    if let Some(f) = out.first_mut() {
        f.gap = gap;
    }
    out
}

fn mono(ui: &Ui, cq: f32, c: Color) -> TextStyle {
    TextStyle::new(Face::Mono400, ui.cq(cq), c)
}

impl View {
    /// Lines in one screen of transcript (for PgUp/PgDn).
    pub fn page_lines(&self) -> usize {
        self.page.max(3)
    }

    pub fn hints(m: &Model) -> Vec<Hint> {
        if let Some(p) = m.pending.front() {
            let mut h = vec![hint("A", "Allow"), hint("B", "Deny")];
            if p.always.is_some() {
                h.push(hint("X", "Always"));
            }
            h.push(hint("↑↓", "Scroll"));
            h
        } else if !m.alive {
            vec![hint("A", "Start again"), hint("MENU", "Apps")]
        } else if m.busy {
            vec![hint("L2", "Stop"), hint("START", "Keyboard"), hint("↑↓", "Scroll")]
        } else {
            vec![hint("START", "Keyboard"), hint("A", "Send"), hint("B", "Delete"), hint("↑↓", "Scroll")]
        }
    }

    pub fn draw(&mut self, ui: &mut Ui, m: &mut Model, status: &StatusInfo) {
        let t = theme::get();
        let hints = Self::hints(m);
        ui.clear(t.ground);
        toolkit::status_bar(ui, status);
        toolkit::hint_bar(ui, &hints);
        // left of the hint bar: model and folder
        let hb = toolkit::hint_bar_h(ui);
        let info = [m.model_name.as_str(), short_path(&m.cwd).as_str()]
            .iter()
            .filter(|s| !s.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(" · ");
        let info = if info.is_empty() { "Claude Code".to_string() } else { info };
        let st = mono(ui, 1.8, t.line_soft);
        ui.text_fit(ui.cq(3.5), ui.height() - hb / 2.0 - st.line_px() / 2.0, &info, &st, ui.cq(36.0));

        let side = ui.cq(4.0);
        let top = toolkit::status_bar_h(ui) + ui.cq(0.5);
        let bottom = ui.height() - hb - ui.cq(1.5);
        let w = ui.width() - 2.0 * side;

        // bottom block: permission card, or the compose field
        let card_h = if let Some(p) = m.pending.front().cloned() {
            self.permission_card(ui, side, bottom, w, &p, m.pending.len(), bottom - top)
        } else {
            self.compose(ui, side, bottom, w, m)
        };
        let body = Rect::new(side, top, w, (bottom - card_h - ui.cq(1.5)) - top);
        self.transcript(ui, body, m);
        if let Some(tst) = &m.toast {
            toolkit::toast(ui, tst);
        }
    }

    fn transcript(&mut self, ui: &mut Ui, body: Rect, m: &mut Model) {
        let t = theme::get();
        if m.entries.is_empty() {
            let cx = body.x + body.w / 2.0;
            toolkit::iso_blocks(ui, cx, body.y + body.h * 0.32, ui.cq(16.0));
            let y = body.y + body.h * 0.55;
            let st = TextStyle::new(Face::SerifItalic, ui.cq(5.2), t.line);
            let title = "Ask Claude";
            let tw = ui.measure(title, &st);
            ui.text(cx - tw / 2.0, y, title, &st);
            let p = TextStyle::new(Face::Body400, ui.cq(2.2), t.line_soft).line_height(1.4);
            let msg = "START opens the keyboard, A sends. When Claude wants to run a command or change a file, A allows it and B denies it.";
            let pw = ui.cq(60.0);
            toolkit::paragraph(ui, cx - pw / 2.0, y + st.line_px() + ui.cq(1.0), pw, msg, &p);
            self.page = 10;
            return;
        }
        // lay out from the newest entry until the screen (plus scroll) is full
        let w = body.w;
        let wkey = w as u32;
        let mut lines: Vec<VLine> = Vec::new();
        let mut need = body.h + m.scroll as f32 * ui.cq(3.0) + ui.cq(10.0);
        let mut reached_start = true;
        let mut used = 0.0;
        for (i, e) in m.entries.iter().enumerate().rev() {
            let h = entry_hash(e);
            let ok = matches!(self.cache.get(&i), Some((hh, ww, _)) if *hh == h && *ww == wkey);
            if !ok {
                self.cache.insert(i, (h, wkey, layout(ui, e, w)));
            }
            let ls = &self.cache[&i].2;
            for l in ls.iter().rev() {
                used += style(ui, l.sty).line_px() + l.gap;
                lines.push(l.clone());
            }
            if used > need {
                reached_start = i == 0;
                break;
            }
            need -= 0.0;
        }
        self.cache.retain(|k, _| *k < m.entries.len());
        lines.reverse();
        let n = lines.len();
        // average line height for the page size
        let avg = if n > 0 { used / n as f32 } else { ui.cq(3.0) };
        self.page = (body.h / avg).floor() as usize;
        // clamp the scroll so the first line can reach the top, not beyond
        let fits = {
            let mut acc = 0.0;
            let mut k = 0;
            for l in lines.iter().rev() {
                acc += style(ui, l.sty).line_px() + l.gap;
                if acc > body.h {
                    break;
                }
                k += 1;
            }
            k
        };
        if reached_start {
            m.scroll = m.scroll.min(n.saturating_sub(fits));
        }
        let end = n.saturating_sub(m.scroll);
        // draw upwards from the bottom of the body
        let mut y = body.bottom();
        let mut idx = end;
        while idx > 0 {
            let l = &lines[idx - 1];
            let st = style(ui, l.sty);
            let h = st.line_px();
            if y - h < body.y {
                break;
            }
            let top = y - h;
            if let Some(c) = l.mark {
                match l.sty {
                    Sty::User => ui.fill_block(Rect::new(body.x, top, ui.cq(0.6), h), c),
                    _ => {
                        let s = ui.cq(1.3);
                        let r = Rect::new(body.x + ui.cq(0.2), top + (h - s) / 2.0, s, s);
                        toolkit::block(ui, r, c);
                    }
                }
            }
            if l.sty == Sty::Code {
                ui.fill_block(Rect::new(body.x + ui.cq(2.0), top, body.w - ui.cq(2.0), h), t.ground_2);
            }
            ui.text(body.x + indent(ui, l.sty), top, &l.text, &st);
            y = top - l.gap;
            idx -= 1;
        }
        if m.scroll > 0 {
            let r = body.right();
            toolkit::tag(ui, r, body.y + ui.cq(1.5), &format!("↑ {} lines · END", m.scroll), PillKind::Lavender);
        }
    }

    fn compose(&mut self, ui: &mut Ui, x: f32, bottom: f32, w: f32, m: &Model) -> f32 {
        let t = theme::get();
        let st = TextStyle::new(Face::Mono400, ui.cq(2.4), t.line);
        let pad = ui.cq(1.6);
        let inner = w - 2.0 * pad - ui.cq(2.0);
        let mut lines = if m.input.is_empty() { vec![String::new()] } else { wrap(ui, &m.input, &st, inner) };
        if lines.len() > 4 {
            lines.drain(..lines.len() - 4);
        }
        let h = lines.len() as f32 * st.line_px() + 2.0 * pad;
        let r = Rect::new(x, bottom - h, w, h);
        let enabled = m.alive;
        toolkit::block(ui, r, if enabled { t.ground } else { t.ground_2 });
        let mut y = r.y + pad;
        let mut caret_x = x + pad;
        for (i, l) in lines.iter().enumerate() {
            let tw = ui.text(x + pad, y, l, &st);
            if i + 1 == lines.len() {
                caret_x = x + pad + tw;
            }
            if i + 1 < lines.len() {
                y += st.line_px();
            }
        }
        if m.input.is_empty() {
            let ph = mono(ui, 2.2, t.line_faint);
            let msg = if !m.alive {
                "Claude Code is not running. A starts it."
            } else if m.busy {
                "Claude is working. You can type the next message."
            } else {
                "Ask Claude…  (START: keyboard)"
            };
            ui.text_vcenter(x + pad + ui.cq(2.0), y + st.line_px() / 2.0, msg, &ph);
        }
        if enabled {
            let ch = st.line_px() * 0.8;
            let cr = Rect::new(caret_x + ui.cq(0.2), y + (st.line_px() - ch) / 2.0, ui.cq(1.2), ch);
            ui.fill_block(cr, t.lime);
            ui.outline(cr, toolkit::hair(ui), t.line);
        }
        // activity tag on the field's top edge
        if let Some(a) = &m.activity {
            if m.busy || !m.pending.is_empty() {
                toolkit::tag(ui, x + w, r.y, a, PillKind::Amber);
            }
        }
        h
    }

    fn permission_card(&mut self, ui: &mut Ui, x: f32, bottom: f32, w: f32, p: &crate::model::Permission, count: usize, max_h: f32) -> f32 {
        let t = theme::get();
        let pad = ui.cq(2.4);
        let title = TextStyle::new(Face::Body600, ui.cq(2.9), t.line);
        let det = TextStyle::new(Face::Mono400, ui.cq(2.2), t.line);
        let inner = w - 2.0 * pad;
        let mut dl = wrap_raw(ui, &p.detail, &det, inner - ui.cq(2.0));
        let mut tl = wrap(ui, &p.title, &title, inner);
        tl.truncate(2);
        let lbl = TextStyle::new(Face::Mono600, ui.cq(1.8), t.line_soft);
        let btn_h = ui.cq(4.6);
        let fixed_for = |n: usize| {
            pad + lbl.line_px() + ui.cq(0.8) + n as f32 * title.line_px() + ui.cq(1.2) + 2.0 * ui.cq(1.2) + ui.cq(1.6) + btn_h + pad
        };
        // as many detail lines as fit (the on-screen keyboard halves the
        // window); with little room the title goes first, the command stays
        let fit = |n: usize| ((max_h - fixed_for(n)) / det.line_px()).floor().max(0.0) as usize;
        while !tl.is_empty() && fit(tl.len()) < dl.len().min(3) {
            tl.pop();
        }
        let tl = &tl[..];
        let fixed = fixed_for(tl.len());
        let max = fit(tl.len()).clamp(1, 9);
        if dl.len() > max {
            let more = dl.len() - max;
            dl.truncate(max);
            if let Some(l) = dl.last_mut() {
                l.push_str(&format!(" … (+{more})"));
            }
        }
        let h = fixed + dl.len() as f32 * det.line_px();
        let r = Rect::new(x, bottom - h, w, h);
        toolkit::block(ui, r, t.ground);
        // a lavender band on the left marks it as "needs you"
        ui.fill_block(Rect::new(x, r.y, ui.cq(1.0), h), t.lavender);
        ui.outline(r, toolkit::hair(ui), t.line);
        let cx = x + pad;
        let mut y = r.y + pad;
        let head = if count > 1 { format!("PERMISSION · {} · 1 OF {count}", p.tool.to_uppercase()) } else { format!("PERMISSION · {}", p.tool.to_uppercase()) };
        ui.text(cx, y, &head, &lbl.tracking(0.08));
        y += lbl.line_px() + ui.cq(0.8);
        for l in tl {
            ui.text(cx, y, l, &title);
            y += title.line_px();
        }
        y += ui.cq(1.2);
        let dh = dl.len() as f32 * det.line_px() + 2.0 * ui.cq(1.2);
        toolkit::panel(ui, Rect::new(cx, y, inner, dh));
        let mut dy = y + ui.cq(1.2);
        for l in &dl {
            let c = if l.starts_with("- ") {
                t.orange_ink
            } else if l.starts_with("+ ") {
                t.lime_ink
            } else {
                t.line
            };
            if l.starts_with("- ") || l.starts_with("+ ") {
                let bg = if l.starts_with("- ") { t.orange } else { t.lime };
                ui.fill_block(Rect::new(cx + ui.cq(0.5), dy, inner - ui.cq(1.0), det.line_px()), bg.mix(t.ground_2, 0.45));
            }
            ui.text(cx + ui.cq(1.0), dy, l, &det.color(c));
            dy += det.line_px();
        }
        y += dh + ui.cq(1.6);
        // buttons: A Allow (lime), B Deny (orange), X Always (lavender)
        let mut bx = cx;
        let mut button = |ui: &mut Ui, cap: &str, label: &str, fill: Color, ink: Color| {
            let lst = TextStyle::new(Face::Body600, ui.cq(2.3), ink);
            let bw = ui.cq(3.2) + ui.cq(1.2) + ui.measure(label, &lst) + ui.cq(3.4);
            let br = Rect::new(bx, y, bw.min(cx + inner - bx), btn_h);
            toolkit::block(ui, br, fill);
            let cw = toolkit::button_cap(ui, bx + ui.cq(1.2), y + btn_h / 2.0, cap);
            ui.text_fit(bx + ui.cq(1.2) + cw + ui.cq(1.0), y + (btn_h - lst.line_px()) / 2.0, label, &lst, br.w - cw - ui.cq(3.4));
            bx += bw + ui.cq(1.6);
        };
        button(ui, "A", "Allow", t.lime, t.lime_ink);
        button(ui, "B", "Deny", t.orange, t.orange_ink);
        if let Some(a) = &p.always {
            button(ui, "X", &format!("Always: {a}"), t.lavender, t.lavender_ink);
        }
        h
    }
}

fn short_path(p: &str) -> String {
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
        && let Some(rest) = p.strip_prefix(&home)
    {
        return format!("~{rest}");
    }
    p.to_string()
}
