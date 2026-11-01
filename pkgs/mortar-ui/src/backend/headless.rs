// SPDX-License-Identifier: GPL-2.0-or-later
//! No display: used by --screenshot, which feeds scripted input and writes
//! the final frame to a PNG.

use super::Backend;
use crate::gfx::Ui;
use crate::input::Event;
use std::time::Duration;

pub struct Headless {
    w: u32,
    h: u32,
}

impl Headless {
    pub fn new(w: u32, h: u32) -> Headless {
        Headless { w, h }
    }
}

impl Backend for Headless {
    fn size(&self) -> (u32, u32) {
        (self.w, self.h)
    }
    fn present(&mut self, _ui: &Ui) -> Result<(), String> {
        Ok(())
    }
    fn wait(&mut self, _timeout: Option<Duration>, out: &mut Vec<Event>) {
        out.push(Event::Quit);
    }
}
