// SPDX-License-Identifier: GPL-2.0-or-later
//! Input: every source (Brick buttons, USB/Bluetooth gamepads and keyboards
//! via evdev, the macOS dev window, scripted screenshot input) is turned into
//! the same logical `Button`s, named after the Brick's printed labels.
//!
//! The Brick's face buttons follow Nintendo positions: the right button is
//! labelled A and reports BTN_EAST, the bottom one is B / BTN_SOUTH. We map by
//! position (EAST = A = accept), so any evdev gamepad that follows the Linux
//! gamepad spec gets "right face button accepts, bottom goes back".

use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Button {
    Up,
    Down,
    Left,
    Right,
    A,
    B,
    X,
    Y,
    L1,
    R1,
    L2,
    R2,
    Select,
    Start,
    Menu,
    VolUp,
    VolDown,
    Power,
    F1,
    F2,
}

impl Button {
    pub fn parse(s: &str) -> Option<Button> {
        use Button::*;
        Some(match s.to_ascii_lowercase().as_str() {
            "up" => Up,
            "down" => Down,
            "left" => Left,
            "right" => Right,
            "a" => A,
            "b" => B,
            "x" => X,
            "y" => Y,
            "l1" | "l" => L1,
            "r1" | "r" => R1,
            "l2" => L2,
            "r2" => R2,
            "select" => Select,
            "start" => Start,
            "menu" => Menu,
            "volup" => VolUp,
            "voldown" => VolDown,
            "power" => Power,
            "f1" => F1,
            "f2" => F2,
            _ => return None,
        })
    }
    /// Buttons that auto-repeat while held.
    fn repeats(self) -> bool {
        matches!(self, Button::Up | Button::Down | Button::Left | Button::Right | Button::VolUp | Button::VolDown)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A press, or a repeat while held (`repeat` = true).
    Press { button: Button, repeat: bool },
    Release(Button),
    /// Text typed on a real keyboard (for PIN/text fields).
    Char(char),
    Quit,
}

// Linux input-event-codes.h values. Kept here (not only in the evdev backend)
// so the mapping is one readable table and testable on any OS.
pub mod codes {
    pub const KEY_ESC: u16 = 1;
    pub const KEY_1: u16 = 2;
    pub const KEY_0: u16 = 11;
    pub const KEY_BACKSPACE: u16 = 14;
    pub const KEY_TAB: u16 = 15;
    pub const KEY_ENTER: u16 = 28;
    pub const KEY_SPACE: u16 = 57;
    pub const KEY_F1: u16 = 59;
    pub const KEY_F2: u16 = 60;
    pub const KEY_KPENTER: u16 = 96;
    pub const KEY_UP: u16 = 103;
    pub const KEY_PAGEUP: u16 = 104;
    pub const KEY_LEFT: u16 = 105;
    pub const KEY_RIGHT: u16 = 106;
    pub const KEY_DOWN: u16 = 108;
    pub const KEY_PAGEDOWN: u16 = 109;
    pub const KEY_VOLUMEDOWN: u16 = 114;
    pub const KEY_VOLUMEUP: u16 = 115;
    pub const KEY_POWER: u16 = 116;
    pub const KEY_COMPOSE: u16 = 127;
    pub const KEY_LEFTBRACE: u16 = 26;
    pub const KEY_RIGHTBRACE: u16 = 27;
    pub const KEY_FN: u16 = 0x1d0;
    pub const BTN_SOUTH: u16 = 0x130;
    pub const BTN_EAST: u16 = 0x131;
    pub const BTN_NORTH: u16 = 0x133;
    pub const BTN_WEST: u16 = 0x134;
    pub const BTN_TL: u16 = 0x136;
    pub const BTN_TR: u16 = 0x137;
    pub const BTN_TL2: u16 = 0x138;
    pub const BTN_TR2: u16 = 0x139;
    pub const BTN_SELECT: u16 = 0x13a;
    pub const BTN_START: u16 = 0x13b;
    pub const BTN_MODE: u16 = 0x13c;
    pub const BTN_DPAD_UP: u16 = 0x220;
    pub const BTN_DPAD_DOWN: u16 = 0x221;
    pub const BTN_DPAD_LEFT: u16 = 0x222;
    pub const BTN_DPAD_RIGHT: u16 = 0x223;
    pub const BTN_TRIGGER_HAPPY1: u16 = 0x2c0;
    pub const BTN_TRIGGER_HAPPY2: u16 = 0x2c1;
}

/// evdev key/button code -> logical button. Covers the Brick's gpio-keys
/// "TrimUI Brick Gamepad" (codes from the kernel DTS), its volume keys, the
/// PMIC power key, standard gamepads and keyboards.
pub fn map_evdev_key(code: u16) -> Option<Button> {
    use codes::*;
    use Button::*;
    Some(match code {
        // Brick gamepad (sun50i-a133-trimui-brick.dts) / Linux gamepad spec
        BTN_DPAD_UP => Up,
        BTN_DPAD_DOWN => Down,
        BTN_DPAD_LEFT => Left,
        BTN_DPAD_RIGHT => Right,
        BTN_EAST => A,
        BTN_SOUTH => B,
        BTN_NORTH => X,
        BTN_WEST => Y,
        BTN_TL => L1,
        BTN_TR => R1,
        BTN_TL2 => L2,
        BTN_TR2 => R2,
        BTN_SELECT => Select,
        BTN_START => Start,
        BTN_MODE => Menu,
        BTN_TRIGGER_HAPPY1 => F1,
        BTN_TRIGGER_HAPPY2 => F2,
        KEY_VOLUMEUP => VolUp,
        KEY_VOLUMEDOWN => VolDown,
        KEY_POWER => Power,
        // keyboards
        KEY_UP => Up,
        KEY_DOWN => Down,
        KEY_LEFT => Left,
        KEY_RIGHT => Right,
        KEY_ENTER | KEY_KPENTER => A,
        KEY_ESC | KEY_BACKSPACE => B,
        KEY_PAGEUP | KEY_LEFTBRACE => L1,
        KEY_PAGEDOWN | KEY_RIGHTBRACE => R1,
        KEY_TAB | KEY_F1 | KEY_COMPOSE => Menu,
        KEY_SPACE => Select,
        _ => return None,
    })
}

/// Digit keys typed on a keyboard (for the PIN pad).
pub fn map_evdev_char(code: u16) -> Option<char> {
    use codes::*;
    match code {
        KEY_1..=10 => Some((b'1' + (code - KEY_1) as u8) as char),
        KEY_0 => Some('0'),
        _ => None,
    }
}

/// Software auto-repeat for held D-pad/volume buttons. The Brick's gamepad
/// gpio-keys node has no `autorepeat`, and keyboards repeat at their own
/// rate; kernel repeats are ignored and this gives one consistent feel.
pub struct Repeater {
    held: Option<(Button, Instant)>,
    delay: Duration,
    rate: Duration,
}

impl Default for Repeater {
    fn default() -> Self {
        Repeater { held: None, delay: Duration::from_millis(380), rate: Duration::from_millis(90) }
    }
}

impl Repeater {
    pub fn press(&mut self, b: Button) {
        if b.repeats() {
            self.held = Some((b, Instant::now() + self.delay));
        }
    }
    pub fn release(&mut self, b: Button) {
        if matches!(self.held, Some((h, _)) if h == b) {
            self.held = None;
        }
    }
    /// How long the event loop may sleep before the next repeat is due.
    pub fn timeout(&self) -> Option<Duration> {
        self.held.map(|(_, t)| t.saturating_duration_since(Instant::now()))
    }
    pub fn poll(&mut self, out: &mut Vec<Event>) {
        if let Some((b, t)) = self.held
            && Instant::now() >= t {
                out.push(Event::Press { button: b, repeat: true });
                self.held = Some((b, t + self.rate));
            }
    }
}
