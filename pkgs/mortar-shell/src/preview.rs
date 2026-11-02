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
        game: false,
        asleep: false,
        pic: None,
    };
    vec![r(2, "claude", "Claude", Art::Claude), r(1, "m8", "M8", Art::M8)]
}

/// A demo library with made-up games (never anything from a device), its
/// resume pictures and a slot or two, as pictures 900+.
fn demo_games(pix: &mut HashMap<Src, Pixmap>) -> crate::games::GamesView {
    use crate::games::{GameStatus, GamesView, Library, Pic, SlotInfo};
    let list = r#"{"systems":[
      {"id":"gbc","name":"Game Boy Color","tag":"GBC","order":11,"count":3,"games":[
        {"path":"/demo/Garden Hop.gbc","name":"Garden Hop","region":"Europe","resume":false},
        {"path":"/demo/Pocket King.gbc","name":"Pocket King","region":"World","resume":true,"thumb":"/demo/pk.png"},
        {"path":"/demo/Tiny Tanks.gbc","name":"Tiny Tanks","region":"USA","resume":true,"thumb":"/demo/tt.png"}]},
      {"id":"nes","name":"NES","tag":"NES","order":20,"count":2,"games":[
        {"path":"/demo/Moon Miner.nes","name":"Moon Miner","resume":false},
        {"path":"/demo/Star Courier.nes","name":"Star Courier","region":"USA","resume":true,"thumb":"/demo/sc.png"}]},
      {"id":"snes","name":"Super Nintendo","tag":"SNES","order":21,"count":0,"games":[]},
      {"id":"2048","name":"2048","tag":"2048","order":90,"noContent":true}],
     "recent":[
      {"system":"gbc","path":"/demo/Pocket King.gbc","name":"Pocket King","tag":"GBC","resume":true,"thumb":"/demo/pk.png"},
      {"system":"nes","path":"/demo/Star Courier.nes","name":"Star Courier","tag":"NES","resume":true,"thumb":"/demo/sc.png"}]}"#;
    let lib = Library::parse(list).ok();
    let mut g = GamesView { lib, library_app: Some("play".into()), ..Default::default() };
    // pictures: flat game-like screens (sky, ground, a block)
    let scene = |sky: (u8, u8, u8), ground: (u8, u8, u8), w: u32, h: u32| {
        let mut pm = Pixmap::new(w, h).unwrap();
        pm.fill(resvg::tiny_skia::Color::from_rgba8(sky.0, sky.1, sky.2, 255));
        let mut paint = resvg::tiny_skia::Paint::default();
        paint.set_color_rgba8(ground.0, ground.1, ground.2, 255);
        let r = |x: f32, y: f32, ww: f32, hh: f32| resvg::tiny_skia::Rect::from_xywh(x, y, ww, hh).unwrap();
        pm.fill_rect(r(0.0, h as f32 * 0.72, w as f32, h as f32 * 0.28), &paint, resvg::tiny_skia::Transform::identity(), None);
        paint.set_color_rgba8(250, 220, 60, 255);
        pm.fill_rect(r(w as f32 * 0.3, h as f32 * 0.5, w as f32 * 0.08, h as f32 * 0.22), &paint, resvg::tiny_skia::Transform::identity(), None);
        paint.set_color_rgba8(240, 240, 240, 255);
        pm.fill_rect(r(w as f32 * 0.6, h as f32 * 0.15, w as f32 * 0.2, h as f32 * 0.07), &paint, resvg::tiny_skia::Transform::identity(), None);
        pm
    };
    for (id, path, sky, ground, w, h) in [
        (900, "/demo/pk.png", (120, 180, 250), (70, 150, 60), 320, 288),
        (901, "/demo/tt.png", (40, 40, 70), (150, 110, 70), 320, 288),
        (902, "/demo/sc.png", (20, 20, 40), (90, 90, 120), 320, 240),
        (903, "/demo/s1.png", (250, 170, 120), (70, 150, 60), 320, 288),
    ] {
        pix.insert(Src::Pic(id), scene(sky, ground, w, h));
        g.thumbs.insert(path.into(), Pic { id, w, h });
    }
    // the running game's screen
    pix.insert(Src::App(3), scene((120, 180, 250), (70, 150, 60), 1024, 768));
    let mut st = GameStatus { can_save: true, ..Default::default() };
    st.slots[0] = Some(SlotInfo { age_s: 420, thumb: Some("/demo/s1.png".into()) });
    st.slots[1] = Some(SlotInfo { age_s: 2 * 86400, thumb: Some("/demo/pk.png".into()) });
    g.status.insert(3, st);
    g
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
    // MORTAR_PREVIEW_TIME=HH:MM: the clock (home's pattern follows the hour)
    let mut sys = Sys::demo(Some(std::env::var("MORTAR_PREVIEW_TIME").unwrap_or_else(|_| "08:42".into())));
    sys.ssh_sessions = 1;
    sys.battery = Some(if s_low(&o.screen) { 12 } else { 84 });
    sys.charging = o.screen == "home-charging";
    sys.plugged = sys.charging;
    let launcher = demo_launcher();
    let running = demo_running();
    let none: Vec<RunningInfo> = Vec::new();
    // the screensaver screens: it comes after 30 s (use --at 29000)
    let screensaver = if o.screen.starts_with("saver") { 30 } else { Prefs::default().screensaver };
    // no first-time tips over the screens (the "tip" screen shows one)
    let tips_seen = ["menu", "keyboard", "game", "controls"].map(String::from).to_vec();
    // home-sun, home-truchet, home-life, home-off: that pattern
    let home_art = match o.screen.as_str() {
        "home-truchet" => crate::homeart::Kind::Truchet,
        "home-life" => crate::homeart::Kind::Life,
        "home-off" => crate::homeart::Kind::Off,
        _ => crate::homeart::Kind::Sun,
    };
    let prefs = Prefs { night: o.night, screensaver, tips_seen, home_art, ..Prefs::default() };
    let s = o.screen.as_str();
    let mut ui = ShellUi::new(prefs, s == "boot");
    let over_app = s.starts_with("menu")
        || s.starts_with("cross")
        || s.starts_with("saver")
        || s.starts_with("page-")
        || matches!(s, "running" | "running-attention" | "running-dash" | "menu-open" | "setup-wifi" | "setup-bluetooth" | "app" | "osd" | "osd-mute" | "osd-bri" | "osd-home" | "toast" | "achievement" | "flash" | "osk" | "osk-chips" | "osk-chips-focus" | "close" | "swap" | "remote" | "tip");
    let mut current = if s == "cross-dark" { Some(1) } else if over_app { Some(2) } else { None };
    // games: a demo library (made-up titles), one game running
    let games = demo_games(&mut demo.pix);
    let mut with_game = running.clone();
    with_game.insert(0, RunningInfo { uid: 3, entry: Some("game:gbc:/demo/Pocket King.gbc".into()), name: "Pocket King · GBC".into(), art: Art::Games, game: true, ..running[0].clone() });
    with_game.push(RunningInfo { uid: 4, entry: Some("game:nes:/demo/Star Courier.nes".into()), name: "Star Courier · NES".into(), art: Art::Games, game: true, asleep: true, has_buffer: false, pic: Some(902), ..running[0].clone() });
    let game_screen = matches!(s, "game-panel" | "game-asleep");
    if game_screen {
        current = Some(3);
    }
    let run_ref: &[RunningInfo] = if matches!(s, "home-empty" | "nothing-running") { &none } else if game_screen { &with_game } else { &running };
    let host = "brick";
    let demo_st = { use mortar_setup::ui::Device; mortar_setup::ui::demo::DemoDevice::settings().status().clone() };
    let net = crate::shellui::NetState { wifi: demo_st.wifi.clone(), bt: demo_st.bluetooth.clone(), error: None };
    let mixer = vec![crate::shellui::MixChan { uid: 2, level: 40, muted: false, playing: false }, crate::shellui::MixChan { uid: 1, level: 70, muted: false, playing: true }];
    // the dashboard: the defaults; home-custom adds Wi-Fi, storage and two
    // command widgets
    let mut widgets = crate::widgets::defaults();
    if s == "home-custom" {
        let cmd = |title: &str| crate::widgets::Widget { title: Some(title.into()), command: vec!["true".into()], ..crate::widgets::Widget::new(crate::widgets::Kind::Command) };
        widgets.push(crate::widgets::Widget::new(crate::widgets::Kind::Wifi));
        widgets.push(crate::widgets::Widget::new(crate::widgets::Kind::Storage));
        widgets.push(cmd("Uptime"));
        widgets.push(cmd("Backup"));
    }
    let mut outputs: Vec<Option<crate::widgets::Output>> = vec![None; widgets.len()];
    if let Some(i) = widgets.iter().position(|w| w.title.as_deref() == Some("Uptime")) {
        outputs[i] = Some(crate::widgets::parse_output("3 days\nload 0.42"));
    }
    if let Some(i) = widgets.iter().position(|w| w.title.as_deref() == Some("Backup")) {
        outputs[i] = Some(crate::widgets::parse_output(r#"{"title":"Backup","value":"OK","accent":"lime"}"#));
    }
    let attention = if s.ends_with("attention") || matches!(s, "home" | "home-custom" | "home-tiles") {
        vec![crate::attention::Item { app: "claude".into(), text: "Approve: systemd-run --user free -m".into(), seq: 1 }]
    } else {
        Vec::new()
    };
    let about = crate::about::About::demo();
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
        games: Some(&games),
        dash: crate::shellui::Dash { attention: &attention, widgets: &widgets, outputs: &outputs, about: Some(&about) },
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
        "boot" | "home" | "home-sun" | "home-truchet" | "home-life" | "home-off" | "home-default" | "home-custom" | "home-attention" | "home-empty" | "nothing-running" | "home-charging" | "home-low" | "app" => {}
        // home, A: into the dashboard's tiles
        "home-tiles" => {
            ui.button(Button::A, false, &v0);
        }
        // home, ▼: the last app selected, the dashboard above it
        "home-down" => {
            ui.button(Button::Down, false, &v0);
        }
        "tip" => ui.show_toast(Toast { sub: "TIP · MENU, THEN A GOES BACK TO THE LAST ONE".into(), ..Toast::info("MENU switches apps") }),
        "menu" | "menu-running" | "running" | "running-attention" | "running-dash" | "menu-open" => {
            ui.menu_key(&v0);
            if s == "running-attention" {
                // Claude, the one waiting
                ui.button(Button::Up, false, &v0);
            }
            if s == "running-dash" {
                // up to the dashboard, over the app in front
                ui.button(Button::Up, false, &v0);
                ui.button(Button::Up, false, &v0);
            }
        }
        "menu-apps" | "cross-apps" | "cross-attention" | "cross-dark" | "home-menu" | "menu-games" | "cross-games" | "menu-music" | "cross-music" | "cross-switch" | "saver-apps" | "saver-games" | "saver-music" | "saver-settings" | "saver-exit" => {
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
        "menu-settings" | "cross-settings" | "menu-adjust" | "page-display" | "page-display-adjust" | "page-sound" | "page-mixer" | "page-wifi" | "page-bluetooth" | "page-updates" | "page-about" | "page-controls" | "page-power" | "page-power-confirm" => {
            // MENU, R1: Settings (Brightness, Volume, Keyboard, Display,
            // Sound, Wi-Fi, Bluetooth, Time zone, Remote access, Updates,
            // About, Power)
            ui.menu_key(&v0);
            ui.button(Button::R1, false, &v0);
            let (down, a): (usize, &[Button]) = match s {
                "menu-adjust" => (1, &[Button::A]),
                "page-display" => (3, &[Button::A]),
                "page-display-adjust" => (3, &[Button::A, Button::A]),
                "page-sound" => (4, &[Button::A]),
                "page-mixer" => (4, &[Button::A, Button::Down, Button::A]),
                "page-wifi" => (5, &[Button::A]),
                "page-bluetooth" => (6, &[Button::A]),
                "page-updates" => (9, &[Button::A]),
                "page-about" => (10, &[Button::A]),
                "page-controls" => (10, &[Button::A, Button::R1, Button::A]),
                "page-power" => (11, &[Button::A]),
                "page-power-confirm" => (11, &[Button::A, Button::Down, Button::Down, Button::Down, Button::A]),
                _ => (0, &[]),
            };
            for _ in 0..down {
                ui.button(Button::Down, false, &v0);
            }
            for b in a {
                settle(&mut ui, &v0, 0.5);
                ui.button(*b, false, &v0);
            }
        }
        "games" | "games-system" => {
            // MENU, ▶ ▶: the cross on Games (Continue selected); A on a
            // system opens its page
            ui.menu_key(&v0);
            ui.button(Button::Right, false, &v0);
            ui.button(Button::Right, false, &v0);
            if s == "games-system" {
                for _ in 0..3 {
                    ui.button(Button::Down, false, &v0);
                }
                ui.button(Button::A, false, &v0);
                settle(&mut ui, &v0, 0.6);
                ui.button(Button::Down, false, &v0);
            }
        }
        "game-panel" | "game-asleep" => {
            // MENU on Running: the game in front, Y opens its panel
            ui.menu_key(&v0);
            if s == "game-asleep" {
                for _ in 0..3 {
                    ui.button(Button::Down, false, &v0);
                }
            } else {
                ui.button(Button::Up, false, &v0);
                ui.button(Button::Y, false, &v0);
                settle(&mut ui, &v0, 0.4);
                ui.button(Button::Right, false, &v0);
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
            // home, ▼ to Claude's card, A: it zooms out of its card
            settle(&mut ui, &v0, 1.0);
            ui.button(Button::Down, false, &v0);
            settle(&mut ui, &v0, 0.6);
            if ui.button(Button::A, false, &v0).iter().any(|o| matches!(o, crate::shellui::Out::Focus(_))) {
                current = Some(2);
            }
        }
        "close" => {
            // MENU over Claude, ▲▲ to the dashboard, A: home (Claude goes
            // to the background, its frosted picture fades)
            ui.menu_key(&v0);
            settle(&mut ui, &v0, 0.8);
            ui.button(Button::Up, false, &v0);
            ui.button(Button::Up, false, &v0);
            settle(&mut ui, &v0, 0.8);
            if ui.button(Button::A, false, &v0).contains(&crate::shellui::Out::Home) {
                current = None;
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
        "page-timezone" | "page-timezone-cities" => {
            ui.menu_key(&v0);
            ui.button(Button::R1, false, &v0);
            for _ in 0..7 {
                ui.button(Button::Down, false, &v0);
            }
            let dir = std::path::Path::new("/usr/share/zoneinfo");
            let ids = crate::tz::list(dir);
            let off = crate::tz::offsets(dir, &ids);
            let zones = ids.into_iter().map(|id| crate::shellui::TzZone { offset: off.get(&id).copied().unwrap_or(0), id }).collect();
            ui.open_tz(zones, "Europe/Lisbon".into());
            if s == "page-timezone-cities" {
                settle(&mut ui, &v0, 0.6);
                ui.button(Button::A, false, &v0);
            }
        }
        other => {
            return Err(format!(
                "unknown preview screen '{other}' (boot, home (the stack on the dashboard, Claude needs you), home-tiles (A: in its tiles), home-down (the last app, the dashboard above), home-default (no one needs you), home-custom (Wi-Fi, storage, scripts), home-sun, home-truchet, home-life, home-off (its pattern; MORTAR_PREVIEW_TIME=HH:MM; --frames to see it change), home-attention, home-empty or nothing-running, home-charging, home-low, app, tip, running, running-attention, running-dash (the dashboard over the app), menu-open (MENU from the app: use --frames), cross-apps, cross-attention, home-menu (the cross over home), menu-games, menu-music, games, games-system, game-panel, game-asleep, cross-settings, menu-adjust, menu-options, cross-games, cross-music, cross-switch (Apps to Games: use --frames), saver-apps, saver-games, saver-music, saver-settings, saver-exit, page-display, page-display-adjust, page-sound, page-mixer, page-wifi, page-bluetooth, page-timezone, page-timezone-cities, page-updates, page-about, page-controls, page-power, page-power-confirm, remote, setup-wifi, setup-bluetooth, launch, close, swap, osd, osd-mute, osd-bri, osd-home, toast, achievement, flash, osk, osk-chips, osk-chips-focus, power)"
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
    // the veil adapts to the backdrop's brightness (the compositor measures
    // its blur; "close": the app sent home fades out behind the stack)
    let back = current.or((s == "close").then_some(2)).map(Src::Blur);
    if let Some(p) = back.and_then(|b| demo.pix.get(&b)) {
        ui.set_backdrop(Some(crate::raster::mean_rgb(p)));
        ui.snap_veil();
    }
    // MORTAR_SHELL_BENCH=N: time N frames of this screen's per-frame CPU
    // work (UI step, scene build, and what the GL renderer does on the CPU
    // per item: text layout, triangulation), then exit
    if let Some(frames) = std::env::var("MORTAR_SHELL_BENCH").ok().and_then(|n| n.parse::<u32>().ok()) {
        bench(&mut ui, &typo, &v, frames);
        return Ok(());
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

/// Per-frame CPU cost of the current screen (see MORTAR_SHELL_BENCH): the
/// UI step, the scene build, and what the GL renderer does on the CPU per
/// item (text layout, triangulating concave fills).
fn bench(ui: &mut ShellUi, typo: &Typo, v: &View, frames: u32) {
    use crate::scene::Item;
    use std::time::Instant;
    let (mut t_step, mut t_build, mut t_items) = (0f64, 0f64, 0f64);
    let mut n_items = 0usize;
    let mut kinds = [0usize; 7];
    // the GPU path's damage: two buffers in turn, each diffed against
    // what it showed (text boxes estimated from the pens)
    let mut bufs: [Option<Vec<crate::damage::Info>>; 2] = [None, None];
    let (mut t_dmg, mut share, mut drawn, mut passes) = (0f64, 0f64, 0usize, 0usize);
    for f in 0..frames {
        let t0 = Instant::now();
        ui.step(1.0 / 30.0, v);
        let t1 = Instant::now();
        let scene = ui.build(typo, v, W, H);
        let t2 = Instant::now();
        let mut work = 0usize;
        for it in &scene.items {
            match it {
                Item::Fill { pts, convex, .. } => {
                    kinds[0] += 1;
                    if !convex {
                        kinds[1] += 1;
                        work += crate::scene::triangulate(pts).len();
                    }
                }
                Item::Stroke { .. } => kinds[2] += 1,
                Item::Shape { .. } => kinds[3] += 1,
                Item::Text(r) => {
                    kinds[4] += 1;
                    work += typo.layout(&r.s, r.face, r.size, r.tracking).len();
                }
                Item::Image { .. } => kinds[5] += 1,
                Item::Clip(_) => kinds[6] += 1,
            }
        }
        std::hint::black_box(work);
        let t3 = Instant::now();
        let mut infos = Vec::new();
        let mut ink = |r: &crate::scene::TextRun| {
            let l = typo.layout(&r.s, r.face, r.size, r.tracking);
            let end = l.last().map_or(0.0, |g| g.1) + r.size;
            Some([r.x - 2.0, r.base - r.size * 1.2, r.x + end, r.base + r.size * 0.4])
        };
        crate::damage::infos(&scene, &mut ink, &mut infos);
        let slot = &mut bufs[f as usize % 2];
        let rects = slot.as_ref().and_then(|old| {
            let mut g = crate::damage::Grid::new(scene.w, scene.h);
            g.diff(old, &infos);
            g.rects()
        });
        match &rects {
            Some(rs) => {
                share += rs.iter().map(|r| (r[2] * r[3]) as f64).sum::<f64>() / (scene.w * scene.h) as f64;
                passes += rs.len();
                for r in rs {
                    drawn += infos.iter().filter(|i| crate::damage::touches(&i.bbox, r)).count();
                }
            }
            None => {
                share += 1.0;
                passes += 1;
                drawn += scene.items.len();
            }
        }
        *slot = Some(infos);
        t_dmg += t3.elapsed().as_secs_f64();
        n_items += scene.items.len();
        t_step += (t1 - t0).as_secs_f64();
        t_build += (t2 - t1).as_secs_f64();
        t_items += (t3 - t2).as_secs_f64();
    }
    let f = frames.max(1) as f64;
    let per = |k: usize| k / frames.max(1) as usize;
    println!(
        "step {:.3} ms, build {:.3} ms, per-item {:.3} ms; {} items (fills {}, concave {}, strokes {}, shapes {}, texts {}, images {}, clips {})",
        t_step / f * 1e3,
        t_build / f * 1e3,
        t_items / f * 1e3,
        per(n_items),
        per(kinds[0]),
        per(kinds[1]),
        per(kinds[2]),
        per(kinds[3]),
        per(kinds[4]),
        per(kinds[5]),
        per(kinds[6]),
    );
    println!(
        "  damage {:.3} ms: {:.1}% of the screen redrawn, {} items drawn in {:.1} passes a frame",
        t_dmg / f * 1e3,
        share / f * 100.0,
        per(drawn),
        passes as f64 / f
    );
}
