// SPDX-License-Identifier: GPL-2.0-or-later
//! macOS dev window (minifb). Keyboard only:
//!   arrows = D-pad, Enter = A, Esc/Backspace = B, X / Y = X / Y,
//!   Q / E or [ / ] = L1 / R1, Z / C = L2 / R2, Tab or M = MENU,
//!   Space = SELECT,
//!   S = START, digits type into PIN fields.

use super::Backend;
use crate::gfx::Ui;
use crate::input::{Button, Event};
use minifb::{Key, KeyRepeat, WindowOptions};
use std::time::{Duration, Instant};

pub struct Window {
    win: minifb::Window,
    buf: Vec<u32>,
    w: u32,
    h: u32,
}

impl Window {
    pub fn open(w: u32, h: u32) -> Result<Window, String> {
        let mut win = minifb::Window::new("Mortar (dev)", w as usize, h as usize, WindowOptions::default())
            .map_err(|e| e.to_string())?;
        win.set_target_fps(60);
        Ok(Window { win, buf: vec![0; (w * h) as usize], w, h })
    }
}

fn map_key(k: Key) -> Option<Button> {
    use Button::*;
    Some(match k {
        Key::Up => Up,
        Key::Down => Down,
        Key::Left => Left,
        Key::Right => Right,
        Key::Enter => A,
        Key::Escape | Key::Backspace => B,
        Key::X => X,
        Key::Y => Y,
        Key::Q | Key::LeftBracket | Key::PageUp => L1,
        Key::E | Key::RightBracket | Key::PageDown => R1,
        Key::Z => L2,
        Key::C => R2,
        Key::Tab | Key::M => Menu,
        Key::Space => Select,
        Key::S => Start,
        Key::Equal => VolUp,
        Key::Minus => VolDown,
        Key::P => Power,
        _ => return None,
    })
}

fn map_char(k: Key) -> Option<char> {
    let d = match k {
        Key::Key0 => 0,
        Key::Key1 => 1,
        Key::Key2 => 2,
        Key::Key3 => 3,
        Key::Key4 => 4,
        Key::Key5 => 5,
        Key::Key6 => 6,
        Key::Key7 => 7,
        Key::Key8 => 8,
        Key::Key9 => 9,
        _ => return None,
    };
    char::from_digit(d, 10)
}

impl Backend for Window {
    fn size(&self) -> (u32, u32) {
        (self.w, self.h)
    }
    fn present(&mut self, ui: &Ui) -> Result<(), String> {
        for y in 0..self.h {
            let o = (y * self.w) as usize;
            ui.xrgb_row(y, &mut self.buf[o..o + self.w as usize]);
        }
        self.win.update_with_buffer(&self.buf, self.w as usize, self.h as usize).map_err(|e| e.to_string())
    }
    fn wait(&mut self, timeout: Option<Duration>, out: &mut Vec<Event>) {
        let deadline = timeout.map(|t| Instant::now() + t);
        loop {
            self.win.update();
            if !self.win.is_open() {
                out.push(Event::Quit);
                return;
            }
            for k in self.win.get_keys_pressed(KeyRepeat::No) {
                if let Some(c) = map_char(k) {
                    out.push(Event::Char(c));
                } else if let Some(b) = map_key(k) {
                    out.push(Event::Press { button: b, repeat: false });
                }
            }
            for k in self.win.get_keys_released() {
                if let Some(b) = map_key(k) {
                    out.push(Event::Release(b));
                }
            }
            if !out.is_empty() || deadline.is_some_and(|d| Instant::now() >= d) {
                return;
            }
        }
    }
}
