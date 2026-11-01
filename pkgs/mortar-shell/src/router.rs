// SPDX-License-Identifier: GPL-2.0-or-later
//! Gamepad routing: who gets a button press. Pure logic, unit-tested; the
//! evdev/uinput plumbing lives in input.rs and pads.rs.
//!
//! Rules (see DESIGN.md, "Input ownership"):
//! - The shell grabs every physical pad (EVIOCGRAB), so no app can read one
//!   directly.
//! - MENU pressed on its own toggles the shell overlay. MENU pressed while
//!   another pad button is held goes to the app instead (m8c quits on
//!   SELECT+MENU).
//! - With the overlay up, buttons drive the overlay.
//! - Otherwise buttons go to the visible app, as its gamepad mode says:
//!   forwarded to its own virtual pad, turned into keys, or dropped.
//! - Whenever the receiver changes (overlay opens, another app becomes
//!   visible) every button the old receiver saw go down gets a release, so
//!   nothing is left stuck in the background.

use crate::config::GamepadMode;
use mortar_ui::input::{codes, map_evdev_key, Button};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Routed {
    ToggleOverlay,
    /// A press/release for the overlay UI.
    Overlay(Button, bool),
    /// Forward a raw key event (code, value) to app `uid`'s virtual pad.
    Forward(u32, u16, i32),
    /// Pad button for an app in keyboard mode (pressed?).
    Keys(u32, Button, bool),
    /// Release these codes on app `uid`'s virtual pad.
    ReleaseAll(u32, Vec<u16>),
}

/// Who receives pad buttons right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Receiver {
    Overlay,
    App(u32, GamepadMode),
    Nobody,
}

#[derive(Default)]
pub struct Router {
    /// Physical buttons held (any pad).
    held: BTreeSet<u16>,
    /// Codes pressed into the current app's virtual pad and not released.
    forwarded: BTreeSet<u16>,
    /// The app the forwarded set belongs to.
    forwarded_to: Option<u32>,
    /// MENU went to the app (chord): its release goes there too.
    menu_to_app: bool,
}

impl Router {
    /// The receiver changed (overlay toggled, focus moved): release
    /// everything the previous app holds.
    pub fn retarget(&mut self, now: Receiver, out: &mut Vec<Routed>) {
        let same = matches!(now, Receiver::App(u, GamepadMode::Forward) if Some(u) == self.forwarded_to);
        if !same && let Some(old) = self.forwarded_to.take() {
            if !self.forwarded.is_empty() {
                out.push(Routed::ReleaseAll(old, std::mem::take(&mut self.forwarded).into_iter().collect()));
            }
            self.menu_to_app = false;
        }
    }

    /// A key event from a physical pad. `value`: 1 press, 0 release
    /// (kernel repeats, 2, are dropped).
    pub fn key(&mut self, code: u16, value: i32, rx: Receiver) -> Vec<Routed> {
        let mut out = Vec::new();
        if value == 2 {
            return out;
        }
        let down = value != 0;
        if down {
            self.held.insert(code);
        } else {
            self.held.remove(&code);
        }

        if code == codes::BTN_MODE {
            let others = self.held.iter().any(|c| *c != codes::BTN_MODE);
            if down {
                if others && let Receiver::App(uid, mode) = rx {
                    self.menu_to_app = true;
                    self.emit(uid, mode, code, value, &mut out);
                    return out;
                }
                out.push(Routed::ToggleOverlay);
            } else if self.menu_to_app {
                self.menu_to_app = false;
                if let Receiver::App(uid, mode) = rx {
                    self.emit(uid, mode, code, value, &mut out);
                }
            }
            return out;
        }

        match rx {
            Receiver::Overlay => {
                if let Some(b) = map_evdev_key(code) {
                    out.push(Routed::Overlay(b, down));
                }
            }
            Receiver::App(uid, mode) => self.emit(uid, mode, code, value, &mut out),
            Receiver::Nobody => {}
        }
        out
    }

    /// Physical buttons currently held (diagnostics).
    pub fn held(&self) -> Vec<u16> {
        self.held.iter().copied().collect()
    }

    fn emit(&mut self, uid: u32, mode: GamepadMode, code: u16, value: i32, out: &mut Vec<Routed>) {
        let down = value != 0;
        match mode {
            GamepadMode::Forward => {
                if self.forwarded_to != Some(uid) {
                    self.retarget(Receiver::App(uid, mode), out);
                    self.forwarded_to = Some(uid);
                }
                if down {
                    self.forwarded.insert(code);
                } else if !self.forwarded.remove(&code) {
                    return; // release of a press the app never saw
                }
                out.push(Routed::Forward(uid, code, value));
            }
            GamepadMode::Keyboard => {
                if let Some(b) = map_evdev_key(code) {
                    out.push(Routed::Keys(uid, b, down));
                }
            }
            GamepadMode::None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codes::*;
    const APP: Receiver = Receiver::App(7, GamepadMode::Forward);

    #[test]
    fn menu_alone_toggles() {
        let mut r = Router::default();
        assert_eq!(r.key(BTN_MODE, 1, APP), vec![Routed::ToggleOverlay]);
        assert_eq!(r.key(BTN_MODE, 0, Receiver::Overlay), vec![]);
    }

    #[test]
    fn menu_chord_goes_to_app() {
        let mut r = Router::default();
        assert_eq!(r.key(BTN_SELECT, 1, APP), vec![Routed::Forward(7, BTN_SELECT, 1)]);
        assert_eq!(r.key(BTN_MODE, 1, APP), vec![Routed::Forward(7, BTN_MODE, 1)]);
        assert_eq!(r.key(BTN_MODE, 0, APP), vec![Routed::Forward(7, BTN_MODE, 0)]);
        assert_eq!(r.key(BTN_SELECT, 0, APP), vec![Routed::Forward(7, BTN_SELECT, 0)]);
    }

    #[test]
    fn overlay_gets_buttons_and_app_gets_releases() {
        let mut r = Router::default();
        r.key(BTN_EAST, 1, APP);
        let mut out = Vec::new();
        r.retarget(Receiver::Overlay, &mut out);
        assert_eq!(out, vec![Routed::ReleaseAll(7, vec![BTN_EAST])]);
        // the physical release now goes nowhere for the app
        assert_eq!(r.key(BTN_EAST, 0, Receiver::Overlay), vec![Routed::Overlay(Button::A, false)]);
        assert_eq!(r.key(BTN_DPAD_DOWN, 1, Receiver::Overlay), vec![Routed::Overlay(Button::Down, true)]);
    }

    #[test]
    fn switching_apps_releases_old_app() {
        let mut r = Router::default();
        r.key(BTN_SOUTH, 1, APP);
        let other = Receiver::App(8, GamepadMode::Forward);
        let out = r.key(BTN_EAST, 1, other);
        assert_eq!(out, vec![Routed::ReleaseAll(7, vec![BTN_SOUTH]), Routed::Forward(8, BTN_EAST, 1)]);
        // SOUTH release: app 8 never saw the press
        assert_eq!(r.key(BTN_SOUTH, 0, other), vec![]);
    }

    #[test]
    fn keyboard_mode_and_repeats() {
        let mut r = Router::default();
        let term = Receiver::App(3, GamepadMode::Keyboard);
        assert_eq!(r.key(BTN_DPAD_UP, 1, term), vec![Routed::Keys(3, Button::Up, true)]);
        assert_eq!(r.key(BTN_DPAD_UP, 2, term), vec![]);
        assert_eq!(r.key(BTN_SOUTH, 1, Receiver::App(4, GamepadMode::None)), vec![]);
    }
}
