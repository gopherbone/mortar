// SPDX-License-Identifier: GPL-2.0-or-later
//! Buttons: every source (SDL gamepads incl. the Brick's gpio-keys pad and
//! the shell's virtual clones of it, keyboards) becomes mortar-ui's logical
//! `Button`, named after the Brick's printed labels and mapped by position
//! (right face button = A). Games see a RetroPad through a per-system map;
//! hotkeys (menu chord, fast-forward) are taken out first.

pub use mortar_ui::input::Button;
use std::collections::HashMap;

/// The buttons a game can get.
pub const GAME_BUTTONS: [Button; 16] = [
    Button::Up,
    Button::Down,
    Button::Left,
    Button::Right,
    Button::A,
    Button::B,
    Button::X,
    Button::Y,
    Button::L1,
    Button::R1,
    Button::L2,
    Button::R2,
    Button::Select,
    Button::Start,
    Button::F1,
    Button::F2,
];

/// RetroPad buttons (RETRO_DEVICE_ID_JOYPAD_*).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum PadId {
    B = 0,
    Y = 1,
    Select = 2,
    Start = 3,
    Up = 4,
    Down = 5,
    Left = 6,
    Right = 7,
    A = 8,
    X = 9,
    L = 10,
    R = 11,
    L2 = 12,
    R2 = 13,
    L3 = 14,
    R3 = 15,
}

impl PadId {
    pub fn parse(s: &str) -> Option<PadId> {
        use PadId::*;
        Some(match s.to_ascii_lowercase().as_str() {
            "b" => B,
            "y" => Y,
            "select" => Select,
            "start" => Start,
            "up" => Up,
            "down" => Down,
            "left" => Left,
            "right" => Right,
            "a" => A,
            "x" => X,
            "l" | "l1" => L,
            "r" | "r1" => R,
            "l2" => L2,
            "r2" => R2,
            "l3" => L3,
            "r3" => R3,
            _ => return None,
        })
    }

    /// The RetroPad button in the same position as a Brick button (the
    /// RetroPad is SNES-shaped, like the Brick's face buttons).
    pub fn same_as(b: Button) -> Option<PadId> {
        use PadId::*;
        Some(match b {
            Button::Up => Up,
            Button::Down => Down,
            Button::Left => Left,
            Button::Right => Right,
            Button::A => A,
            Button::B => B,
            Button::X => X,
            Button::Y => Y,
            Button::L1 => L,
            Button::R1 => R,
            Button::L2 => L2,
            Button::R2 => R2,
            Button::Select => Select,
            Button::Start => Start,
            _ => return None,
        })
    }
}

/// Held buttons across all sources (a button is held while any source
/// holds it).
#[derive(Default, Debug)]
pub struct Held {
    n: HashMap<Button, u32>,
}

impl Held {
    /// Returns true on the first press (edge).
    pub fn down(&mut self, b: Button) -> bool {
        let n = self.n.entry(b).or_insert(0);
        *n += 1;
        *n == 1
    }
    /// Returns true on the last release (edge).
    pub fn up(&mut self, b: Button) -> bool {
        match self.n.get_mut(&b) {
            Some(n) if *n > 1 => {
                *n -= 1;
                false
            }
            Some(_) => {
                self.n.remove(&b);
                true
            }
            None => false,
        }
    }
    pub fn is(&self, b: Button) -> bool {
        self.n.contains_key(&b)
    }
    pub fn clear(&mut self) {
        self.n.clear();
    }

    /// RetroPad bitmask for the held buttons.
    pub fn retropad(&self, map: &HashMap<Button, PadId>) -> u16 {
        let mut m = 0u16;
        for b in self.n.keys() {
            if let Some(p) = map.get(b) {
                m |= 1 << (*p as u32);
            }
        }
        m
    }
}

/// A hotkey chord, e.g. "select+menu": fires when its last button is
/// pressed while the others are held.
#[derive(Clone, Debug, PartialEq)]
pub struct Chord(pub Vec<Button>);

impl Chord {
    pub fn parse(s: &str) -> Option<Chord> {
        let v: Option<Vec<Button>> = s.split('+').map(|p| Button::parse(p.trim())).collect();
        v.filter(|v| !v.is_empty()).map(Chord)
    }
    pub fn fires(&self, pressed: Button, held: &Held) -> bool {
        let Some((last, rest)) = self.0.split_last() else { return false };
        *last == pressed && rest.iter().all(|b| held.is(*b))
    }
}

/// SDL3 gamepad button index (SDL_GamepadButton) -> Brick button, by
/// position: east = A, south = B, north = X, west = Y.
pub fn from_sdl_gamepad(idx: i32) -> Option<Button> {
    Some(match idx {
        0 => Button::B,      // south
        1 => Button::A,      // east
        2 => Button::Y,      // west
        3 => Button::X,      // north
        4 => Button::Select, // back
        5 => Button::Menu,   // guide (BTN_MODE on the Brick)
        6 => Button::Start,
        9 => Button::L1,
        10 => Button::R1,
        11 => Button::Up,
        12 => Button::Down,
        13 => Button::Left,
        14 => Button::Right,
        15 => Button::F1, // misc1
        16 => Button::F2, // right paddle 1
        17 => Button::F1, // left paddle 1
        _ => return None,
    })
}

/// The Brick's gpio-keys pad as a raw SDL joystick: SDL numbers its keys in
/// evdev code order (BTN_SOUTH 0 .. BTN_MODE 10, the four BTN_DPAD_* 11-14,
/// BTN_TRIGGER_HAPPY1/2 15/16). The gamepad mapping SDL builds has no slot
/// for TRIGGER_HAPPY (F1/F2), so those two come from the raw buttons.
pub fn brick_raw_button(idx: u8) -> Option<Button> {
    match idx {
        15 => Some(Button::F1),
        16 => Some(Button::F2),
        _ => None,
    }
}

/// Analog trigger / stick -> digital with hysteresis.
#[derive(Default)]
pub struct AxisLatch {
    on: HashMap<(u32, u8), bool>,
}

impl AxisLatch {
    /// Returns Some(new state) when it changed.
    pub fn update(&mut self, dev: u32, axis: u8, value: i16) -> Option<bool> {
        let cur = self.on.get(&(dev, axis)).copied().unwrap_or(false);
        let a = value.unsigned_abs();
        let now = if cur { a > 8000 } else { a > 16000 };
        if now != cur {
            self.on.insert((dev, axis), now);
            Some(now)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn held_counts_sources() {
        let mut h = Held::default();
        assert!(h.down(Button::A));
        assert!(!h.down(Button::A));
        assert!(!h.up(Button::A));
        assert!(h.is(Button::A));
        assert!(h.up(Button::A));
        assert!(!h.is(Button::A));
        assert!(!h.up(Button::A));
    }

    #[test]
    fn retropad_bits() {
        let mut map = HashMap::new();
        for b in GAME_BUTTONS {
            if let Some(p) = PadId::same_as(b) {
                map.insert(b, p);
            }
        }
        let mut h = Held::default();
        h.down(Button::A);
        h.down(Button::Up);
        h.down(Button::F1); // unmapped
        assert_eq!(h.retropad(&map), 1 << 8 | 1 << 4);
    }

    #[test]
    fn chords() {
        let c = Chord::parse("select+menu").unwrap();
        let mut h = Held::default();
        assert!(!c.fires(Button::Menu, &h));
        h.down(Button::Select);
        assert!(c.fires(Button::Menu, &h));
        assert!(!c.fires(Button::Select, &h));
        assert_eq!(Chord::parse("menu").unwrap().0, vec![Button::Menu]);
        assert!(Chord::parse("bogus").is_none());
    }

    #[test]
    fn axis_hysteresis() {
        let mut l = AxisLatch::default();
        assert_eq!(l.update(1, 4, 20000), Some(true));
        assert_eq!(l.update(1, 4, 12000), None);
        assert_eq!(l.update(1, 4, 5000), Some(false));
    }
}
