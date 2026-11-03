// SPDX-License-Identifier: GPL-2.0-or-later
//! `mortar-files preview`: the real app over a made-up tree of files, no
//! compositor. Scripts press the same key events the shell would send.

use crate::app::{App, Setup};
use crate::listing::{Kind, Sort};
use crate::shell::Shell;
use crate::{demo, keys, ops, view};
use mortar_ui::gfx::{Fonts, Ui, font_dir};
use mortar_ui::input::Button;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Screen name, then the steps that get there from Places.
pub const SCREENS: &[(&str, &str)] = &[
    ("places", ""),
    ("home", "place:Home"),
    ("folder", "place:Home,sel:Pictures"),
    ("pictures", "place:Home,sel:Pictures,a,sel:Harbour at dusk.png"),
    ("games", "place:Games,sel:nes,a,sel:Hexagon Harvest.nes"),
    ("long-name", "place:Games,sel:nes,a,sel:The Long Way Round (Europe) (Rev 1) (Translated) (Extended Edition).nes"),
    ("saves", "place:Saves,sel:gba,a"),
    ("text", "place:Home,sel:today.txt"),
    ("archive", "place:Home,sel:Downloads,a"),
    ("by-size", "place:Home,sel:Downloads,a,r2"),
    ("by-date", "place:Games,sel:nes,a,r2,r2"),
    ("marked", "place:Games,sel:nes,a,y,y,down,y"),
    ("actions", "place:Games,sel:nes,a,x"),
    ("actions-marked", "place:Games,sel:nes,a,y,y,x"),
    ("holding", "place:Games,sel:nes,a,y,y,x,a,l2,place:Home,sel:Documents,a"),
    ("holding-actions", "place:Games,sel:nes,a,y,y,x,a,l2,place:Home,sel:Documents,a,x"),
    ("trash-confirm", "place:Games,sel:nes,a,y,y,y,x,act:Delete"),
    ("forever-confirm", "place:Trash-filled,x,act:DeleteForever"),
    ("forever-hold", "place:Trash-filled,x,act:DeleteForever,hold:600"),
    ("trash", "place:Trash-filled"),
    ("rename", "place:Home,sel:today.txt,x,act:Rename"),
    ("rename-keyboard", "place:Home,sel:today.txt,x,act:Rename,short"),
    ("new-folder", "place:Home,x,act:NewFolder"),
    ("find", "place:Games,sel:nes,a,short,type:qu"),
    ("empty", "place:Home,sel:Empty folder,a"),
    ("read-only", "place:Everything,sel:nix,a,sel:store"),
    ("unreadable", "place:Home,go:/not/there"),
    ("broken-link", "place:Home,sel:Games,a,sel:old link"),
    ("image-view", "place:Home,sel:Pictures,a,sel:Harbour at dusk.png,a"),
    ("text-view", "place:Home,sel:Documents,a,sel:notes.txt,a"),
    ("storage", "place:Storage"),
    ("storage-folder", "place:Home,select"),
    ("storage-inside", "place:Storage,a"),
    ("progress", "job:progress"),
    ("clash", "job:clash"),
    ("failed", "job:failed"),
    ("cant-open", "place:Home,sel:Music,a,sel:mixdown.wav,a"),
];

fn setup_app(root: &Path) -> App {
    let env = demo::build(root);
    let guard = demo::guard(&env);
    let play = Some(demo::play_config(root));
    App::new(Setup { env, guard, clock: demo::clock(), play, shell: Shell::None, reduce_motion: false, wake: crate::bg::no_wake() })
}

/// Run the clock until the app is at rest.
fn settle(app: &mut App, ui: &mut Ui) {
    for _ in 0..400 {
        std::thread::sleep(Duration::from_millis(2));
        app.poll();
        app.tick(app.now + 20);
        if app.dirty || app.animating() {
            app.dirty = false;
            view::draw(app, ui);
        }
        let due = app.next_deadline().is_some_and(|d| app.toast.as_ref().is_none_or(|t| d < t.1));
        if app.idle() && !app.animating() && !due {
            break;
        }
    }
    // the last worker may have finished between its message and the check
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
    let i = app.entries.iter().position(|e| e.display == name).ok_or(format!("no '{name}' in {}", app.dir.display()))?;
    app.sel = i;
    app.scroll_from = i as f32;
    app.scroll_start = 0;
    app.panel = None;
    app.input(crate::keys::Input::Press(Button::Up, false));
    app.input(crate::keys::Input::Press(Button::Down, false));
    app.sel = i;
    app.scroll_from = i as f32;
    app.scroll_start = 0;
    app.refresh_preview();
    Ok(())
}

fn act(app: &mut App, name: &str) -> Result<(), String> {
    use crate::app::{Act, Overlay};
    let a = match name {
        "Delete" => Act::Delete,
        "DeleteForever" => Act::DeleteForever,
        "Rename" => Act::Rename,
        "NewFolder" => Act::NewFolder,
        other => return Err(format!("act {other}?")),
    };
    let i = match &app.overlay {
        Some(Overlay::Actions { rows, .. }) => rows.iter().position(|r| *r == a).ok_or(format!("no {name} here"))?,
        _ => return Err("act needs the actions open".into()),
    };
    for _ in 0..i {
        press(app, Button::Down);
    }
    press(app, Button::A);
    Ok(())
}

fn fill_trash(app: &mut App) {
    let g = app.guard.clone();
    let home = app.env.home.clone();
    let play = app.env.play.clone();
    for p in [home.join("Downloads/soundfont.7z"), home.join("Documents/old manual.pdf"), play.join("roms/nes/Cave Runner II.nes"), home.join("Pictures/Lime hills.png")] {
        let _ = app.trash.put(&g, &p);
    }
}

/// A job that holds at a chosen moment, for a picture of it.
fn job_state(app: &mut App, which: &str, ui: &mut Ui) -> Result<(), String> {
    use crate::app::{ClashUi, JobRun};
    use ops::{Choice, Clash, Job, JobKind, Progress, Side};
    let (_tx, rx) = std::sync::mpsc::channel();
    let (atx, _arx) = std::sync::mpsc::channel();
    // parked for the picture: nothing runs
    std::mem::forget(_tx);
    std::mem::forget(_arx);
    let job = Job { rx, answers: atx, cancel: Default::default() };
    let kinds = vec![Kind::Game, Kind::Game, Kind::Save, Kind::Folder, Kind::Game, Kind::Image];
    let mut run = JobRun { kind: JobKind::Copy, job, progress: Progress::default(), clash: None, cancelling: false, started: 0, shown: true, kinds, count: 6 };
    match which {
        "progress" => {
            run.progress = Progress { files_done: 41, files_total: 96, bytes_done: 412 * 1024 * 1024, bytes_total: 980 * 1024 * 1024, current: "Crystal Orchard.gba".into() };
        }
        "clash" => {
            let t = demo::NOW;
            run.clash = Some(ClashUi {
                clash: Clash {
                    name: "Crystal Orchard.sav".into(),
                    existing: Side { size: 128 * 1024, mtime: t - 3 * 3600, is_dir: false },
                    incoming: Side { size: 64 * 1024, mtime: t - 40 * 86400, is_dir: false },
                    options: vec![Choice::Skip, Choice::KeepBoth, Choice::Replace],
                },
                sel: 0,
                for_rest: false,
            });
        }
        "failed" => {
            let r = ops::Report {
                kind: JobKind::Move,
                total: 5,
                done: 3,
                skipped: 0,
                failed: vec![
                    ops::Failure { name: "Sky Forge.sfc".into(), why: "The disk is full.".into() },
                    ops::Failure { name: "Ring Valley.sfc".into(), why: "You don't have permission.".into() },
                ],
                cancelled: false,
                bytes: 0,
                landed: vec![],
                refused: None,
                kept_originals: 0,
            };
            app.overlay = Some(crate::app::Overlay::Report(r));
            app.dirty = true;
            view::draw(app, ui);
            return Ok(());
        }
        o => return Err(format!("job {o}?")),
    }
    app.job = Some(run);
    app.tick(app.now + 1000);
    app.dirty = false;
    view::draw(app, ui);
    Ok(())
}

fn step(app: &mut App, ui: &mut Ui, tok: &str, settle_after: bool) -> Result<(), String> {
    let tok = tok.trim();
    if tok.is_empty() {
        return Ok(());
    }
    if let Some(n) = tok.strip_prefix("place:") {
        let (n, fill) = match n.strip_suffix("-filled") {
            Some(x) => (x, true),
            None => (n, false),
        };
        if fill {
            fill_trash(app);
            app.poll();
        }
        app.place_sel = app.places.iter().position(|p| p.name == n).ok_or(format!("no place {n}"))?;
        press(app, Button::A);
    } else if let Some(n) = tok.strip_prefix("sel:") {
        select(app, n)?;
    } else if let Some(n) = tok.strip_prefix("act:") {
        act(app, n)?;
    } else if let Some(d) = tok.strip_prefix("go:") {
        app.go_to(PathBuf::from(d));
    } else if let Some(s) = tok.strip_prefix("type:") {
        for c in s.chars() {
            app.input(keys::Input::Char(c));
        }
    } else if let Some(ms) = tok.strip_prefix("wait:") {
        app.tick(app.now + ms.parse::<u64>().map_err(|e| e.to_string())?);
    } else if let Some(ms) = tok.strip_prefix("hold:") {
        app.input(keys::Input::Shoulders { l1: true, r1: true });
        app.tick(app.now + ms.parse::<u64>().map_err(|e| e.to_string())?);
        app.dirty = false;
        view::draw(app, ui);
        return Ok(());
    } else if let Some(w) = tok.strip_prefix("job:") {
        return job_state(app, w, ui);
    } else if tok == "short" {
        app.resized(358);
    } else {
        press(app, Button::parse(tok).ok_or(format!("unknown step '{tok}'"))?);
    }
    if settle_after {
        settle(app, ui);
    }
    Ok(())
}

pub fn main(args: &[String]) -> Result<(), String> {
    let mut screen = None;
    let mut keys_arg = None;
    let mut out = None;
    let mut outdir = None;
    let mut record = None;
    let mut every = 33u64;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--screen" => screen = Some(val()?),
            "--keys" => keys_arg = Some(val()?),
            "--out" => out = Some(PathBuf::from(val()?)),
            "--outdir" => outdir = Some(PathBuf::from(val()?)),
            "--record" => record = Some(val()?),
            "--every" => every = val()?.parse().map_err(|_| "--every wants a number")?,
            "--theme" => {
                let n = val()?;
                mortar_ui::theme::set(mortar_ui::theme::Theme::by_name(&n).ok_or(format!("unknown theme '{n}'"))?);
            }
            "--bench" => return bench(),
            "--all" => {
                let d = PathBuf::from(val()?);
                return all(&d);
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
    ops::fake_space(Some((3_900_000_000, 14_600_000_000)));
    crate::storage::apparent_sizes(true);
    let root = temp_root()?;
    let r = (|| -> Result<(), String> {
        let mut app = setup_app(&root);
        let fonts = Fonts::load(&font_dir())?;
        let mut ui = Ui::new(1024, 768, fonts);
        settle(&mut app, &mut ui);
        if let Some(rec) = record {
            let dir = outdir.clone().ok_or("--record needs --outdir")?;
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            return record_frames(&mut app, &mut ui, &rec, every, &dir);
        }
        let script = match (&screen, &keys_arg) {
            (Some(s), _) => SCREENS.iter().find(|x| x.0 == s).ok_or(format!("unknown screen '{s}' (--list)"))?.1.to_string(),
            (None, Some(k)) => k.clone(),
            _ => String::new(),
        };
        for tok in script.split(',') {
            step(&mut app, &mut ui, tok, true)?;
        }
        let out = out.clone().ok_or("--out FILE")?;
        finish(&mut app, &mut ui, &out, screen.as_deref())?;
        println!("wrote {}", out.display());
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&root);
    r
}

/// Short-window screens draw on a canvas of the height the keyboard leaves.
fn finish(app: &mut App, ui: &mut Ui, out: &Path, _screen: Option<&str>) -> Result<(), String> {
    if app.short {
        let fonts = Fonts::load(&font_dir())?;
        let mut small = Ui::new(1024, 358, fonts);
        app.dirty = false;
        app.slide = None;
        view::draw(app, &mut small);
        return small.save_png(out);
    }
    ui.save_png(out)
}

fn temp_root() -> Result<PathBuf, String> {
    let d = std::env::temp_dir().join(format!("mortar-files-preview-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    std::fs::canonicalize(d).map_err(|e| e.to_string())
}

/// Every screen, each from a fresh start, in one folder.
fn all(dir: &Path) -> Result<(), String> {
    ops::fake_space(Some((3_900_000_000, 14_600_000_000)));
    crate::storage::apparent_sizes(true);
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    for (name, script) in SCREENS {
        let root = temp_root()?;
        let mut app = setup_app(&root);
        let mut ui = Ui::new(1024, 768, Fonts::load(&font_dir())?);
        settle(&mut app, &mut ui);
        for tok in script.split(',') {
            step(&mut app, &mut ui, tok, true).map_err(|e| format!("{name}: {e}"))?;
        }
        finish(&mut app, &mut ui, &dir.join(format!("{name}.png")), Some(name))?;
        let _ = std::fs::remove_dir_all(&root);
    }
    println!("wrote {} screens to {}", SCREENS.len(), dir.display());
    Ok(())
}

/// Each step, then frames of whatever it set moving.
fn record_frames(app: &mut App, ui: &mut Ui, script: &str, every: u64, dir: &Path) -> Result<(), String> {
    let mut n = 0;
    let mut shot = |app: &mut App, ui: &mut Ui| -> Result<(), String> {
        std::thread::sleep(Duration::from_millis(6));
        app.poll();
        app.tick(app.now + every);
        if app.dirty || app.animating() {
            app.dirty = false;
            view::draw(app, ui);
        }
        n += 1;
        ui.save_png(&dir.join(format!("f{n:05}.png")))
    };
    // hold the first picture a moment
    for _ in 0..12 {
        shot(app, ui)?;
    }
    let _ = Sort::Name;
    for tok in script.split(',') {
        let (tok, pause) = match tok.split_once('/') {
            Some((t, p)) => (t, p.parse::<usize>().map_err(|_| "step/FRAMES")?),
            None => (tok, 12usize),
        };
        step(app, ui, tok, false)?;
        // wait for reads a real device would also wait for, on the real clock
        for _ in 0..pause {
            shot(app, ui)?;
        }
    }
    println!("wrote {n} frames to {}", dir.display());
    Ok(())
}

/// What a frame costs on this machine: a full draw of a list, and a frame of
/// a slide.
fn bench() -> Result<(), String> {
    use std::time::Instant;
    ops::fake_space(Some((3_900_000_000, 14_600_000_000)));
    let root = temp_root()?;
    let mut app = setup_app(&root);
    let mut ui = Ui::new(1024, 768, Fonts::load(&font_dir())?);
    settle(&mut app, &mut ui);
    for tok in ["place:Games", "sel:nes", "a"] {
        step(&mut app, &mut ui, tok, true)?;
    }
    let t = Instant::now();
    let n = 200;
    for i in 0..n {
        app.sel = i % 8;
        app.dirty = false;
        view::draw(&mut app, &mut ui);
    }
    println!("full frame of the list: {:.2} ms", t.elapsed().as_secs_f64() * 1000.0 / n as f64);
    app.slide_req = Some(crate::app::SlideDir::Forward);
    view::draw(&mut app, &mut ui);
    let start = app.now;
    let t = Instant::now();
    for i in 0..n {
        app.now = start + (i as u64 % 150);
        view::draw(&mut app, &mut ui);
    }
    println!("frame of a slide: {:.2} ms", t.elapsed().as_secs_f64() * 1000.0 / n as f64);
    let _ = std::fs::remove_dir_all(&root);
    Ok(())
}
