// SPDX-License-Identifier: GPL-2.0-or-later
//! Presentation + input backends. The UI renders into `Ui::pm` in software;
//! a backend only has to put those pixels somewhere and deliver button
//! events. This is the seam for a future GPU path: an EGL backend on the
//! PowerVR stack would upload the canvas as a texture (or render natively)
//! and implement the same trait.

use crate::gfx::Ui;
use crate::input::Event;
use std::time::Duration;

pub mod headless;
#[cfg(target_os = "linux")]
pub mod evdev_in;
#[cfg(target_os = "linux")]
pub mod kms;
#[cfg(target_os = "macos")]
pub mod window;

pub trait Backend {
    /// Screen size in pixels the UI should render at.
    fn size(&self) -> (u32, u32);
    /// Show the current canvas.
    fn present(&mut self, ui: &Ui) -> Result<(), String>;
    /// Block up to `timeout` (forever if None) and append raw press/release
    /// events. Auto-repeat is added by the caller.
    fn wait(&mut self, timeout: Option<Duration>, out: &mut Vec<Event>);
    /// Hand the display back (restore the console mode, drop DRM master)
    /// before a session takes over the screen.
    fn release(&mut self) {}
}

pub fn open(name: &str, w: u32, h: u32) -> Result<Box<dyn Backend>, String> {
    match name {
        "headless" => Ok(Box::new(headless::Headless::new(w, h))),
        #[cfg(target_os = "linux")]
        "kms" | "auto" => Ok(Box::new(kms::Kms::open()?)),
        #[cfg(target_os = "macos")]
        "window" | "auto" => Ok(Box::new(window::Window::open(w, h)?)),
        other => Err(format!("backend '{other}' is not available on this platform")),
    }
}
