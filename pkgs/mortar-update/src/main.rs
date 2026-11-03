// SPDX-License-Identifier: GPL-2.0-or-later
//! mortar-update: over-the-air updates on the Brick.
//!
//!   check     read the channel manifest, download the system closure from
//!             the bucket (`nix copy --from s3://…`, signatures checked by
//!             Nix), make it the next boot entry, tell the shell
//!   init      boot: take credentials from the card's mortar.toml, write
//!             the first status
//!   provision credentials from stdin (`mortar update-creds`)
//!   auto      Auto-check daily on/off
//!   rollback  make the previous generation the next boot entry
//!   status    print the status
//!
//! Format and setup: docs/updates.md.

mod config;
mod creds;
mod fsutil;
mod power;
mod status;

use config::Config;
use mortar_update::{manifest, Bucket, Credentials, FetchError, Manifest};
use status::{plan, Phase, Plan, State, Status};
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

const USAGE: &str = "\
usage: mortar-update [--config FILE] <command>

  check [--auto] [--force]   check for an update; download and stage it
                             (--auto: only if Auto-check daily is on;
                             --force: also take a manifest that is not newer
                             than the last one taken)
  status [--json]            what the last check found
  auto on|off                Auto-check daily
  provision --stdin          store bucket credentials read from stdin
                             (access_key_id = \"…\" / secret_access_key = \"…\")
  init                       boot: take [update] credentials from the card's
                             mortar.toml, write the first status
  rollback                   make the previous system the next boot entry

config: /etc/mortar-update/config.json (modules/core/update.nix)";

struct Ctx {
    cfg: Config,
    state: State,
}

impl Ctx {
    fn auto_check(&self) -> bool {
        self.state.auto_check.unwrap_or(self.cfg.auto_check)
    }
    fn status(&self, phase: Phase, msg: impl Into<String>) -> Status {
        let mut s = Status::new(phase, msg, self.auto_check());
        s.checked_at = self.state.last_check;
        s.running = resolve(&self.cfg.current_system);
        s
    }
    fn write(&self, s: &Status) {
        s.save(&self.cfg.status_file());
    }
    fn save_state(&self) {
        if let Err(e) = self.state.save(&self.cfg.state_file()) {
            eprintln!("mortar-update: {}: {e}", self.cfg.state_file().display());
        }
    }
}

/// Where a profile or /run/current-system points (the store path).
fn resolve(p: &Path) -> Option<String> {
    fs::canonicalize(p).ok().map(|p| p.to_string_lossy().into_owned())
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut cfg_path = PathBuf::from(config::DEFAULT_PATH);
    if args.first().map(String::as_str) == Some("--config") {
        if args.len() < 2 {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
        cfg_path = args[1].clone().into();
        args.drain(..2);
    }
    let Some(cmd) = args.first().cloned() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    if matches!(cmd.as_str(), "-h" | "--help" | "help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let cfg = match Config::load(&cfg_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("mortar-update: {e}");
            return ExitCode::FAILURE;
        }
    };
    let state = State::load(&cfg.state_file());
    let mut cx = Ctx { cfg, state };
    let rest = &args[1..];
    let r = match cmd.as_str() {
        "check" => check(&mut cx, rest.iter().any(|a| a == "--auto"), rest.iter().any(|a| a == "--force")),
        "status" => show_status(&cx, rest.iter().any(|a| a == "--json")),
        "auto" => match rest.first().map(String::as_str) {
            Some("on") => set_auto(&mut cx, true),
            Some("off") => set_auto(&mut cx, false),
            _ => Err("auto on|off".into()),
        },
        "provision" if rest.iter().any(|a| a == "--stdin") => provision_stdin(&mut cx),
        "init" => init(&mut cx),
        "rollback" => rollback(&mut cx),
        other => Err(format!("unknown command '{other}'\n{USAGE}")),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("mortar-update: {e}");
            ExitCode::FAILURE
        }
    }
}

// ------------------------------------------------------------- commands ----

fn show_status(cx: &Ctx, json: bool) -> Result<(), String> {
    let s = Status::load(&cx.cfg.status_file()).unwrap_or_else(|| cx.status(Phase::Idle, "Not checked yet"));
    if json {
        println!("{}", serde_json::to_string_pretty(&s).unwrap());
        return Ok(());
    }
    println!("{}{}", s.message, if s.detail.is_empty() { String::new() } else { format!(" ({})", s.detail) });
    if let Some(m) = &s.available {
        println!("update:  {} ({})", m.short(), m.toplevel);
        for c in m.changelog.iter().take(10) {
            println!("  - {c}");
        }
    }
    println!("running: {}", s.running.as_deref().unwrap_or("?"));
    println!("auto-check daily: {}", if s.auto_check { "on" } else { "off" });
    if let Some(t) = s.checked_at {
        println!("last check: {}", mortar_update::time::rfc3339(t));
    }
    Ok(())
}

fn set_auto(cx: &mut Ctx, on: bool) -> Result<(), String> {
    cx.state.auto_check = Some(on);
    cx.state.save(&cx.cfg.state_file()).map_err(|e| e.to_string())?;
    let mut s = Status::load(&cx.cfg.status_file()).unwrap_or_else(|| cx.status(Phase::Idle, "Check now"));
    s.auto_check = on;
    cx.write(&s);
    println!("auto-check daily: {}", if on { "on" } else { "off" });
    Ok(())
}

/// Boot: credentials from the card, then the first status.
fn init(cx: &mut Ctx) -> Result<(), String> {
    let card = cx.cfg.card_file.clone();
    if let Ok(text) = fs::read_to_string(&card) {
        match creds::parse_card(&text) {
            Ok(u) => {
                for w in &u.warnings {
                    eprintln!("mortar-update: {}: {w}", card.display());
                }
                if let Some(c) = &u.credentials {
                    creds::save(&cx.cfg.credentials_file(), c).map_err(|e| format!("{}: {e}", cx.cfg.credentials_file().display()))?;
                    eprintln!("mortar-update: stored update credentials from {} (key {})", card.display(), c.access_key_id);
                    if let Some(scrubbed) = creds::scrub_card(&text)
                        && let Err(e) = fsutil::write_atomic(&card, scrubbed.as_bytes(), 0o600)
                    {
                        eprintln!("mortar-update: could not remove the secret from {}: {e}", card.display());
                    }
                }
                if let Some(a) = u.auto_check
                    && cx.state.auto_check != Some(a)
                {
                    cx.state.auto_check = Some(a);
                    cx.save_state();
                }
            }
            Err(e) => eprintln!("mortar-update: {}: {}", card.display(), e.lines().next().unwrap_or(&e)),
        }
    }
    let s = if creds::load(&cx.cfg.credentials_file()).is_some() {
        cx.status(Phase::Idle, "Check now")
    } else {
        cx.status(Phase::Unconfigured, "Not set up").detail("no bucket credentials (docs/updates.md)")
    };
    cx.write(&s);
    Ok(())
}

fn provision_stdin(cx: &mut Ctx) -> Result<(), String> {
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text).map_err(|e| e.to_string())?;
    let c = creds::parse(&text)?;
    creds::save(&cx.cfg.credentials_file(), &c).map_err(|e| e.to_string())?;
    println!("stored credentials (key {}) in {}", c.access_key_id, cx.cfg.credentials_file().display());
    // try them
    let b = cx.cfg.bucket()?;
    match b.get(&c, &manifest::key(&cx.cfg.channel), &cx.cfg.curl) {
        Ok(bytes) => match Manifest::parse(&bytes) {
            Ok(m) => println!("bucket ok: {} has {}", manifest::key(&cx.cfg.channel), m.short()),
            Err(e) => println!("bucket ok, but {e}"),
        },
        Err(FetchError::NotFound) => println!("bucket ok (nothing published on channel '{}' yet)", cx.cfg.channel),
        Err(e) => return Err(format!("stored, but the bucket said: {e}")),
    }
    cx.write(&cx.status(Phase::Idle, "Check now"));
    Ok(())
}

fn rollback(cx: &mut Ctx) -> Result<(), String> {
    let p = cx.cfg.profile.to_string_lossy().into_owned();
    run(Command::new(&cx.cfg.nix_env).args(["-p", &p, "--rollback"]), "nix-env --rollback")?;
    let target = resolve(&cx.cfg.profile).ok_or("profile vanished")?;
    run(Command::new(format!("{target}/bin/switch-to-configuration")).arg("boot"), "switch-to-configuration boot")?;
    println!("next boot: {target}");
    let mut s = cx.status(Phase::Ready, "Restart to roll back");
    s.detail = "the previous system is the next boot entry".into();
    cx.write(&s);
    Ok(())
}

// ---------------------------------------------------------------- check ----

/// One check at a time (the timer, the shell's Check now, a person on SSH).
fn lock(cfg: &Config) -> Result<fs::File, String> {
    let p = cfg.lock_file();
    if let Some(d) = p.parent() {
        let _ = fs::create_dir_all(d);
    }
    let f = fs::OpenOptions::new().create(true).truncate(false).write(true).open(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    match f.try_lock() {
        Ok(()) => Ok(f),
        Err(fs::TryLockError::WouldBlock) => Err("busy".into()),
        Err(fs::TryLockError::Error(e)) => Err(format!("{}: {e}", p.display())),
    }
}

fn check(cx: &mut Ctx, auto: bool, force: bool) -> Result<(), String> {
    if auto && !cx.auto_check() {
        eprintln!("mortar-update: auto-check is off");
        return Ok(());
    }
    let _lock = match lock(&cx.cfg) {
        Ok(l) => l,
        Err(e) if e == "busy" => {
            println!("a check is already running");
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    let s = do_check(cx, force);
    cx.write(&s);
    println!("{}{}", s.message, if s.detail.is_empty() { String::new() } else { format!(": {}", s.detail) });
    match s.state {
        Phase::Error | Phase::Unconfigured => Err(s.message),
        _ => Ok(()),
    }
}

fn do_check(cx: &mut Ctx, force: bool) -> Status {
    let Some(creds) = creds::load(&cx.cfg.credentials_file()) else {
        return cx.status(Phase::Unconfigured, "Not set up").detail("no bucket credentials (docs/updates.md)");
    };
    let bucket = match cx.cfg.bucket() {
        Ok(b) => b,
        Err(e) => return cx.status(Phase::Error, "Bad configuration").detail(e),
    };
    cx.write(&cx.status(Phase::Checking, "Checking…"));

    let key = manifest::key(&cx.cfg.channel);
    let m = match bucket.get(&creds, &key, &cx.cfg.curl) {
        Ok(b) => match Manifest::parse(&b) {
            Ok(m) => m,
            Err(e) => return cx.status(Phase::Error, "Bad update manifest").detail(e),
        },
        Err(FetchError::Offline(e)) => return cx.status(Phase::Offline, "Offline").detail(e),
        Err(FetchError::Denied) => return cx.status(Phase::Error, "Access denied").detail("the bucket refused the credentials"),
        Err(FetchError::NotFound) => {
            cx.state.last_check = Some(mortar_update::time::now());
            cx.save_state();
            return cx.status(Phase::UpToDate, "Up to date").detail(format!("nothing published on '{}' yet", cx.cfg.channel));
        }
        Err(FetchError::Other(e)) => return cx.status(Phase::Error, "Check failed").detail(e),
    };
    cx.state.last_check = Some(mortar_update::time::now());
    cx.save_state();
    if m.system != cx.cfg.system {
        return cx.status(Phase::Error, "Wrong system").detail(format!("the manifest is for {}", m.system));
    }

    let running = resolve(&cx.cfg.current_system);
    let staged = resolve(&cx.cfg.profile);
    match plan(&m, running.as_deref(), staged.as_deref(), &cx.state, force) {
        Plan::Running | Plan::NotNewer => {
            let mut s = cx.status(Phase::UpToDate, "Up to date");
            s.detail = format!("latest: {}", m.short());
            return s;
        }
        Plan::Staged => return ready(cx, m),
        Plan::Update => {}
    }

    let pw = power::read_power(&cx.cfg.power_supply_dir);
    if !pw.ok_for_update(cx.cfg.min_battery) {
        let mut s = cx.status(Phase::LowBattery, format!("Charge to {}% first", cx.cfg.min_battery));
        s.detail = format!("battery {}%, not charging; update {} waits", pw.capacity.unwrap_or(0), m.short());
        s.available = Some(m);
        return s;
    }

    if let Err(e) = ensure_space(&cx.cfg) {
        return cx.status(Phase::Error, "Not enough space").detail(e);
    }

    let mut s = cx.status(Phase::Downloading, "Downloading…");
    s.detail = m.short();
    s.available = Some(m.clone());
    cx.write(&s);
    if let Err((offline, e)) = download(&cx.cfg, &bucket, &creds, &m.toplevel) {
        let (phase, msg) = if offline { (Phase::Offline, "Download paused") } else { (Phase::Error, "Download failed") };
        let mut s = cx.status(phase, msg).detail(e);
        s.available = Some(m);
        return s;
    }

    let mut s = cx.status(Phase::Installing, "Installing…");
    s.available = Some(m.clone());
    cx.write(&s);
    if let Err(e) = stage(&cx.cfg, &m.toplevel) {
        let mut s = cx.status(Phase::Error, "Install failed").detail(e);
        s.available = Some(m);
        return s;
    }
    cx.state.last_published = m.published;
    cx.state.last_toplevel = Some(m.toplevel.clone());
    cx.save_state();
    ready(cx, m)
}

fn ready(cx: &Ctx, m: Manifest) -> Status {
    let mut s = cx.status(Phase::Ready, "Restart to update");
    s.detail = m.changelog.first().cloned().unwrap_or_else(|| m.short());
    s.available = Some(m);
    s
}

fn ensure_space(cfg: &Config) -> Result<(), String> {
    let want = cfg.min_free_mib * 1024 * 1024;
    let store = Path::new("/nix/store");
    let free = || fsutil::free_bytes(store).unwrap_or(u64::MAX);
    if free() >= want {
        return Ok(());
    }
    eprintln!("mortar-update: {} MiB free, collecting garbage", free() / 1048576);
    let _ = Command::new(&cfg.nix_collect_garbage).status();
    if free() >= want {
        Ok(())
    } else {
        Err(format!("{} MiB free in /nix/store, want {}", free() / 1048576, cfg.min_free_mib))
    }
}

/// `nix copy --from s3://…` into the local store. Every store path arrives
/// whole or not at all, so an interrupted download resumes where it
/// stopped. Err((offline, message)).
fn download(cfg: &Config, bucket: &Bucket, creds: &Credentials, toplevel: &str) -> Result<(), (bool, String)> {
    let url = bucket.store_url(&[]);
    eprintln!("mortar-update: nix copy --from {url} {toplevel}");
    let mut c = Command::new(&cfg.nix);
    c.args(["--extra-experimental-features", "nix-command", "copy", "--from", &url])
        .args(["--option", "connect-timeout", "20", "--option", "stalled-download-timeout", "120"])
        .args(["--option", "download-attempts", "3"])
        .arg(toplevel)
        .envs(creds.env())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = c.spawn().map_err(|e| (false, format!("{}: {e}", cfg.nix)))?;
    // pass the log through to the journal, keep the tail for the status
    let mut tail: Vec<String> = Vec::new();
    for line in BufReader::new(child.stderr.take().expect("piped")).lines().map_while(Result::ok) {
        eprintln!("{line}");
        tail.push(line);
        if tail.len() > 20 {
            tail.remove(0);
        }
    }
    let ok = child.wait().map(|s| s.success()).unwrap_or(false);
    if ok && Path::new(toplevel).join("bin/switch-to-configuration").exists() {
        return Ok(());
    }
    if ok {
        return Err((false, "the download is not a NixOS system".into()));
    }
    let all = tail.join("\n");
    let err = tail.iter().rev().find(|l| l.contains("error")).cloned().unwrap_or_else(|| tail.last().cloned().unwrap_or_default());
    let low = all.to_ascii_lowercase();
    if low.contains("signature") {
        return Err((false, "not signed by a trusted key (nix.settings.trusted-public-keys)".into()));
    }
    let offline = ["could not resolve", "couldn't resolve", "couldn't connect", "could not connect", "timeout", "timed out", "network is unreachable", "connection reset"]
        .iter()
        .any(|k| low.contains(k));
    Err((offline, err.trim().trim_start_matches("error:").trim().to_string()))
}

/// Make `toplevel` the next boot entry, keeping the older generations
/// (pruned to keep_generations) for rollback.
fn stage(cfg: &Config, toplevel: &str) -> Result<(), String> {
    let p = cfg.profile.to_string_lossy().into_owned();
    let before = resolve(&cfg.profile);
    run(Command::new(&cfg.nix_env).args(["-p", &p, "--set", toplevel]), "nix-env --set")?;
    if cfg.keep_generations > 0 {
        let keep = format!("+{}", cfg.keep_generations);
        if let Err(e) = run(Command::new(&cfg.nix_env).args(["-p", &p, "--delete-generations", &keep]), "nix-env --delete-generations") {
            eprintln!("mortar-update: {e}");
        }
    }
    if let Err(e) = run(Command::new(format!("{toplevel}/bin/switch-to-configuration")).arg("boot"), "switch-to-configuration boot") {
        // undo: the previous generation is the boot entry again
        eprintln!("mortar-update: {e}; rolling back the profile");
        let new_link = fs::read_link(&cfg.profile).ok();
        let _ = run(Command::new(&cfg.nix_env).args(["-p", &p, "--rollback"]), "nix-env --rollback");
        if let Some(n) = new_link.as_ref().and_then(|l| generation_number(&l.to_string_lossy())) {
            let _ = run(Command::new(&cfg.nix_env).args(["-p", &p, "--delete-generations", &n.to_string()]), "nix-env --delete-generations");
        }
        if let Some(b) = before {
            let _ = run(Command::new(format!("{b}/bin/switch-to-configuration")).arg("boot"), "switch-to-configuration boot");
        }
        return Err(e);
    }
    Ok(())
}

/// "system-42-link" -> 42
fn generation_number(link: &str) -> Option<u32> {
    let name = link.rsplit('/').next()?;
    name.strip_suffix("-link")?.rsplit('-').next()?.parse().ok()
}

fn run(c: &mut Command, what: &str) -> Result<(), String> {
    let out = c.stdout(Stdio::inherit()).stderr(Stdio::piped()).output().map_err(|e| format!("{what}: {e}"))?;
    let err = String::from_utf8_lossy(&out.stderr);
    if !err.trim().is_empty() {
        eprint!("{err}");
    }
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("{what} failed: {}", err.lines().last().unwrap_or("").trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generations() {
        assert_eq!(generation_number("system-42-link"), Some(42));
        assert_eq!(generation_number("/nix/var/nix/profiles/system-7-link"), Some(7));
        assert_eq!(generation_number("/nix/store/x"), None);
    }
}
