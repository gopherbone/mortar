// SPDX-License-Identifier: GPL-2.0-or-later
//! `mortar publish`: what CI does (.github/workflows/build.yml), from this
//! computer. Build the system, push its closure signed to the bucket, then write
//! the channel manifest the Brick reads, signed with the same key
//! (docs/updates.md). With `--upstream` the bucket gets only what the upstream
//! cache doesn't have ([`push_new_paths`]). `mortar publish-image` puts an SD image
//! next to that. Also `mortar update` and `mortar update-creds` (the Brick's side,
//! over SSH).

use crate::{build_toplevel, die, remote, say, ssh_cmd};
use mortar_update::manifest::SecretKey;
use mortar_update::{get_many, manifest, time, Bucket, Credentials, FetchError, Manifest};
use std::collections::HashSet;
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
    /// Caches the Brick also reads: their paths aren't uploaded.
    pub upstream: Vec<String>,
    /// `mortar publish-image`: the compressed SD image.
    pub image: Option<PathBuf>,
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
    upstreams: Vec<String>,
}

/// What makes the bucket a substituter a Nix will use. Nix itself only ever writes
/// StoreDir. Priority: just after cache.nixos.org (40), so a closure's paths are asked
/// of that first and this cache isn't asked for the nine in ten it doesn't hold.
const CACHE_INFO: &str = "StoreDir: /nix/store\nWantMassQuery: 1\nPriority: 41\n";

/// Never in a public cache, by name: a second line behind mortar.public, as in
/// scripts/check-public-closure.sh, for a `--path` nothing else has looked at.
const NEVER_PUBLIC: &[&str] = &["claude-code", "mortar-claude", "pico-8", "pico8", "acgc", "GAFE01", "openxcom-data", "UFO", "liar", "redemption-liar"];

fn never_public(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    let name = name.get(32..).unwrap_or(name);
    NEVER_PUBLIC.iter().any(|w| {
        name.match_indices(&format!("-{w}")).any(|(i, m)| matches!(name[i + m.len()..].chars().next(), None | Some('-' | '.')))
    })
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
        return Settings { bucket, channel: o.channel.clone().unwrap_or_else(|| "mortar".into()), config: o.config.clone(), upstreams: o.upstream.clone() };
    }
    let attr = format!("{}#nixosConfigurations.{}.config.mortar.update", o.flake, o.config);
    let out = Command::new("nix")
        .args(["eval", "--json", &attr, "--apply", "u: { inherit (u) enable baseUrl bucket endpoint region channel configName; }"])
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
    if !s("baseUrl").is_empty() {
        // the public URL is read-only, and the bucket's account isn't in the repository
        die("mortar.update reads a public cache: say where to write, with --endpoint, --bucket and --upstream (docs/updates.md)");
    }
    let bucket = Bucket::new(&s("endpoint"), &s("bucket"), &s("region")).unwrap_or_else(|e| die(&e));
    // the name the Brick compares with: an alias (brick-mainline) publishes under its own
    let config = Some(s("configName")).filter(|c| !c.is_empty()).unwrap_or_else(|| o.config.clone());
    Settings { bucket, channel: o.channel.clone().unwrap_or_else(|| s("channel")), config, upstreams: o.upstream.clone() }
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

    put_text(&st, &creds, "nix-cache-info", CACHE_INFO, "text/x-nix-cache-info");
    if st.upstreams.is_empty() {
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
    } else {
        push_new_paths(&st, &creds, &key, &top);
    }

    // the closure is complete; now point the channel at it
    write_manifest(&st, &creds, &m);
    say(&format!("Published {} on '{}' ({})", m.short(), st.channel, top));
    for c in m.changelog.iter().take(10) {
        println!("  - {c}");
    }
}

/// Push what the closure needs and neither an upstream nor the bucket has.
///
/// Nix won't do this by itself: `nix copy --to` refuses a path whose references aren't
/// in the destination. So it copies to a directory in which every path the Brick can
/// get elsewhere already seems to be, its narinfo and no NAR, and Nix adds the rest.
/// Those stand-ins go no further: only what Nix added is uploaded. Each NAR goes before
/// its narinfo and references before what refers to them, so any narinfo in the bucket
/// means that path and all it needs can be fetched, even after an upload that stopped.
fn push_new_paths(st: &Settings, creds: &Credentials, key: &Path, top: &str) {
    let out = Command::new("nix-store").args(["--query", "--requisites", top]).stderr(Stdio::inherit()).output().unwrap_or_else(|e| die(&format!("nix-store: {e}")));
    if !out.status.success() {
        die(&format!("{top}: not in this machine's store"));
    }
    // dependencies first
    let closure: Vec<String> = String::from_utf8_lossy(&out.stdout).lines().map(str::to_string).collect();
    let hash = |p: &str| -> String { p.rsplit('/').next().unwrap_or(p).chars().take(32).collect() };

    let bad: Vec<&String> = closure.iter().filter(|p| never_public(p)).collect();
    if !bad.is_empty() {
        die(&format!("this closure holds what must not be public: {bad:?}"));
    }

    let stage = env::temp_dir().join(format!("mortar-stage-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage).unwrap_or_else(|e| die(&format!("{}: {e}", stage.display())));
    let file = |h: &str| stage.join(format!("{h}.narinfo"));
    let fail = |e: String| -> ! {
        let _ = std::fs::remove_dir_all(&stage);
        die(&e)
    };
    std::fs::write(stage.join("nix-cache-info"), CACHE_INFO).unwrap_or_else(|e| fail(e.to_string()));

    // Where a lookup goes wrong the path just counts as missing, and is uploaded.
    let mut stand_ins = HashSet::new();
    for up in &st.upstreams {
        let up = up.trim_end_matches('/');
        let items: Vec<_> = closure.iter().map(|p| (format!("{up}/{}.narinfo", hash(p)), file(&hash(p)))).filter(|(_, f)| !stand_ins.contains(f)).collect();
        stand_ins.extend(get_many(&items, None, "curl").unwrap_or_else(|e| fail(e)));
    }
    let upstream = stand_ins.len();
    let items: Vec<_> = closure.iter().map(|p| (st.bucket.object_url(&format!("{}.narinfo", hash(p))), file(&hash(p)))).filter(|(_, f)| !stand_ins.contains(f)).collect();
    stand_ins.extend(get_many(&items, Some((&st.bucket, creds)), "curl").unwrap_or_else(|e| fail(e)));
    let already = stand_ins.len() - upstream;
    say(&format!("{} paths: {upstream} upstream, {already} in the bucket already, {} to upload", closure.len(), closure.len() - upstream - already));

    let url = format!("file://{}?compression=zstd&secret-key={}", stage.display(), key.display());
    let ok = Command::new("nix").args(["--extra-experimental-features", "nix-command", "copy", "--to", &url, top]).status().map(|s| s.success()).unwrap_or(false);
    if !ok {
        fail("nix copy failed (nothing was uploaded, the manifest was not changed)".into());
    }

    let mut bytes = 0;
    for p in &closure {
        let info = file(&hash(p));
        if stand_ins.contains(&info) {
            continue;
        }
        let text = std::fs::read_to_string(&info).unwrap_or_else(|e| fail(format!("{}: {e}", info.display())));
        let nar = text.lines().find_map(|l| l.strip_prefix("URL: ")).unwrap_or_else(|| fail(format!("{}: no URL", info.display()))).to_string();
        let local = stage.join(&nar);
        bytes += std::fs::metadata(&local).map(|m| m.len()).unwrap_or(0);
        for (k, f, ty) in [(nar.as_str(), &local, "application/x-nix-nar"), (&format!("{}.narinfo", hash(p)), &info, "text/x-nix-narinfo")] {
            if let Err(e) = st.bucket.put(creds, k, f, ty, "curl") {
                fail(format!("uploading {k}: {e} (the manifest was not changed)"));
            }
        }
    }
    say(&format!("Uploaded {} MiB", bytes / (1024 * 1024)));
    let _ = std::fs::remove_dir_all(&stage);
}

fn put_text(st: &Settings, creds: &Credentials, key: &str, text: &str, content_type: &str) {
    let tmp = env::temp_dir().join(format!("mortar-object-{}", std::process::id()));
    std::fs::write(&tmp, text).unwrap_or_else(|e| die(&format!("{}: {e}", tmp.display())));
    let r = st.bucket.put(creds, key, &tmp, content_type, "curl");
    let _ = std::fs::remove_file(&tmp);
    if let Err(e) = r {
        die(&format!("uploading {key}: {e}"));
    }
}

/// The manifest and its copy in the channel's history.
fn write_manifest(st: &Settings, creds: &Credentials, m: &Manifest) {
    let short_rev = m.rev.as_deref().map(|r| &r[..r.len().min(12)]).unwrap_or("local");
    let history = format!("channels/{}/{}/{}-{}.json", st.channel, st.config, m.published, short_rev);
    for k in [&history, &manifest::key(&st.channel, &st.config)] {
        put_text(st, creds, k, &m.to_json(), "application/json");
    }
}

/// `images/<channel>/<config>-latest.img.zst`, under the name that stays.
fn image_latest(channel: &str, config: &str) -> String {
    format!("images/{channel}/{config}-latest.img.zst")
}

/// `images/<channel>/v/<config>-<rev>.img.zst`: this build's. Under `v/` so that a
/// lifecycle rule can expire old builds without touching the latest.
fn image_versioned(channel: &str, config: &str, rev: &str) -> String {
    format!("images/{channel}/v/{config}-{}.img.zst", &rev[..rev.len().min(12)])
}

/// `sha256sum -c` format, with the name the file has next to it.
fn sha256_line(hash: &str, key: &str) -> String {
    format!("{hash}  {}\n", key.rsplit('/').next().unwrap_or(key))
}

/// `mortar publish-image`: an SD image goes to images/<channel>/, under this build's
/// name and the name that stays, each with a sha256.
pub fn publish_image(o: &Opts) {
    let st = settings(o);
    let creds = write_creds(o);
    let image = o.image.as_deref().unwrap_or_else(|| die("publish-image --image FILE.img.zst"));
    let rev = rev(o).unwrap_or_else(|| die("no commit to name the image by (MORTAR_REV)"));
    let out = Command::new("nix-hash").args(["--type", "sha256", "--flat", "--base16"]).arg(image).output().unwrap_or_else(|e| die(&format!("nix-hash: {e}")));
    if !out.status.success() {
        die(&format!("{}: {}", image.display(), String::from_utf8_lossy(&out.stderr).trim()));
    }
    let hash = String::from_utf8_lossy(&out.stdout).trim().to_string();

    let (versioned, latest) = (image_versioned(&st.channel, &st.config, &rev), image_latest(&st.channel, &st.config));
    say(&format!("Uploading {} as {versioned}", image.display()));
    if let Err(e) = st.bucket.put(&creds, &versioned, image, "application/zstd", "curl") {
        die(&format!("uploading {versioned}: {e}"));
    }
    put_text(&st, &creds, &format!("{versioned}.sha256"), &sha256_line(&hash, &versioned), "text/plain; charset=utf-8");
    if let Err(e) = st.bucket.copy(&creds, &versioned, &latest, "curl") {
        die(&format!("{latest}: {e}"));
    }
    put_text(&st, &creds, &format!("{latest}.sha256"), &sha256_line(&hash, &latest), "text/plain; charset=utf-8");
    say(&format!("Published {latest} ({hash})"));
}

/// What `mortar promote` does for the image: this build's, from one channel to another.
/// An image that was never published (or has since expired) doesn't stop a release.
fn promote_image(st: &Settings, creds: &Credentials, from: &str, m: &Manifest) {
    let Some(rev) = m.rev.as_deref() else { return };
    let src = image_versioned(from, &st.config, rev);
    let hash = match st.bucket.get(creds, &format!("{src}.sha256"), "curl") {
        Ok(b) => String::from_utf8_lossy(&b).split_whitespace().next().unwrap_or("").to_string(),
        Err(FetchError::NotFound) => {
            eprintln!("mortar: warning: no image at {src}: the release has no image of its own");
            return;
        }
        Err(e) => die(&format!("{src}.sha256: {e}")),
    };
    let (versioned, latest) = (image_versioned(&st.channel, &st.config, rev), image_latest(&st.channel, &st.config));
    for to in [&versioned, &latest] {
        if let Err(e) = st.bucket.copy(creds, &src, to, "curl") {
            die(&format!("{src} -> {to}: {e}"));
        }
        put_text(st, creds, &format!("{to}.sha256"), &sha256_line(&hash, to), "text/plain; charset=utf-8");
    }
    say(&format!("Promoted the image to {latest}"));
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
    promote_image(&st, &creds, from, &m);
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

    #[test]
    fn what_must_not_be_public() {
        let h = "/nix/store/0c7f8ms0x6r4n3pq7x2a9w1kz5lbv8yd";
        for bad in ["-claude-code-2.1.3", "-pico-8", "-liar", "-liar-1.0", "-redemption-liar-1", "-mortar-claude.sh", "-GAFE01.iso"] {
            assert!(never_public(&format!("{h}{bad}")), "{bad}");
        }
        for fine in ["-familiar-1.0", "-mortar-shell-0.1.0", "-liar2", "-nixos-system-mortar"] {
            assert!(!never_public(&format!("{h}{fine}")), "{fine}");
        }
    }

    #[test]
    fn image_names() {
        let rev = "0123456789abcdef";
        assert_eq!(image_versioned("testing", "brick-bare", rev), "images/testing/v/brick-bare-0123456789ab.img.zst");
        assert_eq!(image_latest("testing", "brick-bare"), "images/testing/brick-bare-latest.img.zst");
        assert_eq!(sha256_line("ff", "images/testing/x.img.zst"), "ff  x.img.zst\n");
    }

    #[test]
    fn cache_info_makes_a_usable_substituter() {
        for line in ["StoreDir: /nix/store", "WantMassQuery: 1", "Priority: 41"] {
            assert!(CACHE_INFO.lines().any(|l| l == line), "{line}");
        }
    }
}
