// SPDX-License-Identifier: GPL-2.0-or-later
//! Where frames go and buttons come from.
//!
//! - `wayland`: inside mortar-shell (WAYLAND_DISPLAY set). A plain Wayland
//!   client: dma-bufs the shell puts straight on the display plane (the
//!   frame at its own size, scaled by the display engine through a
//!   viewport, or CPU scaled to the screen), else wl_shm (one copy in the
//!   shell). SDL still provides audio and gamepads (the shell's per-app
//!   virtual pad, isolated by SDL hints).
//! - `sdl`: anywhere else (greeter session on the PowerVR GPU through
//!   mortar-sdl-run, KMSDRM, the macOS dev window): SDL window + renderer,
//!   the game texture is scaled by the GPU.
//! - `headless`: tests and screenshots, no SDL at all.

use crate::audio::{AudioOut, NullAudio};
use crate::input::Button;
use crate::video::{Dst, Filter, Frame, Scaler};
use mortar_ui::gfx::Ui;
use std::time::Duration;

#[cfg(target_os = "linux")]
pub mod dumb;
pub mod sdl;
#[cfg(target_os = "linux")]
pub mod wayland;

#[derive(Clone, Debug, PartialEq)]
pub enum PEvent {
    Press(Button),
    Release(Button),
    /// Every button released (focus lost, pad unplugged).
    ReleaseAll,
    /// The window became visible / hidden (shell overlay, app switch).
    Visible(bool),
    Quit,
}

pub trait Display {
    /// Logical screen size the UI and layouts use.
    fn size(&self) -> (u32, u32);
    /// Show a game frame in `dst` (black around it), scaled with `filter`.
    fn present_game(&mut self, f: &Frame, dst: Dst, filter: Filter);
    /// Show a full-screen UI canvas.
    fn present_ui(&mut self, ui: &Ui);
    /// Window-system events (Wayland); SDL events come through `Platform`.
    fn poll(&mut self, _out: &mut Vec<PEvent>) {}
    /// Block until window-system input or `timeout`.
    fn wait(&mut self, timeout: Duration) {
        std::thread::sleep(timeout.min(Duration::from_millis(10)));
    }
    /// The last presented screen (headless screenshots).
    fn snapshot(&self) -> Option<Frame> {
        None
    }
    fn name(&self) -> String;
}

/// UI canvas (RGBA, opaque) -> XRGB words.
pub fn ui_to_xrgb(ui: &Ui, out: &mut [u32], stride: usize) {
    let w = ui.pm.width() as usize;
    for y in 0..ui.pm.height() as usize {
        ui.xrgb_row(y as u32, &mut out[y * stride..y * stride + w]);
    }
}

/// Composes into memory; keeps the last screen for screenshots.
pub struct Headless {
    pub screen: Frame,
    scaler: Scaler,
}

impl Headless {
    pub fn new(w: u32, h: u32) -> Headless {
        Headless { screen: Frame::new(w, h), scaler: Scaler::default() }
    }
}

impl Display for Headless {
    fn size(&self) -> (u32, u32) {
        (self.screen.w, self.screen.h)
    }
    fn present_game(&mut self, f: &Frame, dst: Dst, _filter: Filter) {
        self.screen.px.fill(0);
        let stride = self.screen.w as usize;
        self.scaler.scale(f, dst, &mut self.screen.px, stride);
    }
    fn present_ui(&mut self, ui: &Ui) {
        let stride = self.screen.w as usize;
        ui_to_xrgb(ui, &mut self.screen.px, stride);
    }
    fn snapshot(&self) -> Option<Frame> {
        Some(self.screen.clone())
    }
    fn name(&self) -> String {
        "headless".into()
    }
}

pub struct Platform {
    pub display: Box<dyn Display>,
    pub sdl: Option<sdl::SdlInput>,
}

impl Platform {
    /// `mode`: auto | sdl | wayland | headless.
    pub fn open(mode: &str, w: u32, h: u32) -> Result<(Platform, Box<dyn AudioOut>), String> {
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
        let mode = match mode {
            "auto" if wayland && cfg!(target_os = "linux") => "wayland",
            "auto" => "sdl",
            m => m,
        };
        match mode {
            "headless" => Ok((Platform { display: Box::new(Headless::new(w, h)), sdl: None }, Box::new(NullAudio { rate: 44100 }))),
            #[cfg(target_os = "linux")]
            "wayland" => {
                let display = wayland::WaylandDisplay::connect(w, h)?;
                let (input, audio) = sdl::SdlInput::init(false)?;
                Ok((Platform { display: Box::new(display), sdl: Some(input) }, audio))
            }
            "sdl" => {
                let (mut input, audio) = sdl::SdlInput::init(true)?;
                let display = input.open_window(w, h)?;
                Ok((Platform { display: Box::new(display), sdl: Some(input) }, audio))
            }
            m => Err(format!("unknown display mode '{m}' (auto, sdl, wayland, headless)")),
        }
    }

    pub fn poll(&mut self, out: &mut Vec<PEvent>) {
        if let Some(s) = &mut self.sdl {
            s.poll(out);
        }
        self.display.poll(out);
    }

    /// Idle until input (UI screens): short sleeps, input arrives via SDL.
    pub fn wait(&mut self, timeout: Duration) {
        self.display.wait(timeout);
    }
}
