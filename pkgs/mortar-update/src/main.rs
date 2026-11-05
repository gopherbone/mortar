// SPDX-License-Identifier: GPL-2.0-or-later
//! mortar-update: over-the-air updates on the Brick.
//!
//!   check     read the channel manifest and check its signature, channel,
//!             configuration and age, download the system closure (HTTPS mode:
//!             `nix-store --realise`, from the public cache and its upstreams;
//!             S3 mode: `nix copy --from s3://…`; signatures checked by Nix),
//!             check nothing in it came from elsewhere (guard.rs),
//!             make it the next boot entry, tell the shell
//!   init      boot: take settings and any credentials from the card's
//!             mortar.toml, write the first status
//!   provision S3 mode: credentials from stdin (`mortar update-creds`)
//!   auto      Auto-check daily on/off
//!   rollback  make the previous generation the next boot entry
//!   status    print the status
//!
//! Format and setup: docs/updates.md.

mod config;
mod creds;
mod guard;
mod status;

use config::{Config, Source};
use mortar_index::{fsutil, nixargs, power};
use mortar_update::{Credentials, FetchError, Manifest};
use status::{plan, Allow, Phase, Plan, State, Status};
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

const USAGE: &str = "\
usage: mortar-update [--config FILE] <command>

  check [--auto] [--force] [--allow-downgrade] [--allow-config-change] [--allow-foreign-paths]
                             check for an update; download and stage it
                             (--auto: only if Auto-check daily is on;
                             --force: take the manifest last taken again;
                             --allow-downgrade: also one older than that;
                             --allow-config-change: also one of another
                             configuration, e.g. bare to full;
                             --allow-foreign-paths: even if the store holds some
                             of its paths signed only by someone else)
  status [--json]            what the last check found
  auto on|off                Auto-check daily
  provision --stdin          S3 mode: store bucket credentials read from stdin
                             (access_key_id = \"…\" / secret_access_key = \"…\")
  init                       boot: take the [update] section of the card's
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
        "check" => {
            let has = |f: &str| rest.iter().any(|a| a == f);
            let allow = Allow { same: has("--force"), downgrade: has("--allow-downgrade"), config_change: has("--allow-config-change"), foreign: has("--allow-foreign-paths") };
            check(&mut cx, has("--auto"), allow)
        }
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
    // HTTPS mode needs no credentials, so it is set up as it stands.
    let s = if !matches!(cx.cfg.source(), Ok(Source::S3(_))) || creds::load(&cx.cfg.credentials_file()).is_some() {
        cx.status(Phase::Idle, "Check now")
    } else {
        cx.status(Phase::Unconfigured, "Not set up").detail("no bucket credentials (docs/updates.md)")
    };
    cx.write(&s);
    Ok(())
}

fn provision_stdin(cx: &mut Ctx) -> Result<(), String> {
    let Source::S3(b) = cx.cfg.source()? else {
        return Err("this Brick reads a public cache and needs no credentials".into());
    };
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text).map_err(|e| e.to_string())?;
    let c = creds::parse(&text)?;
    creds::save(&cx.cfg.credentials_file(), &c).map_err(|e| e.to_string())?;
    println!("stored credentials (key {}) in {}", c.access_key_id, cx.cfg.credentials_file().display());
    // try them
    match b.get(&c, &cx.cfg.manifest_key(), &cx.cfg.curl) {
        Ok(bytes) => match Manifest::parse(&bytes).and_then(|m| m.verify(&cx.cfg.trusted_keys).map(|()| m)) {
            Ok(m) => println!("bucket ok: {} has {}", cx.cfg.manifest_key(), m.short()),
            Err(e) => println!("bucket ok, but {e}"),
        },
        Err(FetchError::NotFound) => println!("bucket ok (nothing published on channel '{}' yet)", cx.cfg.channel),
        Err(e) => return Err(format!("stored, but the bucket said: {e}")),
    }
    cx.write(&cx.status(Phase::Idle, "Check now"));
    Ok(())
}

fn rollback(cx: &mut Ctx) -> Result<(), String> {
    let r = do_rollback(cx);
    if let Err(e) = &r {
        cx.write(&cx.status(Phase::Error, "Could not roll back").detail(e.clone()));
    }
    r
}

fn do_rollback(cx: &mut Ctx) -> Result<(), String> {
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

fn check(cx: &mut Ctx, auto: bool, allow: Allow) -> Result<(), String> {
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
    let s = do_check(cx, allow);
    cx.write(&s);
    println!("{}{}", s.message, if s.detail.is_empty() { String::new() } else { format!(": {}", s.detail) });
    match s.state {
        Phase::Error | Phase::Unconfigured => Err(s.message),
        _ => Ok(()),
    }
}

/// What the signed manifest says against what this Brick is: the check before
/// anything is downloaded.
fn refuse(cfg: &Config, m: &Manifest, allow: Allow) -> Option<(&'static str, String)> {
    if let Err(e) = m.verify(&cfg.trusted_keys) {
        return Some(("Update not trusted", e));
    }
    if m.channel != cfg.channel {
        return Some(("Wrong channel", format!("the manifest is for '{}', this Brick follows '{}'", m.channel, cfg.channel)));
    }
    if m.system != cfg.system {
        return Some(("Wrong system", format!("the manifest is for {}", m.system)));
    }
    if !allow.config_change && cfg.config_name.as_deref() != Some(m.config.as_str()) {
        let mine = cfg.config_name.as_deref().unwrap_or("not set");
        return Some(("Different configuration", format!("the update is {}, this Brick is {mine} (--allow-config-change)", m.config)));
    }
    None
}

fn do_check(cx: &mut Ctx, allow: Allow) -> Status {
    let source = match cx.cfg.source() {
        Ok(s) => s,
        Err(e) => return cx.status(Phase::Error, "Bad configuration").detail(e),
    };
    let creds = creds::load(&cx.cfg.credentials_file());
    if matches!(source, Source::S3(_)) && creds.is_none() {
        return cx.status(Phase::Unconfigured, "Not set up").detail("no bucket credentials (docs/updates.md)");
    }
    cx.write(&cx.status(Phase::Checking, "Checking…"));

    let key = cx.cfg.manifest_key();
    let fetched = match &source {
        Source::Https(base) => mortar_update::get_public(&format!("{base}/{key}"), &cx.cfg.curl),
        Source::S3(bucket) => bucket.get(creds.as_ref().expect("checked above"), &key, &cx.cfg.curl),
    };
    let m = match fetched {
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
    if let Some((msg, why)) = refuse(&cx.cfg, &m, allow) {
        return cx.status(Phase::Error, msg).detail(why);
    }

    let running = resolve(&cx.cfg.current_system);
    let staged = resolve(&cx.cfg.profile);
    match plan(&m, running.as_deref(), staged.as_deref(), &cx.state, allow) {
        Plan::Running | Plan::Staged if m.published > cx.state.last_published => {
            // already here (deployed, or from before this was kept): the floor moves up
            cx.state.last_published = m.published;
            cx.state.last_toplevel = Some(m.toplevel.clone());
            cx.save_state();
            if running.as_deref() == Some(m.toplevel.as_str()) {
                let mut s = cx.status(Phase::UpToDate, "Up to date");
                s.detail = format!("latest: {}", m.short());
                return s;
            }
            return ready(cx, m);
        }
        Plan::Running | Plan::NotNewer => {
            let mut s = cx.status(Phase::UpToDate, "Up to date");
            s.detail = format!("latest: {}", m.short());
            return s;
        }
        Plan::Downgrade => {
            return cx.status(Phase::Error, "Update is older").detail(format!(
                "{} is older than the last update taken here (--allow-downgrade)",
                m.short()
            ));
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
    if let Err((offline, e)) = download(&cx.cfg, &source, creds.as_ref(), &m.toplevel) {
        let (phase, msg) = if offline { (Phase::Offline, "Download paused") } else { (Phase::Error, "Download failed") };
        let mut s = cx.status(phase, msg).detail(e);
        s.available = Some(m);
        return s;
    }

    // Every path of it is what was published, before it can run as root at boot.
    if !allow.foreign {
        let verdict = match guard::check(&cx.cfg, &source, creds.as_ref(), &m.toplevel) {
            Ok(v) => v,
            Err(e) => {
                let mut s = cx.status(Phase::Error, "Could not check the update").detail(e);
                s.available = Some(m);
                return s;
            }
        };
        if verdict.unsigned > 0 {
            eprintln!("mortar-update: {} paths in this system have no signature: from the card, a deploy or a local build", verdict.unsigned);
        }
        if !verdict.foreign.is_empty() {
            for p in &verdict.foreign {
                eprintln!("mortar-update: signed by someone else, and not what was published: {p}");
            }
            let mut s = cx.status(Phase::Error, "Update blocked").detail(guard::refusal_detail(&verdict.foreign));
            s.available = Some(m);
            return s;
        }
    }

    let mut s = cx.status(Phase::Installing, "Installing…");
    s.available = Some(m.clone());
    cx.write(&s);
    if let Err(e) = stage(&cx.cfg, &m.toplevel) {
        let mut s = cx.status(Phase::Error, "Install failed").detail(e);
        s.available = Some(m);
        return s;
    }
    // never lowered, also by an --allow-downgrade
    cx.state.last_published = cx.state.last_published.max(m.published);
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

/// How the closure reaches the local store. In both, only the keys named here
/// count, not what nix.conf trusts for other caches: a path signed by someone
/// else isn't part of a published system.
fn download_command(cfg: &Config, source: &Source, creds: Option<&Credentials>, toplevel: &str) -> Command {
    match source {
        // `nix copy --from` wants the whole closure in one store, which the
        // public cache doesn't have: substituting walks the closure and takes
        // each path from whichever cache holds it.
        Source::Https(base) => {
            let mut substituters = vec![base.clone()];
            substituters.extend(cfg.upstreams.iter().map(|u| u.url.clone()));
            let mut c = Command::new(&cfg.nix_store);
            c.args(["--realise", toplevel])
                .args(nixargs::realise_options(&substituters, &cfg.closure_keys()));
            c
        }
        Source::S3(bucket) => {
            let url = bucket.store_url(&[]);
            let mut c = Command::new(&cfg.nix);
            c.args(["--extra-experimental-features", "nix-command", "copy", "--from", &url])
                .args(["--option", "trusted-public-keys", &cfg.trusted_keys.join(" ")])
                .args(nixargs::timeouts())
                .arg(toplevel);
            if let Some(creds) = creds {
                c.envs(creds.env());
            }
            c
        }
    }
}

/// Download into the local store. Every store path arrives whole or not at all,
/// so an interrupted download resumes where it stopped. Err((offline, message)).
fn download(cfg: &Config, source: &Source, creds: Option<&Credentials>, toplevel: &str) -> Result<(), (bool, String)> {
    let mut c = download_command(cfg, source, creds, toplevel);
    eprintln!("mortar-update: {:?}", c.get_args().collect::<Vec<_>>());
    c.stdout(Stdio::null()).stderr(Stdio::piped());
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
    if low.contains("signature") || low.contains("not signed") {
        return Err((false, "not signed by a trusted key (mortar.update.trustedKeys)".into()));
    }
    let offline = ["could not resolve", "couldn't resolve", "couldn't connect", "could not connect", "failed to connect", "unable to download", "timeout", "timed out", "network is unreachable", "connection reset"]
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

    fn config() -> Config {
        serde_json::from_str(
            r#"{"bucket":"b","endpoint":"http://x","configName":"brick-bare",
                "trustedKeys":["test-1:IEXeMnbLZHPuxalArGnT5CiHpciUFtH+toxGnI/q3kQ="]}"#,
        )
        .unwrap()
    }

    fn manifest() -> Manifest {
        let mut m: Manifest = serde_json::from_str(
            r#"{"schema":1,"channel":"mortar","system":"aarch64-linux","config":"brick-bare",
                "toplevel":"/nix/store/0c7f8ms0x6r4n3pq7x2a9w1kz5lbv8yd-x","published":5,"date":"x"}"#,
        )
        .unwrap();
        m.sign(&mortar_index::SecretKey::parse("test-1:JMPmnQgjB/X0wg+yK5jOkfHTpB0SnK3WQnrptJSIo5UgRd4ydstkc+7FqUCsadPkKIelyJQW0f62jEacj+reRA==").unwrap());
        m
    }

    /// HTTPS mode: every path is checked against the publisher's and the
    /// upstreams' keys and nothing else, from these caches and no others.
    #[test]
    fn https_download_trusts_only_the_named_keys() {
        let cfg: Config = serde_json::from_str(
            r#"{"baseUrl":"https://cache.example","configName":"brick-bare","trustedKeys":["mine:AA=="],
                "upstreams":[{"url":"https://up.example","publicKey":"up:BB=="}]}"#,
        )
        .unwrap();
        let c = download_command(&cfg, &cfg.source().unwrap(), None, "/nix/store/x-y");
        let args: Vec<String> = c.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        let opt = |name: &str| args.iter().position(|a| a == name).map(|i| args[i + 1].as_str());
        assert_eq!(opt("trusted-public-keys"), Some("mine:AA== up:BB=="));
        assert_eq!(opt("substituters"), Some("https://cache.example https://up.example"));
        assert_eq!(opt("require-sigs"), Some("true"));
        assert_eq!(opt("--realise"), Some("/nix/store/x-y"));
        assert_eq!(c.get_envs().count(), 0);
    }

    #[test]
    fn refusals() {
        let (cfg, m, no) = (config(), manifest(), Allow::default());
        assert_eq!(refuse(&cfg, &m, no), None);

        let mut x = m.clone();
        x.signature = None;
        assert_eq!(refuse(&cfg, &x, no).unwrap().0, "Update not trusted");

        // signed, but for something else
        let sign = |f: fn(&mut Manifest)| {
            let mut x = manifest();
            f(&mut x);
            x.sign(&mortar_index::SecretKey::parse("test-1:JMPmnQgjB/X0wg+yK5jOkfHTpB0SnK3WQnrptJSIo5UgRd4ydstkc+7FqUCsadPkKIelyJQW0f62jEacj+reRA==").unwrap());
            x
        };
        assert_eq!(refuse(&cfg, &sign(|m| m.channel = "testing".into()), no).unwrap().0, "Wrong channel");
        assert_eq!(refuse(&cfg, &sign(|m| m.system = "x86_64-linux".into()), no).unwrap().0, "Wrong system");
        let full = sign(|m| m.config = "brick-full".into());
        assert_eq!(refuse(&cfg, &full, no).unwrap().0, "Different configuration");
        assert_eq!(refuse(&cfg, &full, Allow { config_change: true, ..no }), None);

        // a Brick that doesn't know its configuration refuses too
        let mut unnamed = config();
        unnamed.config_name = None;
        assert_eq!(refuse(&unnamed, &m, no).unwrap().0, "Different configuration");

        // no trust list: nothing is trusted
        let mut empty = config();
        empty.trusted_keys.clear();
        assert_eq!(refuse(&empty, &m, no).unwrap().0, "Update not trusted");

        // an upstream's key counts for closures, never for a manifest
        let mut only_upstream = config();
        only_upstream.upstreams = vec![config::Upstream { url: "https://u".into(), public_key: only_upstream.trusted_keys[0].clone() }];
        only_upstream.trusted_keys.clear();
        assert_eq!(refuse(&only_upstream, &m, no).unwrap().0, "Update not trusted");
    }
}
