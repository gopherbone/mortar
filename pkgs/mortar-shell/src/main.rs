// SPDX-License-Identifier: GPL-2.0-or-later
//! mortar-shell: a controller-driven Wayland compositor for Mortar (NixOS on
//! the TrimUI Brick). See DESIGN.md.

mod apps;
#[cfg(target_os = "linux")]
mod comp;
mod config;
mod osk;
mod overlay;
mod preview;
mod router;

use mortar_ui::input::Button;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("MORTAR_SHELL_LOG").unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let r = match args.first().map(String::as_str) {
        Some("preview") => preview_cmd(&args[1..]),
        #[cfg(target_os = "linux")]
        Some("ctl") => ctl_cmd(&args[1..]),
        #[cfg(target_os = "linux")]
        Some("run") => run_cmd(&args[1..]),
        Some("-h" | "--help") | None => Err(String::new()),
        Some(other) => Err(format!("unknown command '{other}'")),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.is_empty() => {
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
        Err(e) => {
            eprintln!("mortar-shell: {e}");
            ExitCode::FAILURE
        }
    }
}

const USAGE: &str = "\
usage: mortar-shell <command> [options]

commands:
  run        the compositor
               --backend drm|headless   default drm (libseat + KMS)
               --config FILE            default /etc/mortar-shell/config.json
               --socket NAME            Wayland socket name (default: wayland-N)
               --ctl PATH               control socket ($XDG_RUNTIME_DIR/mortar-shell.sock)
               --size WxH               headless output size, default 1024x768
               --input direct           headless: also read /dev/input directly
  ctl CMD    talk to a running shell: state | press BUTTON | down/up BUTTON |
               key CODE | launch ID | focus ID | close ID | home |
               screenshot PATH | quit      (--ctl PATH before CMD)
  preview    render a shell screen to PNG without a compositor
               --screen home|home-settings|menu|menu-launch|menu-settings|osk|osd
               --out FILE [--keys a,down,...] [--theme light|dark]

environment:
  MORTAR_SHELL_LOG          log filter (tracing EnvFilter), default info
  MORTAR_DRM_DEVICE         DRM card to use
  MORTAR_SHELL_NO_SCANOUT   never scan out client buffers directly
  MORTAR_UI_FONTS           font directory (default: baked in)
";

#[cfg(target_os = "linux")]
fn run_cmd(args: &[String]) -> Result<(), String> {
    let mut backend = "drm".to_string();
    let mut config = PathBuf::from("/etc/mortar-shell/config.json");
    let mut config_given = false;
    let mut socket = None;
    let mut ctl = None;
    let mut size = (1024, 768);
    let mut direct_input = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--backend" => backend = val()?,
            "--config" => {
                config = val()?.into();
                config_given = true;
            }
            "--socket" => socket = Some(val()?),
            "--ctl" => ctl = Some(PathBuf::from(val()?)),
            "--input" => direct_input = val()? == "direct",
            "--size" => {
                let v = val()?;
                let (w, h) = v.split_once('x').ok_or("--size wants WxH")?;
                size = (w.parse().map_err(|_| "bad width")?, h.parse().map_err(|_| "bad height")?);
            }
            s => return Err(format!("unknown argument {s}")),
        }
    }
    let cfg = if config.exists() || config_given {
        config::Config::load(&config)?
    } else {
        tracing::warn!("{} missing; no launcher entries", config.display());
        config::Config::default()
    };
    comp::run(comp::state::Opts { backend, config: cfg, socket, ctl, direct_input, size })
}

#[cfg(target_os = "linux")]
fn ctl_cmd(args: &[String]) -> Result<(), String> {
    let (path, words) = match args {
        [flag, p, rest @ ..] if flag == "--ctl" => (PathBuf::from(p), rest),
        rest => (comp::ctl::default_path(), rest),
    };
    if words.is_empty() {
        return Err(String::new());
    }
    println!("{}", comp::ctl::client(&path, words)?);
    Ok(())
}

fn preview_cmd(args: &[String]) -> Result<(), String> {
    let mut screen = "home".to_string();
    let mut out: Option<PathBuf> = None;
    let mut keys = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--screen" => screen = val()?,
            "--out" => out = Some(val()?.into()),
            "--theme" => {
                let n = val()?;
                mortar_ui::theme::set(mortar_ui::theme::Theme::by_name(&n).ok_or(format!("unknown theme {n}"))?);
            }
            "--keys" => {
                for k in val()?.split(',').filter(|k| !k.is_empty()) {
                    keys.push(Button::parse(k).ok_or(format!("unknown button {k}"))?);
                }
            }
            s => return Err(format!("unknown argument {s}")),
        }
    }
    let out = out.ok_or("--out FILE is required")?;
    preview::run(&screen, &keys, &out, (1024, 768))?;
    println!("wrote {}", out.display());
    Ok(())
}
