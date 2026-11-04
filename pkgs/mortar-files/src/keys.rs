// SPDX-License-Identifier: GPL-2.0-or-later
//! Input, as it arrives. The shell runs this app in `keyboard` gamepad mode,
//! so the pad comes as key codes on wl_keyboard (pkgs/mortar-shell/src/osk.rs):
//!
//!   D-pad arrows · A Enter · B Backspace · X Tab · Y Space
//!   L1 PgUp · R1 PgDn · L2 Esc · R2 Ctrl (held) · SELECT Ctrl+C · START the OSK
//!
//! F1 and F2 have no key in that mode, so nothing depends on them. A real
//! keyboard and the on-screen keyboard arrive the same way. This file turns
//! codes into `Input`s, with a fixed US layout: the shell's keymap is "us".

use mortar_ui::input::Button;

pub const ESC: u16 = 1;
pub const BACKSPACE: u16 = 14;
pub const TAB: u16 = 15;
pub const ENTER: u16 = 28;
pub const LEFTCTRL: u16 = 29;
pub const LEFTSHIFT: u16 = 42;
pub const C: u16 = 46;
pub const RIGHTSHIFT: u16 = 54;
pub const SPACE: u16 = 57;
pub const KPENTER: u16 = 96;
pub const RIGHTCTRL: u16 = 97;
pub const HOME: u16 = 102;
pub const UP: u16 = 103;
pub const PAGEUP: u16 = 104;
pub const LEFT: u16 = 105;
pub const RIGHT: u16 = 106;
pub const END: u16 = 107;
pub const DOWN: u16 = 108;
pub const PAGEDOWN: u16 = 109;
pub const DELETE: u16 = 111;

/// (unshifted, shifted) for printable keys.
pub fn printable(code: u16) -> Option<(char, char)> {
    const ROW1: &[u8; 12] = b"1234567890-=";
    const ROW1S: &[u8; 12] = b"!@#$%^&*()_+";
    Some(match code {
        2..=13 => (ROW1[(code - 2) as usize] as char, ROW1S[(code - 2) as usize] as char),
        16..=25 => {
            let c = b"qwertyuiop"[(code - 16) as usize] as char;
            (c, c.to_ascii_uppercase())
        }
        26 => ('[', '{'),
        27 => (']', '}'),
        30..=38 => {
            let c = b"asdfghjkl"[(code - 30) as usize] as char;
            (c, c.to_ascii_uppercase())
        }
        39 => (';', ':'),
        40 => ('\'', '"'),
        41 => ('`', '~'),
        43 => ('\\', '|'),
        44..=50 => {
            let c = b"zxcvbnm"[(code - 44) as usize] as char;
            (c, c.to_ascii_uppercase())
        }
        51 => (',', '<'),
        52 => ('.', '>'),
        53 => ('/', '?'),
        _ => return None,
    })
}

/// One thing the person did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    /// A pad button (or the key that stands for it). `repeat`: held down.
    Press(Button, bool),
    /// A character typed (on the OSK or a real keyboard).
    Char(char),
    /// Editing keys the pad has no button for.
    Home,
    End,
    Delete,
    /// L1 or R1 went up or down, for chords.
    Shoulders { l1: bool, r1: bool },
}

/// Tracks modifiers across key events.
#[derive(Debug, Default)]
pub struct Decoder {
    shift: bool,
    ctrl: bool,
    /// Ctrl went down and nothing else since: that's R2 when it comes up.
    ctrl_alone: bool,
    l1: bool,
    r1: bool,
}

impl Decoder {
    pub fn reset(&mut self) {
        *self = Decoder::default();
    }

    /// A key event. May give nothing (a release, a modifier).
    pub fn key(&mut self, code: u16, down: bool, repeat: bool) -> Vec<Input> {
        let mut out = Vec::new();
        match code {
            LEFTSHIFT | RIGHTSHIFT => self.shift = down,
            LEFTCTRL | RIGHTCTRL => {
                if down {
                    self.ctrl = true;
                    self.ctrl_alone = true;
                } else {
                    self.ctrl = false;
                    if std::mem::take(&mut self.ctrl_alone) {
                        out.push(Input::Press(Button::R2, false));
                    }
                }
            }
            _ => {}
        }
        if matches!(code, PAGEUP | PAGEDOWN) {
            if !repeat {
                if code == PAGEUP {
                    self.l1 = down;
                } else {
                    self.r1 = down;
                }
                out.push(Input::Shoulders { l1: self.l1, r1: self.r1 });
            }
            if down {
                out.push(Input::Press(if code == PAGEUP { Button::L1 } else { Button::R1 }, repeat));
            }
            return out;
        }
        if !down || matches!(code, LEFTSHIFT | RIGHTSHIFT | LEFTCTRL | RIGHTCTRL) {
            return out;
        }
        self.ctrl_alone = false;
        if self.ctrl {
            if code == C {
                out.push(Input::Press(Button::Select, false));
            }
            return out;
        }
        let b = match code {
            UP => Some(Button::Up),
            DOWN => Some(Button::Down),
            LEFT => Some(Button::Left),
            RIGHT => Some(Button::Right),
            ENTER | KPENTER => Some(Button::A),
            BACKSPACE => Some(Button::B),
            TAB => Some(Button::X),
            SPACE => Some(Button::Y),
            ESC => Some(Button::L2),
            _ => None,
        };
        if let Some(b) = b {
            out.push(Input::Press(b, repeat));
        } else {
            match code {
                HOME => out.push(Input::Home),
                END => out.push(Input::End),
                DELETE => out.push(Input::Delete),
                _ => {
                    if let Some((a, s)) = printable(code) {
                        out.push(Input::Char(if self.shift { s } else { a }));
                    }
                }
            }
        }
        out
    }
}

/// The key events the shell sends for a pad button: the preview and the
/// tests drive the app the way the real thing does.
pub fn pad_events(b: Button) -> Vec<(u16, bool)> {
    let tap = |c: u16| vec![(c, true), (c, false)];
    match b {
        Button::Up => tap(UP),
        Button::Down => tap(DOWN),
        Button::Left => tap(LEFT),
        Button::Right => tap(RIGHT),
        Button::A => tap(ENTER),
        Button::B => tap(BACKSPACE),
        Button::X => tap(TAB),
        Button::Y => tap(SPACE),
        Button::L1 => tap(PAGEUP),
        Button::R1 => tap(PAGEDOWN),
        Button::L2 => tap(ESC),
        Button::R2 => tap(LEFTCTRL),
        Button::Select => vec![(LEFTCTRL, true), (C, true), (C, false), (LEFTCTRL, false)],
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(d: &mut Decoder, ev: &[(u16, bool)]) -> Vec<Input> {
        ev.iter().flat_map(|(c, down)| d.key(*c, *down, false)).collect()
    }

    #[test]
    fn every_button_the_shell_sends_comes_back() {
        for b in [Button::Up, Button::Down, Button::Left, Button::Right, Button::A, Button::B, Button::X, Button::Y, Button::L2, Button::R2, Button::Select] {
            let mut d = Decoder::default();
            assert_eq!(run(&mut d, &pad_events(b)), [Input::Press(b, false)], "{b:?}");
        }
    }

    #[test]
    fn select_is_not_also_r2() {
        // Ctrl+C from the shell's SELECT mapping: Ctrl alone would be R2
        let mut d = Decoder::default();
        let v = run(&mut d, &pad_events(Button::Select));
        assert_eq!(v, [Input::Press(Button::Select, false)]);
    }

    #[test]
    fn shoulders_track_a_chord() {
        let mut d = Decoder::default();
        let v = run(&mut d, &[(PAGEUP, true), (PAGEDOWN, true), (PAGEUP, false)]);
        assert!(v.contains(&Input::Shoulders { l1: true, r1: true }));
        assert_eq!(v.last(), Some(&Input::Shoulders { l1: false, r1: true }));
        assert!(v.contains(&Input::Press(Button::L1, false)));
    }

    #[test]
    fn typing() {
        let mut d = Decoder::default();
        // shift+a, what the OSK sends for a capital
        let v = run(&mut d, &[(LEFTSHIFT, true), (30, true), (30, false), (LEFTSHIFT, false), (30, true), (30, false), (12, true), (12, false)]);
        assert_eq!(v, [Input::Char('A'), Input::Char('a'), Input::Char('-')]);
    }
}
