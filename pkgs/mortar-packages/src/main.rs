// SPDX-License-Identifier: GPL-2.0-or-later
//! mortar-packages: get more for your Brick.
//!
//!   mortar-packages                   the app (Wayland, in the shell)
//!   mortar-packages preview --screen NAME --out FILE.png [--theme dark]
//!   mortar-packages preview --keys a,down,x --out FILE.png
//!   mortar-packages preview --all DIR | --list
//!   mortar-packages fake-service --socket PATH --log FILE
//!
//! The preview renders the real app with no compositor, over made-up packages, on any
//! OS. The fake service is the same made-up data on a socket, for the headless test.

use mortar_files::bg;
use mortar_files::shell::Shell;
use mortar_packages::app::{App, Setup};
use mortar_packages::backend::Socket;
use mortar_packages::{demo, preview_cli};
use std::path::PathBuf;
use std::sync::Arc;

fn usage() -> ! {
    eprintln!("usage:\n  mortar-packages\n  mortar-packages preview --screen NAME --out FILE.png [--theme light|dark]\n  mortar-packages preview --keys K,K,... --out FILE.png\n  mortar-packages preview --all DIR\n  mortar-packages preview --list\n  mortar-packages fake-service --socket PATH --log FILE");
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
    let (wake_fd, wake) = bg::wake_pipe()?;
    let now_secs = mortar_index::time::now();
    let mut app = App::new(Setup { backend: Arc::new(Socket::detect()), shell: Shell::detect(), now_secs, home, reduce_motion: reduce_motion(), wake });
    #[cfg(target_os = "linux")]
    {
        use mortar_files::wl::Client;
        struct W<'a>(&'a mut App);
        impl Client for W<'_> {
            fn tick(&mut self, now: u64) {
                self.0.tick(now)
            }
            fn next_deadline(&self) -> Option<u64> {
                self.0.next_deadline()
            }
            fn done(&self) -> bool {
                self.0.done
            }
            fn poll(&mut self) {
                self.0.poll();
            }
            fn focus(&mut self, on: bool) {
                self.0.focus(on)
            }
            fn resized(&mut self, h: i32) {
                self.0.resized(h)
            }
            fn repeats(&self, code: u16) -> bool {
                self.0.repeats(code)
            }
            fn key(&mut self, code: u16, down: bool, repeat: bool) {
                self.0.key(code, down, repeat)
            }
            fn wants_frame(&self) -> bool {
                self.0.dirty || self.0.animating()
            }
            fn draw(&mut self, ui: &mut mortar_ui::gfx::Ui) {
                self.0.dirty = false;
                mortar_packages::view::draw(self.0, ui);
            }
        }
        mortar_files::wl::run(&mut W(&mut app), wake_fd, "Packages", "mortar-packages")
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
        Some("fake-service") => {
            let get = |f: &str| args.iter().position(|a| a == f).and_then(|i| args.get(i + 1)).map(PathBuf::from);
            match (get("--socket"), get("--log")) {
                (Some(s), Some(l)) => demo::serve(&s, &l),
                _ => usage(),
            }
        }
        Some(_) => usage(),
    };
    if let Err(e) = r {
        eprintln!("mortar-packages: {e}");
        std::process::exit(1);
    }
}
