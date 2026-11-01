// SPDX-License-Identifier: GPL-2.0-or-later
//! The suggestion strip along the top of the on-screen keyboard: chips with
//! completions for the word being typed in the terminal, from zsh's own
//! completion system, and the history suggestion first.
//!
//! zsh (pkgs/mortar-shell/zsh/mortar-osk.zsh) connects to the shell's
//! `mortar-osk.sock` (comp/suggest.rs). While the OSK is open over its
//! terminal the shell sends `q SEQ` shortly after the last key it typed,
//! and zsh answers one JSON line `{"seq":SEQ,"chips":[{"k","d","t","l"}]}`
//! (kind, backspaces, text to type, label). With the OSK closed nothing is
//! sent and zsh does nothing.
//!
//! Pad: Up from the top row of keys moves into the strip, Left/Right move
//! along it, A types the chip, Down goes back to the keys. R1 types the
//! first chip from anywhere (the history suggestion when there is one,
//! like → in fish). Everything else (B, X, Y, L1, R2, L2, START) keeps
//! its keyboard meaning.
//!
//! This file is the model, the protocol parser and the drawing (pure,
//! unit-tested); the socket is in comp/suggest.rs.

use crate::osk::{self, Tap};
use mortar_ui::gfx::{Color, Face, Rect, TextStyle, Ui};
use mortar_ui::input::Button;
use mortar_ui::toolkit;
use serde::Deserialize;

/// Most chips kept from one answer.
pub const MAX_CHIPS: usize = 12;
/// Longest text a chip may type, and most backspaces before it.
const MAX_TEXT: usize = 512;
const MAX_DEL: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    History,
    Command,
    Dir,
    File,
    Option,
    Other,
}

impl Kind {
    fn parse(s: &str) -> Kind {
        match s {
            "h" => Kind::History,
            "c" => Kind::Command,
            "d" => Kind::Dir,
            "f" => Kind::File,
            "o" => Kind::Option,
            _ => Kind::Other,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chip {
    pub kind: Kind,
    /// Backspaces before typing (the completion replaces the word).
    pub del: usize,
    /// What to type.
    pub text: String,
    pub label: String,
}

impl Chip {
    /// The keys that apply it: `del` backspaces, then the text.
    pub fn taps(&self) -> Vec<Tap> {
        let bs = Tap { code: osk::k::BACKSPACE, shift: false, ctrl: false, alt: false };
        std::iter::repeat_n(bs, self.del).chain(self.text.chars().filter_map(osk::char_tap)).collect()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Reply {
    Chips(u64, Vec<Chip>),
    /// zsh still had keys to read: ask again.
    Retry(u64),
}

#[derive(Deserialize)]
struct Wire {
    seq: u64,
    #[serde(default)]
    retry: bool,
    #[serde(default)]
    chips: Vec<WireChip>,
}

#[derive(Deserialize)]
struct WireChip {
    k: String,
    #[serde(default)]
    d: usize,
    t: String,
    #[serde(default)]
    l: String,
}

/// One line from zsh. Chips the keyboard cannot type (non-ASCII, control
/// characters) or that are oversized are dropped.
pub fn parse(line: &str) -> Option<Reply> {
    let w: Wire = serde_json::from_str(line).ok()?;
    if w.retry {
        return Some(Reply::Retry(w.seq));
    }
    let chips = w
        .chips
        .into_iter()
        .filter(|c| !c.t.is_empty() && c.t.len() <= MAX_TEXT && c.d <= MAX_DEL)
        .filter(|c| c.t.chars().all(|ch| ch != '\t' && osk::char_tap(ch).is_some()))
        .take(MAX_CHIPS)
        .map(|c| {
            let label = if c.l.is_empty() { c.t.trim_end().to_string() } else { c.l.chars().filter(|ch| !ch.is_control()).collect() };
            Chip { kind: Kind::parse(&c.k), del: c.d, text: c.t, label }
        })
        .collect();
    Some(Reply::Chips(w.seq, chips))
}

/// What a pad button did to the strip.
#[derive(Debug, PartialEq, Eq)]
pub enum Nav {
    /// Not ours: the keyboard handles it.
    Pass,
    /// Moved in or along the strip.
    Moved,
    /// Type this chip.
    Accept(Chip),
}

#[derive(Default, Debug)]
pub struct Strip {
    pub chips: Vec<Chip>,
    /// The highlighted chip while the strip has the cursor.
    pub focus: Option<usize>,
}

impl Strip {
    pub fn set(&mut self, chips: Vec<Chip>) {
        self.chips = chips;
        if self.chips.is_empty() {
            self.focus = None;
        } else if let Some(f) = self.focus.as_mut() {
            *f = (*f).min(self.chips.len() - 1);
        }
    }

    pub fn clear(&mut self) {
        self.chips.clear();
        self.focus = None;
    }

    /// A pad button while the OSK is up; `at_top`: the key cursor is on
    /// the top row.
    pub fn button(&mut self, b: Button, at_top: bool) -> Nav {
        use Button::*;
        let n = self.chips.len();
        match (self.focus, b) {
            (_, R1) if n > 0 => {
                let c = self.chips[self.focus.unwrap_or(0)].clone();
                self.focus = None;
                Nav::Accept(c)
            }
            (Some(i), Left) => {
                self.focus = Some((i + n - 1) % n);
                Nav::Moved
            }
            (Some(i), Right) => {
                self.focus = Some((i + 1) % n);
                Nav::Moved
            }
            (Some(i), A) => {
                self.focus = None;
                Nav::Accept(self.chips[i].clone())
            }
            (Some(_), Down) => {
                self.focus = None;
                Nav::Moved
            }
            (Some(_), Up) => Nav::Moved,
            (None, Up) if at_top && n > 0 => {
                self.focus = Some(0);
                Nav::Moved
            }
            _ => Nav::Pass,
        }
    }
}

// ------------------------------------------------------------- drawing ----

/// Height of the strip.
pub fn strip_h(ui: &Ui) -> f32 {
    ui.cq(5.0).round()
}

const PINK: Color = Color::hex(0xff77a8);

/// The kind's tag: glyph, fill, ink.
fn tag(kind: Kind) -> (&'static str, Color, Color) {
    let t = mortar_ui::theme::get();
    match kind {
        Kind::History => ("↑", t.yellow, t.yellow_ink),
        Kind::Command => ("›", t.lavender, t.lavender_ink),
        Kind::Dir => ("/", t.orange, t.orange_ink),
        Kind::Option => ("-", PINK, Color::hex(0x1d1d1d)),
        Kind::File | Kind::Other => ("·", t.ground_2, t.line),
    }
}

/// Keep labels short: the end of long ones (a history line's tail is what
/// it adds).
fn short(label: &str, max: usize) -> String {
    let n = label.chars().count();
    if n <= max {
        return label.to_string();
    }
    let tail: String = label.chars().skip(n - (max - 1)).collect();
    format!("…{tail}")
}

/// Draw the strip into `area` (the top of the keyboard, already filled).
pub fn draw(ui: &mut Ui, area: Rect, strip: &Strip) {
    let t = mortar_ui::theme::get();
    let hw = toolkit::hair(ui);
    let pad = ui.cq(1.0);
    let ch = ui.cq(3.4).round();
    let cy = area.y + pad / 2.0 + (area.h - pad / 2.0) / 2.0;
    let top = (cy - ch / 2.0).round();
    let st_hint = toolkit::st_body(ui, 1.9, t.line_soft);

    // hints on the right
    let hints: Vec<(&str, &str)> = if strip.chips.is_empty() {
        vec![("X", "space"), ("Y", "enter"), ("B", "del"), ("L1", "shift"), ("R2", "ctrl"), ("START", "hide")]
    } else if strip.focus.is_some() {
        vec![("A", "type"), ("▼", "keys")]
    } else {
        vec![("R1", "first"), ("▲", "choose")]
    };
    let cap_w = |ui: &Ui, l: &str| {
        if l.chars().count() <= 1 { ui.cq(3.2) } else { ui.measure(l, &TextStyle::new(Face::Mono600, ui.cq(1.8), t.line)) + ui.cq(2.4) }
    };
    let (gap_items, gap_in) = (ui.cq(2.2), ui.cq(0.8));
    let hints_w: f32 = hints.iter().map(|(b, l)| cap_w(ui, b) + gap_in + ui.measure(l, &st_hint)).sum::<f32>()
        + gap_items * hints.len().saturating_sub(1) as f32;
    let mut x = area.right() - pad - hints_w;
    let chips_right = x - ui.cq(2.0);
    for (b, l) in &hints {
        x += toolkit::button_cap(ui, x, cy, b) + gap_in;
        x += ui.text_vcenter(x, cy, l, &st_hint) + gap_items;
    }

    if strip.chips.is_empty() {
        return;
    }

    // chips, scrolled so the focused one shows
    let gap = ui.cq(0.8);
    let tag_w = ch;
    let widths: Vec<f32> = strip
        .chips
        .iter()
        .map(|c| {
            let max = if c.kind == Kind::History { 34 } else { 24 };
            tag_w + ui.cq(1.2) * 2.0 + ui.measure(&short(&c.label, max), &chip_style(ui, false))
        })
        .collect();
    let avail = chips_right - (area.x + pad);
    let mut first = 0;
    if let Some(f) = strip.focus {
        while first < f && widths[first..=f].iter().map(|w| w + gap).sum::<f32>() > avail {
            first += 1;
        }
    }
    let mut x = area.x + pad;
    for (i, c) in strip.chips.iter().enumerate().skip(first) {
        let w = widths[i];
        if x + w > chips_right {
            break;
        }
        let sel = strip.focus == Some(i);
        let r = Rect::new(x.round(), top, w.round(), ch);
        // the chip: a square block, the kind's tag on the left
        ui.fill_block(r, if sel { t.lime } else { t.ground });
        let (glyph, fill, ink) = tag(c.kind);
        let tr = Rect::new(r.x, r.y, tag_w, ch);
        ui.fill_block(tr, fill);
        ui.fill_block(Rect::new(tr.right() - hw, r.y, hw, ch), t.line);
        let gst = TextStyle::new(Face::Mono600, ui.cq(2.0), ink);
        let gw = ui.measure(glyph, &gst);
        ui.text_vcenter(tr.x + (tag_w - gw) / 2.0, cy, glyph, &gst);
        ui.outline(r, hw, t.line);
        let max = if c.kind == Kind::History { 34 } else { 24 };
        let label = short(&c.label, max);
        let lx = tr.right() + ui.cq(1.2);
        let st = chip_style(ui, sel);
        // the history line: what is typed already is dim
        let n = c.label.chars().count();
        let typed = if c.kind == Kind::History { n.saturating_sub(c.text.chars().count()) } else { 0 };
        let (dropped, lead) = if n > max { (n - (max - 1), 1) } else { (0, 0) };
        let dim = if typed == 0 { 0 } else { lead + typed.saturating_sub(dropped) };
        if dim > 0 && dim < label.chars().count() {
            let (a, b): (String, String) = (label.chars().take(dim).collect(), label.chars().skip(dim).collect());
            let soft = st.color(if sel { t.lime_ink.with_alpha(0.6) } else { t.line_faint });
            let aw = ui.text_vcenter(lx, cy, &a, &soft);
            ui.text_vcenter(lx + aw, cy, &b, &st);
        } else {
            ui.text_vcenter(lx, cy, &label, &st);
        }
        x += w + gap;
    }
}

fn chip_style(ui: &Ui, sel: bool) -> TextStyle {
    let t = mortar_ui::theme::get();
    TextStyle::new(if sel { Face::Mono600 } else { Face::Mono400 }, ui.cq(2.1), if sel { t.lime_ink } else { t.line })
}

/// Chips for previews: `git ch` typed after `git status` in history.
pub fn demo(focus: Option<usize>) -> Strip {
    let c = |kind, text: &str, label: &str| Chip { kind, del: 0, text: text.into(), label: label.into() };
    Strip {
        chips: vec![
            c(Kind::History, "eckout -b osk-chips", "git checkout -b osk-chips"),
            c(Kind::Command, "eckout ", "checkout"),
            c(Kind::Command, "erry-pick ", "cherry-pick"),
            c(Kind::Command, "erry ", "cherry"),
            c(Kind::Command, "eck-ignore ", "check-ignore"),
        ],
        focus,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chip(kind: Kind, text: &str) -> Chip {
        Chip { kind, del: 0, text: text.into(), label: text.trim().into() }
    }

    #[test]
    fn parses_zsh_lines() {
        let l = r#"{"seq":7,"chips":[{"k":"h","d":0,"t":"t status","l":"git status"},{"k":"d","d":2,"t":"src/","l":"src/"},{"k":"c","d":0,"t":"é ","l":"café"},{"k":"x","d":0,"t":"a\tb","l":"tab"}]}"#;
        let Some(Reply::Chips(7, cs)) = parse(l) else { panic!() };
        assert_eq!(cs.len(), 2, "non-ASCII and tabs are dropped");
        assert_eq!(cs[0], Chip { kind: Kind::History, del: 0, text: "t status".into(), label: "git status".into() });
        assert_eq!(cs[1].kind, Kind::Dir);
        assert_eq!(cs[1].del, 2);
        assert_eq!(parse(r#"{"seq":9,"retry":true}"#), Some(Reply::Retry(9)));
        assert_eq!(parse("garbage"), None);
        assert_eq!(parse(r#"{"seq":1,"chips":[{"k":"c","d":999,"t":"x"}]}"#), Some(Reply::Chips(1, vec![])));
    }

    #[test]
    fn caps_the_chip_count() {
        let many: Vec<String> = (0..40).map(|i| format!(r#"{{"k":"c","t":"c{i} "}}"#)).collect();
        let l = format!(r#"{{"seq":1,"chips":[{}]}}"#, many.join(","));
        let Some(Reply::Chips(_, cs)) = parse(&l) else { panic!() };
        assert_eq!(cs.len(), MAX_CHIPS);
        assert_eq!(cs[0].label, "c0", "label defaults to the text");
    }

    #[test]
    fn taps_delete_then_type() {
        let c = Chip { kind: Kind::Other, del: 2, text: "Ab/".into(), label: String::new() };
        let t = c.taps();
        assert_eq!(t.len(), 5);
        assert!(t[..2].iter().all(|t| t.code == osk::k::BACKSPACE));
        assert_eq!((t[2].code, t[2].shift), (30, true));
        assert_eq!((t[3].code, t[3].shift), (48, false));
        assert_eq!(t[4].code, osk::k::SLASH);
    }

    #[test]
    fn pad_navigation() {
        let mut s = Strip::default();
        // nothing to pick: everything is the keyboard's
        assert_eq!(s.button(Button::Up, true), Nav::Pass);
        assert_eq!(s.button(Button::R1, false), Nav::Pass);
        s.set(vec![chip(Kind::History, "a"), chip(Kind::Command, "b "), chip(Kind::Dir, "c/")]);
        // Up only enters from the top row
        assert_eq!(s.button(Button::Up, false), Nav::Pass);
        assert_eq!(s.button(Button::Up, true), Nav::Moved);
        assert_eq!(s.focus, Some(0));
        assert_eq!(s.button(Button::Left, true), Nav::Moved);
        assert_eq!(s.focus, Some(2), "wraps");
        assert_eq!(s.button(Button::Right, true), Nav::Moved);
        assert_eq!(s.button(Button::Right, true), Nav::Moved);
        // B, X, Y, START keep their keyboard meaning in the strip
        assert_eq!(s.button(Button::B, true), Nav::Pass);
        assert_eq!(s.button(Button::A, true), Nav::Accept(chip(Kind::Command, "b ")));
        assert_eq!(s.focus, None, "back on the keys");
        // R1: the first chip from the keys
        assert_eq!(s.button(Button::R1, false), Nav::Accept(chip(Kind::History, "a")));
        // Down leaves the strip
        s.button(Button::Up, true);
        assert_eq!(s.button(Button::Down, true), Nav::Moved);
        assert_eq!(s.focus, None);
    }

    #[test]
    fn focus_survives_new_chips() {
        let mut s = Strip { chips: vec![chip(Kind::Other, "a"), chip(Kind::Other, "b")], focus: Some(1) };
        s.set(vec![chip(Kind::Other, "z")]);
        assert_eq!(s.focus, Some(0));
        s.set(vec![]);
        assert_eq!(s.focus, None);
    }

    #[test]
    fn short_labels_keep_the_end() {
        assert_eq!(short("abc", 5), "abc");
        assert_eq!(short("abcdefgh", 5), "…efgh");
    }
}
