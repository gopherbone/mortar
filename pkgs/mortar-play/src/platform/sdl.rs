// SPDX-License-Identifier: GPL-2.0-or-later
//! SDL3: gamepads, keyboard, audio, and (outside the shell) the window.

use super::{Display, PEvent};
use crate::audio::AudioOut;
use crate::input::{AxisLatch, Button, brick_raw_button, from_sdl_gamepad};
use crate::video::{Dst, Frame};
use mortar_ui::gfx::Ui;
use sdl3::audio::{AudioDevice, AudioFormat, AudioSpec, AudioStreamOwner};
use sdl3::event::{Event, WindowEvent};
use sdl3::gamepad::{Axis, Gamepad};
use sdl3::joystick::Joystick;
use sdl3::keyboard::Keycode;
use sdl3::sys;
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub struct SdlInput {
    sdl: sdl3::Sdl,
    video: Option<sdl3::VideoSubsystem>,
    pump: sdl3::EventPump,
    gamepad: Option<sdl3::GamepadSubsystem>,
    joystick: Option<sdl3::JoystickSubsystem>,
    pads: HashMap<u32, Gamepad>,
    brick_raw: HashMap<u32, Joystick>,
    axes: AxisLatch,
    debug: bool,
}

/// Keyboard (macOS dev window, USB keyboards on KMS) -> Brick buttons.
/// Arrows = D-pad, X = A, Z = B, S = X, A = Y, Q/W = L1/R1, 1/2 = L2/R2,
/// Return = START, Right Shift / Backspace = SELECT, Esc / Tab = MENU,
/// F1 / F2 = F1 / F2 (Space also F2: fast-forward on most systems).
pub fn key_button(k: Keycode) -> Option<Button> {
    Some(match k {
        Keycode::Up => Button::Up,
        Keycode::Down => Button::Down,
        Keycode::Left => Button::Left,
        Keycode::Right => Button::Right,
        Keycode::X => Button::A,
        Keycode::Z => Button::B,
        Keycode::S => Button::X,
        Keycode::A => Button::Y,
        Keycode::Q => Button::L1,
        Keycode::W => Button::R1,
        Keycode::_1 => Button::L2,
        Keycode::_2 => Button::R2,
        Keycode::Return | Keycode::KpEnter => Button::Start,
        Keycode::RShift | Keycode::Backspace => Button::Select,
        Keycode::Escape | Keycode::Tab => Button::Menu,
        Keycode::F1 => Button::F1,
        Keycode::F2 | Keycode::Space => Button::F2,
        _ => return None,
    })
}

impl SdlInput {
    /// Init SDL (video only when `with_video`). Returns the audio output too.
    pub fn init(with_video: bool) -> Result<(SdlInput, Box<dyn AudioOut>), String> {
        sdl3::hint::set("SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS", "1");
        // the shell configures the window; don't let SDL inhibit the screensaver etc.
        let sdl = sdl3::init().map_err(|e| format!("SDL: {e}"))?;
        let video = if with_video { Some(sdl.video().map_err(|e| format!("SDL video: {e}"))?) } else { None };
        let gamepad = sdl.gamepad().ok();
        let joystick = sdl.joystick().ok();
        let audio: Box<dyn AudioOut> = match sdl.audio().map_err(|e| e.to_string()).and_then(|a| SdlAudio::open(&a)) {
            Ok(a) => Box::new(a),
            Err(e) => {
                eprintln!("mortar-play: no audio ({e}); continuing silent");
                Box::new(crate::audio::NullAudio { rate: 44100 })
            }
        };
        let pump = sdl.event_pump().map_err(|e| format!("SDL events: {e}"))?;
        let debug = std::env::var_os("MORTAR_PLAY_DEBUG_INPUT").is_some();
        Ok((SdlInput { sdl, video, pump, gamepad, joystick, pads: HashMap::new(), brick_raw: HashMap::new(), axes: AxisLatch::default(), debug }, audio))
    }

    pub fn open_window(&mut self, w: u32, h: u32) -> Result<SdlDisplay, String> {
        let video = self.video.as_ref().ok_or("no SDL video")?;
        let driver = video.current_video_driver().to_string();
        let fullscreen = !matches!(driver.as_str(), "cocoa" | "windows" | "x11" | "wayland");
        let mut wb = video.window("Games", w, h);
        if fullscreen {
            wb.fullscreen();
        } else {
            wb.resizable();
        }
        let window = wb.build().map_err(|e| format!("SDL window: {e}"))?;
        if fullscreen {
            self.sdl.mouse().show_cursor(false);
        }
        SdlDisplay::new(window, w, h, driver)
    }

    pub fn poll(&mut self, out: &mut Vec<PEvent>) {
        let events: Vec<Event> = self.pump.poll_iter().collect();
        for ev in events {
            self.event(ev, out);
        }
    }

    fn event(&mut self, ev: Event, out: &mut Vec<PEvent>) {
        if self.debug && !matches!(ev, Event::MouseMotion { .. }) {
            eprintln!("input: {ev:?}");
        }
        match ev {
            Event::Quit { .. } => out.push(PEvent::Quit),
            Event::Window { win_event, .. } => match win_event {
                WindowEvent::CloseRequested => out.push(PEvent::Quit),
                WindowEvent::FocusLost => out.push(PEvent::ReleaseAll),
                WindowEvent::Hidden | WindowEvent::Minimized | WindowEvent::Occluded => out.push(PEvent::Visible(false)),
                WindowEvent::Shown | WindowEvent::Restored | WindowEvent::Exposed => out.push(PEvent::Visible(true)),
                _ => {}
            },
            Event::KeyDown { keycode: Some(k), repeat: false, .. } => {
                if let Some(b) = key_button(k) {
                    out.push(PEvent::Press(b));
                }
            }
            Event::KeyUp { keycode: Some(k), .. } => {
                if let Some(b) = key_button(k) {
                    out.push(PEvent::Release(b));
                }
            }
            Event::GamepadAdded { which, .. } => {
                if let Some(g) = &self.gamepad {
                    match g.open(which) {
                        Ok(p) => {
                            let name = p.name().unwrap_or_default();
                            eprintln!("mortar-play: gamepad {name}");
                            if name.to_ascii_lowercase().contains("trimui brick")
                                && let Some(j) = &self.joystick
                                && let Ok(js) = j.open(which)
                            {
                                self.brick_raw.insert(which.raw(), js);
                            }
                            self.pads.insert(which.raw(), p);
                        }
                        Err(e) => eprintln!("mortar-play: gamepad open: {e}"),
                    }
                }
            }
            Event::GamepadRemoved { which, .. } => {
                self.pads.remove(&which.raw());
                self.brick_raw.remove(&which.raw());
                out.push(PEvent::ReleaseAll);
            }
            Event::GamepadButtonDown { which, button, .. } if self.pads.contains_key(&which.raw()) => {
                if let Some(b) = from_sdl_gamepad(button as i32) {
                    out.push(PEvent::Press(b));
                }
            }
            Event::GamepadButtonUp { which, button, .. } if self.pads.contains_key(&which.raw()) => {
                if let Some(b) = from_sdl_gamepad(button as i32) {
                    out.push(PEvent::Release(b));
                }
            }
            Event::GamepadAxisMotion { which, axis, value, .. } => {
                let (id, pos, neg) = match axis {
                    Axis::TriggerLeft => (4u8, Button::L2, Button::L2),
                    Axis::TriggerRight => (5, Button::R2, Button::R2),
                    Axis::LeftX => (0, Button::Right, Button::Left),
                    Axis::LeftY => (1, Button::Down, Button::Up),
                    _ => return,
                };
                // one latch per direction (triggers only go positive)
                let v = value as i32;
                for (slot, b, mag) in [(id * 2, pos, v.max(0)), (id * 2 + 1, neg, (-v).max(0))] {
                    if slot % 2 == 1 && pos == neg {
                        continue;
                    }
                    match self.axes.update(which.raw(), slot, mag.min(32767) as i16) {
                        Some(true) => out.push(PEvent::Press(b)),
                        Some(false) => out.push(PEvent::Release(b)),
                        None => {}
                    }
                }
            }
            Event::JoyButtonDown { which, button_idx, .. } if self.brick_raw.contains_key(&which.raw()) => {
                if let Some(b) = brick_raw_button(button_idx) {
                    out.push(PEvent::Press(b));
                }
            }
            Event::JoyButtonUp { which, button_idx, .. } if self.brick_raw.contains_key(&which.raw()) => {
                if let Some(b) = brick_raw_button(button_idx) {
                    out.push(PEvent::Release(b));
                }
            }
            _ => {}
        }
    }
}

// ------------------------------------------------------------------ audio ----

pub struct SdlAudio {
    device: AudioDevice,
    stream: AudioStreamOwner,
    rate: u32,
}

impl SdlAudio {
    fn open(a: &sdl3::AudioSubsystem) -> Result<SdlAudio, String> {
        let device = a.open_playback_device(&AudioSpec { freq: None, channels: None, format: None }).map_err(|e| e.to_string())?;
        let rate = device.format().ok().and_then(|(s, _)| s.freq).filter(|&r| r > 0).unwrap_or(44100) as u32;
        let src = AudioSpec { freq: Some(rate as i32), channels: Some(2), format: Some(AudioFormat::s16_sys()) };
        let stream = a.new_playback_stream(&src, None).map_err(|e| e.to_string())?;
        device.bind_stream(&stream).map_err(|e| e.to_string())?;
        device.resume();
        eprintln!("mortar-play: audio {} at {rate} Hz", device.name().unwrap_or_default());
        Ok(SdlAudio { device, stream, rate })
    }
}

impl AudioOut for SdlAudio {
    fn rate(&self) -> u32 {
        self.rate
    }
    fn queued(&self) -> usize {
        self.stream.queued_bytes().unwrap_or(0).max(0) as usize / 4
    }
    fn push(&mut self, s: &[i16]) {
        let _ = self.stream.put_data_i16(s);
    }
    fn set_paused(&mut self, paused: bool) {
        if paused {
            self.device.pause();
        } else {
            self.device.resume();
        }
    }
    fn clear(&mut self) {
        let _ = self.stream.clear();
    }
}

// ----------------------------------------------------------------- window ----

pub struct SdlDisplay {
    window: sdl3::video::Window,
    r: *mut sys::render::SDL_Renderer,
    game: *mut sys::render::SDL_Texture,
    game_size: (u32, u32),
    ui: *mut sys::render::SDL_Texture,
    logical: (u32, u32),
    driver: String,
    renderer: String,
}

impl SdlDisplay {
    fn new(window: sdl3::video::Window, w: u32, h: u32, driver: String) -> Result<SdlDisplay, String> {
        unsafe {
            let r = sys::render::SDL_CreateRenderer(window.raw(), std::ptr::null());
            if r.is_null() {
                return Err(format!("SDL renderer: {}", sdl3::get_error()));
            }
            // Pacing is ours (frame clock); a blocking present would fight it.
            sys::render::SDL_SetRenderVSync(r, 0);
            let name = sys::render::SDL_GetRendererName(r);
            let renderer = if name.is_null() { "?".into() } else { std::ffi::CStr::from_ptr(name).to_string_lossy().into_owned() };
            let ui = sys::render::SDL_CreateTexture(r, sys::pixels::SDL_PIXELFORMAT_RGBA32, sys::render::SDL_TEXTUREACCESS_STREAMING, w as i32, h as i32);
            sys::render::SDL_SetTextureScaleMode(ui, sys::surface::SDL_SCALEMODE_LINEAR);
            eprintln!("mortar-play: SDL video {driver}, renderer {renderer}");
            Ok(SdlDisplay { window, r, game: std::ptr::null_mut(), game_size: (0, 0), ui, logical: (w, h), driver, renderer })
        }
    }

    fn scale(&self) -> (f32, f32) {
        let (mut ow, mut oh) = (0, 0);
        unsafe { sys::render::SDL_GetRenderOutputSize(self.r, &mut ow, &mut oh) };
        if ow <= 0 || oh <= 0 {
            return (1.0, 1.0);
        }
        (ow as f32 / self.logical.0 as f32, oh as f32 / self.logical.1 as f32)
    }
}

impl Drop for SdlDisplay {
    fn drop(&mut self) {
        unsafe {
            if !self.game.is_null() {
                sys::render::SDL_DestroyTexture(self.game);
            }
            sys::render::SDL_DestroyTexture(self.ui);
            sys::render::SDL_DestroyRenderer(self.r);
        }
        let _ = &self.window;
    }
}

impl Display for SdlDisplay {
    fn size(&self) -> (u32, u32) {
        self.logical
    }

    fn present_game(&mut self, f: &Frame, dst: Dst, sharp: bool) {
        if f.is_empty() {
            return;
        }
        unsafe {
            if self.game_size != (f.w, f.h) || self.game.is_null() {
                if !self.game.is_null() {
                    sys::render::SDL_DestroyTexture(self.game);
                }
                self.game = sys::render::SDL_CreateTexture(
                    self.r,
                    sys::pixels::SDL_PIXELFORMAT_XRGB8888,
                    sys::render::SDL_TEXTUREACCESS_STREAMING,
                    f.w as i32,
                    f.h as i32,
                );
                self.game_size = (f.w, f.h);
            }
            sys::render::SDL_SetTextureScaleMode(self.game, if sharp { sys::surface::SDL_SCALEMODE_NEAREST } else { sys::surface::SDL_SCALEMODE_LINEAR });
            sys::render::SDL_UpdateTexture(self.game, std::ptr::null(), f.px.as_ptr() as *const _, (f.w * 4) as i32);
            let (sx, sy) = self.scale();
            let d = sys::rect::SDL_FRect { x: dst.x as f32 * sx, y: dst.y as f32 * sy, w: dst.w as f32 * sx, h: dst.h as f32 * sy };
            sys::render::SDL_SetRenderDrawColor(self.r, 0, 0, 0, 255);
            sys::render::SDL_RenderClear(self.r);
            sys::render::SDL_RenderTexture(self.r, self.game, std::ptr::null(), &d);
            sys::render::SDL_RenderPresent(self.r);
        }
    }

    fn present_ui(&mut self, ui: &Ui) {
        unsafe {
            sys::render::SDL_UpdateTexture(self.ui, std::ptr::null(), ui.pm.data().as_ptr() as *const _, (ui.pm.width() * 4) as i32);
            sys::render::SDL_RenderTexture(self.r, self.ui, std::ptr::null(), std::ptr::null());
            sys::render::SDL_RenderPresent(self.r);
        }
    }

    fn wait(&mut self, timeout: Duration) {
        let t = Instant::now();
        std::thread::sleep(timeout.min(Duration::from_millis(8)));
        let _ = t;
    }

    fn name(&self) -> String {
        format!("sdl/{}/{}", self.driver, self.renderer)
    }
}
