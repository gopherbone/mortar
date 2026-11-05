// SPDX-License-Identifier: GPL-2.0-or-later
//! The Nix commands, each with the source's URLs and keys on its command line, as
//! mortar-update's. Nothing else trusts what nix.conf says here.

use crate::protocol::Size;
use mortar_index::nixargs;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};

/// `nix-store --realise --dry-run`'s report, on stderr.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DryRun {
    Fetch(Size),
    /// No cache that's allowed has it, or its signature isn't one of the keys.
    Unavailable(String),
}

fn bytes(n: &str, unit: &str) -> Option<u64> {
    let v: f64 = n.parse().ok()?;
    let m = match unit {
        "bytes" | "B" => 1.0,
        "KiB" => 1024.0,
        "MiB" => 1048576.0,
        "GiB" => 1073741824.0,
        "TiB" => 1099511627776.0,
        _ => return None,
    };
    Some((v * m) as u64)
}

/// Read what a dry run said. It says nothing when all of it is here.
pub fn parse_dry_run(text: &str) -> DryRun {
    let mut size = Size::default();
    for l in text.lines() {
        let low = l.to_ascii_lowercase();
        if low.contains("don't know how to build") || low.contains("will be built") {
            return DryRun::Unavailable("It isn't on the cache, or isn't signed by the key it should be.".into());
        }
        if let Some(i) = l.find("will be fetched (") {
            // "these 3 paths will be fetched (0.05 MiB download, 0.20 MiB unpacked):"
            size.paths = l.split_whitespace().nth(1).and_then(|n| n.parse().ok()).unwrap_or(1);
            let inner = &l[i + "will be fetched (".len()..];
            let inner = inner.split(')').next().unwrap_or("");
            for part in inner.split(',') {
                let w: Vec<&str> = part.split_whitespace().collect();
                if let [n, unit, what] = w[..]
                    && let Some(b) = bytes(n, unit)
                {
                    match what {
                        "download" => size.download = b,
                        "unpacked" => size.unpacked = b,
                        _ => {}
                    }
                }
            }
        }
    }
    DryRun::Fetch(size)
}

pub fn dry_run(nix_store: &str, path: &str, subs: &[String], keys: &[String]) -> Result<DryRun, (bool, String)> {
    let out = Command::new(nix_store)
        .args(["--realise", "--dry-run", path])
        .args(nixargs::realise_options(subs, keys))
        .stdin(Stdio::null())
        .output()
        .map_err(|e| (false, format!("{nix_store}: {e}")))?;
    let text = String::from_utf8_lossy(&out.stderr);
    let parsed = parse_dry_run(&text);
    if out.status.success() || matches!(parsed, DryRun::Unavailable(_)) {
        return Ok(parsed);
    }
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    Err(nixargs::classify_failure(&lines))
}

/// What `--log-format internal-json` says about bytes unpacked: the copy-path
/// activities' progress, added up.
#[derive(Default)]
pub struct Progress {
    copies: HashMap<u64, u64>,
}

const ACT_COPY_PATH: u64 = 100;
const RES_PROGRESS: u64 = 105;

impl Progress {
    /// One `@nix {…}` line. The bytes done so far, if that changed.
    pub fn line(&mut self, json: &str) -> Option<u64> {
        let v: serde_json::Value = serde_json::from_str(json).ok()?;
        match v.get("action")?.as_str()? {
            "start" if v.get("type")?.as_u64()? == ACT_COPY_PATH => {
                self.copies.insert(v.get("id")?.as_u64()?, 0);
                None
            }
            "result" if v.get("type")?.as_u64()? == RES_PROGRESS => {
                let done = self.copies.get_mut(&v.get("id")?.as_u64()?)?;
                let n = v.get("fields")?.get(0)?.as_u64()?;
                if n == *done {
                    return None;
                }
                *done = n;
                Some(self.copies.values().sum())
            }
            _ => None,
        }
    }
}

/// Download a path and all it needs. `running` holds the process's id while it runs, so
/// a cancel can reach it. Every path arrives whole or not at all, so a stopped download
/// picks up where it was.
pub fn realise(nix_store: &str, path: &str, root: &Path, subs: &[String], keys: &[String], running: &AtomicI32, mut on_bytes: impl FnMut(u64)) -> Result<(), (bool, String)> {
    let mut c = Command::new(nix_store);
    c.args(["--realise", path, "--add-root"]).arg(root).args(nixargs::realise_options(subs, keys)).args(["--log-format", "internal-json"]);
    c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = c.spawn().map_err(|e| (false, format!("{nix_store}: {e}")))?;
    running.store(child.id() as i32, Ordering::SeqCst);
    let mut tail: Vec<String> = Vec::new();
    let mut progress = Progress::default();
    for line in BufReader::new(child.stderr.take().expect("piped")).lines().map_while(Result::ok) {
        let Some(json) = line.strip_prefix("@nix ") else {
            remember(&mut tail, line);
            continue;
        };
        if let Some(n) = progress.line(json) {
            on_bytes(n);
        } else if let Some(m) = message(json) {
            remember(&mut tail, m);
        }
    }
    let ok = child.wait().map(|s| s.success()).unwrap_or(false);
    running.store(0, Ordering::SeqCst);
    if ok { Ok(()) } else { Err(nixargs::classify_failure(&tail)) }
}

fn remember(tail: &mut Vec<String>, line: String) {
    tail.push(line);
    if tail.len() > 20 {
        tail.remove(0);
    }
}

fn message(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    if v.get("action")?.as_str()? != "msg" {
        return None;
    }
    // what stands out in a terminal has colours in it
    Some(strip_ansi(v.get("msg")?.as_str()?))
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\u{1b}' {
            for c in it.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn run(c: &mut Command, what: &str) -> Result<(), String> {
    let out = c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output().map_err(|e| format!("{what}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    Err(format!("{what} failed: {}", err.lines().last().unwrap_or("").trim()))
}

/// The last, atomic step: the profile points at this path, or it doesn't.
pub fn set_profile(nix_env: &str, profile: &Path, path: &str) -> Result<(), String> {
    run(Command::new(nix_env).arg("-p").arg(profile).args(["--set", path]), "nix-env --set")
}

/// Keep this generation and the one before it.
pub fn prune(nix_env: &str, profile: &Path) -> Result<(), String> {
    run(Command::new(nix_env).arg("-p").arg(profile).args(["--delete-generations", "+2"]), "nix-env --delete-generations")
}

/// `<profile>-<N>-link`, oldest first.
pub fn generations(profile: &Path) -> Vec<(u32, PathBuf)> {
    let Some(name) = profile.file_name().and_then(|n| n.to_str()) else { return vec![] };
    let Some(dir) = profile.parent() else { return vec![] };
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let prefix = format!("{name}-");
    let mut out: Vec<(u32, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().into_string().ok()?;
            let num = n.strip_prefix(&prefix)?.strip_suffix("-link")?.parse().ok()?;
            Some((num, e.path()))
        })
        .collect();
    out.sort();
    out
}

/// Back one generation, and forget the one left: another update's previous version is
/// what's running now, not what was rolled back from.
pub fn rollback(nix_env: &str, profile: &Path) -> Result<(), String> {
    let current = std::fs::read_link(profile).map_err(|e| format!("{}: {e}", profile.display()))?;
    run(Command::new(nix_env).arg("-p").arg(profile).arg("--rollback"), "nix-env --rollback")?;
    let name = current.file_name().map(PathBuf::from).unwrap_or(current);
    if let Some(dir) = profile.parent() {
        let _ = std::fs::remove_file(dir.join(name));
    }
    Ok(())
}

/// The profile and its generations. The profile goes first, so an app leaves the shell
/// whole. The data was never in here.
pub fn remove_profile(profile: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(profile) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    for (_, g) in generations(profile) {
        std::fs::remove_file(g)?;
    }
    Ok(())
}

pub fn resolve(profile: &Path) -> Option<String> {
    std::fs::canonicalize(profile).ok().map(|p| p.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_a_dry_run_says() {
        let s = "these 16 paths will be fetched (52.31 MiB download, 209.00 MiB unpacked):\n  /nix/store/a\n";
        assert_eq!(parse_dry_run(s), DryRun::Fetch(Size { download: 54851010, unpacked: 219152384, paths: 16 }));
        let one = "this path will be fetched (84.5 KiB download, 3 MiB unpacked):\n";
        assert_eq!(parse_dry_run(one), DryRun::Fetch(Size { download: 86528, unpacked: 3145728, paths: 1 }));
        assert_eq!(parse_dry_run(""), DryRun::Fetch(Size::default()));
        assert!(matches!(parse_dry_run("don't know how to build these paths:\n  /nix/store/a"), DryRun::Unavailable(_)));
        assert!(matches!(parse_dry_run("these 2 derivations will be built:\n"), DryRun::Unavailable(_)));
    }

    #[test]
    fn adds_up_bytes_as_nix_reports_them() {
        let mut p = Progress::default();
        assert_eq!(p.line(r#"{"action":"start","id":7,"type":100,"text":"copying"}"#), None);
        assert_eq!(p.line(r#"{"action":"start","id":8,"type":100,"text":"copying"}"#), None);
        assert_eq!(p.line(r#"{"action":"start","id":9,"type":101,"text":"downloading"}"#), None);
        assert_eq!(p.line(r#"{"action":"result","id":7,"type":105,"fields":[1000,5000,0,0]}"#), Some(1000));
        assert_eq!(p.line(r#"{"action":"result","id":8,"type":105,"fields":[500,500,0,0]}"#), Some(1500));
        assert_eq!(p.line(r#"{"action":"result","id":7,"type":105,"fields":[1000,5000,0,0]}"#), None);
        assert_eq!(p.line(r#"{"action":"result","id":9,"type":105,"fields":[99999,99999,0,0]}"#), None);
        assert_eq!(p.line("not json"), None);
    }

    #[test]
    fn messages_lose_their_colours() {
        let m = message("{\"action\":\"msg\",\"level\":0,\"msg\":\"\\u001b[31;1merror:\\u001b[0m it broke\"}");
        assert_eq!(m.as_deref(), Some("error: it broke"));
    }

    #[test]
    fn generations_in_order_and_gone_with_the_profile() {
        let d = std::env::temp_dir().join(format!("mortar-packages-gens-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("app");
        for n in ["app-10-link", "app-2-link", "app-other-link", "app2-3-link"] {
            std::os::unix::fs::symlink("/nowhere", d.join(n)).unwrap();
        }
        std::os::unix::fs::symlink("app-10-link", &p).unwrap();
        assert_eq!(generations(&p).iter().map(|g| g.0).collect::<Vec<_>>(), [2, 10]);
        remove_profile(&p).unwrap();
        let mut left: Vec<String> = std::fs::read_dir(&d).unwrap().flatten().map(|e| e.file_name().into_string().unwrap()).collect();
        left.sort();
        assert_eq!(left, ["app-other-link", "app2-3-link"]);
        remove_profile(&p).unwrap();
        std::fs::remove_dir_all(&d).unwrap();
    }
}
