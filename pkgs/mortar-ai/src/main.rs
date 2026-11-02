// SPDX-License-Identifier: GPL-2.0-or-later
//! mortar-ai: a controller-friendly Claude Code front end for the Mortar
//! shell. It drives `claude -p` over stream-json, streams the reply, and
//! turns tool-permission requests into a card: A allows, B denies, X allows
//! always. Prompts are typed with the shell's on-screen keyboard (START) or
//! a real keyboard. See DESIGN.md.
//!
//!   mortar-ai run [--cwd DIR] [--model M] [--permission-mode M] [--continue] [-- CLAUDE ARGS]
//!   mortar-ai ask [same options] PROMPT    (plain terminal, y/a/N on stdin)
//!   mortar-ai pair --url LOGIN_URL          (QR + phone page; prints the code)
//!   mortar-ai preview --screen NAME --out FILE.png [--size WxH] [--scroll N]

#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

mod attention;
mod chat;
mod cli;
mod keys;
mod model;
mod pair;
mod preview;
mod proto;
mod screen;
mod view;
#[cfg(target_os = "linux")]
mod wl;

use std::path::PathBuf;
use std::process::exit;

fn usage() -> ! {
    eprintln!(
        "usage:\n  mortar-ai run [--cwd DIR] [--model M] [--permission-mode M] [--continue] [-- CLAUDE ARGS]\n  mortar-ai ask [same options] PROMPT\n  mortar-ai pair --url LOGIN_URL\n  mortar-ai preview --screen ({}) --out FILE.png [--size WxH] [--scroll N]",
        preview::SCREENS.join("|")
    );
    exit(2)
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        args.push("run".into());
    }
    let cmd = args.remove(0);
    let mut it = args.into_iter();
    match cmd.as_str() {
        "run" | "ask" => {
            let mut s = proto::Spawn::default();
            let mut prompt = Vec::new();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--cwd" => s.cwd = Some(PathBuf::from(it.next().unwrap_or_else(|| usage()))),
                    "--model" => s.model = it.next(),
                    "--permission-mode" => s.permission_mode = it.next(),
                    "--continue" => s.extra.push("--continue".into()),
                    "--" => s.extra.extend(it.by_ref()),
                    p if cmd == "ask" && !p.starts_with("--") => prompt.push(a.clone()),
                    _ => usage(),
                }
            }
            if let Some(d) = &s.cwd {
                let _ = std::fs::create_dir_all(d);
            }
            if cmd == "ask" {
                if prompt.is_empty() {
                    usage();
                }
                if let Err(e) = cli::ask(s, &prompt.join(" ")) {
                    eprintln!("mortar-ai: {e}");
                    exit(1);
                }
                return;
            }
            #[cfg(target_os = "linux")]
            if let Err(e) = wl::run(&mut chat::Chat::new(s), "mortar-ai") {
                eprintln!("mortar-ai: {e}");
                exit(1);
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = s;
                eprintln!("mortar-ai run needs Wayland (Linux); use `preview` here");
                exit(1);
            }
        }
        "pair" => {
            let mut url = None;
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--url" => url = it.next(),
                    _ => usage(),
                }
            }
            let Some(url) = url else { usage() };
            #[cfg(target_os = "linux")]
            {
                let mut p = pair::Pair::new(&url, true);
                if let Err(e) = wl::run(&mut p, "mortar-ai-pair") {
                    eprintln!("mortar-ai: {e}");
                    exit(1);
                }
                match &p.code {
                    Some(c) => println!("{c}"),
                    None => exit(1),
                }
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = url;
                eprintln!("mortar-ai pair needs Wayland (Linux); use `preview --screen pair` here");
                exit(1);
            }
        }
        "preview" => {
            let (mut screen, mut out, mut size, mut scroll) = (None, None, (1024, 768), 0);
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--screen" => screen = it.next(),
                    "--out" => out = it.next().map(PathBuf::from),
                    "--size" => {
                        let v = it.next().unwrap_or_else(|| usage());
                        let (w, h) = v.split_once('x').unwrap_or_else(|| usage());
                        size = (w.parse().unwrap_or_else(|_| usage()), h.parse().unwrap_or_else(|_| usage()));
                    }
                    "--scroll" => scroll = it.next().and_then(|v| v.parse().ok()).unwrap_or_else(|| usage()),
                    _ => usage(),
                }
            }
            let (Some(screen), Some(out)) = (screen, out) else { usage() };
            if let Err(e) = preview::run(&screen, &out, size, scroll) {
                eprintln!("mortar-ai: {e}");
                exit(1);
            }
        }
        _ => usage(),
    }
}
