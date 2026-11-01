// SPDX-License-Identifier: GPL-2.0-or-later
//! mortar-setup: first-boot setup and remote access for Mortar.

use mortar_setup::control::{Client, Request, Response, DEFAULT_SOCKET};
use mortar_setup::daemon::{Config, Daemon};
use mortar_setup::state::Paths;
use mortar_setup::ui::demo::DemoDevice;
use mortar_setup::ui::{Device, Mode, SetupUi, SocketDevice};
use mortar_setup::{apply, sys};
use mortar_ui::backend;
use mortar_ui::gfx::{self, Ui};
use mortar_ui::input::{Button, Event, Repeater};
use mortar_ui::status::Sys;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

const USAGE: &str = "\
usage: mortar-setup <command> [options]

commands:
  apply            apply <boot>/mortar.toml (boot service)
  daemon           pairing service (usb0), control socket, Wi-Fi
  ui               first-boot setup screens (exits when setup is done)
  status           print the daemon's state as JSON
  finish           mark first-boot setup as done (skip the screens)
  new-code         show a fresh pairing code (as if Y was pressed)
  screenshot       render one screen with demo data to a PNG
  check-password   PAM helper (pam_exec expose_authtok): is the password
                   the pairing code? Uses PAM_USER, PAM_RHOST.
  wait-ui          wait until the first-boot screens have finished

options:
  --root DIR       (apply, daemon) treat DIR as / (tests, dev)
  --socket PATH    control socket (default /run/mortar-setup/control.sock)
  --iface NAME     (daemon) pairing interface, repeatable (default usb0)
  --allow CIDR     (daemon) peers allowed to pair (default 10.55.0.0/24)
  --listen ADDR    (daemon) listen here instead of on interfaces (tests)
  --port N         (daemon) pairing port (default 7330)
  --socket-group G (daemon) group for the control socket
  --sshd-reload CMD (daemon) command that reloads sshd (whitespace-split)
  --no-iwd, --no-ssh-password, --no-system   (daemon)
  --screen NAME    (ui, screenshot) welcome setup-file usb wifi keyboard
                   joining ready remote keys; (ui) also remote, wifi
  --demo NAME      (screenshot) first-boot | setup-file | ready | settings
  --keys LIST      (screenshot) buttons to press first, e.g. down,a
  --out FILE       (screenshot) PNG to write
  --backend NAME   (ui) auto | kms | window | headless
  --dev            (ui) demo device in a window
  --theme NAME     light | dark
";

#[derive(Default)]
struct Args {
    cmd: String,
    root: Option<PathBuf>,
    socket: Option<PathBuf>,
    ifaces: Vec<String>,
    allow: Vec<String>,
    listen: Option<String>,
    port: Option<u16>,
    group: Option<String>,
    sshd_reload: Option<String>,
    no_iwd: bool,
    no_ssh_password: bool,
    no_system: bool,
    screen: Option<String>,
    demo: Option<String>,
    keys: Vec<Button>,
    out: Option<PathBuf>,
    backend: Option<String>,
    dev: bool,
}

fn parse() -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    let val = |it: &mut dyn Iterator<Item = String>, n: &str| it.next().ok_or(format!("{n} needs a value"));
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => return Err(String::new()),
            "--root" => a.root = Some(val(&mut it, &arg)?.into()),
            "--socket" => a.socket = Some(val(&mut it, &arg)?.into()),
            "--iface" => a.ifaces.push(val(&mut it, &arg)?),
            "--allow" => a.allow.push(val(&mut it, &arg)?),
            "--listen" => a.listen = Some(val(&mut it, &arg)?),
            "--port" => a.port = Some(val(&mut it, &arg)?.parse().map_err(|_| "--port wants a number")?),
            "--socket-group" => a.group = Some(val(&mut it, &arg)?),
            "--sshd-reload" => a.sshd_reload = Some(val(&mut it, &arg)?),
            "--no-iwd" => a.no_iwd = true,
            "--no-ssh-password" => a.no_ssh_password = true,
            "--no-system" => a.no_system = true,
            "--screen" => a.screen = Some(val(&mut it, &arg)?),
            "--demo" => a.demo = Some(val(&mut it, &arg)?),
            "--out" => a.out = Some(val(&mut it, &arg)?.into()),
            "--backend" => a.backend = Some(val(&mut it, &arg)?),
            "--dev" => a.dev = true,
            "--theme" => {
                let n = val(&mut it, &arg)?;
                let t = mortar_ui::theme::Theme::by_name(&n).ok_or(format!("unknown theme '{n}'"))?;
                mortar_ui::theme::set(t);
            }
            "--keys" => {
                for k in val(&mut it, &arg)?.split(',').filter(|k| !k.is_empty()) {
                    a.keys.push(Button::parse(k).ok_or(format!("unknown button '{k}'"))?);
                }
            }
            s if a.cmd.is_empty() && !s.starts_with('-') => a.cmd = s.into(),
            s => return Err(format!("unknown argument '{s}'")),
        }
    }
    if a.cmd.is_empty() {
        return Err(String::new());
    }
    Ok(a)
}

fn paths(a: &Args) -> Paths {
    match &a.root {
        Some(r) => Paths::under(r),
        None => Paths::system(),
    }
}

fn socket(a: &Args) -> PathBuf {
    a.socket.clone().unwrap_or_else(|| match &a.root {
        Some(_) => paths(a).socket(),
        None => PathBuf::from(DEFAULT_SOCKET),
    })
}

fn demo(name: &str) -> Result<DemoDevice, String> {
    Ok(match name {
        "first-boot" => DemoDevice::first_boot(),
        "setup-file" => DemoDevice::setup_file(),
        "ready" => DemoDevice::ready(),
        "settings" => DemoDevice::settings(),
        n => return Err(format!("unknown demo '{n}'")),
    })
}

/// The demo state that fits a screen best.
fn demo_for(screen: &str) -> &'static str {
    match screen {
        "setup-file" => "setup-file",
        "ready" => "ready",
        "remote" | "keys" => "settings",
        _ => "first-boot",
    }
}

fn screenshot(a: &Args) -> Result<(), String> {
    let screen = a.screen.clone().unwrap_or_else(|| "welcome".into());
    let out = a.out.clone().ok_or("screenshot needs --out FILE")?;
    let dev = demo(a.demo.as_deref().unwrap_or(demo_for(&screen)))?;
    let mode = if matches!(screen.as_str(), "remote" | "keys") { Mode::Settings } else { Mode::FirstBoot };
    let fonts = gfx::Fonts::load(&gfx::font_dir())?;
    let mut ui = Ui::new(1024, 768, fonts);
    let mut sys = Sys::demo(Some("20:15".into()));
    sys.battery = Some(82);
    let mut s = SetupUi::named(&screen, Box::new(dev), sys, mode).ok_or(format!("unknown screen '{screen}'"))?;
    s.freeze();
    for k in &a.keys {
        s.button(*k);
    }
    s.draw(&mut ui);
    ui.save_png(&out)?;
    println!("wrote {}", out.display());
    Ok(())
}

/// Exists while the first-boot screens own the display: created by the
/// unit's ExecStartPre, removed by its ExecStopPost (also after a crash).
const UI_RUNNING: &str = "/run/mortar-setup/ui.running";

fn run_ui(a: &Args) -> Result<i32, String> {
    let fonts = gfx::Fonts::load(&gfx::font_dir())?;
    let dev: Box<dyn Device> = if a.dev {
        Box::new(demo(a.demo.as_deref().unwrap_or("first-boot"))?)
    } else {
        // the daemon may still be starting
        let sock = socket(a);
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match Client::new(&sock).status() {
                Ok(st) if st.setup_done && a.screen.is_none() => {
                    eprintln!("setup is already done");
                    return Ok(0);
                }
                Ok(_) => break,
                Err(e) if Instant::now() > deadline => return Err(e),
                Err(_) => std::thread::sleep(Duration::from_millis(300)),
            }
        }
        Box::new(SocketDevice::new(&sock))
    };
    let sys = if a.dev { Sys::demo(None) } else { Sys::read() };
    let mut setup = match a.screen.as_deref() {
        None => SetupUi::first_boot(dev, sys),
        Some("remote") => SetupUi::remote_access(dev, sys),
        Some("wifi") => SetupUi::wifi(dev, sys),
        Some(n) => SetupUi::named(n, dev, sys, Mode::FirstBoot).ok_or(format!("unknown screen '{n}'"))?,
    };
    let mut be = backend::open(a.backend.as_deref().unwrap_or("auto"), 1024, 768)?;
    let (w, h) = be.size();
    let mut ui = Ui::new(w, h, fonts);
    let mut rep = Repeater::default();
    let mut dirty = true;
    let mut next_tick = Instant::now() + setup.next_tick();
    let mut next_sys = Instant::now() + Duration::from_secs(2);
    let mut evs = Vec::new();
    while !setup.exited() {
        if dirty {
            setup.draw(&mut ui);
            if let Err(e) = be.present(&ui) {
                eprintln!("present: {e}");
            }
            dirty = false;
        }
        let now = Instant::now();
        let mut timeout = next_tick.min(next_sys).saturating_duration_since(now);
        if let Some(t) = rep.timeout() {
            timeout = timeout.min(t);
        }
        evs.clear();
        be.wait(Some(timeout), &mut evs);
        rep.poll(&mut evs);
        for ev in &evs {
            match ev {
                Event::Press { button, repeat } => {
                    if !repeat {
                        rep.press(*button);
                    }
                    // repeats only for moving around and deleting
                    if *repeat && !matches!(button, Button::Up | Button::Down | Button::Left | Button::Right | Button::B) {
                        continue;
                    }
                    setup.button(*button);
                    dirty = true;
                }
                Event::Release(b) => rep.release(*b),
                Event::Quit => {
                    setup.button(Button::B);
                    if a.dev {
                        return Ok(0);
                    }
                }
                Event::Char(_) => {}
            }
            if setup.exited() {
                break;
            }
        }
        if Instant::now() >= next_tick {
            dirty |= setup.tick();
            next_tick = Instant::now() + setup.next_tick();
        }
        if Instant::now() >= next_sys {
            if !a.dev {
                let t = setup.cx.sys.brightness;
                setup.cx.sys = Sys::read();
                setup.cx.sys.brightness = t;
            }
            dirty = true;
            next_sys = Instant::now() + Duration::from_secs(2);
        }
    }
    be.release();
    Ok(0)
}

fn wait_ui() {
    // the first-boot screens own the display until they exit
    while Path::new(UI_RUNNING).exists() {
        std::thread::sleep(Duration::from_millis(300));
    }
}

fn check_password(a: &Args) -> i32 {
    let user = std::env::var("PAM_USER").unwrap_or_default();
    let rhost = std::env::var("PAM_RHOST").unwrap_or_default();
    let mut buf = Vec::new();
    let _ = std::io::stdin().take(256).read_to_end(&mut buf);
    let pw = String::from_utf8_lossy(&buf);
    let pw = pw.trim_end_matches(['\0', '\n', '\r']).to_string();
    let req = Request::CheckPassword { user, rhost, password: pw };
    match Client::new(socket(a)).call(&req) {
        Ok(Response::Allowed { allowed: true }) => 0,
        _ => 1,
    }
}

fn run() -> Result<i32, String> {
    let a = parse()?;
    match a.cmd.as_str() {
        "apply" => {
            let s = apply::apply(&paths(&a), &apply::Options { system: a.root.is_none() && !a.no_system });
            for w in &s.warnings {
                eprintln!("mortar.toml: {w}");
            }
            println!(
                "setup file: {}",
                if s.has_content { format!("applied ({} keys, Wi-Fi {})", s.keys.len(), s.wifi_ssid.as_deref().unwrap_or("not set")) } else { "nothing to apply".into() }
            );
            Ok(0)
        }
        "daemon" => {
            let mut allow = Vec::new();
            for c in if a.allow.is_empty() { vec!["10.55.0.0/24".to_string()] } else { a.allow.clone() } {
                allow.push(sys::parse_cidr(&c).ok_or(format!("bad --allow '{c}'"))?);
            }
            let cfg = Config {
                paths: paths(&a),
                port: a.port.unwrap_or(mortar_pair::PORT),
                interfaces: if a.ifaces.is_empty() { vec!["usb0".into()] } else { a.ifaces.clone() },
                listen: a.listen.as_deref().map(|l| l.parse().map_err(|_| format!("bad --listen '{l}'"))).transpose()?,
                allow,
                usb_iface: "usb0".into(),
                usb_address: mortar_pair::USB_ADDR.into(),
                socket_group: a.group.clone(),
                sshd_reload: a.sshd_reload.as_deref().map(|s| s.split_whitespace().map(String::from).collect()).unwrap_or_default(),
                ssh_password: !a.no_ssh_password,
                system: a.root.is_none() && !a.no_system,
                iwd: !a.no_iwd,
            };
            let d = Daemon::new(cfg);
            d.run()?;
            Ok(0)
        }
        "status" => {
            let s = Client::new(socket(&a)).status()?;
            println!("{}", serde_json::to_string_pretty(&s).unwrap_or_default());
            Ok(0)
        }
        "finish" => Client::new(socket(&a)).act(Request::Finish).map(|_| 0),
        "new-code" => {
            let c = Client::new(socket(&a));
            c.act(Request::NewCode)?;
            println!("{}", c.status()?.pairing.map(|p| p.code).unwrap_or_default());
            Ok(0)
        }
        "ui" => run_ui(&a),
        "screenshot" => screenshot(&a).map(|_| 0),
        "check-password" => Ok(check_password(&a)),
        "wait-ui" => {
            wait_ui();
            Ok(0)
        }
        c => Err(format!("unknown command '{c}'")),
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
            eprintln!("mortar-setup: {e}");
            ExitCode::FAILURE
        }
    }
}
