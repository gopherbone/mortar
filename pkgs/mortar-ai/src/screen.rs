// SPDX-License-Identifier: GPL-2.0-or-later
//! What the window loop (wl.rs) drives: a full-screen page that draws itself,
//! takes keys (evdev codes) and may have a fd to wake on.

use mortar_ui::gfx::Ui;
use std::os::fd::RawFd;

pub trait Screen {
    fn title(&self) -> &str;
    /// A fd that becomes readable when `poll` has work.
    fn wake_fd(&self) -> Option<RawFd>;
    /// Called after every wakeup (and at least every few seconds). Returns
    /// true if the screen changed.
    fn poll(&mut self) -> bool;
    fn key(&mut self, code: u16, down: bool);
    /// Keyboard focus left: forget held modifiers.
    fn reset_keys(&mut self);
    fn draw(&mut self, ui: &mut Ui);
    /// Close the window and exit.
    fn done(&self) -> bool;
}
