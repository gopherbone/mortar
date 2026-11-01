// SPDX-License-Identifier: GPL-2.0-or-later
//! `mortar-shell preview`: render the shell's own screens (home, MENU
//! overlay tabs, on-screen keyboard) to PNG without a compositor, with demo
//! data. Works on macOS for UI work; the integration test screenshots the
//! real thing.

use crate::osk::{self, Osk};
use crate::overlay::{draw_osd, Action, LauncherInfo, Overlay, RunningInfo, Tab, View};
use mortar_ui::gfx::{Fonts, Rect, Ui};
use mortar_ui::icons::Icon;
use mortar_ui::input::Button;
use mortar_ui::status::Sys;
use std::path::Path;

pub fn demo_launcher() -> Vec<LauncherInfo> {
    let l = |id: &str, name: &str, d: &str, icon: Icon, running: bool| LauncherInfo {
        id: id.into(),
        name: name.into(),
        description: d.into(),
        icon,
        running,
    };
    vec![
        l("terminal", "Terminal", "foot, Claude Code", Icon::Term, true),
        l("m8", "M8", "Tracker, top USB-C", Icon::Usb, true),
        l("games", "PuzzleScript", "Puzzle games", Icon::Pad, false),
        l("files", "Files", "Card & home", Icon::Folder, false),
        l("settings", "Settings", "Wi-Fi, SSH…", Icon::Gear, false),
    ]
}

pub fn demo_running() -> Vec<RunningInfo> {
    vec![
        RunningInfo { uid: 2, name: "Terminal".into(), icon: Icon::Term, visible: true, windows: 1, closing: false },
        RunningInfo { uid: 1, name: "M8".into(), icon: Icon::Usb, visible: false, windows: 1, closing: false },
    ]
}

pub fn run(screen: &str, keys: &[Button], out: &Path, size: (u32, u32)) -> Result<(), String> {
    let fonts = Fonts::load(&mortar_ui::gfx::font_dir())?;
    let mut ui = Ui::new(size.0, size.1, fonts);
    let mut sys = Sys::demo(Some("21:40".into()));
    sys.ssh_sessions = 1;
    let launcher = demo_launcher();
    let running = demo_running();
    let none: Vec<RunningInfo> = Vec::new();
    let home = screen == "home" || screen == "home-settings";
    let run_ref: &[RunningInfo] = if home { &none } else { &running };
    let v = View {
        running: run_ref,
        launcher: &launcher,
        sys: &sys,
        volume: Some(62),
        current: if home { None } else { Some(&running[0]) },
        keyboard_app: true,
    };
    let mut o = Overlay::default();
    match screen {
        "home" => {}
        "home-settings" => o.tab = Tab::Settings,
        "menu" => o.open(&v),
        "menu-launch" => {
            o.open(&v);
            o.tab = Tab::Launch;
        }
        "menu-settings" => {
            o.open(&v);
            o.tab = Tab::Settings;
        }
        "osk" | "osd" => {}
        // Settings > Remote access / Wi-Fi (mortar-setup pages, demo data)
        "remote" | "wifi" => {
            use mortar_setup::ui::{demo::DemoDevice, SetupUi};
            o.open(&v);
            o.tab = Tab::Settings;
            let dev = Box::new(DemoDevice::settings());
            o.open_page(if screen == "wifi" { SetupUi::wifi(dev, sys.clone()) } else { SetupUi::remote_access(dev, sys.clone()) });
        }
        s => return Err(format!("unknown preview screen '{s}' (home, home-settings, menu, menu-launch, menu-settings, remote, wifi, osk, osd)")),
    }
    if screen == "osk" || screen == "osd" {
        // a stand-in "app" (terminal-ish night block) with the keyboard
        let t = mortar_ui::theme::get();
        ui.clear(t.night);
        let st = mortar_ui::toolkit::st_mono(&ui, 2.2, mortar_ui::gfx::Color::hex(0xe1ebf7));
        ui.text(ui.cq(2.0), ui.cq(2.0), "~ $ claude", &st);
        if screen == "osk" {
            let mut k = Osk::default();
            k.show();
            for b in keys {
                k.button(*b);
            }
            let h = osk::height(&ui);
            let area = Rect::new(0.0, ui.height() - h, ui.width(), h);
            k.draw(&mut ui, area);
        } else {
            draw_osd(&mut ui, "Volume 62%");
        }
    } else {
        for b in keys {
            if o.button(*b, &v) != Action::None {
                break;
            }
        }
        o.draw(&mut ui, &v);
    }
    ui.save_png(out)
}
