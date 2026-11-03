// SPDX-License-Identifier: MIT
//! PuzzleScript, SDL3 shell. The engine is the native C port in ../src (compiled by build.rs);
//! this crate is the platform side: window, input, audio, menus, saves.
mod app;
mod audio;
mod ffi;
mod gfx;
mod input;

use app::App;
use input::{Bindings, Btn, State};
use sdl3::event::{Event, WindowEvent};
use sdl3::gamepad::{Axis, Button as PadButton, ButtonLabel, Gamepad};
use sdl3::joystick::{HatState, Joystick, JoystickId};
use sdl3::keyboard::Keycode;
use sdl3::pixels::PixelFormat;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const USAGE: &str = "\
usage: puzzlescript [options] [GAME.txt]

  GAME.txt          open this game directly (its directory becomes the games directory)
  --games DIR       games directory (default: $PUZZLESCRIPT_GAMES, else the games folder
                    in the data directory; no games ship with the engine)
  --data DIR        saves and settings (default: $PUZZLESCRIPT_DATA, else
                    $XDG_DATA_HOME/puzzlescript or ~/.local/share/puzzlescript)
  --config DIR      controls.cfg / gamecontrollerdb.txt (default: $XDG_CONFIG_HOME/puzzlescript
                    or ~/.config/puzzlescript)
  --fullscreen      start fullscreen (default on KMSDRM / no window system)
  --windowed        start in a window
  --size WxH        window / framebuffer size (default 1024x768)
  --mute            no sound
  --script FILE     scripted input (see sdl/README.md), screenshots to --shots
  --shots DIR       screenshot directory (default: ./shots); F12 saves one at any time
  --headless        no visible window (SDL dummy video/audio drivers); for --script
  --print-controls  print the controls.cfg format and the default key bindings
";

struct Opts {
    games: Option<PathBuf>,
    data: Option<PathBuf>,
    config: Option<PathBuf>,
    open: Option<PathBuf>,
    fullscreen: Option<bool>,
    size: (u32, u32),
    mute: bool,
    script: Option<PathBuf>,
    shots: PathBuf,
    headless: bool,
}

fn parse_args() -> Opts {
    let mut o = Opts {
        games: None,
        data: None,
        config: None,
        open: None,
        fullscreen: None,
        size: (1024, 768),
        mute: false,
        script: None,
        shots: PathBuf::from("shots"),
        headless: false,
    };
    let mut args = std::env::args().skip(1);
    let need = |a: Option<String>, flag: &str| -> String {
        a.unwrap_or_else(|| {
            eprintln!("{flag} needs a value\n{USAGE}");
            std::process::exit(2)
        })
    };
    while let Some(a) = args.next() {
        match a.as_str() {
            "--games" => o.games = Some(need(args.next(), &a).into()),
            "--data" => o.data = Some(need(args.next(), &a).into()),
            "--config" => o.config = Some(need(args.next(), &a).into()),
            "--fullscreen" => o.fullscreen = Some(true),
            "--windowed" => o.fullscreen = Some(false),
            "--mute" => o.mute = true,
            "--script" => o.script = Some(need(args.next(), &a).into()),
            "--shots" => o.shots = need(args.next(), &a).into(),
            "--headless" => o.headless = true,
            "--size" => {
                let v = need(args.next(), &a);
                let p: Vec<u32> = v.split('x').filter_map(|x| x.parse().ok()).collect();
                if p.len() != 2 || p[0] < 64 || p[1] < 64 {
                    eprintln!("bad --size {v}");
                    std::process::exit(2);
                }
                o.size = (p[0], p[1]);
            }
            "--print-controls" => {
                print!("{}", input::CONFIG_HELP);
                let b = Bindings::defaults();
                for btn in input::ALL {
                    let mut ks: Vec<String> = b.keys.iter().filter(|(_, v)| **v == btn).map(|(k, _)| k.name()).collect();
                    ks.sort();
                    if !ks.is_empty() {
                        println!("key.{} = {}", btn.name(), ks.join(", "));
                    }
                }
                std::process::exit(0);
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            s if !s.starts_with("--") => o.open = Some(s.into()),
            _ => {
                eprintln!("unknown option {a}\n{USAGE}");
                std::process::exit(2);
            }
        }
    }
    o
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() => PathBuf::from(v).join("puzzlescript"),
        _ => home().join(fallback).join("puzzlescript"),
    }
}

// ---- engine hooks ------------------------------------------------------------------------------

thread_local! {
    static FRAME_START: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

unsafe extern "C" fn log_hook(msg: *const std::ffi::c_char) {
    if let Some(m) = ffi::cstr(msg) {
        eprintln!("[ps] {m}");
    }
}

/// Engine safety valve: a pathological rule loop gives up after 4 s in one frame instead of
/// freezing the device (the C engine treats it like hitting its iteration caps).
unsafe extern "C" fn over_budget() -> bool {
    FRAME_START.with(|f| f.get().map_or(false, |t| t.elapsed() > Duration::from_secs(4)))
}

// ---- scripted input ------------------------------------------------------------------------------

struct Script {
    lines: Vec<String>,
    pos: usize,
    wait: u32,
    release_next: Vec<Btn>,
    done: bool,
}

impl Script {
    fn load(p: &Path) -> Script {
        let text = std::fs::read_to_string(p).unwrap_or_else(|e| {
            eprintln!("script {}: {e}", p.display());
            std::process::exit(2)
        });
        Script { lines: text.lines().map(|l| l.trim().to_string()).collect(), pos: 0, wait: 0, release_next: Vec::new(), done: false }
    }

    /// Runs commands until one consumes a frame.
    fn step(&mut self, app: &mut App, inp: &mut State, shot: &mut Vec<String>) {
        for b in self.release_next.drain(..) {
            inp.up(b);
        }
        if self.wait > 0 {
            self.wait -= 1;
            return;
        }
        while self.pos < self.lines.len() {
            let line = self.lines[self.pos].clone();
            self.pos += 1;
            let (cmd, arg) = line.split_once(' ').unwrap_or((line.as_str(), ""));
            let arg = arg.trim();
            let btn = || Btn::from_name(arg).unwrap_or_else(|| panic!("script: unknown button '{arg}'"));
            match cmd {
                "" => {}
                c if c.starts_with('#') => {}
                "wait" => {
                    self.wait = arg.parse::<u32>().unwrap_or(1).saturating_sub(1);
                    return;
                }
                "press" => {
                    inp.down(btn());
                    self.release_next.push(btn());
                    return;
                }
                "hold" => {
                    inp.down(btn());
                    return;
                }
                "release" => {
                    inp.up(btn());
                    return;
                }
                "shot" => {
                    shot.push(arg.to_string());
                    return;
                }
                "select" => {
                    app.select_file(arg);
                }
                "play" => {
                    let (n, file) = arg.split_once(' ').unwrap_or(("0", arg));
                    app.play_level(file, n.parse().unwrap_or(0));
                    return;
                }
                "end" => {
                    self.done = true;
                    return;
                }
                _ => eprintln!("script: unknown command '{line}'"),
            }
        }
        self.done = true;
    }
}

// ---- analog/hat -> d-pad ---------------------------------------------------------------------------

/// Direction buttons held by each analog source (a stick axis or a hat).
#[derive(Default)]
struct Analog {
    held: HashMap<(u32, u8), Vec<Btn>>,
}

impl Analog {
    fn set(&mut self, inp: &mut State, key: (u32, u8), now: Vec<Btn>) {
        let old = self.held.remove(&key).unwrap_or_default();
        for b in &old {
            if !now.contains(b) {
                inp.up(*b);
            }
        }
        for b in &now {
            if !old.contains(b) {
                inp.down(*b);
            }
        }
        self.held.insert(key, now);
    }
    /// One axis: 0 = x, 1 = y. Hysteresis keeps a held direction until the stick is nearly centred.
    fn axis(&mut self, inp: &mut State, id: u32, axis: u8, v: i16) {
        let key = (id, axis);
        let cur = self.held.get(&key).cloned().unwrap_or_default();
        let (neg, pos) = if axis == 0 { (Btn::Left, Btn::Right) } else { (Btn::Up, Btn::Down) };
        let v = v as i32;
        let now = if v < -16000 || (v < -8000 && cur.contains(&neg)) {
            vec![neg]
        } else if v > 16000 || (v > 8000 && cur.contains(&pos)) {
            vec![pos]
        } else {
            vec![]
        };
        if now != cur {
            self.set(inp, key, now);
        }
    }
    fn hat(&mut self, inp: &mut State, id: u32, hat: u8, h: HatState) {
        let bits = h as u8;
        let mut now = Vec::new();
        for (m, b) in [(1u8, Btn::Up), (2, Btn::Right), (4, Btn::Down), (8, Btn::Left)] {
            if bits & m != 0 {
                now.push(b);
            }
        }
        self.set(inp, (id, 16 + hat), now);
    }
}

fn default_joy_button(i: u8) -> Option<Btn> {
    // guess for unmapped joysticks; override with joy.<button> in controls.cfg
    Some(match i {
        0 => Btn::A,
        1 => Btn::B,
        2 => Btn::X,
        3 => Btn::Y,
        4 => Btn::L,
        5 => Btn::R,
        6 => Btn::Select,
        7 => Btn::Start,
        _ => return None,
    })
}

// ---- main ------------------------------------------------------------------------------------------

fn main() {
    let o = parse_args();
    if let Err(e) = ffi::check_layout() {
        eprintln!("{e}");
        std::process::exit(1);
    }
    unsafe {
        ffi::ps_log_hook = Some(log_hook);
        ffi::ps_over_budget_hook = Some(over_budget);
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        ffi::ps_rng_set_entropy(t.as_millis() as u32);
    }

    let data_dir = o
        .data
        .clone()
        .or_else(|| std::env::var_os("PUZZLESCRIPT_DATA").map(PathBuf::from))
        .unwrap_or_else(|| xdg("XDG_DATA_HOME", ".local/share"));
    // No games ship with the engine: they are the user's own .txt files.
    let mut games_dir = o
        .games
        .clone()
        .or_else(|| std::env::var_os("PUZZLESCRIPT_GAMES").map(PathBuf::from))
        .unwrap_or_else(|| data_dir.join("games"));
    let mut open_file = None;
    if let Some(f) = &o.open {
        games_dir = f.parent().filter(|p| !p.as_os_str().is_empty()).map(Path::to_path_buf).unwrap_or_else(|| ".".into());
        open_file = f.file_name().map(|n| n.to_string_lossy().into_owned());
    }
    let config_dir = o.config.clone().unwrap_or_else(|| xdg("XDG_CONFIG_HOME", ".config"));
    eprintln!("games: {}\ndata: {}\nconfig: {}", games_dir.display(), data_dir.display(), config_dir.display());

    if o.headless {
        sdl3::hint::set("SDL_VIDEO_DRIVER", "dummy");
        sdl3::hint::set("SDL_AUDIO_DRIVER", "dummy");
    }
    sdl3::hint::set("SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS", "1");
    let sdl = sdl3::init().expect("SDL init");
    let video = sdl.video().expect("SDL video");
    let audio_sys = if o.mute { None } else { sdl.audio().ok() };
    let gamepads = sdl.gamepad().ok();
    let joysticks = sdl.joystick().ok();

    let bindings = Bindings::load(&config_dir.join("controls.cfg"));
    if let Some(g) = &gamepads {
        let db = config_dir.join("gamecontrollerdb.txt");
        if db.exists() {
            match g.load_mappings(&db) {
                Ok(n) => eprintln!("loaded {n} gamepad mappings from {}", db.display()),
                Err(e) => eprintln!("{}: {e:?}", db.display()),
            }
        }
    }

    let driver = video.current_video_driver();
    let fullscreen = o.fullscreen.unwrap_or(matches!(driver, "kmsdrm" | "evdev" | "fbdev" | "mali" | "vivante" | "rpi"));
    let mut wb = video.window("PuzzleScript", o.size.0, o.size.1);
    wb.resizable().high_pixel_density();
    if fullscreen {
        wb.fullscreen();
    }
    let window = wb.build().expect("SDL window");
    let mut canvas = window.into_canvas();
    let tc = canvas.texture_creator();
    if fullscreen {
        sdl.mouse().show_cursor(false);
    }
    eprintln!("video driver: {driver}, renderer: {}", canvas.renderer_name);

    let pixel_size = |c: &sdl3::render::WindowCanvas| -> (i32, i32) {
        let (w, h) = c.output_size().unwrap_or(o.size);
        (w.max(64) as i32, h.max(64) as i32)
    };
    let (w, h) = if o.headless { (o.size.0 as i32, o.size.1 as i32) } else { pixel_size(&canvas) };
    let mut cv = gfx::Canvas::new(w, h);
    let mut tex = opaque(tc.create_texture_streaming(PixelFormat::ARGB8888, w as u32, h as u32).ok());

    let mut audio = audio::Audio::new(audio_sys.as_ref());
    audio.muted = o.mute;
    let mut app = App::new(games_dir, data_dir, audio);
    if o.mute {
        app.audio.muted = true;
    }
    if let Some(f) = &open_file {
        if app.select_file(f) {
            app.screen = app::Screen::Loading;
        }
    }

    let mut events = sdl.event_pump().expect("SDL events");
    let mut inp = State::default();
    let mut analog = Analog::default();
    let mut pads: HashMap<JoystickId, (Gamepad, bool)> = HashMap::new();
    let mut joys: HashMap<JoystickId, Joystick> = HashMap::new();
    let mut script = o.script.as_deref().map(Script::load);
    let mut shot_n = 0;
    let mut last = Instant::now();
    // SDL's KMSDRM backend rebuilds its EGL surface at the first present after a
    // fullscreen/size change and drops what was drawn into the old one, so a
    // single redraw after a resize would leave the screen black. Draw twice.
    let mut redraw = 2u8;
    let frame = Duration::from_micros(16_667);

    'main: loop {
        let t0 = Instant::now();
        FRAME_START.with(|f| f.set(Some(t0)));
        let mut shots: Vec<String> = Vec::new();
        let mut resized = false;
        for ev in events.poll_iter() {
            match ev {
                Event::Quit { .. } => break 'main,
                Event::Window { win_event, .. } => match win_event {
                    WindowEvent::PixelSizeChanged(..) | WindowEvent::Resized(..) => resized = true,
                    WindowEvent::Exposed | WindowEvent::Restored | WindowEvent::Shown => app.mark_dirty(),
                    WindowEvent::FocusLost => inp.release_all(),
                    _ => {}
                },
                Event::KeyDown { keycode: Some(k), repeat: false, .. } => match k {
                    Keycode::F11 => app.want_fullscreen_toggle = true,
                    Keycode::F12 => shots.push(String::new()),
                    k => {
                        if let Some(&b) = bindings.keys.get(&k) {
                            inp.down(b)
                        }
                    }
                },
                Event::KeyUp { keycode: Some(k), .. } => {
                    if let Some(&b) = bindings.keys.get(&k) {
                        inp.up(b)
                    }
                }
                Event::GamepadAdded { which, .. } => {
                    if let Some(g) = &gamepads {
                        match g.open(which) {
                            Ok(p) => {
                                // The TrimUI Brick's gpio-keys pad ("TrimUI Brick Gamepad") reports Linux-gamepad
                                // codes with A = BTN_EAST (Nintendo positions); SDL3 builds its mapping
                                // from the evdev codes but can't know the labels, so match it by name.
                                let name = p.name().unwrap_or_default();
                                let east_is_a = p.button_label_for_gamepad_type(PadButton::East) == ButtonLabel::A
                                    || name.to_ascii_lowercase().contains("trimui");
                                eprintln!(
                                    "gamepad: {} ({})",
                                    name,
                                    if east_is_a { "A on the right" } else { "A at the bottom" }
                                );
                                joys.remove(&which);
                                pads.insert(which, (p, east_is_a));
                            }
                            Err(e) => eprintln!("gamepad open: {e:?}"),
                        }
                    }
                }
                Event::GamepadRemoved { which, .. } => {
                    pads.remove(&which);
                    inp.release_all();
                    analog.held.clear();
                }
                Event::GamepadButtonDown { which, button, .. } => {
                    if let Some((_, east)) = pads.get(&which) {
                        if let Some(b) = bindings.pad(button, *east) {
                            inp.down(b);
                        }
                    }
                }
                Event::GamepadButtonUp { which, button, .. } => {
                    if let Some((_, east)) = pads.get(&which) {
                        if let Some(b) = bindings.pad(button, *east) {
                            inp.up(b);
                        }
                    }
                }
                Event::GamepadAxisMotion { which, axis, value, .. } => match axis {
                    Axis::LeftX => analog.axis(&mut inp, which.raw(), 0, value),
                    Axis::LeftY => analog.axis(&mut inp, which.raw(), 1, value),
                    _ => {}
                },
                // raw joysticks: only ones SDL has no gamepad mapping for
                Event::JoyDeviceAdded { which, .. } => {
                    let is_pad = gamepads.as_ref().map_or(false, |g| g.is_gamepad(which));
                    if !is_pad {
                        if let Some(j) = &joysticks {
                            if let Ok(js) = j.open(which) {
                                eprintln!("joystick without a gamepad mapping: {} (buttons via joy.* in controls.cfg)", js.name());
                                joys.insert(which, js);
                            }
                        }
                    }
                }
                Event::JoyDeviceRemoved { which, .. } => {
                    if joys.remove(&which).is_some() {
                        inp.release_all();
                        analog.held.clear();
                    }
                }
                Event::JoyButtonDown { which, button_idx, .. } if joys.contains_key(&which) => {
                    let b = if bindings.joy_buttons.is_empty() { default_joy_button(button_idx) } else { bindings.joy_buttons.get(&button_idx).copied() };
                    if let Some(b) = b {
                        inp.down(b);
                    }
                }
                Event::JoyButtonUp { which, button_idx, .. } if joys.contains_key(&which) => {
                    let b = if bindings.joy_buttons.is_empty() { default_joy_button(button_idx) } else { bindings.joy_buttons.get(&button_idx).copied() };
                    if let Some(b) = b {
                        inp.up(b);
                    }
                }
                Event::JoyHatMotion { which, hat_idx, state, .. } if joys.contains_key(&which) => {
                    analog.hat(&mut inp, which.raw(), hat_idx, state)
                }
                Event::JoyAxisMotion { which, axis_idx, value, .. } if joys.contains_key(&which) && axis_idx < 2 => {
                    analog.axis(&mut inp, which.raw(), axis_idx, value)
                }
                _ => {}
            }
        }

        if let Some(s) = &mut script {
            s.step(&mut app, &mut inp, &mut shots);
        }

        // fixed steps under a script (deterministic shots), wall-clock time otherwise
        let now = Instant::now();
        let dt = if script.is_some() { 1.0 / 60.0 } else { (now - last).as_secs_f32().min(0.1) };
        last = now;
        app.update(&inp, dt);
        inp.clear_edges();

        if app.want_fullscreen_toggle {
            app.want_fullscreen_toggle = false;
            let win = canvas.window_mut();
            let fs = win.fullscreen_state() != sdl3::video::FullscreenType::Off;
            let _ = win.set_fullscreen(!fs);
            sdl.mouse().show_cursor(fs);
            resized = true;
        }
        if resized && !o.headless {
            let (w, h) = pixel_size(&canvas);
            if (w, h) != (cv.w, cv.h) {
                cv = gfx::Canvas::new(w, h);
                tex = opaque(tc.create_texture_streaming(PixelFormat::ARGB8888, w as u32, h as u32).ok());
            }
            app.mark_dirty();
            redraw = 2;
        }

        let dirty = app.take_dirty() || redraw > 0;
        redraw = redraw.saturating_sub(dirty as u8);
        if dirty {
            app.draw(&mut cv);
            if let Some(t) = &mut tex {
                let bytes: &[u8] = unsafe { std::slice::from_raw_parts(cv.px.as_ptr() as *const u8, cv.px.len() * 4) };
                let _ = t.update(None, bytes, cv.w as usize * 4);
                canvas.clear();
                let _ = canvas.copy(t, None, None);
                canvas.present();
            }
        }
        for name in shots {
            if !dirty {
                app.draw(&mut cv);
            }
            let name = if name.is_empty() {
                shot_n += 1;
                format!("shot_{}_{shot_n:03}", std::process::id())
            } else {
                name
            };
            let path = o.shots.join(format!("{name}.png"));
            match gfx::save_png(&cv, &path) {
                Ok(()) => {
                    eprintln!("screenshot: {}", path.display());
                    if script.is_none() {
                        app.toast("screenshot saved");
                    }
                }
                Err(e) => eprintln!("screenshot {}: {e}", path.display()),
            }
        }

        if app.quit || script.as_ref().map_or(false, |s| s.done) {
            break;
        }
        if script.is_none() {
            let spent = t0.elapsed();
            if spent < frame {
                std::thread::sleep(frame - spent);
            }
        }
    }
}

/// The canvas is always opaque. Without this SDL alpha-blends ARGB textures,
/// which with a software GL (llvmpipe) reads back the write-combined scanout
/// buffer on every redraw.
fn opaque(tex: Option<sdl3::render::Texture<'_>>) -> Option<sdl3::render::Texture<'_>> {
    tex.map(|mut t| {
        t.set_blend_mode(sdl3::render::BlendMode::None);
        t
    })
}
