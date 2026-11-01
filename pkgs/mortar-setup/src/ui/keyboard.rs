// SPDX-License-Identifier: GPL-2.0-or-later
//! Page 5: the on-screen keyboard for a Wi-Fi password. The fallback, so
//! it is built for the D-pad: shoulder buttons do shift (L1) and symbols
//! (R1), so you rarely leave the letter rows; B deletes, START joins.

use super::pages::WifiJoin;
use super::{Cx, Nav, Page};
use crate::control::Request;
use mortar_ui::gfx::{Rect, Ui};
use mortar_ui::input::Button;
use mortar_ui::toolkit::{self, hint};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Key {
    Char(char),
    Shift,
    Symbols,
    Space,
}

impl Key {
    fn label(self, shift: bool) -> String {
        match self {
            Key::Char(c) if shift => c.to_uppercase().collect(),
            Key::Char(c) => c.to_string(),
            Key::Shift => "shift".into(),
            Key::Symbols => "#+=".into(),
            Key::Space => "space".into(),
        }
    }
    /// Width in key units.
    fn units(self) -> f32 {
        match self {
            Key::Shift | Key::Symbols => 1.5,
            Key::Space => 6.0,
            Key::Char(_) => 1.0,
        }
    }
}

fn chars(s: &str) -> Vec<Key> {
    s.chars().map(Key::Char).collect()
}

/// The rows of a layer.
pub fn layout(symbols: bool) -> Vec<Vec<Key>> {
    let mut rows = if symbols {
        vec![chars("1234567890"), chars("!@#$%^&*()"), chars("-_=+[]{};:"), chars("'\",.<>/?\\|")]
    } else {
        vec![chars("1234567890"), chars("qwertyuiop"), chars("asdfghjkl"), chars("zxcvbnm")]
    };
    rows[3].insert(0, Key::Shift);
    rows[3].push(Key::Symbols);
    let mut last = vec![Key::Space];
    if symbols {
        last = vec![Key::Char('`'), Key::Char('~'), Key::Space, Key::Char('€'), Key::Char('£')];
    }
    rows.push(last);
    rows
}

/// Keyboard state: what is typed and where the cursor key is.
#[derive(Clone, Debug, Default)]
pub struct Keyboard {
    pub text: String,
    pub row: usize,
    pub col: usize,
    pub shift: bool,
    /// Shift stays on (L1 twice, or shift key twice).
    pub caps: bool,
    pub symbols: bool,
    pub show: bool,
    pub max: usize,
}

impl Keyboard {
    pub fn new(max: usize) -> Keyboard {
        Keyboard { row: 1, max, ..Default::default() }
    }

    fn rows(&self) -> Vec<Vec<Key>> {
        layout(self.symbols)
    }

    pub fn current(&self) -> Key {
        let rows = self.rows();
        let r = &rows[self.row.min(rows.len() - 1)];
        r[self.col.min(r.len() - 1)]
    }

    /// Centre of key `c` in row `r`, in key units from the row's left
    /// edge of the widest row (rows are centred).
    fn centre(rows: &[Vec<Key>], r: usize, c: usize) -> f32 {
        let widest = rows.iter().map(|row| row.iter().map(|k| k.units()).sum::<f32>()).fold(0.0, f32::max);
        let row = &rows[r];
        let total: f32 = row.iter().map(|k| k.units()).sum();
        let mut x = (widest - total) / 2.0;
        for k in &row[..c] {
            x += k.units();
        }
        x + row[c].units() / 2.0
    }

    fn move_v(&mut self, d: i32) {
        let rows = self.rows();
        let n = rows.len() as i32;
        let x = Self::centre(&rows, self.row, self.col.min(rows[self.row].len() - 1));
        self.row = ((self.row as i32 + d).rem_euclid(n)) as usize;
        let row = &rows[self.row];
        self.col = (0..row.len())
            .min_by(|a, b| {
                let da = (Self::centre(&rows, self.row, *a) - x).abs();
                let db = (Self::centre(&rows, self.row, *b) - x).abs();
                da.partial_cmp(&db).unwrap()
            })
            .unwrap_or(0);
    }

    fn type_char(&mut self, c: char) {
        if self.text.chars().count() >= self.max {
            return;
        }
        let up = self.shift || self.caps;
        if up {
            self.text.extend(c.to_uppercase());
        } else {
            self.text.push(c);
        }
        if !self.caps {
            self.shift = false;
        }
    }

    fn toggle_shift(&mut self) {
        if self.shift && !self.caps {
            self.caps = true;
        } else if self.caps {
            self.caps = false;
            self.shift = false;
        } else {
            self.shift = true;
        }
    }

    /// Handle a button. Returns true if START (submit) was pressed.
    pub fn button(&mut self, b: Button) -> bool {
        let rows = self.rows();
        match b {
            Button::Left => {
                let n = rows[self.row].len();
                self.col = (self.col.min(n - 1) + n - 1) % n;
            }
            Button::Right => {
                let n = rows[self.row].len();
                self.col = (self.col.min(n - 1) + 1) % n;
            }
            Button::Up => self.move_v(-1),
            Button::Down => self.move_v(1),
            Button::A => match self.current() {
                Key::Char(c) => self.type_char(c),
                Key::Space => self.type_char(' '),
                Key::Shift => self.toggle_shift(),
                Key::Symbols => {
                    self.symbols = !self.symbols;
                    self.col = self.col.min(self.rows()[self.row].len() - 1);
                }
            },
            Button::Y => self.type_char(' '),
            Button::B => {
                self.text.pop();
            }
            Button::L1 => self.toggle_shift(),
            Button::R1 => {
                self.symbols = !self.symbols;
                self.col = self.col.min(self.rows()[self.row].len() - 1);
            }
            Button::Select => self.show = !self.show,
            Button::Start => return true,
            _ => {}
        }
        false
    }

    /// The field text: dots, with the last character visible (as phones
    /// do), or everything with SELECT.
    pub fn display(&self) -> String {
        if self.show {
            return self.text.clone();
        }
        let n = self.text.chars().count();
        match self.text.chars().last() {
            Some(c) => format!("{}{c}", "•".repeat(n - 1)),
            None => String::new(),
        }
    }

    pub fn draw(&self, ui: &mut Ui, area: Rect) {
        let rows = self.rows();
        let gap = ui.cq(0.9);
        let widest = rows.iter().map(|row| row.iter().map(|k| k.units()).sum::<f32>()).fold(0.0, f32::max);
        let max_in_row = rows.iter().map(|r| r.len()).max().unwrap_or(1) as f32;
        let unit = ((area.w - gap * (max_in_row - 1.0)) / widest).min(ui.cq(7.6));
        let kh = ((area.h - gap * (rows.len() as f32 - 1.0)) / rows.len() as f32).min(ui.cq(6.4));
        let up = self.shift || self.caps;
        for (r, row) in rows.iter().enumerate() {
            let total: f32 = row.iter().map(|k| k.units() * unit).sum::<f32>() + gap * (row.len() as f32 - 1.0);
            let mut x = area.x + (area.w - total) / 2.0;
            let y = area.y + r as f32 * (kh + gap);
            for (c, k) in row.iter().enumerate() {
                let w = k.units() * unit + (k.units() - 1.0).max(0.0) * 0.0;
                let selected = r == self.row && c == self.col.min(row.len() - 1);
                let small = !matches!(k, Key::Char(_));
                let mut label = k.label(up);
                if *k == Key::Shift && self.caps {
                    label = "caps".into();
                }
                if *k == Key::Symbols && self.symbols {
                    label = "abc".into();
                }
                toolkit::key_cap(ui, Rect::new(x, y, w, kh), &label, selected, small);
                if *k == Key::Shift && up && !selected {
                    // shift is on: a lime bar under its label
                    let t = mortar_ui::theme::get();
                    ui.fill_block(Rect::new(x + ui.cq(1.0), y + kh - ui.cq(1.0), w - ui.cq(2.0), ui.cq(0.4)), t.lime);
                }
                x += w + gap;
            }
        }
    }
}

/// Page 5.
pub struct PasswordPage {
    ssid: String,
    kb: Keyboard,
}

impl PasswordPage {
    pub fn new(ssid: &str) -> PasswordPage {
        PasswordPage { ssid: ssid.into(), kb: Keyboard::new(63) }
    }

    /// For screenshots: pretend some typing happened.
    pub fn with_text(mut self, text: &str) -> Self {
        self.kb.text = text.into();
        self
    }
}

impl Page for PasswordPage {
    fn draw(&mut self, ui: &mut Ui, cx: &Cx) {
        let hints = [hint("L1", "Shift"), hint("R1", "#+="), hint("B", "Delete"), hint("A", "Type"), hint("START", "Join")];
        let body = cx.frame(ui, false, &hints);
        let mut y = body.y + ui.cq(0.6);
        y += toolkit::title_h2(ui, body.x, y, &format!("Password for {}", self.ssid)) + ui.cq(1.0);
        let hint_text = if self.kb.show { "SELECT: hide" } else { "SELECT: show" };
        y += toolkit::text_field(ui, body.x, y, body.w, &self.kb.display(), Some(hint_text)) + ui.cq(2.4);
        let area = Rect::new(body.x, y, body.w, body.bottom() - y - ui.cq(1.6));
        self.kb.draw(ui, area);
    }

    fn button(&mut self, b: Button, cx: &mut Cx) -> Nav {
        if b == Button::B && self.kb.text.is_empty() {
            return Nav::Pop;
        }
        if self.kb.button(b) {
            let n = self.kb.text.chars().count();
            if n < 8 {
                cx.toast("Wi-Fi passwords have at least 8 characters");
                return Nav::Stay;
            }
            let req = Request::WifiConnect { ssid: self.ssid.clone(), passphrase: Some(self.kb.text.clone()) };
            if cx.act(req) {
                return Nav::Replace(Box::new(WifiJoin::new(&self.ssid, false)));
            }
        }
        Nav::Stay
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing() {
        let mut k = Keyboard::new(63);
        // row 1 col 0 = q
        assert_eq!(k.current(), Key::Char('q'));
        k.button(Button::A);
        k.button(Button::L1);
        k.button(Button::Right);
        k.button(Button::A); // W
        k.button(Button::A); // w (shift was one-shot)
        k.button(Button::R1);
        k.button(Button::Left);
        k.button(Button::A); // '!' at row 1 col 0 of symbols
        assert_eq!(k.text, "qWw!");
        assert_eq!(k.display(), "•••!");
        k.button(Button::Select);
        assert_eq!(k.display(), "qWw!");
        k.button(Button::B);
        assert_eq!(k.text, "qWw");
        assert!(k.button(Button::Start));
    }

    #[test]
    fn vertical_moves_keep_column() {
        let mut k = Keyboard::new(63);
        k.col = 9; // p
        k.button(Button::Down); // row 2 has 9 keys: nearest to p is l
        assert_eq!(k.current(), Key::Char('l'));
        k.button(Button::Down); // row 3: shift z..m #+= ; under l is #+= or m
        assert!(matches!(k.current(), Key::Symbols | Key::Char('m')));
        k.button(Button::Down);
        assert_eq!(k.current(), Key::Space);
        k.button(Button::Down); // wraps to the number row
        assert_eq!(k.row, 0);
    }

    #[test]
    fn caps_lock() {
        let mut k = Keyboard::new(3);
        k.button(Button::L1);
        k.button(Button::L1);
        k.button(Button::A);
        k.button(Button::A);
        k.button(Button::A);
        k.button(Button::A); // over max
        assert_eq!(k.text, "QQQ");
    }
}
