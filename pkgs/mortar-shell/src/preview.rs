// SPDX-License-Identifier: GPL-2.0-or-later
//! `mortar-shell preview`: render the shell's screens and transition frames
//! to PNG without a compositor, with demo apps (stand-in screens). Works on
//! macOS for UI work; the integration test screenshots the real thing.
//!
//!   mortar-shell preview --screen menu --out menu.png
//!   mortar-shell preview --screen launch --at 120 --frames 12 --every 33 --out /tmp/launch.png
//!     (writes launch-000.png ... : the transition, 33 ms apart)

use crate::art::Art;
use crate::config::Category;
use crate::osk::{self, Osk};
use crate::raster::{Images, Raster};
use crate::scene::{Layer, Src, Typo};
use crate::shellui::{LauncherInfo, Prefs, Radios, RunningInfo, ShellUi, Toast, View, H, W};
use mortar_ui::gfx::{Color, Face, Fonts, TextStyle, Ui};
use mortar_ui::input::Button;
use mortar_ui::status::Sys;
use resvg::tiny_skia::Pixmap;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub fn demo_launcher() -> Vec<LauncherInfo> {
    let l = |id: &str, name: &str, d: &str, art: Art, running: bool| LauncherInfo {
        id: id.into(),
        name: name.into(),
        description: d.into(),
        art,
        running,
        category: Category::guess(art, id, name),
        single_instance: id != "terminal-new",
    };
    vec![
        l("m8", "M8", "Tracker · top USB-C", Art::M8, true),
        l("choochoo", "ChooChoo", "Tracker · groovebox", Art::Generic, false),
        l("pico8", "PICO-8", "Splore · carts", Art::Pico8, false),
        l("play", "Games", "16 systems · libretro", Art::Games, false),
        l("claude", "Claude", "Claude Code · approvals", Art::Claude, true),
        l("games", "PuzzleScript", "94 puzzles", Art::Puzzle, false),
        l("terminal", "Terminal", "foot · on-screen keys", Art::Term, false),
        l("terminal-new", "New terminal", "Another foot window", Art::Term, false),
    ]
}

pub fn demo_running() -> Vec<RunningInfo> {
    let r = |uid: u32, entry: &str, name: &str, art: Art| RunningInfo {
        uid,
        entry: Some(entry.into()),
        name: name.into(),
        art,
        has_buffer: true,
        closing: false,
        geom: [0.0, 0.0, W, H],
    };
    vec![r(2, "claude", "Claude", Art::Claude), r(1, "m8", "M8", Art::M8)]
}

struct Demo {
    pix: HashMap<Src, Pixmap>,
}

impl Images for Demo {
    fn get(&self, s: Src) -> Option<&Pixmap> {
        self.pix.get(&s)
    }
}

/// Stand-in app screens, drawn with the toolkit.
fn stand_ins(fonts_dir: &Path) -> Result<HashMap<Src, Pixmap>, String> {
    let mut m = HashMap::new();
    // M8: black, cyan headers, hex columns
    let mut ui = Ui::new(W as u32, H as u32, Fonts::load(fonts_dir)?);
    ui.clear(Color::hex(0x000000));
    let st = |c: u32| TextStyle::new(Face::Mono600, 22.0, Color::hex(c));
    ui.text(48.0, 44.0, "SONG", &st(0x4ed0d8));
    ui.text(150.0, 44.0, "PHRASE 03", &st(0x6c7480));
    for r in 0..16 {
        let row = if r == 5 { 0x4ed0d8 } else { 0x5d636c };
        ui.text(48.0, 100.0 + r as f32 * 36.0, &format!("{r:02X}"), &st(row));
        for c in 0..8 {
            let on = (r * 7 + c * 3) % 11 < 3 && c < 5;
            let txt = if on { format!("{:02X}", (r * 13 + c * 5) % 255) } else { "--".into() };
            ui.text(120.0 + c as f32 * 64.0, 100.0 + r as f32 * 36.0, &txt, &st(if on { 0xc6ccd4 } else { 0x3a3f45 }));
        }
    }
    ui.outline(mortar_ui::gfx::Rect::new(700.0, 70.0, 280.0, 120.0), 2.0, Color::hex(0x3a3f45));
    let wave: Vec<(f32, f32)> = (0..=70).map(|i| (700.0 + i as f32 * 4.0, 130.0 + ((i as f32) * 0.36).sin() * 26.0)).collect();
    ui.stroke_poly(&wave, false, 3.0, Color::hex(0x4ed0d8));
    m.insert(Src::App(1), ui.pm.clone());
    // Claude Code in a terminal-ish app
    ui.clear(Color::hex(0xe1ebf7));
    let body = |c: u32| TextStyle::new(Face::Body500, 26.0, Color::hex(c));
    ui.text(68.0, 96.0, "free -m shows 612 MB available. Want a timer that", &body(0x1d1d1d));
    ui.text(68.0, 132.0, "logs it every minute while you play?", &body(0x1d1d1d));
    ui.fill_rect(mortar_ui::gfx::Rect::new(48.0, 270.0, 928.0, 300.0), Color::hex(0xe1ebf7));
    ui.outline(mortar_ui::gfx::Rect::new(48.0, 270.0, 928.0, 300.0), 1.5, Color::hex(0x1d1d1d));
    ui.fill_rect(mortar_ui::gfx::Rect::new(48.0, 270.0, 8.0, 300.0), Color::hex(0xcba6f7));
    ui.text(80.0, 296.0, "PERMISSION · BASH", &TextStyle::new(Face::Mono600, 18.0, Color::hex(0x4c5164)));
    ui.text(80.0, 330.0, "Start a timer that logs free memory", &TextStyle::new(Face::Body600, 30.0, Color::hex(0x1d1d1d)));
    for (i, (t, c)) in [("A  Allow", 0xcef058), ("B  Deny", 0xff9f67), ("X  Always", 0xcba6f7)].iter().enumerate() {
        let r = mortar_ui::gfx::Rect::new(80.0 + i as f32 * 200.0, 480.0, 180.0, 56.0);
        ui.fill_rect(r, Color::hex(*c));
        ui.outline(r, 1.0, Color::hex(0x1d1d1d));
        ui.text(100.0 + i as f32 * 200.0, 492.0, t, &TextStyle::new(Face::Body600, 22.0, Color::hex(0x1d1d1d)));
    }
    m.insert(Src::App(2), ui.pm.clone());
    // their blurred copies (the MENU's backdrop)
    for u in [1, 2] {
        if let Some(b) = m.get(&Src::App(u)).and_then(crate::raster::blur_quarter) {
            m.insert(Src::Blur(u), b);
        }
    }
    // the on-screen keyboard strip
    let mut k = Osk::default();
    k.show();
    let h = osk::height(&ui);
    let area = mortar_ui::gfx::Rect::new(0.0, ui.height() - h, ui.width(), h);
    k.draw(&mut ui, area);
    let sr = osk::strip_rect(&ui, area);
    crate::osk_suggest::draw(&mut ui, sr, &crate::osk_suggest::Strip::default());
    let mut strip = Pixmap::new(W as u32, h as u32).ok_or("osk size")?;
    let stride = W as usize * 4;
    let y0 = (ui.height() - h) as usize;
    strip.data_mut().copy_from_slice(&ui.pm.data()[y0 * stride..(y0 + h as usize) * stride]);
    m.insert(Src::Layer(Layer::Osk), strip);
    Ok(m)
}

/// `osk-chips`: the terminal (foot with the Mortar zsh prompt, in the
/// palette's colours) above the keyboard, with suggestion chips for `git ch`.
fn terminal_with_chips(fonts_dir: &Path, pix: &mut HashMap<Src, Pixmap>, strip: &crate::osk_suggest::Strip, night: bool) -> Result<(), String> {
    mortar_ui::theme::set(if night { mortar_ui::theme::DARK } else { mortar_ui::theme::LIGHT });
    let mut ui = Ui::new(W as u32, H as u32, Fonts::load(fonts_dir)?);
    let h = osk::height(&ui);
    // the terminal: ground, ink text, the prompt's lime key cap
    let (bg, fg, soft, lav) =
        if night { (0x1d1d1d, 0xe1ebf7, 0x9ba7b5, 0xcba6f7) } else { (0xe1ebf7, 0x1d1d1d, 0x4c5164, 0x6b3fb3) };
    ui.clear(Color::hex(bg));
    let st = |c: u32| TextStyle::new(Face::Mono400, 21.0, Color::hex(c));
    let bold = |c: u32| TextStyle::new(Face::Mono600, 21.0, Color::hex(c));
    let lh = 30.0;
    let prompt = |ui: &mut Ui, y: f32, cmd: &str| {
        let mut x = 16.0;
        x += ui.text(x, y, "~/mortar ", &bold(fg));
        x += ui.text(x, y, "main ", &st(lav));
        let w = ui.measure(" › ", &bold(0x1d1d1d));
        ui.fill_rect(mortar_ui::gfx::Rect::new(x, y + 2.0, w, lh - 4.0), Color::hex(0xcef058));
        ui.text(x, y, " › ", &bold(0x1d1d1d));
        x += w + ui.measure(" ", &st(fg));
        ui.text(x, y, cmd, &st(fg)) + x
    };
    let mut y = 14.0;
    ui.text(16.0, y, "mortar · 84% · up 2h 13m", &st(soft));
    y += lh * 1.5;
    prompt(&mut ui, y, "git status --short");
    y += lh;
    ui.text(16.0, y, " M pkgs/mortar-shell/src/osk.rs", &st(if night { 0xff9f67 } else { 0xc4452a }));
    y += lh;
    ui.text(16.0, y, "?? pkgs/mortar-shell/src/osk_suggest.rs", &st(soft));
    y += lh * 1.3;
    let x = prompt(&mut ui, y, "git ch");
    // autosuggestion ghost text, then the cursor
    ui.text(x, y, "eckout -b osk-chips", &st(soft));
    ui.fill_rect(mortar_ui::gfx::Rect::new(x, y + 2.0, 12.0, lh - 4.0), Color::hex(0xcef058));
    pix.insert(Src::App(2), ui.pm.clone());
    // the keyboard with the strip
    let mut k = Osk::default();
    k.show();
    k.in_strip = strip.focus.is_some();
    let area = mortar_ui::gfx::Rect::new(0.0, ui.height() - h, ui.width(), h);
    k.draw(&mut ui, area);
    let sr = osk::strip_rect(&ui, area);
    crate::osk_suggest::draw(&mut ui, sr, strip);
    let mut layer = Pixmap::new(W as u32, h as u32).ok_or("osk size")?;
    let stride = W as usize * 4;
    let y0 = (ui.height() - h) as usize;
    layer.data_mut().copy_from_slice(&ui.pm.data()[y0 * stride..(y0 + h as usize) * stride]);
    pix.insert(Src::Layer(Layer::Osk), layer);
    Ok(())
}

pub struct Opts {
    pub screen: String,
    pub keys: Vec<Button>,
    pub out: PathBuf,
    pub at_ms: u32,
    pub frames: u32,
    pub every_ms: u32,
    pub night: bool,
}

pub fn run(o: &Opts) -> Result<(), String> {
    let dir = mortar_ui::gfx::font_dir();
    let typo = Typo::new(Fonts::load(&dir)?);
    let mut demo = Demo { pix: stand_ins(&dir)? };
    let mut sys = Sys::demo(Some("08:42".into()));
    sys.ssh_sessions = 1;
    sys.battery = Some(if s_low(&o.screen) { 12 } else { 84 });
    sys.charging = o.screen == "home-charging";
    sys.plugged = sys.charging;
    let launcher = demo_launcher();
    let running = demo_running();
    let none: Vec<RunningInfo> = Vec::new();
    // the screensaver screens: it comes after 30 s (use --at 29000)
    let screensaver = if o.screen.starts_with("saver") { 30 } else { Prefs::default().screensaver };
    let prefs = Prefs { night: o.night, screensaver, ..Prefs::default() };
    let s = o.screen.as_str();
    let mut ui = ShellUi::new(prefs, s == "boot");
    let over_app = s.starts_with("menu") || s.starts_with("cross") || s.starts_with("saver") || matches!(s, "running" | "cross-switch" | "mixer" | "bluetooth" | "setup-wifi" | "setup-bluetooth" | "settings" | "app" | "osd" | "osd-mute" | "osd-bri" | "osd-home" | "toast" | "achievement" | "flash" | "osk" | "osk-chips" | "osk-chips-focus" | "close" | "swap" | "remote" | "wifi");
    let mut current = if s == "cross-dark" { Some(1) } else if over_app { Some(2) } else { None };
    let run_ref: &[RunningInfo] = if s == "home-empty" { &none } else { &running };
    let host = "brick";
    let demo_st = { use mortar_setup::ui::Device; mortar_setup::ui::demo::DemoDevice::settings().status().clone() };
    let net = crate::shellui::NetState { wifi: demo_st.wifi.clone(), bt: demo_st.bluetooth.clone(), error: None };
    let mixer = vec![crate::shellui::MixChan { uid: 2, level: 40, muted: false }, crate::shellui::MixChan { uid: 1, level: 70, muted: false }];
    let update = crate::shellui::UpdateView { message: "Restart to update".into(), ready: true, busy: false, auto_check: true };
    let mk = |current: Option<u32>, osk_h: f32| View {
        running: run_ref,
        launcher: &launcher,
        sys: &sys,
        volume: Some(70),
        current,
        keyboard_app: true,
        osk_h,
        osk_drop: 0.0,
        host,
        timezone: "Europe/Lisbon",
        radios: Radios { wifi: Some(true), bluetooth: Some(true) },
        update: Some(&update),
        muted: false,
        net: Some(&net),
        mixer: &mixer,
    };
    let dt = 1.0 / 120.0;
    let settle = |ui: &mut ShellUi, v: &View, secs: f64| {
        let mut t = 0.0;
        while t < secs {
            ui.step(dt, v);
            t += dt;
        }
    };
    let v0 = mk(current, 0.0);
    settle(&mut ui, &v0, 0.05);
    match s {
        "boot" | "home" | "home-empty" | "home-charging" | "home-low" | "app" => {}
        "menu" | "menu-running" | "running" => {
            ui.menu_key(&v0);
        }
        "menu-apps" | "cross-apps" | "cross-dark" | "home-menu" | "menu-games" | "cross-games" | "menu-music" | "cross-music" | "cross-switch" | "saver-apps" | "saver-games" | "saver-music" | "saver-settings" | "saver-exit" => {
            ui.menu_key(&v0);
            let n = match s {
                "menu-games" | "cross-games" | "saver-games" => 2,
                "menu-music" | "cross-music" | "saver-music" => 3,
                "saver-settings" => 4,
                _ => 1,
            };
            for _ in 0..n {
                ui.button(Button::Right, false, &v0);
            }
            if s == "saver-exit" {
                // the screensaver has filled the screen; a press brings it back
                settle(&mut ui, &v0, 33.0);
                ui.button(Button::A, false, &v0);
            }
            if s == "cross-switch" {
                // settled on Apps, then Games: the patterns trade places
                settle(&mut ui, &v0, 1.0);
                ui.button(Button::Right, false, &v0);
            }
        }
        "menu-settings" | "cross-settings" | "menu-adjust" | "menu-confirm" | "settings" | "wifi" | "bluetooth" | "mixer" => {
            // MENU, R1: Settings (Brightness, Volume, Mixer, Wi-Fi,
            // Bluetooth, Sleep, Keyboard, More settings, Restart, Power off)
            ui.menu_key(&v0);
            ui.button(Button::R1, false, &v0);
            let (down, a) = match s {
                "menu-adjust" => (1, 1),
                "menu-confirm" => (8, 1),
                "settings" => (7, 1),
                "mixer" => (2, 1),
                "wifi" => (3, 1),
                "bluetooth" => (4, 1),
                _ => (0, 0),
            };
            for _ in 0..down {
                ui.button(Button::Down, false, &v0);
            }
            for _ in 0..a {
                ui.button(Button::A, false, &v0);
            }
        }
        "menu-options" => {
            ui.menu_key(&v0);
            ui.button(Button::Y, false, &v0);
        }
        "remote" | "setup-wifi" | "setup-bluetooth" => {
            use mortar_setup::ui::{demo::DemoDevice, SetupUi};
            ui.menu_key(&v0);
            let dev = Box::new(DemoDevice::settings());
            ui.open_page(match s {
                "setup-wifi" => SetupUi::wifi(dev, sys.clone()),
                "setup-bluetooth" => SetupUi::bluetooth(dev, sys.clone()),
                _ => SetupUi::remote_access(dev, sys.clone()),
            });
        }
        "launch" => {
            settle(&mut ui, &v0, 1.0);
            ui.button(Button::Right, false, &v0);
            settle(&mut ui, &v0, 0.5);
            ui.button(Button::Left, false, &v0);
            settle(&mut ui, &v0, 0.5);
            ui.button(Button::A, false, &v0);
            current = Some(1); // the M8 is running: it shows as it grows
        }
        "close" => {
            ui.menu_key(&v0);
            settle(&mut ui, &v0, 0.8);
            ui.button(Button::Up, false, &v0);
            ui.button(Button::Up, false, &v0);
            ui.button(Button::A, false, &v0);
            // the MENU fades, the app shrinks into its tile
            let mut t = 0.0;
            while t < 0.8 {
                let v = mk(current, 0.0);
                let outs = ui.step(dt, &v);
                if outs.contains(&crate::shellui::Out::Home) {
                    current = None;
                    break;
                }
                t += dt;
            }
        }
        "swap" => {
            ui.menu_key(&v0);
            settle(&mut ui, &v0, 0.8);
            // A on the previous app: it zooms out of its row
            let outs = ui.button(Button::A, false, &v0);
            if outs.iter().any(|o| matches!(o, crate::shellui::Out::Focus(_))) {
                current = Some(1);
            }
        }
        "osd" => ui.osd(crate::shellui::OsdKind::Volume, 70, false),
        "osd-mute" => ui.osd(crate::shellui::OsdKind::Volume, 0, true),
        "osd-bri" => ui.osd(crate::shellui::OsdKind::Brightness, 40, false),
        "osd-home" => {
            current = None;
            ui.osd(crate::shellui::OsdKind::Volume, 35, false)
        }
        "toast" => {
            ui.screenshot();
            settle(&mut ui, &v0, 0.4);
            let mut t = Toast::info("Screenshot saved");
            t.icon = crate::shellui::ToastIcon::Shot;
            t.sub = "~/PICTURES/SCREENSHOTS/0842.PNG".into();
            ui.show_toast(t);
        }
        "achievement" => ui.show_toast(Toast::achievement("First jump", "Games · Game Boy")),
        "flash" => ui.screenshot(),
        "osk" => {}
        "osk-chips" | "osk-chips-focus" => {
            let strip = crate::osk_suggest::demo(if s == "osk-chips-focus" { Some(1) } else { None });
            terminal_with_chips(&dir, &mut demo.pix, &strip, o.night)?;
        }
        "power" => ui.open_power_menu(),
        "timezone" | "timezone-zones" => {
            ui.menu_key(&v0);
            let dir = std::path::Path::new("/usr/share/zoneinfo");
            let ids = crate::tz::list(dir);
            let off = crate::tz::offsets(dir, &ids);
            let zones = ids.into_iter().map(|id| crate::shellui::TzZone { offset: off.get(&id).copied().unwrap_or(0), id }).collect();
            ui.open_tz(zones, "Europe/Lisbon".into());
            if s == "timezone-zones" {
                settle(&mut ui, &v0, 0.6);
                ui.button(Button::A, false, &v0);
            }
        }
        other => {
            return Err(format!(
                "unknown preview screen '{other}' (boot, home, home-empty, app, running, cross-apps, home-menu (the cross over home), menu-games, menu-music, cross-settings, menu-adjust, menu-confirm, menu-options, wifi, bluetooth, mixer, cross-games, cross-music, cross-switch (Apps to Games: use --frames), settings, remote, setup-wifi, setup-bluetooth, timezone, timezone-zones, launch, close, swap, osd, osd-mute, osd-bri, osd-home, toast, achievement, flash, osk, osk-chips, osk-chips-focus, power)"
            ))
        }
    }
    let osk_h = if s.starts_with("osk") { demo.pix[&Src::Layer(Layer::Osk)].height() as f32 } else { 0.0 };
    let v = mk(current, osk_h);
    for b in &o.keys {
        ui.button(*b, false, &v);
        settle(&mut ui, &v, 0.3);
    }
    settle(&mut ui, &v, o.at_ms as f64 / 1000.0);
    let mut raster = Raster::default();
    let mut pm = Pixmap::new(W as u32, H as u32).ok_or("canvas")?;
    // the MENU over home blurs the home screen (as the compositor does)
    let home = ui.build_home(&typo, &v, W, H);
    raster.render(&mut pm, &home, &typo, &demo);
    if let Some(b) = crate::raster::blur_quarter(&pm) {
        demo.pix.insert(Src::HomeBlur, b);
    }
    // the veil adapts to the backdrop's brightness (the compositor measures
    // its blur)
    let back = match current { Some(u) => Src::Blur(u), None => Src::HomeBlur };
    if let Some(p) = demo.pix.get(&back) {
        ui.set_backdrop(Some(crate::raster::mean_rgb(p)));
        ui.snap_veil();
    }
    let n = o.frames.max(1);
    let mut page_ui = Ui::new(W as u32, H as u32, Fonts::load(&dir)?);
    for f in 0..n {
        if let Some(p) = ui.page_mut() {
            p.draw(&mut page_ui);
            demo.pix.insert(Src::Layer(Layer::Page), page_ui.pm.clone());
        }
        let scene = ui.build(&typo, &v, W, H);
        raster.render(&mut pm, &scene, &typo, &demo);
        let path = if n == 1 {
            o.out.clone()
        } else {
            let stem = o.out.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "frame".into());
            o.out.with_file_name(format!("{stem}-{f:03}.png"))
        };
        pm.save_png(&path).map_err(|e| e.to_string())?;
        settle(&mut ui, &v, o.every_ms as f64 / 1000.0);
    }
    Ok(())
}

/// The low-battery home preview.
fn s_low(screen: &str) -> bool {
    screen == "home-low"
}
