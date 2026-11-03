// SPDX-License-Identifier: GPL-2.0-or-later
//! `mortar publish`: what CI does (.github/workflows/build.yml), from this
//! computer. Build the system, push its closure signed to the bucket
//! (`nix copy --to s3://…&secret-key=…`), then write the channel manifest the
//! Brick reads, signed with the same key (docs/updates.md). Also `mortar update` and
//! `mortar update-creds` (the Brick's side, over SSH).

use crate::{build_toplevel, die, remote, say, ssh_cmd};
use mortar_update::manifest::SecretKey;
use mortar_update::{manifest, time, Bucket, Credentials, FetchError, Manifest};
use std::env;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub struct Opts {
    pub flake: String,
    pub config: String,
    pub path: Option<String>,
    pub channel: Option<String>,
    /// Bucket settings given directly (else read from mortar.update in the flake).
    pub endpoint: Option<String>,
    pub bucket: Option<String>,
    pub region: Option<String>,
    pub key: Option<PathBuf>,
    pub creds: Option<PathBuf>,
    pub publisher: String,
    pub force: bool,
    pub dry_run: bool,
    /// `mortar promote`: the channel to take the manifest from.
    pub from: Option<String>,
    /// `mortar promote`: refuse unless the manifest is of this commit.
    pub rev: Option<String>,
}

/// mortar.update from the flake (no secrets in there).
struct Settings {
    bucket: Bucket,
    channel: String,
    /// What the manifest calls this configuration.
    config: String,
}

fn config_dir() -> PathBuf {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("mortar")
}

fn settings(o: &Opts) -> Settings {
    if let (Some(e), Some(b)) = (&o.endpoint, &o.bucket) {
        let bucket = Bucket::new(e, b, o.region.as_deref().unwrap_or("auto")).unwrap_or_else(|e| die(&e));
        return Settings { bucket, channel: o.channel.clone().unwrap_or_else(|| "mortar".into()), config: o.config.clone() };
    }
    let attr = format!("{}#nixosConfigurations.{}.config.mortar.update", o.flake, o.config);
    let out = Command::new("nix")
        .args(["eval", "--json", &attr, "--apply", "u: { inherit (u) enable bucket endpoint region channel configName; }"])
        .stderr(Stdio::inherit())
        .output()
        .unwrap_or_else(|e| die(&format!("nix: {e}")));
    if !out.status.success() {
        die("could not read mortar.update from the flake");
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| die(&format!("mortar.update: {e}")));
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    if v.get("enable").and_then(|e| e.as_bool()) != Some(true) {
        die("mortar.update.enable is false: fill in mortar.update in your configuration first (docs/updates.md)");
    }
    if s("endpoint").contains("ACCOUNT_ID") {
        die("mortar.update.endpoint is still the placeholder (docs/updates.md)");
    }
    let bucket = Bucket::new(&s("endpoint"), &s("bucket"), &s("region")).unwrap_or_else(|e| die(&e));
    // the name the Brick compares with: an alias (brick-mainline) publishes under its own
    let config = Some(s("configName")).filter(|c| !c.is_empty()).unwrap_or_else(|| o.config.clone());
    Settings { bucket, channel: o.channel.clone().unwrap_or_else(|| s("channel")), config }
}

/// `key = "value"` lines (the same format as the Brick's credentials file).
fn read_kv(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == key).then(|| v.trim().trim_matches('"').to_string()).filter(|v| !v.is_empty())
    })
}

/// Write credentials: $AWS_ACCESS_KEY_ID/$AWS_SECRET_ACCESS_KEY, else
/// ~/.config/mortar/r2-write (or --credentials FILE).
fn write_creds(o: &Opts) -> Credentials {
    if o.creds.is_none() {
        if let Some(c) = Credentials::from_env() {
            return c;
        }
    }
    let p = o.creds.clone().unwrap_or_else(|| config_dir().join("r2-write"));
    let t = std::fs::read_to_string(&p).unwrap_or_else(|e| {
        die(&format!("no bucket credentials: set AWS_ACCESS_KEY_ID/AWS_SECRET_ACCESS_KEY or write {} ({e})", p.display()))
    });
    match (read_kv(&t, "access_key_id"), read_kv(&t, "secret_access_key")) {
        (Some(a), Some(s)) => Credentials { access_key_id: a, secret_access_key: s },
        _ => die(&format!("{}: want access_key_id = \"…\" and secret_access_key = \"…\"", p.display())),
    }
}

fn signing_key(o: &Opts) -> PathBuf {
    let p = o
        .key
        .clone()
        .or_else(|| env::var_os("MORTAR_SIGNING_KEY_FILE").map(PathBuf::from))
        .unwrap_or_else(|| config_dir().join("signing-key"));
    if !p.is_file() {
        die(&format!("no signing key at {} (--key FILE; docs/updates.md)", p.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::metadata(&p).is_ok_and(|m| m.permissions().mode() & 0o077 != 0) {
            eprintln!("mortar: warning: {} is readable by others (chmod 600 it)", p.display());
        }
    }
    p
}

fn read_secret(p: &Path) -> SecretKey {
    let t = std::fs::read_to_string(p).unwrap_or_else(|e| die(&format!("{}: {e}", p.display())));
    SecretKey::parse(&t).unwrap_or_else(|e| die(&format!("{}: {e}", p.display())))
}

/// `mortar sign-manifest --key FILE`: sign the manifest on stdin, print it. For
/// tests and repairs; `mortar publish` signs what it writes.
pub fn sign_stdin(key: &Path) {
    let mut text = Vec::new();
    io::Read::read_to_end(&mut io::stdin(), &mut text).unwrap_or_else(|e| die(&format!("stdin: {e}")));
    let mut m: Manifest = serde_json::from_slice(&text).unwrap_or_else(|e| die(&format!("manifest: {e}")));
    m.sign(&read_secret(key));
    print!("{}", m.to_json());
}

fn git(flake: &str, args: &[&str]) -> Option<String> {
    let dir = if Path::new(flake).is_dir() { flake } else { "." };
    let out = Command::new("git").arg("-C").arg(dir).args(args).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn rev(o: &Opts) -> Option<String> {
    if let Some(r) = env::var("MORTAR_REV").or_else(|_| env::var("GITHUB_SHA")).ok().filter(|r| !r.is_empty()) {
        return Some(r);
    }
    let head = git(&o.flake, &["rev-parse", "HEAD"])?;
    let dirty = git(&o.flake, &["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
    Some(if dirty { format!("{head}-dirty") } else { head })
}

fn changelog(o: &Opts, prev: Option<&Manifest>, rev: Option<&str>) -> Vec<String> {
    let head = rev.map(|r| r.trim_end_matches("-dirty")).unwrap_or("HEAD");
    let prev_rev = prev.and_then(|m| m.rev.as_deref()).map(|r| r.trim_end_matches("-dirty"));
    let range = match prev_rev {
        Some(p) if git(&o.flake, &["cat-file", "-e", &format!("{p}^{{commit}}")]).is_some() => format!("{p}..{head}"),
        _ => head.to_string(),
    };
    let max = if range.contains("..") { "30" } else { "5" };
    git(&o.flake, &["log", "--no-merges", "--format=%s", "-n", max, &range])
        .map(|s| s.lines().map(str::to_string).filter(|l| !l.is_empty()).collect())
        .unwrap_or_default()
}

fn label(top: &str) -> Option<String> {
    std::fs::read_to_string(Path::new(top).join("nixos-version")).ok().map(|s| s.trim().to_string())
}

pub fn publish(o: &Opts) {
    let st = settings(o);
    let creds = write_creds(o);
    let key = signing_key(o);
    let top = match &o.path {
        Some(p) => p.clone(),
        None => build_toplevel(&o.flake, &o.config),
    };
    if !manifest::valid_store_path(&top) {
        die(&format!("not a store path: {top}"));
    }
    let key_name = manifest::key(&st.channel, &st.config);

    say(&format!("Reading {key_name}"));
    let prev = match st.bucket.get(&creds, &key_name, "curl") {
        Ok(b) => match Manifest::parse(&b) {
            Ok(m) => Some(m),
            Err(e) => {
                eprintln!("mortar: the current manifest is unreadable ({e}); replacing it");
                None
            }
        },
        Err(FetchError::NotFound) => None,
        Err(e) => die(&format!("{key_name}: {e}")),
    };
    if let Some(p) = prev.as_ref().filter(|p| p.toplevel == top && !o.force) {
        say(&format!("Unchanged: {} is already published ({})", top, p.short()));
        return;
    }

    let rev = rev(o);
    let now = time::now();
    let m = Manifest {
        schema: manifest::SCHEMA,
        channel: st.channel.clone(),
        // a NixOS toplevel names its platform
        system: std::fs::read_to_string(Path::new(&top).join("system")).map(|s| s.trim().to_string()).unwrap_or_else(|_| "aarch64-linux".into()),
        config: st.config.clone(),
        toplevel: top.clone(),
        // strictly newer than the previous one, even with a skewed clock
        published: now.max(prev.as_ref().map(|p| p.published + 1).unwrap_or(0)),
        date: time::rfc3339(now),
        rev: rev.clone(),
        label: label(&top),
        changelog: changelog(o, prev.as_ref(), rev.as_deref()),
        publisher: o.publisher.clone(),
        signature: None,
    };
    let mut m = m;
    m.sign(&read_secret(&key));
    if o.dry_run {
        print!("{}", m.to_json());
        return;
    }

    let url = st.bucket.store_url(&[("secret-key", &key.to_string_lossy()), ("compression", "zstd")]);
    say(&format!("Pushing the closure of {top} to s3://{}", st.bucket.bucket));
    let ok = Command::new("nix")
        .args(["--extra-experimental-features", "nix-command", "copy", "--to", &url, &top])
        .envs(creds.env())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        die("nix copy to the bucket failed (the manifest was not changed)");
    }

    // the closure is complete; now point the channel at it
    write_manifest(&st, &creds, &m);
    say(&format!("Published {} on '{}' ({})", m.short(), st.channel, top));
    for c in m.changelog.iter().take(10) {
        println!("  - {c}");
    }
}

/// The manifest and its copy in the channel's history.
fn write_manifest(st: &Settings, creds: &Credentials, m: &Manifest) {
    let tmp = env::temp_dir().join(format!("mortar-manifest-{}.json", std::process::id()));
    std::fs::write(&tmp, m.to_json()).unwrap_or_else(|e| die(&format!("{}: {e}", tmp.display())));
    let short_rev = m.rev.as_deref().map(|r| &r[..r.len().min(12)]).unwrap_or("local");
    let history = format!("channels/{}/{}/{}-{}.json", st.channel, st.config, m.published, short_rev);
    let key_name = manifest::key(&st.channel, &st.config);
    for k in [&history, &key_name] {
        if let Err(e) = st.bucket.put(creds, k, &tmp, "application/json", "curl") {
            let _ = std::fs::remove_file(&tmp);
            die(&format!("uploading {k}: {e}"));
        }
    }
    let _ = std::fs::remove_file(&tmp);
}

/// `mortar promote`: what is on one channel (testing) goes on another (the
/// release channel). Its closure is in the bucket already. The manifest is
/// re-signed: the channel is part of what's signed.
pub fn promote(o: &Opts) {
    let st = settings(o);
    let creds = write_creds(o);
    let secret = read_secret(&signing_key(o));
    let from = o.from.as_deref().unwrap_or("testing");
    if from == st.channel {
        die(&format!("'{from}' is the channel being promoted to"));
    }
    let read = |k: &str| match st.bucket.get(&creds, k, "curl") {
        Ok(b) => Manifest::parse(&b).map(Some).unwrap_or_else(|e| die(&format!("{k}: {e}"))),
        Err(FetchError::NotFound) => None,
        Err(e) => die(&format!("{k}: {e}")),
    };
    let src_key = manifest::key(from, &st.config);
    let mut m = read(&src_key).unwrap_or_else(|| die(&format!("nothing published at {src_key}")));
    m.verify(&[secret.public()]).unwrap_or_else(|e| die(&format!("{src_key}: {e}")));
    if let Some(want) = &o.rev {
        if m.rev.as_deref() != Some(want.as_str()) {
            die(&format!("{src_key} is {}, not {want}", m.rev.as_deref().unwrap_or("unknown")));
        }
    }
    let prev = read(&manifest::key(&st.channel, &st.config));
    if let Some(p) = prev.as_ref().filter(|p| p.toplevel == m.toplevel && !o.force) {
        say(&format!("Unchanged: {} is already on '{}' ({})", m.toplevel, st.channel, p.short()));
        return;
    }
    let now = time::now();
    m.changelog = changelog(o, prev.as_ref(), m.rev.as_deref());
    m.channel = st.channel.clone();
    m.published = now.max(prev.as_ref().map(|p| p.published + 1).unwrap_or(0));
    m.date = time::rfc3339(now);
    m.publisher = o.publisher.clone();
    m.sign(&secret);
    if o.dry_run {
        print!("{}", m.to_json());
        return;
    }
    write_manifest(&st, &creds, &m);
    say(&format!("Promoted {} from '{from}' to '{}' ({})", m.short(), st.channel, m.toplevel));
}

/// `mortar update`: check now on the Brick (it downloads and stages).
pub fn update_now(host: &str) {
    say(&format!("Checking for an update on {host}"));
    if !remote(host, "mortar-update check && mortar-update status") {
        die("mortar-update check failed");
    }
}

/// `mortar update-creds`: put the read-only bucket credentials on the Brick
/// over SSH (stdin, never argv).
pub fn update_creds(host: &str, key_id: Option<String>) {
    let id = key_id
        .or_else(|| env::var("MORTAR_UPDATE_KEY_ID").ok())
        .unwrap_or_else(|| prompt("Read-only Access Key ID: ", false));
    let secret = env::var("MORTAR_UPDATE_SECRET").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| prompt("Secret Access Key: ", true));
    if id.is_empty() || secret.is_empty() {
        die("both are needed");
    }
    let q = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    let body = format!("access_key_id = {}\nsecret_access_key = {}\n", q(&id), q(&secret));
    let mut child = ssh_cmd(host)
        .arg("mortar-update provision --stdin && mortar-update check; mortar-update status")
        .stdin(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| die(&format!("ssh: {e}")));
    child.stdin.take().expect("stdin").write_all(body.as_bytes()).unwrap_or_else(|e| die(&format!("ssh: {e}")));
    if !child.wait().map(|s| s.success()).unwrap_or(false) {
        die("could not store the credentials");
    }
}

fn prompt(label: &str, secret: bool) -> String {
    eprint!("{label}");
    let _ = io::stderr().flush();
    let echo_off = secret && Command::new("stty").arg("-echo").stdin(Stdio::inherit()).status().is_ok_and(|s| s.success());
    let mut line = String::new();
    let _ = io::stdin().lock().read_line(&mut line);
    if echo_off {
        let _ = Command::new("stty").arg("echo").stdin(Stdio::inherit()).status();
        eprintln!();
    }
    line.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kv() {
        let t = "# comment\naccess_key_id = \"abc\"\nsecret_access_key=xyz\n";
        assert_eq!(read_kv(t, "access_key_id").as_deref(), Some("abc"));
        assert_eq!(read_kv(t, "secret_access_key").as_deref(), Some("xyz"));
        assert_eq!(read_kv(t, "nope"), None);
    }
}
