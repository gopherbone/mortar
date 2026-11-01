// SPDX-License-Identifier: GPL-2.0-or-later
//! Controller keyboard for apps in "keyboard" gamepad mode (the terminal).
//!
//! OSK hidden: the pad is a small keyboard. D-pad = arrows, A = Enter,
//! B = Backspace, X = Tab, Y = Space, L1/R1 = Page Up/Down, L2 = Esc,
//! R2 = Ctrl (held), SELECT = Ctrl+C, START = show the on-screen keyboard.
//! Presses and releases are passed through as key down/up, so the client's
//! own key repeat works (hold B to delete a word).
//!
//! OSK shown: a US layout drawn along the bottom of the screen while the app
//! is resized to the space above it. D-pad moves, A types, B = Backspace,
//! X = Space, Y = Enter, L1 = Shift (twice: caps lock), R2 = Ctrl (one
//! shot), L2 = Esc, START or B on "hide" closes it. A strip above the keys
//! holds suggestion chips (osk_suggest.rs): Up from the top row enters it,
//! R1 types the first chip.
//!
//! Keys are evdev codes; the shell sends them through its wl_keyboard with
//! an xkb "us" keymap, wrapping shifted characters in Shift.

use mortar_ui::gfx::{Rect, Ui};
use mortar_ui::input::Button;
use mortar_ui::toolkit;

pub mod k {
    pub const ESC: u16 = 1;
    pub const MINUS: u16 = 12;
    pub const EQUAL: u16 = 13;
    pub const BACKSPACE: u16 = 14;
    pub const TAB: u16 = 15;
    pub const LEFTBRACE: u16 = 26;
    pub const RIGHTBRACE: u16 = 27;
    pub const ENTER: u16 = 28;
    pub const LEFTCTRL: u16 = 29;
    pub const SEMICOLON: u16 = 39;
    pub const APOSTROPHE: u16 = 40;
    pub const GRAVE: u16 = 41;
    pub const LEFTSHIFT: u16 = 42;
    pub const BACKSLASH: u16 = 43;
    pub const COMMA: u16 = 51;
    pub const DOT: u16 = 52;
    pub const SLASH: u16 = 53;
    pub const LEFTALT: u16 = 56;
    pub const SPACE: u16 = 57;
    pub const C: u16 = 46;
    pub const HOME: u16 = 102;
    pub const UP: u16 = 103;
    pub const PAGEUP: u16 = 104;
    pub const LEFT: u16 = 105;
    pub const RIGHT: u16 = 106;
    pub const END: u16 = 107;
    pub const DOWN: u16 = 108;
    pub const PAGEDOWN: u16 = 109;
}

/// What a pad button does for a keyboard-mode app while the OSK is hidden.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PadKey {
    /// Pass press/release through as this key.
    Key(u16),
    /// Press/release with Ctrl held around it.
    CtrlKey(u16),
    ShowOsk,
}

pub fn pad_key(b: Button) -> Option<PadKey> {
    use Button::*;
    Some(match b {
        Up => PadKey::Key(k::UP),
        Down => PadKey::Key(k::DOWN),
        Left => PadKey::Key(k::LEFT),
        Right => PadKey::Key(k::RIGHT),
        A => PadKey::Key(k::ENTER),
        B => PadKey::Key(k::BACKSPACE),
        X => PadKey::Key(k::TAB),
        Y => PadKey::Key(k::SPACE),
        L1 => PadKey::Key(k::PAGEUP),
        R1 => PadKey::Key(k::PAGEDOWN),
        L2 => PadKey::Key(k::ESC),
        R2 => PadKey::Key(k::LEFTCTRL),
        Select => PadKey::CtrlKey(k::C),
        Start => PadKey::ShowOsk,
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    /// A character key: evdev code, label, shifted label.
    Char(u16, &'static str, &'static str),
    /// A named key (no shift variant).
    Named(u16, &'static str),
    Shift,
    Ctrl,
    Alt,
    Hide,
}

#[derive(Clone, Copy, Debug)]
struct Key {
    act: Act,
    /// Width in key units.
    w: f32,
}

const fn ch(code: u16, l: &'static str, s: &'static str) -> Key {
    Key { act: Act::Char(code, l, s), w: 1.0 }
}
const fn named(code: u16, l: &'static str, w: f32) -> Key {
    Key { act: Act::Named(code, l), w }
}

const ROW0: &[Key] = &[
    ch(k::GRAVE, "`", "~"), ch(2, "1", "!"), ch(3, "2", "@"), ch(4, "3", "#"), ch(5, "4", "$"), ch(6, "5", "%"),
    ch(7, "6", "^"), ch(8, "7", "&"), ch(9, "8", "*"), ch(10, "9", "("), ch(11, "0", ")"), ch(k::MINUS, "-", "_"),
    ch(k::EQUAL, "=", "+"), named(k::BACKSPACE, "del", 2.0),
];
const ROW1: &[Key] = &[
    named(k::TAB, "tab", 1.5), ch(16, "q", "Q"), ch(17, "w", "W"), ch(18, "e", "E"), ch(19, "r", "R"), ch(20, "t", "T"),
    ch(21, "y", "Y"), ch(22, "u", "U"), ch(23, "i", "I"), ch(24, "o", "O"), ch(25, "p", "P"), ch(k::LEFTBRACE, "[", "{"),
    ch(k::RIGHTBRACE, "]", "}"), Key { act: Act::Char(k::BACKSLASH, "\\", "|"), w: 1.5 },
];
const ROW2: &[Key] = &[
    named(k::ESC, "esc", 1.75), ch(30, "a", "A"), ch(31, "s", "S"), ch(32, "d", "D"), ch(33, "f", "F"), ch(34, "g", "G"),
    ch(35, "h", "H"), ch(36, "j", "J"), ch(37, "k", "K"), ch(38, "l", "L"), ch(k::SEMICOLON, ";", ":"),
    ch(k::APOSTROPHE, "'", "\""), named(k::ENTER, "enter", 2.25),
];
const ROW3: &[Key] = &[
    Key { act: Act::Shift, w: 2.25 }, ch(44, "z", "Z"), ch(45, "x", "X"), ch(46, "c", "C"), ch(47, "v", "V"),
    ch(48, "b", "B"), ch(49, "n", "N"), ch(50, "m", "M"), ch(k::COMMA, ",", "<"), ch(k::DOT, ".", ">"),
    ch(k::SLASH, "/", "?"), named(k::UP, "↑", 1.0), named(k::HOME, "home", 1.75),
];
const ROW4: &[Key] = &[
    Key { act: Act::Ctrl, w: 1.75 }, Key { act: Act::Alt, w: 1.5 }, named(k::SPACE, "space", 6.25),
    named(k::LEFT, "←", 1.0), named(k::DOWN, "↓", 1.0), named(k::RIGHT, "→", 1.0), named(k::END, "end", 1.0),
    Key { act: Act::Hide, w: 1.5 },
];
const ROWS: [&[Key]; 5] = [ROW0, ROW1, ROW2, ROW3, ROW4];
const UNITS: f32 = 15.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Latch {
    #[default]
    Off,
    Once,
    Locked,
}

/// A key to send: press `code` with the given modifiers held, then release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tap {
    pub code: u16,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OskOut {
    Tap(Tap),
    Hide,
}

#[derive(Default)]
pub struct Osk {
    pub visible: bool,
    row: usize,
    col: usize,
    shift: Latch,
    ctrl: bool,
    alt: bool,
    /// The cursor is up in the suggestion strip: no key is highlighted.
    pub in_strip: bool,
}

/// Height of the keyboard (the suggestion strip, then 5 rows) on this
/// screen.
pub fn height(ui: &Ui) -> f32 {
    (crate::osk_suggest::strip_h(ui) + ui.cq(2.0) + 5.0 * ui.cq(6.2) + 4.0 * ui.cq(0.8)).round()
}

/// The suggestion strip along the top of the keyboard's `area`
/// (osk_suggest.rs draws it).
pub fn strip_rect(ui: &Ui, area: Rect) -> Rect {
    area.take_top(crate::osk_suggest::strip_h(ui)).0
}

/// The key that types `c` on the "us" layout (with Shift if needed), for
/// typing a suggestion.
pub fn char_tap(c: char) -> Option<Tap> {
    let tap = |code, shift| Some(Tap { code, shift, ctrl: false, alt: false });
    match c {
        ' ' => return tap(k::SPACE, false),
        '\t' => return tap(k::TAB, false),
        _ => {}
    }
    let mut buf = [0u8; 4];
    let s: &str = c.encode_utf8(&mut buf);
    for row in ROWS {
        for key in row.iter() {
            if let Act::Char(code, l, sh) = key.act {
                if l == s {
                    return tap(code, false);
                }
                if sh == s {
                    return tap(code, true);
                }
            }
        }
    }
    None
}

impl Osk {
    /// The cursor is on the top row (Up from here goes to the suggestions).
    pub fn at_top(&self) -> bool {
        self.row == 0
    }

    pub fn show(&mut self) {
        self.visible = true;
        self.row = 2;
        self.col = 1;
    }

    fn key(&self) -> Key {
        ROWS[self.row][self.col.min(ROWS[self.row].len() - 1)]
    }

    /// x centre (in units) of a key.
    fn center(row: usize, col: usize) -> f32 {
        let r = ROWS[row];
        r[..col].iter().map(|k| k.w).sum::<f32>() + r[col].w / 2.0
    }

    /// Move to the key in `row` nearest to x-centre `x`.
    fn nearest(row: usize, x: f32) -> usize {
        let mut best = (0, f32::MAX);
        for c in 0..ROWS[row].len() {
            let d = (Self::center(row, c) - x).abs();
            if d < best.1 {
                best = (c, d);
            }
        }
        best.0
    }

    fn tap(&mut self, code: u16, shiftable: bool) -> OskOut {
        let t = Tap { code, shift: shiftable && self.shift != Latch::Off, ctrl: self.ctrl, alt: self.alt };
        if self.shift == Latch::Once {
            self.shift = Latch::Off;
        }
        self.ctrl = false;
        self.alt = false;
        OskOut::Tap(t)
    }

    fn activate(&mut self) -> Option<OskOut> {
        match self.key().act {
            Act::Char(code, _, _) => Some(self.tap(code, true)),
            Act::Named(code, _) => Some(self.tap(code, false)),
            Act::Shift => {
                self.toggle_shift();
                None
            }
            Act::Ctrl => {
                self.ctrl = !self.ctrl;
                None
            }
            Act::Alt => {
                self.alt = !self.alt;
                None
            }
            Act::Hide => {
                self.visible = false;
                Some(OskOut::Hide)
            }
        }
    }

    fn toggle_shift(&mut self) {
        self.shift = match self.shift {
            Latch::Off => Latch::Once,
            Latch::Once => Latch::Locked,
            Latch::Locked => Latch::Off,
        };
    }

    /// A pad button press while the OSK is visible.
    pub fn button(&mut self, b: Button) -> Option<OskOut> {
        use Button::*;
        match b {
            Up | Down => {
                let x = Self::center(self.row, self.col);
                self.row = if b == Up { (self.row + ROWS.len() - 1) % ROWS.len() } else { (self.row + 1) % ROWS.len() };
                self.col = Self::nearest(self.row, x);
                None
            }
            Left => {
                let n = ROWS[self.row].len();
                self.col = (self.col + n - 1) % n;
                None
            }
            Right => {
                self.col = (self.col + 1) % ROWS[self.row].len();
                None
            }
            A => self.activate(),
            B => Some(self.tap(k::BACKSPACE, false)),
            X => Some(self.tap(k::SPACE, false)),
            Y => Some(self.tap(k::ENTER, false)),
            L1 => {
                self.toggle_shift();
                None
            }
            R2 => {
                self.ctrl = !self.ctrl;
                None
            }
            L2 => Some(self.tap(k::ESC, false)),
            Start => {
                self.visible = false;
                Some(OskOut::Hide)
            }
            _ => None,
        }
    }

    /// Draw the keyboard into `area` (bottom of the screen).
    pub fn draw(&self, ui: &mut Ui, area: Rect) {
        let t = mortar_ui::theme::get();
        toolkit::block(ui, area, t.ground_2);
        let area = area.take_top(crate::osk_suggest::strip_h(ui)).1;
        let pad = ui.cq(1.0);
        let gap = ui.cq(0.8);
        let kh = ui.cq(6.2);
        let inner = area.inset(pad, pad);
        let unit = (inner.w + gap) / UNITS;
        let shifted = self.shift != Latch::Off;
        for (r, row) in ROWS.iter().enumerate() {
            let y = inner.y + r as f32 * (kh + gap);
            let mut x = inner.x;
            for (c, key) in row.iter().enumerate() {
                let w = key.w * unit - gap;
                let rect = Rect::new(x, y, w, kh);
                let sel = !self.in_strip && r == self.row && c == self.col;
                let (label, small): (String, bool) = match key.act {
                    Act::Char(_, l, s) => ((if shifted { s } else { l }).to_string(), false),
                    Act::Named(_, l) => (l.to_string(), l.chars().count() > 1),
                    Act::Shift => (if self.shift == Latch::Locked { "CAPS".into() } else { "shift".into() }, true),
                    Act::Ctrl => ("ctrl".into(), true),
                    Act::Alt => ("alt".into(), true),
                    Act::Hide => ("hide".into(), true),
                };
                let on = match key.act {
                    Act::Shift => shifted,
                    Act::Ctrl => self.ctrl,
                    Act::Alt => self.alt,
                    _ => false,
                };
                if on && !sel {
                    toolkit::block(ui, rect, t.lavender);
                    let st = mortar_ui::toolkit::st_mono(ui, 2.0, t.lavender_ink);
                    let lw = ui.measure(&label, &st);
                    ui.text_vcenter(rect.x + (rect.w - lw) / 2.0, rect.y + rect.h / 2.0, &label, &st);
                } else {
                    toolkit::key_cap(ui, rect, &label, sel, small);
                }
                x += key.w * unit;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_full_width() {
        for r in ROWS {
            let w: f32 = r.iter().map(|k| k.w).sum();
            assert!((w - UNITS).abs() < 0.01, "row width {w}");
        }
    }

    #[test]
    fn typing_with_shift_once() {
        let mut o = Osk::default();
        o.show();
        assert_eq!(o.key().act, Act::Char(30, "a", "A"));
        o.button(Button::L1);
        assert_eq!(o.button(Button::A), Some(OskOut::Tap(Tap { code: 30, shift: true, ctrl: false, alt: false })));
        // shift was one-shot
        assert_eq!(o.button(Button::A), Some(OskOut::Tap(Tap { code: 30, shift: false, ctrl: false, alt: false })));
    }

    #[test]
    fn vertical_moves_keep_column() {
        let mut o = Osk::default();
        o.show(); // row 2 'a'
        o.button(Button::Up); // row 1 -> 'q'
        assert_eq!(o.key().act, Act::Char(16, "q", "Q"));
        o.button(Button::Down);
        o.button(Button::Down); // row 3 -> 'z'
        assert_eq!(o.key().act, Act::Char(44, "z", "Z"));
        o.button(Button::Down); // row 4: under z/x is... alt or space
        assert!(matches!(o.key().act, Act::Alt | Act::Named(k::SPACE, _)));
    }

    #[test]
    fn ctrl_c_and_hide() {
        let mut o = Osk::default();
        o.show();
        o.button(Button::R2);
        o.row = 3;
        o.col = 3; // c
        assert_eq!(o.button(Button::A), Some(OskOut::Tap(Tap { code: k::C, shift: false, ctrl: true, alt: false })));
        assert_eq!(o.button(Button::Start), Some(OskOut::Hide));
        assert!(!o.visible);
    }

    #[test]
    fn char_taps() {
        assert_eq!(char_tap('a'), Some(Tap { code: 30, shift: false, ctrl: false, alt: false }));
        assert_eq!(char_tap('A'), Some(Tap { code: 30, shift: true, ctrl: false, alt: false }));
        assert_eq!(char_tap('/'), Some(Tap { code: k::SLASH, shift: false, ctrl: false, alt: false }));
        assert_eq!(char_tap('~'), Some(Tap { code: k::GRAVE, shift: true, ctrl: false, alt: false }));
        assert_eq!(char_tap(' ').map(|t| t.code), Some(k::SPACE));
        assert_eq!(char_tap('é'), None);
        // every printable ASCII character can be typed
        for c in ' '..='~' {
            assert!(char_tap(c).is_some(), "{c:?}");
        }
    }

    #[test]
    fn pad_keys() {
        assert_eq!(pad_key(Button::A), Some(PadKey::Key(k::ENTER)));
        assert_eq!(pad_key(Button::Start), Some(PadKey::ShowOsk));
        assert_eq!(pad_key(Button::Menu), None);
    }
}
