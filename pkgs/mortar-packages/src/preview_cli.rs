// SPDX-License-Identifier: GPL-2.0-or-later
//! `mortar-packages preview`: the real app over made-up packages, no compositor and no
//! service, as mortar-files' preview. Scripts press the same key events the shell sends.

use crate::app::{App, Entry, Setup};
use crate::backend::Backend;
use crate::demo::{self, Demo, DemoBackend};
use crate::protocol::*;
use crate::view;
use mortar_files::bg;
use mortar_files::keys;
use mortar_files::shell::Shell;
use mortar_ui::gfx::{Fonts, Ui, font_dir};
use mortar_ui::input::Button;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Screen name, then the steps that get there from the start.
pub const SCREENS: &[(&str, &str)] = &[
    ("list", ""),
    ("list-available", "sel:Harbour Browser"),
    ("list-needs-files", "sel:Cave Runner II"),
    ("list-installed", "sel:Tracker Nine"),
    ("list-bundle", "sel:Sky Forge"),
    ("list-tools-row", "sel:Terminal tools"),
    ("list-update-format", "data:newer"),
    ("list-refused", "data:refused"),
    ("list-offline", "data:offline"),
    ("list-installing", "sel:Harbour Browser,a,a,job:40,b"),
    ("list-installing-row", "sel:Harbour Browser,a,a,job:40,b,down"),
    ("details", "sel:Harbour Browser,a"),
    ("details-update", "sel:Pond Radio,a"),
    ("details-installed", "sel:Tracker Nine,a"),
    ("details-needs-files", "sel:Cave Runner II,a"),
    ("details-files-here", "files:cave-runner,sel:Cave Runner II,a"),
    ("details-bundle", "sel:Sky Forge,a"),
    ("details-elsewhere", "sel:Moon Miner Deluxe,a"),
    ("details-low-battery", "battery:12,sel:Harbour Browser,a"),
    ("details-no-room", "free:300,sel:Harbour Browser,a"),
    ("details-not-signed", "unavailable,sel:Harbour Browser,a"),
    ("progress", "sel:Harbour Browser,a,a,job:46"),
    ("progress-starting", "sel:Harbour Browser,a,a"),
    ("progress-update", "sel:Pond Radio,a,a,job:70"),
    ("progress-cancelling", "sel:Harbour Browser,a,a,job:46,x"),
    ("progress-nearly", "sel:Harbour Browser,a,a,job:install"),
    ("confirm-remove", "sel:Tracker Nine,a,x"),
    ("confirm-remove-nudge", "sel:Tracker Nine,a,x,a"),
    ("confirm-rollback", "sel:Tracker Nine,a,y"),
    ("tools", "sel:Terminal tools,a"),
    ("tools-search", "sel:Terminal tools,a,short,type:line"),
    ("tools-results", "sel:Terminal tools,a,short,type:line,a,long"),
    ("tools-details", "sel:Terminal tools,a,short,type:ripline,a,long,a"),
    ("tools-nothing", "sel:Terminal tools,a,short,type:zzz,a,long"),
    ("report-bad-signature", "report:bad-signature"),
    ("report-wrong-device", "report:wrong-device"),
    ("report-no-room", "report:no-room"),
    ("report-older", "report:older"),
    ("empty", "data:empty"),
    ("no-repository", "data:none"),
    ("newer-format-empty", "data:newer-empty"),
    ("no-service", "data:unreachable"),
];

struct Unreachable;

impl Backend for Unreachable {
    fn ask(&self, _: &Request) -> Result<Reply, String> {
        Err("/run/mortar-packages/socket: Connection refused".into())
    }
    fn watch(&self, _: &mut dyn FnMut(JobView)) -> Result<(), String> {
        Ok(())
    }
}

fn setup_app(backend: Arc<dyn Backend>, home: &Path) -> App {
    App::new(Setup { backend, shell: Shell::None, now_secs: demo::NOW, home: home.to_path_buf(), reduce_motion: true, wake: bg::no_wake() })
}

/// Run the clock until the app is at rest.
fn settle(app: &mut App, ui: &mut Ui) {
    for _ in 0..400 {
        std::thread::sleep(Duration::from_millis(1));
        app.poll();
        app.tick(app.now + 20);
        if app.dirty || app.animating() {
            app.dirty = false;
            view::draw(app, ui);
        }
        if app.idle() && !app.animating() && app.next_deadline().is_none_or(|d| app.toast.as_ref().is_some_and(|t| t.1 == d)) {
            break;
        }
    }
    app.poll();
    app.dirty = false;
    view::draw(app, ui);
}

fn press(app: &mut App, b: Button) {
    for (c, d) in keys::pad_events(b) {
        app.key(c, d, false);
    }
}

fn select(app: &mut App, name: &str) -> Result<(), String> {
    let i = app
        .entries
        .iter()
        .position(|e| match e {
            Entry::Package(r) => r.name == name,
            Entry::Tools { .. } => name == "Terminal tools",
        })
        .ok_or(format!("no '{name}' listed"))?;
    app.home_list.sel = i;
    app.home_list.scroll_from = i as f32;
    Ok(())
}

fn problem(code: &str) -> Problem {
    match code {
        "bad-signature" => mortar_index::Refusal::BadSignature("bad signature for key mortar-1".into()).into(),
        "wrong-device" => mortar_index::Refusal::WrongDevice { id: "harbour-browser".into(), device: "brick".into() }.into(),
        "older" => mortar_index::Refusal::Older { published: demo::NOW - 40 * 86400, seen: demo::NOW - 3 * 86400 }.into(),
        "no-room" => Problem::new("no-room", "Not enough room", "It needs 1.2 GB and 300 MB is free. Removing things you don't use makes room."),
        o => Problem::new(o, "That didn't work", o),
    }
}

fn step(app: &mut App, ui: &mut Ui, d: &Arc<Demo>, tok: &str) -> Result<(), String> {
    let tok = tok.trim();
    if tok.is_empty() {
        return Ok(());
    }
    if let Some(n) = tok.strip_prefix("sel:") {
        select(app, n)?;
        app.dirty = true;
    } else if let Some(w) = tok.strip_prefix("data:") {
        d.with(|s| match w {
            "empty" => s.rows.clear(),
            "none" => {
                s.rows.clear();
                s.repos.clear();
            }
            "newer" | "newer-empty" => {
                if w == "newer-empty" {
                    s.rows.clear();
                    s.repos[0].published = 0;
                    s.repos[0].tools = 0;
                }
                s.repos[0].problem = Some(mortar_index::Refusal::NewerFormat(2).into());
            }
            "refused" => s.repos[0].problem = Some(mortar_index::Refusal::BadSignature("bad signature for key mortar-1".into()).into()),
            "offline" => s.repos[0].problem = Some(Problem::new("offline", "Can't reach it right now", "Could not resolve host")),
            _ => {}
        });
        // read again what changed
        for (c, down) in keys::pad_events(Button::Y) {
            app.key(c, down, false);
        }
    } else if let Some(n) = tok.strip_prefix("battery:") {
        d.with(|s| s.battery = n.parse().ok());
    } else if let Some(n) = tok.strip_prefix("free:") {
        d.with(|s| s.free = n.parse::<u64>().unwrap_or(0) * 1048576);
    } else if tok == "unavailable" {
        d.with(|s| s.details_problem = Some(Problem::new("unavailable", "Not available", "It isn't on the cache, or isn't signed by the key it should be.")));
    } else if let Some(id) = tok.strip_prefix("files:") {
        let dir = app.home.join(format!(".local/share/{id}"));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        for f in ["CAVES.DAT", "SOUND.DAT"] {
            std::fs::write(dir.join(f), "made up").map_err(|e| e.to_string())?;
        }
    } else if let Some(j) = tok.strip_prefix("job:") {
        // a job held at a moment, for a picture of it
        let percent = if j == "install" { None } else { Some(j.parse::<u64>().map_err(|e| e.to_string())?) };
        d.with(|s| {
            if let Some(job) = s.job.as_mut() {
                match percent {
                    None => {
                        job.phase = Phase::Installing;
                        job.done = job.total;
                    }
                    Some(p) => {
                        job.phase = Phase::Downloading;
                        job.done = job.total * p / 100;
                    }
                }
            }
        });
        std::thread::sleep(Duration::from_millis(5));
    } else if let Some(p) = tok.strip_prefix("report:") {
        app.overlay = Some(crate::app::Overlay::Report(problem(p)));
        app.dirty = true;
    } else if let Some(s) = tok.strip_prefix("type:") {
        for c in s.chars() {
            app.input(keys::Input::Char(c));
        }
        app.tick(app.now + 1000);
    } else if tok == "short" {
        app.resized(358);
    } else if tok == "long" {
        app.resized(768);
    } else {
        press(app, Button::parse(tok).ok_or(format!("unknown step '{tok}'"))?);
    }
    settle(app, ui);
    Ok(())
}

fn temp_home() -> Result<PathBuf, String> {
    let d = std::env::temp_dir().join(format!("mortar-packages-preview-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    Ok(d)
}

fn finish(app: &mut App, ui: &mut Ui, out: &Path) -> Result<(), String> {
    app.slide = None;
    if app.short {
        let mut small = Ui::new(1024, 358, Fonts::load(&font_dir())?);
        app.dirty = false;
        view::draw(app, &mut small);
        return small.save_png(out);
    }
    app.dirty = false;
    view::draw(app, ui);
    ui.save_png(out)
}

fn one(name: &str, script: &str, out: &Path) -> Result<(), String> {
    let home = temp_home()?;
    let d = Demo::new();
    let backend: Arc<dyn Backend> = if script == "data:unreachable" { Arc::new(Unreachable) } else { Arc::new(DemoBackend(d.clone())) };
    let mut app = setup_app(backend, &home);
    let mut ui = Ui::new(1024, 768, Fonts::load(&font_dir())?);
    settle(&mut app, &mut ui);
    for tok in script.split(',') {
        if tok != "data:unreachable" {
            step(&mut app, &mut ui, &d, tok).map_err(|e| format!("{name}: {e}"))?;
        }
    }
    finish(&mut app, &mut ui, out)?;
    let _ = std::fs::remove_dir_all(&home);
    Ok(())
}

pub fn main(args: &[String]) -> Result<(), String> {
    let mut screen = None;
    let mut keys_arg = None;
    let mut out = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--screen" => screen = Some(val()?),
            "--keys" => keys_arg = Some(val()?),
            "--out" => out = Some(PathBuf::from(val()?)),
            "--theme" => {
                let n = val()?;
                mortar_ui::theme::set(mortar_ui::theme::Theme::by_name(&n).ok_or(format!("unknown theme '{n}'"))?);
            }
            "--all" => {
                let dir = PathBuf::from(val()?);
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                for (name, script) in SCREENS {
                    one(name, script, &dir.join(format!("{name}.png")))?;
                }
                println!("wrote {} screens to {}", SCREENS.len(), dir.display());
                return Ok(());
            }
            "--list" => {
                for (n, _) in SCREENS {
                    println!("{n}");
                }
                return Ok(());
            }
            o => return Err(format!("unknown argument '{o}'")),
        }
    }
    let script = match (&screen, &keys_arg) {
        (Some(s), _) => SCREENS.iter().find(|x| x.0 == s).ok_or(format!("unknown screen '{s}' (--list)"))?.1.to_string(),
        (None, Some(k)) => k.clone(),
        _ => String::new(),
    };
    let out = out.ok_or("--out FILE")?;
    one("preview", &script, &out)?;
    println!("wrote {}", out.display());
    Ok(())
}
