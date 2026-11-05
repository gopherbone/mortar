// SPDX-License-Identifier: GPL-2.0-or-later
//! mortar-files: a file manager for the Brick's buttons.
//!
//!   mortar-files                   the app (Wayland, in the shell)
//!   mortar-files preview --screen NAME --out FILE.png [--theme dark]
//!   mortar-files preview --keys a,down,x --out FILE.png
//!   mortar-files preview --record KEYS --every MS --outdir DIR   (frames for a GIF)
//!   mortar-files preview --list
//!
//! The preview renders the real app, with no compositor, over a made-up tree
//! of files in a temp folder, on any OS.

use mortar_files::app::{App, Setup};
use mortar_files::bg;
use mortar_files::listing::Clock;
use mortar_files::ops::Guard;
use mortar_files::places::{PlaceEnv, PlayConfig};
use mortar_files::preview_cli;
use mortar_files::shell::Shell;
use std::path::PathBuf;

fn usage() -> ! {
    eprintln!("usage:\n  mortar-files\n  mortar-files preview --screen NAME --out FILE.png [--theme light|dark] [--keys K,K,...]\n  mortar-files preview --record K,K,... [--every MS] --outdir DIR\n  mortar-files preview --list");
    std::process::exit(2);
}

fn run() -> Result<(), String> {
    match std::env::var("MORTAR_PALETTE").as_deref() {
        Ok("night") => {
            mortar_ui::theme::set(mortar_ui::theme::DARK);
        }
        Ok("day") => {
            mortar_ui::theme::set(mortar_ui::theme::LIGHT);
        }
        _ => {}
    }
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME isn't set")?);
    let play_path = std::env::var_os("MORTAR_PLAY_CONFIG").map(PathBuf::from).unwrap_or_else(|| "/etc/mortar-play/config.json".into());
    let play = PlayConfig::load(&play_path, &home);
    let play_root = play.as_ref().and_then(|p| p.data_dir.clone()).unwrap_or_else(|| PathBuf::from("/var/lib/mortar-play"));
    let env = PlaceEnv::system(home.clone(), play_root.clone());
    let mut pinned: Vec<PathBuf> = ["roms", "saves", "states", "bios"].iter().map(|d| play_root.join(d)).collect();
    pinned.push(play_root);
    pinned.extend([env.trash.clone(), env.trash.join("files"), env.trash.join("info")]);
    let guard = Guard::system(Some(home), pinned);
    let (wake_fd, wake) = bg::wake_pipe()?;
    let setup = Setup { env, guard, clock: Clock::system(), play, shell: Shell::detect(), reduce_motion: reduce_motion(), wake };
    let mut app = App::new(setup);
    #[cfg(target_os = "linux")]
    {
        mortar_files::wl::run(&mut app, wake_fd, "Files", "mortar-files")
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (&mut app, wake_fd);
        Err("the window needs Wayland (Linux); use `preview` here".into())
    }
}

/// The shell's Settings > Display > Reduce motion: read once at start.
fn reduce_motion() -> bool {
    let state = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")));
    let Some(s) = state else { return false };
    let Ok(t) = std::fs::read_to_string(s.join("mortar-shell/prefs.json")) else { return false };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) else { return false };
    v["reduceMotion"].as_bool().or_else(|| v["reduce_motion"].as_bool()).unwrap_or(false)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
        None => run(),
        Some("preview") => preview_cli::main(&args[1..]),
        Some(_) => usage(),
    };
    if let Err(e) = r {
        eprintln!("mortar-files: {e}");
        std::process::exit(1);
    }
}
