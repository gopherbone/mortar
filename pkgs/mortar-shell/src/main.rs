// SPDX-License-Identifier: GPL-2.0-or-later
//! mortar-shell: a controller-driven Wayland compositor for Mortar (NixOS on
//! the TrimUI Brick). See DESIGN.md.

mod about;
mod apps;
mod art;
mod attention;
#[cfg(target_os = "linux")]
mod comp;
mod config;
mod damage;
mod games;
mod haptics;
mod patterns;
mod mixer;
mod motion;
mod osk;
mod osk_suggest;
mod preview;
mod radio;
mod raster;
mod router;
mod scene;
mod sfx;
mod shellui;
mod tz;
mod widgets;

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
        Some("sfx") => sfx_cmd(&args[1..]),
        // debugging: does a running process follow /etc/localtime changes?
        Some("clock-watch") => {
            for _ in 0..args.get(1).and_then(|n| n.parse().ok()).unwrap_or(5u32) {
                println!("{}", mortar_ui::status::clock());
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            Ok(())
        }
        Some("tz-offsets") => tz::helper_main(args.get(1).map(String::as_str).unwrap_or("/etc/zoneinfo")),
        #[cfg(target_os = "linux")]
        Some("ctl") => ctl_cmd(&args[1..]),
        #[cfg(target_os = "linux")]
        Some("run") => run_cmd(&args[1..]),
        #[cfg(target_os = "linux")]
        Some("toast") => toast_cmd(&args[1..]),
        #[cfg(target_os = "linux")]
        Some("attention") => attention_cmd(&args[1..]),
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
               screenshot PATH | shot | toast JSON | boot | time manual|real|+MS |
               quit      (--ctl PATH before CMD)
  toast      show a toast in the running shell (for apps, e.g. achievements):
               toast [--achievement] [--sound NAME|none] [--secs S] TITLE [SUBTITLE]
  attention  ask for the person's attention (Needs you: Home, Running, a toast):
               attention set [--app ID] TEXT | attention clear [--app ID]
               (ID defaults to $MORTAR_APP, set for every app the shell starts)
  sfx NAME   play a UI sound [--wav FILE: write it instead]
  preview    render shell screens / transition frames to PNG without a compositor
               --screen home|home-attention|running-attention|cross-settings|page-display|
                        page-sound|page-mixer|page-wifi|page-timezone|page-about|page-controls|
                        page-power|... (the full list: an unknown name prints it)
               --out FILE [--keys a,down,...] [--theme light|dark]
               [--at MS] [--frames N --every MS]   (FILE-000.png ...)

environment:
  MORTAR_SHELL_LOG          log filter (tracing EnvFilter), default info
  MORTAR_DRM_DEVICE         DRM card to use
  MORTAR_SHELL_NO_SCANOUT   never scan out client buffers directly
  MORTAR_SHELL_ALLOCATOR    dumb (default; falls back to gbm) | gbm
  MORTAR_SHELL_RENDERER     gpu | cpu: override the config's ui.renderer
  MORTAR_SHELL_GL_MSAA      0 | 4: override ui.msaa
  MORTAR_SHELL_RUMBLE       rumble device (default: the gpio-vibrator event node)
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
        rest => (comp::ctl::client_path(), rest),
    };
    if words.is_empty() {
        return Err(String::new());
    }
    println!("{}", comp::ctl::client(&path, words)?);
    Ok(())
}

/// `mortar-shell toast [--achievement] [--sound S] [--haptic H] [--secs N] TITLE [SUB]`
#[cfg(target_os = "linux")]
fn toast_cmd(args: &[String]) -> Result<(), String> {
    let mut kind = "info";
    let mut sound: Option<String> = None;
    let mut secs: Option<f32> = None;
    let mut haptic: Option<String> = None;
    let mut words = Vec::new();
    let mut ctl = comp::ctl::client_path();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--achievement" => kind = "achievement",
            "--sound" => sound = Some(it.next().ok_or("--sound needs a value")?.clone()),
            "--haptic" => haptic = Some(it.next().ok_or("--haptic needs a value")?.clone()),
            "--secs" => secs = Some(it.next().ok_or("--secs needs a value")?.parse().map_err(|_| "--secs: a number")?),
            "--ctl" => ctl = PathBuf::from(it.next().ok_or("--ctl needs a path")?),
            w => words.push(w.to_string()),
        }
    }
    let title = words.first().ok_or("toast needs a title")?;
    let mut j = serde_json::json!({ "title": title, "sub": words.get(1).cloned().unwrap_or_default(), "kind": kind });
    if let Some(s) = sound {
        j["sound"] = s.into();
    }
    if let Some(h) = haptic {
        j["haptic"] = h.into();
    }
    if let Some(s) = secs {
        j["secs"] = s.into();
    }
    comp::ctl::client(&ctl, &["toast".to_string(), j.to_string()])?;
    Ok(())
}

/// `mortar-shell attention set|clear [--app ID] [TEXT]`
#[cfg(target_os = "linux")]
fn attention_cmd(args: &[String]) -> Result<(), String> {
    let mut app = std::env::var("MORTAR_APP").ok();
    let mut ctl = comp::ctl::client_path();
    let mut words = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--app" => app = Some(it.next().ok_or("--app needs an id")?.clone()),
            "--ctl" => ctl = PathBuf::from(it.next().ok_or("--ctl needs a path")?),
            w => words.push(w.to_string()),
        }
    }
    let op = match words.first().map(String::as_str) {
        Some(o @ ("set" | "clear")) => o.to_string(),
        _ => return Err("attention set [--app ID] TEXT | attention clear [--app ID]".into()),
    };
    let app = app.filter(|a| !a.is_empty()).ok_or("attention: --app ID (or run it from an app: $MORTAR_APP)")?;
    let j = serde_json::json!({ "op": op, "app": app, "text": words[1..].join(" ") });
    comp::ctl::client(&ctl, &["attention".to_string(), j.to_string()])?;
    Ok(())
}

fn preview_cmd(args: &[String]) -> Result<(), String> {
    let mut o = preview::Opts {
        screen: "home".into(),
        keys: Vec::new(),
        out: PathBuf::new(),
        at_ms: 1500,
        frames: 1,
        every_ms: 33,
        night: false,
    };
    let mut out_given = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().ok_or(format!("{a} needs a value"));
        let num = |v: String| v.parse::<u32>().map_err(|_| format!("{v}: not a number"));
        match a.as_str() {
            "--screen" => o.screen = val()?,
            "--out" => {
                o.out = val()?.into();
                out_given = true;
            }
            "--theme" => {
                let n = val()?;
                let t = mortar_ui::theme::Theme::by_name(&n).ok_or(format!("unknown theme {n}"))?;
                o.night = t.name == "dark";
                mortar_ui::theme::set(t);
            }
            "--at" => o.at_ms = num(val()?)?,
            "--frames" => o.frames = num(val()?)?,
            "--every" => o.every_ms = num(val()?)?,
            "--keys" => {
                for k in val()?.split(',').filter(|k| !k.is_empty()) {
                    o.keys.push(Button::parse(k).ok_or(format!("unknown button {k}"))?);
                }
            }
            s => return Err(format!("unknown argument {s}")),
        }
    }
    if !out_given {
        return Err("--out FILE is required".into());
    }
    preview::run(&o)?;
    println!("wrote {}", o.out.display());
    Ok(())
}

/// `mortar-shell sfx NAME [--wav FILE]`: play a UI sound (or write it).
fn sfx_cmd(args: &[String]) -> Result<(), String> {
    let name = args.first().ok_or("sfx needs a sound name")?;
    let s = sfx::Sfx::parse(name).ok_or(format!("unknown sound {name} ({})", sfx::Sfx::NAMES.join(", ")))?;
    match args.get(1).map(String::as_str) {
        Some("--wav") => {
            let p = args.get(2).ok_or("--wav needs a file")?;
            std::fs::write(p, sfx::wav(&sfx::render(s, 0.6))).map_err(|e| format!("{p}: {e}"))?;
        }
        _ => {
            let n = sfx::render(s, 0.6).len() as u64 * 1000 / sfx::RATE as u64;
            let p = sfx::Player::new(Some(std::env::var("MORTAR_SHELL_AUDIO").unwrap_or_else(|_| "default".into())));
            p.play(s, 60);
            std::thread::sleep(std::time::Duration::from_millis(n + 300));
        }
    }
    Ok(())
}
