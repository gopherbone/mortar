// SPDX-License-Identifier: GPL-2.0-or-later
//! mortar-ui: the on-device UI for Mortar (NixOS on the TrimUI Brick).

// Parts of the toolkit exist for screens 1-8 (setup, Wi-Fi, keyboard,
// settings) that are not built yet, and evdev mapping is Linux-only.
#![allow(dead_code)]

mod app;
mod config;
mod greetd;
mod greeter;
mod quickmenu;

// The shared toolkit lives in the library (src/lib.rs); these imports keep
// `crate::gfx` & co. working for the binary's own modules.
use mortar_ui::{backend, gfx, icons, input, status, toolkit};

use app::{Ctx, Runner, Screen};
use config::Config;
use input::{Button, Event};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
usage: mortar-ui <command> [options]

commands:
  greeter          login screen; run by greetd (talks to $GREETD_SOCK)
  quick-menu       the MENU-button menu, for use inside a session
  screenshot       render one screen offscreen and write a PNG

options:
  --config FILE    greeter config (JSON). Default /etc/mortar-greeter/config.json,
                   or the built-in demo config with --dev / screenshot
  --state-dir DIR  override the config's state directory
  --dev            develop on a desktop: window backend, fake greetd,
                   demo status values, actions only announced
  --backend NAME   auto | kms | window | headless
  --session ID     (quick-menu) the session that is running
  --screen NAME    (screenshot) greeter | quick-menu | session-menu | pin
  --keys LIST      (screenshot) buttons to press first, e.g. r1,down,down
  --out FILE       (screenshot) PNG to write
  --size WxH       (screenshot) canvas size, default 1024x768
  --time HH:MM     (screenshot) clock text, default 21:40
  --ssh N          (screenshot) pretend N SSH sessions are logged in
  --theme NAME     light (default) | dark; also $MORTAR_UI_THEME

environment:
  MORTAR_UI_FONTS     font directory (default: baked in at build time)
  MORTAR_DRM_DEVICE   DRM card for the kms backend
";

#[derive(Default)]
struct Args {
    cmd: String,
    config: Option<PathBuf>,
    state_dir: Option<PathBuf>,
    dev: bool,
    backend: Option<String>,
    session: Option<String>,
    screen: String,
    keys: Vec<Button>,
    out: Option<PathBuf>,
    size: (u32, u32),
    time: String,
    ssh: u32,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args { size: (1024, 768), time: "21:40".into(), screen: "greeter".into(), ..Default::default() };
    let mut it = std::env::args().skip(1);
    let val = |it: &mut dyn Iterator<Item = String>, name: &str| it.next().ok_or(format!("{name} needs a value"));
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => return Err(String::new()),
            "--config" => a.config = Some(val(&mut it, &arg)?.into()),
            "--state-dir" => a.state_dir = Some(val(&mut it, &arg)?.into()),
            "--dev" => a.dev = true,
            "--backend" => a.backend = Some(val(&mut it, &arg)?),
            "--session" => a.session = Some(val(&mut it, &arg)?),
            "--screen" => a.screen = val(&mut it, &arg)?,
            "--out" => a.out = Some(val(&mut it, &arg)?.into()),
            "--time" => a.time = val(&mut it, &arg)?,
            "--theme" => {
                let n = val(&mut it, &arg)?;
                let t = mortar_ui::theme::Theme::by_name(&n).ok_or(format!("unknown theme '{n}'"))?;
                mortar_ui::theme::set(t);
            }
            "--ssh" => a.ssh = val(&mut it, &arg)?.parse().map_err(|_| "--ssh wants a number")?,
            "--keys" => {
                for k in val(&mut it, &arg)?.split(',').filter(|k| !k.is_empty()) {
                    a.keys.push(Button::parse(k).ok_or(format!("unknown button '{k}'"))?);
                }
            }
            "--size" => {
                let v = val(&mut it, &arg)?;
                let (w, h) = v.split_once('x').ok_or("--size wants WxH")?;
                a.size = (w.parse().map_err(|_| "bad width")?, h.parse().map_err(|_| "bad height")?);
            }
            s if a.cmd.is_empty() && !s.starts_with('-') => a.cmd = s.to_string(),
            s => return Err(format!("unknown argument '{s}'")),
        }
    }
    if a.cmd.is_empty() {
        return Err(String::new());
    }
    Ok(a)
}

fn load_config(a: &Args, demo_ok: bool) -> Result<Config, String> {
    let mut cfg = match &a.config {
        Some(p) => Config::load(p)?,
        None if demo_ok => Config::demo(),
        None => Config::load(std::path::Path::new("/etc/mortar-greeter/config.json"))?,
    };
    if let Some(d) = &a.state_dir {
        cfg.state_dir = d.clone();
    }
    Ok(cfg)
}

fn run() -> Result<i32, String> {
    let a = parse_args()?;
    let fonts = gfx::Fonts::load(&gfx::font_dir())?;
    match a.cmd.as_str() {
        "screenshot" => {
            let cfg = load_config(&a, true)?;
            let out = a.out.clone().ok_or("screenshot needs --out FILE")?;
            let fake = Box::new(greetd::Fake::new(&cfg));
            let mut ctx = Ctx::new(cfg, fake, Some(Some(a.time.clone())), true);
            ctx.sys.ssh_sessions = a.ssh;
            let root: Box<dyn Screen> = match a.screen.as_str() {
                "greeter" | "quick-menu" | "pin" => Box::new(greeter::Greeter::new(&ctx)),
                "session-menu" => {
                    let id = a.session.clone().or_else(|| ctx.cfg.sessions.first().map(|s| s.id.clone()));
                    Box::new(quickmenu::QuickMenu::for_session(&id.unwrap_or_default()))
                }
                s => return Err(format!("unknown screen '{s}'")),
            };
            let mut runner = Runner::new(root, &mut ctx);
            if a.screen == "quick-menu" {
                runner.event(&Event::Press { button: Button::Menu, repeat: false }, &mut ctx);
            }
            if a.screen == "pin" {
                let user = ctx.cfg.profiles[0].user.clone();
                let sid = ctx.cfg.sessions.first().map(|s| s.id.clone()).unwrap_or_default();
                let pad = greeter::PinPad::new(&user, &sid, "PIN:", true);
                runner = Runner::new(Box::new(pad), &mut ctx);
            }
            for k in &a.keys {
                runner.event(&Event::Press { button: *k, repeat: false }, &mut ctx);
            }
            let mut ui = gfx::Ui::new(a.size.0, a.size.1, fonts);
            runner.draw(&mut ui, &ctx);
            ui.save_png(&out)?;
            println!("wrote {}", out.display());
            Ok(0)
        }
        "greeter" | "quick-menu" => {
            let cfg = load_config(&a, a.dev)?;
            let gd: Box<dyn greetd::Greetd> = if a.dev || a.cmd == "quick-menu" {
                Box::new(greetd::Fake::new(&cfg))
            } else {
                Box::new(greetd::Real::connect()?)
            };
            let demo = if a.dev { Some(None) } else { None };
            let mut ctx = Ctx::new(cfg, gd, demo, a.dev);
            let mut be = backend::open(a.backend.as_deref().unwrap_or("auto"), a.size.0, a.size.1)?;
            let (w, h) = be.size();
            let mut ui = gfx::Ui::new(w, h, fonts);
            let root: Box<dyn Screen> = if a.cmd == "greeter" {
                Box::new(greeter::Greeter::new(&ctx))
            } else {
                let id = a.session.clone().or_else(|| std::env::var("MORTAR_SESSION").ok()).unwrap_or_default();
                Box::new(quickmenu::QuickMenu::for_session(&id))
            };
            let mut runner = Runner::new(root, &mut ctx);
            Ok(runner.run(be.as_mut(), &mut ui, &mut ctx))
        }
        other => Err(format!("unknown command '{other}'")),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(c) => ExitCode::from(c as u8),
        Err(e) if e.is_empty() => {
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
        Err(e) => {
            eprintln!("mortar-ui: {e}");
            ExitCode::FAILURE
        }
    }
}
