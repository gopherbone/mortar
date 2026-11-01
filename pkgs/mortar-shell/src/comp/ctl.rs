// SPDX-License-Identifier: GPL-2.0-or-later
//! Control socket: one command per connection, one reply. Used by tests
//! (inject buttons, screenshot) and handy over SSH:
//!
//!   mortar-shell ctl state
//!   mortar-shell ctl press menu
//!   mortar-shell ctl launch terminal
//!   mortar-shell ctl screenshot /tmp/screen.png
//!
//! Commands: state | press B | down B | up B | key CODE | launch ID |
//! focus UID|ID | close UID|ID | screenshot PATH | home | quit, and for the
//! UI: shot (the screenshot combo: flash, sound, file in screenshotDir),
//! toast JSON ({"title", "sub", "kind": "info"|"achievement", "sound", "haptic",
//! "secs"}; `mortar-shell toast` builds it), boot (replay the boot screen),
//! time manual|real|+MS (freeze UI time / step it, for test frames),
//! prefs (the current settings).
//! The socket is $XDG_RUNTIME_DIR/mortar-shell.sock (mode 0600). Clients
//! without one of their own (root over SSH) find the session's in
//! /run/user/*/.

use super::state::Shell;
use mortar_ui::input::Button;
use serde_json::json;
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub fn default_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    dir.join("mortar-shell.sock")
}

/// Where `mortar-shell ctl` / `toast` connect: our own socket, else the
/// first session's under /run/user.
pub fn client_path() -> PathBuf {
    let own = default_path();
    if own.exists() {
        return own;
    }
    // Clients outside the session (root over SSH): the session user's shell.
    let mut found: Vec<PathBuf> = std::fs::read_dir("/run/user")
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path().join("mortar-shell.sock"))
        .filter(|p| p.exists())
        .collect();
    found.sort();
    found.into_iter().next().unwrap_or(own)
}

pub fn listen(handle: &LoopHandle<'static, Shell>, path: &Path) -> Result<(), String> {
    let _ = std::fs::remove_file(path);
    let l = UnixListener::bind(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    l.set_nonblocking(true).map_err(|e| e.to_string())?;
    handle
        .insert_source(Generic::new(l, Interest::READ, Mode::Level), |_, l, shell: &mut Shell| {
            while let Ok((s, _)) = l.accept() {
                serve(shell, s);
            }
            Ok(PostAction::Continue)
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn serve(shell: &mut Shell, s: UnixStream) {
    let _ = s.set_nonblocking(false);
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let mut line = String::new();
    let mut r = BufReader::new(&s);
    if r.read_line(&mut line).is_err() {
        return;
    }
    let reply = match command(shell, line.trim()) {
        Ok(v) => v,
        Err(e) => format!("error: {e}"),
    };
    let mut w = &s;
    let _ = w.write_all(reply.as_bytes());
    let _ = w.write_all(b"\n");
}

fn find_app(shell: &Shell, arg: &str) -> Result<u32, String> {
    if arg.is_empty() {
        return shell.visible_app().ok_or("no app on screen".into());
    }
    if let Ok(uid) = arg.parse::<u32>()
        && shell.apps.get(uid).is_some()
    {
        return Ok(uid);
    }
    shell.apps.by_entry(arg).map(|a| a.uid).ok_or(format!("no running app '{arg}'"))
}

pub fn command(shell: &mut Shell, line: &str) -> Result<String, String> {
    let (cmd, arg) = line.split_once(' ').map(|(a, b)| (a, b.trim())).unwrap_or((line, ""));
    let button = || Button::parse(arg).ok_or(format!("unknown button '{arg}'"));
    let r = match cmd {
        "state" => state(shell),
        "press" => {
            let b = button()?;
            shell.inject_button(b, 1);
            shell.inject_button(b, 0);
            "ok".into()
        }
        "down" => {
            shell.inject_button(button()?, 1);
            "ok".into()
        }
        "up" => {
            shell.inject_button(button()?, 0);
            "ok".into()
        }
        "key" => {
            let code: u16 = arg.parse().map_err(|_| "key wants an evdev code")?;
            shell.send_key(code, true);
            shell.send_key(code, false);
            "ok".into()
        }
        "launch" => format!("{}", shell.launch(arg)?),
        "focus" => {
            let uid = find_app(shell, arg)?;
            shell.show_app(uid);
            "ok".into()
        }
        "close" => {
            let uid = find_app(shell, arg)?;
            shell.close_app(uid);
            "ok".into()
        }
        "home" => {
            shell.at_home = true;
            shell.refocus();
            "ok".into()
        }
        "gpushot" => {
            // what the GPU UI renderer last put on screen (debugging)
            let super::backend::Backend::Drm(d) = &mut shell.backend else { return Err("no GPU UI".into()) };
            let g = d.gpu.as_mut().ok_or("no GPU UI")?;
            let (w, h, px) = g.read_front().ok_or("no GPU frame on screen")?;
            let pm = resvg::tiny_skia::Pixmap::from_vec(px, resvg::tiny_skia::IntSize::from_wh(w, h).ok_or("size")?).ok_or("pixmap")?;
            super::ui::save_rgb_png(&pm, Path::new(arg))?;
            "ok".into()
        }
        "power" => {
            // the power key: `power` taps, `power hold` holds it for the menu
            shell.on_key(super::input::Kind::Keys, mortar_ui::input::codes::KEY_POWER, 1);
            if arg != "hold" {
                shell.on_key(super::input::Kind::Keys, mortar_ui::input::codes::KEY_POWER, 0);
            }
            "ok".into()
        }
        "power-up" => {
            shell.on_key(super::input::Kind::Keys, mortar_ui::input::codes::KEY_POWER, 0);
            "ok".into()
        }
        "shot" => {
            shell.screenshot_combo();
            "ok".into()
        }
        "toast" => {
            shell.toast_json(arg)?;
            "ok".into()
        }
        "boot" => {
            shell.sui.play_boot();
            shell.sync_cover();
            shell.needs_redraw = true;
            "ok".into()
        }
        "time" => {
            match arg {
                "manual" => shell.manual_time = true,
                "real" => {
                    shell.manual_time = false;
                    shell.last_step = std::time::Instant::now();
                }
                a if a.starts_with('+') => {
                    let ms: u32 = a[1..].parse().map_err(|_| "time +MS")?;
                    shell.ui_advance(ms);
                }
                _ => return Err("time manual | real | +MS".into()),
            }
            "ok".into()
        }
        "prefs" => serde_json::to_string(&shell.sui.prefs).map_err(|e| e.to_string())?,
        "screenshot" => {
            if arg.is_empty() {
                return Err("screenshot needs a path".into());
            }
            shell.tick(); // bring the shell screens up to date
            super::backend::screenshot(shell, Path::new(arg))?;
            "ok".into()
        }
        "quit" => {
            shell.running = false;
            shell.signal.stop();
            "ok".into()
        }
        "" => return Err("empty command".into()),
        other => return Err(format!("unknown command '{other}'")),
    };
    shell.tick();
    Ok(r)
}

fn state(shell: &Shell) -> String {
    let vis = shell.visible_app();
    let apps: Vec<_> = shell
        .apps
        .ordered()
        .into_iter()
        .filter_map(|u| shell.apps.get(u))
        .map(|a| {
            json!({
                "uid": a.uid, "id": a.entry_id, "name": a.name, "pid": a.pid, "slot": a.slot,
                "windows": a.windows.len(), "visible": Some(a.uid) == vis,
                "closing": a.closing.is_some(), "gamepad": format!("{:?}", a.gamepad),
                "geometry": a.windows.last().map(|w| { let g = w.geometry(); [g.loc.x, g.loc.y, g.size.w, g.size.h] }),
                "buffer": a.windows.last().and_then(|w| w.toplevel()).and_then(|t| smithay::wayland::compositor::with_states(t.wl_surface(), |st| {
                    st.data_map.get::<smithay::backend::renderer::utils::RendererSurfaceStateUserData>()
                        .and_then(|d| d.lock().unwrap().view()).map(|v| [v.src.size.w as i32, v.src.size.h as i32, v.dst.w, v.dst.h])
                })),
            })
        })
        .collect();
    json!({
        "socket": shell.socket_name,
        "overlay": shell.sui.menu_open() || shell.sui.page_open(),
        "category": shell.sui.menu_category().map(|c| c.name()),
        "settings": shell.sui.settings_open(),
        "home": shell.visible_app().is_none() && !shell.sui.menu_open(),
        "ui": {
            "layer": shell.ui_layer(),
            "booting": shell.sui.booting(),
            "pace": format!("{:?}", shell.sui.pace(shell.visible_app().is_some())),
            "renderer": if shell.gpu_ui { "gpu" } else { "cpu" },
            "prefs": serde_json::to_value(&shell.sui.prefs).unwrap_or_default(),
        },
        "visible": vis,
        "osk": shell.osk.visible,
        "sleep": format!("{:?}", shell.sui.sleep_phase()),
        "power_menu": shell.sui.power_menu_open(),
        "resumes": shell.resumes,
        "timezone": shell.timezone,
        "held": shell.router.held(),
        "apps": apps,
        "frames": shell.backend.frames(),
        "gpu_frames": shell.backend.gpu_frames(),
        "scanout_frames": shell.backend.scanout_frames(),
        "rss_kib": rss_kib(),
        "inputs": shell.inputs.summary(),
        "pads": shell.pads.phys_names(),
    })
    .to_string()
}

fn rss_kib() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    let l = s.lines().find(|l| l.starts_with("VmRSS:"))?;
    l.split_whitespace().nth(1)?.parse().ok()
}

/// The client side: `mortar-shell ctl <command...>`.
pub fn client(path: &Path, words: &[String]) -> Result<String, String> {
    let mut s = UnixStream::connect(path).map_err(|e| format!("{}: {e}", path.display()))?;
    s.write_all(words.join(" ").as_bytes()).and_then(|_| s.write_all(b"\n")).map_err(|e| e.to_string())?;
    let mut out = String::new();
    BufReader::new(&s).read_line(&mut out).map_err(|e| e.to_string())?;
    let out = out.trim_end().to_string();
    if let Some(e) = out.strip_prefix("error: ") {
        return Err(e.to_string());
    }
    Ok(out)
}
