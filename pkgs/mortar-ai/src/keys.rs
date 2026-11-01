// SPDX-License-Identifier: GPL-2.0-or-later
//! Keyboard input as evdev codes (what wl_keyboard sends, and what the
//! shell's pad-as-keyboard mode and on-screen keyboard type), mapped with a
//! fixed US layout: the shell's keymap is "us" too, and this avoids an xkb
//! dependency.

pub const ESC: u16 = 1;
pub const BACKSPACE: u16 = 14;
pub const TAB: u16 = 15;
pub const ENTER: u16 = 28;
pub const LEFTCTRL: u16 = 29;
pub const LEFTSHIFT: u16 = 42;
pub const RIGHTSHIFT: u16 = 54;
pub const LEFTALT: u16 = 56;
pub const CAPSLOCK: u16 = 58;
pub const RIGHTCTRL: u16 = 97;
pub const KPENTER: u16 = 96;
pub const HOME: u16 = 102;
pub const UP: u16 = 103;
pub const PAGEUP: u16 = 104;
pub const END: u16 = 107;
pub const DOWN: u16 = 108;
pub const PAGEDOWN: u16 = 109;
pub const DELETE: u16 = 111;
pub const C: u16 = 46;
pub const L: u16 = 38;
pub const U: u16 = 22;
pub const W: u16 = 17;

/// (unshifted, shifted) for printable keys.
pub fn printable(code: u16) -> Option<(char, char)> {
    const ROW1: &[u8; 13] = b"1234567890-=\0";
    const ROW1S: &[u8; 13] = b"!@#$%^&*()_+\0";
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
        57 => (' ', ' '),
        _ => return None,
    })
}

/// Modifier state tracked from key down/up.
#[derive(Clone, Copy, Debug, Default)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub caps: bool,
}

impl Mods {
    /// Update for a modifier key; returns true if `code` was one.
    pub fn update(&mut self, code: u16, down: bool) -> bool {
        match code {
            LEFTSHIFT | RIGHTSHIFT => self.shift = down,
            LEFTCTRL | RIGHTCTRL => self.ctrl = down,
            LEFTALT => self.alt = down,
            CAPSLOCK => {
                if down {
                    self.caps = !self.caps
                }
            }
            _ => return false,
        }
        true
    }

    pub fn char_for(&self, code: u16) -> Option<char> {
        let (lo, hi) = printable(code)?;
        let letter = lo.is_ascii_lowercase();
        let up = self.shift ^ (self.caps && letter);
        Some(if up { hi } else { lo })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn us_layout() {
        let mut m = Mods::default();
        assert_eq!(m.char_for(35), Some('h'));
        assert_eq!(m.char_for(2), Some('1'));
        assert_eq!(m.char_for(53), Some('/'));
        m.update(LEFTSHIFT, true);
        assert_eq!(m.char_for(35), Some('H'));
        assert_eq!(m.char_for(2), Some('!'));
        m.update(LEFTSHIFT, false);
        m.update(CAPSLOCK, true);
        assert_eq!(m.char_for(35), Some('H'));
        assert_eq!(m.char_for(2), Some('1'));
        assert_eq!(m.char_for(ENTER), None);
    }
}
