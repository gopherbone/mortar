// SPDX-License-Identifier: GPL-2.0-or-later
//! mortar-play: a minimal, preconfigured libretro frontend for Mortar.
//!
//!   mortar-play [--config FILE] [--display auto|sdl|wayland|headless]
//!   mortar-play run SYSTEM [ROM] [...]      play one game, quit with it
//!   mortar-play selftest SYSTEM [ROM] --out DIR [--frames N] [--shots 60,120] [--resume] [--paced]
//!   mortar-play check                       load every core, validate option presets
//!   mortar-play make-test-rom OUT.nes       the built-in NES test program
//!   mortar-play list --json                 the library as JSON (mortar-shell's Games)
//!   mortar-play --script "wait 30, press a, shot x.png" [...]   scripted input
//!
//! The library UI starts by default. Config: /etc/mortar-play/config.json
//! (modules/packs/emulation.nix), or $MORTAR_PLAY_CONFIG.

mod app;
mod audio;
mod config;
mod control;
mod display;
mod input;
mod library;
mod lr;
mod platform;
mod selftest;
mod session;
mod testrom;
mod ui;
mod video;

use std::path::PathBuf;

const USAGE: &str = "usage: mortar-play [run SYSTEM [ROM] | selftest SYSTEM [ROM]] [--config FILE] [--display auto|sdl|wayland|headless] [--size WxH] [--script STEPS]
  selftest options: --out DIR --frames N --shots 60,120,300 [--press FRAME:BUTTON:FRAMES,...] [--resume] [--paced] [--view FILTER/FIT]";

struct Args {
    cmd: Option<String>,
    pos: Vec<String>,
    config: PathBuf,
    display: String,
    size: (u32, u32),
    script: Option<String>,
    out: Option<PathBuf>,
    frames: u64,
    shots: Vec<u64>,
    press: Vec<(u64, String, u64)>,
    resume: bool,
    paced: bool,
    view: Option<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        cmd: None,
        pos: Vec::new(),
        config: std::env::var_os("MORTAR_PLAY_CONFIG").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/etc/mortar-play/config.json")),
        display: "auto".into(),
        size: (1024, 768),
        script: None,
        out: None,
        frames: 300,
        shots: Vec::new(),
        press: Vec::new(),
        resume: false,
        paced: false,
        view: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = || it.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "-h" | "--help" => return Err(USAGE.into()),
            "--config" => a.config = PathBuf::from(val()?),
            "--display" => a.display = val()?,
            "--headless" => a.display = "headless".into(),
            "--size" => {
                let v = val()?;
                let (w, h) = v.split_once('x').ok_or("--size WxH")?;
                a.size = (w.parse().map_err(|_| "--size")?, h.parse().map_err(|_| "--size")?);
            }
            "--script" => a.script = Some(val()?),
            "--json" => {}
            "--resume" => a.resume = true,
            "--paced" => a.paced = true,
            "--view" => a.view = Some(val()?),
            "--out" => a.out = Some(PathBuf::from(val()?)),
            "--frames" => a.frames = val()?.parse().map_err(|_| "--frames N")?,
            "--shots" => a.shots = val()?.split(',').filter_map(|s| s.trim().parse().ok()).collect(),
            "--press" => {
                for p in val()?.split(',') {
                    let f: Vec<&str> = p.split(':').collect();
                    if f.len() != 3 {
                        return Err("--press FRAME:BUTTON:FRAMES".into());
                    }
                    a.press.push((f[0].parse().map_err(|_| "--press")?, f[1].to_string(), f[2].parse().map_err(|_| "--press")?));
                }
            }
            s if s.starts_with("--") => return Err(format!("unknown option {s}\n{USAGE}")),
            s if a.cmd.is_none() && matches!(s, "run" | "selftest" | "make-test-rom" | "check" | "list") => a.cmd = Some(s.into()),
            s => a.pos.push(s.into()),
        }
    }
    Ok(a)
}

fn main() {
    let a = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    if a.cmd.as_deref() == Some("make-test-rom") {
        let out = a.pos.first().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("mortar-test.nes"));
        if let Err(e) = std::fs::write(&out, testrom::nes()) {
            eprintln!("mortar-play: {}: {e}", out.display());
            std::process::exit(1);
        }
        return;
    }
    let cfg = match config::Config::load(&a.config) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("mortar-play: {e}");
            std::process::exit(1);
        }
    };
    if a.cmd.as_deref() == Some("list") {
        // `--json` is the only format
        println!("{}", library::list_json(&cfg));
        return;
    }
    if a.cmd.as_deref() == Some("check") {
        std::process::exit(selftest::check(&cfg));
    }
    if a.cmd.as_deref() == Some("selftest") {
        let Some(sys) = a.pos.first() else {
            eprintln!("{USAGE}");
            std::process::exit(2);
        };
        let out = a.out.clone().unwrap_or_else(|| PathBuf::from("selftest-out"));
        let opts = selftest::Opts { frames: a.frames, shots: a.shots.clone(), press: a.press.clone(), out, size: a.size, resume: a.resume, paced: a.paced, view: a.view.clone() };
        match selftest::run(&cfg, sys, a.pos.get(1).map(PathBuf::from).as_deref(), &opts) {
            Ok(r) => {
                println!("{}", serde_json::to_string_pretty(&r).unwrap_or_default());
                std::process::exit(if r.ok { 0 } else { 1 });
            }
            Err(e) => {
                eprintln!("mortar-play: {e}");
                std::process::exit(1);
            }
        }
    }

    app::install_signal_handlers();
    let (pf, audio) = match platform::Platform::open(&a.display, a.size.0, a.size.1) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("mortar-play: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("mortar-play: display {}", pf.display.name());
    let mut app = match app::App::new(cfg, pf, audio) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("mortar-play: {e}");
            std::process::exit(1);
        }
    };
    if let Some(s) = &a.script {
        match app::parse_script(s) {
            Ok(steps) => app.set_script(steps),
            Err(e) => {
                eprintln!("mortar-play: {e}");
                std::process::exit(2);
            }
        }
    }
    if a.cmd.as_deref() == Some("run") {
        let Some(sys) = a.pos.first() else {
            eprintln!("{USAGE}");
            std::process::exit(2);
        };
        let rom = a.pos.get(1).map(PathBuf::from);
        if let Err(e) = app.launch_single(sys, rom.as_deref()) {
            eprintln!("mortar-play: {e}");
            std::process::exit(1);
        }
    }
    app.run();
    eprintln!("mortar-play: bye ({} frames)", app.frames);
}
