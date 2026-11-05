// SPDX-License-Identifier: GPL-2.0-or-later
//! Before a system is staged: is what's in the store really what was published?
//!
//! Nix identifies a path by its name, not its contents, and takes whatever a trusted
//! key signed. So a third-party repository could publish its own contents under a path
//! a future Mortar system will use. Nix would then see that path already there during
//! the update, keep it, and it would run as root at boot.
//!
//! So every path of the new system has to pass, with the publisher's and the upstreams'
//! keys and no others:
//!
//! 1. A valid signature by one of those keys (`nix store verify`, which checks the
//!    cryptography: a repository can write any key's *name* on a signature).
//! 2. Signed only by someone else: its signatures are looked up in those caches, and
//!    only count if the NAR hash there is the one on this device
//!    (`nix store copy-sigs`). A third party that signed the very same contents is no
//!    harm, and passes. Different contents don't, and the update is refused.
//! 3. No signature at all: it's let through, and noted. A repository can't leave one of
//!    these: everything from a substituter arrives with `require-sigs`, and keeps
//!    its signature. They come from root on this device: an SD image registers its
//!    whole store unsigned, `mortar deploy` and any other `nix copy --no-check-sigs`
//!    from a computer leaves them, and a local build too. Refusing them would stop
//!    every card ever written from updating.

use crate::config::{Config, Source};
use mortar_update::Credentials;
use std::collections::BTreeSet;
use std::process::{Command, Stdio};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Verdict {
    /// Signed by someone else only: refuse.
    pub foreign: Vec<String>,
    /// Not signed by anyone: root put them here.
    pub unsigned: usize,
}

/// Every store path named in `nix store verify`'s complaints.
pub fn untrusted_paths(stderr: &str) -> BTreeSet<String> {
    stderr
        .lines()
        .filter(|l| l.contains("untrusted") || l.contains("lacks a signature"))
        .flat_map(|l| l.split(|c: char| c.is_whitespace() || c == '\'' || c == '"' || c == '`'))
        .filter(|w| mortar_update::manifest::valid_store_path(w))
        .map(str::to_string)
        .collect()
}

/// What `nix path-info --json` says about signatures, in either of its shapes (a list,
/// or a map by path in later Nix): the paths with at least one.
pub fn signed_paths(json: &str) -> Result<BTreeSet<String>, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("nix path-info: {e}"))?;
    let has_sigs = |i: &serde_json::Value| i.get("signatures").and_then(|s| s.as_array()).is_some_and(|a| !a.is_empty());
    let mut out = BTreeSet::new();
    match &v {
        serde_json::Value::Array(a) => {
            for i in a {
                if has_sigs(i)
                    && let Some(p) = i.get("path").and_then(|p| p.as_str())
                {
                    out.insert(p.to_string());
                }
            }
        }
        serde_json::Value::Object(m) => {
            for (p, i) in m {
                if has_sigs(i) {
                    out.insert(p.clone());
                }
            }
        }
        _ => return Err("nix path-info: not what was expected".into()),
    }
    Ok(out)
}

fn nix(cfg: &Config) -> Command {
    let mut c = Command::new(&cfg.nix);
    c.args(["--extra-experimental-features", "nix-command"]);
    c
}

/// The paths of `what` no key vouches for. `Err`: it couldn't be checked, which is no
/// reason to believe it.
fn unverified(cfg: &Config, what: &[&str], recursive: bool) -> Result<BTreeSet<String>, String> {
    let mut c = nix(cfg);
    c.args(["store", "verify", "--no-contents", "--sigs-needed", "1"]);
    c.args(["--option", "trusted-public-keys", &cfg.closure_keys().join(" ")]);
    if recursive {
        c.arg("--recursive");
    }
    c.args(what).stdin(Stdio::null()).stdout(Stdio::null());
    let out = c.output().map_err(|e| format!("{}: {e}", cfg.nix))?;
    if out.status.success() {
        return Ok(BTreeSet::new());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let paths = untrusted_paths(&err);
    if paths.is_empty() {
        return Err(format!("nix store verify: {}", err.lines().last().unwrap_or("failed")));
    }
    Ok(paths)
}

/// Which of the paths carry any signature at all.
fn with_signatures(cfg: &Config, paths: &BTreeSet<String>) -> Result<BTreeSet<String>, String> {
    let mut signed = BTreeSet::new();
    // a fresh card has thousands unsigned, and an argument list has a limit
    let list: Vec<&String> = paths.iter().collect();
    for chunk in list.chunks(500) {
        let mut c = nix(cfg);
        c.args(["path-info", "--json"]).args(chunk.iter().map(|s| s.as_str())).stdin(Stdio::null());
        let out = c.output().map_err(|e| format!("{}: {e}", cfg.nix))?;
        if !out.status.success() {
            return Err(format!("nix path-info: {}", String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("failed")));
        }
        signed.extend(signed_paths(&String::from_utf8_lossy(&out.stdout))?);
    }
    Ok(signed)
}

/// Ask the caches for their signatures on these paths: Nix only takes them when the
/// path's NAR hash is the one here. A cache that can't be reached leaves it unproven,
/// which is what the second look finds.
fn copy_signatures(cfg: &Config, source: &Source, creds: Option<&Credentials>, paths: &[&str]) {
    let mut subs: Vec<String> = match source {
        Source::Https(base) => vec![base.clone()],
        Source::S3(bucket) => vec![bucket.store_url(&[])],
    };
    subs.extend(cfg.upstreams.iter().map(|u| u.url.clone()));
    let mut c = nix(cfg);
    c.args(["store", "copy-sigs"]);
    for s in &subs {
        c.args(["--substituter", s]);
    }
    c.args(["--option", "connect-timeout", "20", "--option", "download-attempts", "2"]).args(paths).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::inherit());
    if let Some(creds) = creds {
        c.envs(creds.env());
    }
    if let Err(e) = c.status() {
        eprintln!("mortar-update: {}: {e}", cfg.nix);
    }
}

/// Look at every path of the new system. Run after the download and before staging.
pub fn check(cfg: &Config, source: &Source, creds: Option<&Credentials>, toplevel: &str) -> Result<Verdict, String> {
    let unproven = unverified(cfg, &[toplevel], true)?;
    if unproven.is_empty() {
        return Ok(Verdict::default());
    }
    let signed = with_signatures(cfg, &unproven)?;
    let unsigned = unproven.len() - signed.iter().filter(|p| unproven.contains(*p)).count();
    if signed.is_empty() {
        return Ok(Verdict { foreign: vec![], unsigned });
    }
    let list: Vec<&str> = signed.iter().map(String::as_str).collect();
    copy_signatures(cfg, source, creds, &list);
    let foreign = unverified(cfg, &list, false)?.into_iter().collect();
    Ok(Verdict { foreign, unsigned })
}

/// The status line for a refusal: what, and what to do.
pub fn refusal_detail(foreign: &[String]) -> String {
    let first = foreign.first().map(String::as_str).unwrap_or("");
    let name = first.rsplit('/').next().and_then(|n| n.get(33..)).unwrap_or(first);
    let more = match foreign.len() {
        0 | 1 => String::new(),
        n => format!(" and {} more", n - 1),
    };
    format!("{name}{more} in this store came from another source and isn't what this update needs. Remove what you installed from other repositories, collect garbage and try again (--allow-foreign-paths: take it anyway).")
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "/nix/store/0c7f8ms0x6r4n3pq7x2a9w1kz5lbv8yd-coreutils-9.5";
    const B: &str = "/nix/store/1c7f8ms0x6r4n3pq7x2a9w1kz5lbv8yd-glibc-2.40";

    #[test]
    fn finds_what_nix_complains_about() {
        let e = format!("checking...\npath '{A}' is untrusted\nerror: path '{B}' lacks a signature\nsome other error about /nix/store/nothing\n");
        let p = untrusted_paths(&e);
        assert_eq!(p.into_iter().collect::<Vec<_>>(), [A, B]);
        assert!(untrusted_paths("error: out of cheese").is_empty());
    }

    #[test]
    fn reads_both_shapes_of_path_info() {
        let list = format!(r#"[{{"path":"{A}","signatures":["x:AA=="]}},{{"path":"{B}","signatures":[]}}]"#);
        assert_eq!(signed_paths(&list).unwrap().into_iter().collect::<Vec<_>>(), [A]);
        let map = format!(r#"{{"{A}":{{"signatures":["x:AA=="]}},"{B}":{{"ultimate":true}}}}"#);
        assert_eq!(signed_paths(&map).unwrap().into_iter().collect::<Vec<_>>(), [A]);
        assert!(signed_paths("nope").is_err());
    }

    #[test]
    fn says_what_to_do() {
        let d = refusal_detail(&[A.to_string(), B.to_string()]);
        assert!(d.starts_with("coreutils-9.5 and 1 more"), "{d}");
        assert!(d.contains("--allow-foreign-paths"));
    }
}
