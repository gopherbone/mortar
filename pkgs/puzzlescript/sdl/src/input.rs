// SPDX-License-Identifier: MIT
//! Physical inputs (keyboard, SDL gamepads, raw joysticks) -> abstract buttons.
//!
//! Defaults (all overridable in `controls.cfg`, see `Bindings::load`):
//!   keyboard: arrows/WASD move, X/Space/Enter action, Z/U/Backspace undo, R restart,
//!             Esc menu, Tab level select, PageUp/PageDown page lists
//!   gamepad:  D-pad/left stick move, A action, B undo (hold: restart), Y restart,
//!             X undo, Start/Guide menu, Back(Select) level select, L/R page lists
//! Gamepad A/B follow the controller's printed labels when SDL knows them, else Xbox
//! positions. The TrimUI Brick ("TrimUI Brick Gamepad", gpio-keys emitting BTN_* codes, so SDL3
//! sees an auto-mapped gamepad) has Nintendo positions, A = BTN_EAST; main.rs detects it by name.
use sdl3::gamepad::Button as PadButton;
use sdl3::keyboard::Keycode;
use std::collections::HashMap;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Btn {
    Up,
    Down,
    Left,
    Right,
    A,
    B,
    X,
    Y,
    L,
    R,
    Start,
    Select,
}

pub const ALL: [Btn; 12] = [
    Btn::Up,
    Btn::Down,
    Btn::Left,
    Btn::Right,
    Btn::A,
    Btn::B,
    Btn::X,
    Btn::Y,
    Btn::L,
    Btn::R,
    Btn::Start,
    Btn::Select,
];

impl Btn {
    pub fn name(self) -> &'static str {
        match self {
            Btn::Up => "up",
            Btn::Down => "down",
            Btn::Left => "left",
            Btn::Right => "right",
            Btn::A => "a",
            Btn::B => "b",
            Btn::X => "x",
            Btn::Y => "y",
            Btn::L => "l",
            Btn::R => "r",
            Btn::Start => "start",
            Btn::Select => "select",
        }
    }
    pub fn from_name(s: &str) -> Option<Btn> {
        ALL.iter().copied().find(|b| b.name() == s)
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum PadLayout {
    Auto,
    Xbox,
    Nintendo,
}

pub struct Bindings {
    pub keys: HashMap<Keycode, Btn>,
    /// raw joystick button index -> button, for joysticks SDL has no gamepad mapping for
    pub joy_buttons: HashMap<u8, Btn>,
    pub pad_layout: PadLayout,
}

const DEFAULT_KEYS: &[(&str, &[&str])] = &[
    ("up", &["Up", "W"]),
    ("down", &["Down", "S"]),
    ("left", &["Left", "A"]),
    ("right", &["Right", "D"]),
    ("a", &["X", "Space", "Return", "Keypad Enter", "C"]),
    ("b", &["Z", "U", "Backspace"]),
    ("y", &["R"]),
    ("start", &["Escape"]),
    ("select", &["Tab", "L"]),
    ("l", &["PageUp"]),
    ("r", &["PageDown"]),
];

pub const CONFIG_HELP: &str = "\
# PuzzleScript controls. One binding per line, comma-separated values; a line replaces that
# button's defaults. Buttons: up down left right a b x y l r start select
#   key.<button>  = SDL key names (e.g. Up, X, Space, Return, Escape, Left Shift)
#   joy.<button>  = raw joystick button numbers, for devices SDL has no gamepad mapping for
#   pad_layout    = auto | xbox | nintendo   (which face button is A on a gamepad)
# Gamepad mappings themselves come from SDL; add lines in SDL_GameControllerDB format to
# gamecontrollerdb.txt next to this file, or set SDL_GAMECONTROLLERCONFIG.
";

impl Bindings {
    pub fn defaults() -> Bindings {
        let mut b = Bindings { keys: HashMap::new(), joy_buttons: HashMap::new(), pad_layout: PadLayout::Auto };
        for (btn, names) in DEFAULT_KEYS {
            b.set_keys(Btn::from_name(btn).unwrap(), names.iter().copied());
        }
        b
    }

    fn set_keys<'a>(&mut self, btn: Btn, names: impl Iterator<Item = &'a str>) {
        self.keys.retain(|_, v| *v != btn);
        for n in names {
            match Keycode::from_name(n.trim()) {
                Some(k) => {
                    self.keys.insert(k, btn);
                }
                None => eprintln!("controls: unknown key name '{n}'"),
            }
        }
    }

    /// Defaults, then `controls.cfg` (if present) on top.
    pub fn load(path: &std::path::Path) -> Bindings {
        let mut b = Bindings::defaults();
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => return b,
        };
        for (n, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                eprintln!("controls.cfg:{}: expected key = value", n + 1);
                continue;
            };
            let (k, v) = (k.trim(), v.trim());
            if k == "pad_layout" {
                b.pad_layout = match v {
                    "xbox" => PadLayout::Xbox,
                    "nintendo" => PadLayout::Nintendo,
                    _ => PadLayout::Auto,
                };
            } else if let Some(btn) = k.strip_prefix("key.").and_then(Btn::from_name) {
                b.set_keys(btn, v.split(',').filter(|s| !s.trim().is_empty()));
            } else if let Some(btn) = k.strip_prefix("joy.").and_then(Btn::from_name) {
                b.joy_buttons.retain(|_, x| *x != btn);
                for s in v.split(',') {
                    if let Ok(i) = s.trim().parse::<u8>() {
                        b.joy_buttons.insert(i, btn);
                    }
                }
            } else {
                eprintln!("controls.cfg:{}: unknown setting '{k}'", n + 1);
            }
        }
        b
    }

    /// Maps a gamepad button. `east_is_a`: the pad's label for the east face button is "A".
    pub fn pad(&self, b: PadButton, east_is_a: bool) -> Option<Btn> {
        let nintendo = match self.pad_layout {
            PadLayout::Auto => east_is_a,
            PadLayout::Xbox => false,
            PadLayout::Nintendo => true,
        };
        Some(match b {
            PadButton::DPadUp => Btn::Up,
            PadButton::DPadDown => Btn::Down,
            PadButton::DPadLeft => Btn::Left,
            PadButton::DPadRight => Btn::Right,
            PadButton::South => if nintendo { Btn::B } else { Btn::A },
            PadButton::East => if nintendo { Btn::A } else { Btn::B },
            PadButton::West => if nintendo { Btn::Y } else { Btn::X },
            PadButton::North => if nintendo { Btn::X } else { Btn::Y },
            PadButton::LeftShoulder => Btn::L,
            PadButton::RightShoulder => Btn::R,
            PadButton::Start | PadButton::Guide => Btn::Start,
            PadButton::Back => Btn::Select,
            _ => return None,
        })
    }
}

/// Tracks held state per button across all devices, with edge events.
#[derive(Default)]
pub struct State {
    held: HashMap<Btn, u32>, // number of physical sources holding the button
    pub pressed: Vec<Btn>,
    pub released: Vec<Btn>,
}

impl State {
    pub fn down(&mut self, b: Btn) {
        let n = self.held.entry(b).or_insert(0);
        *n += 1;
        if *n == 1 {
            self.pressed.push(b);
        }
    }
    pub fn up(&mut self, b: Btn) {
        if let Some(n) = self.held.get_mut(&b) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.held.remove(&b);
                self.released.push(b);
            }
        }
    }
    pub fn held(&self, b: Btn) -> bool {
        self.held.contains_key(&b)
    }
    pub fn was_pressed(&self, b: Btn) -> bool {
        self.pressed.contains(&b)
    }
    pub fn clear_edges(&mut self) {
        self.pressed.clear();
        self.released.clear();
    }
    pub fn release_all(&mut self) {
        let bs: Vec<Btn> = self.held.keys().copied().collect();
        for b in bs {
            self.held.remove(&b);
            self.released.push(b);
        }
    }
}
