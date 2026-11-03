// SPDX-License-Identifier: GPL-2.0-or-later
//! root's authorized keys: `<card>/ssh-keys/root` (the card's FAT partition,
//! editable from any computer) and `~root/.ssh/authorized_keys` (where
//! ssh-copy-id puts them). sshd reads both (modules/core/usb.nix).

use crate::control::KeyView;
use crate::state::{write_atomic, Paths};
use mortar_pair::PublicKey;
use std::fs;
use std::io;
use std::path::Path;

/// Parse a key line that may carry authorized_keys options in front
/// (`from="…",no-pty ssh-ed25519 AAAA… c`): find the key type token.
pub fn parse_line(line: &str) -> Option<PublicKey> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    if let Ok(k) = PublicKey::parse(line) {
        return Some(k);
    }
    // skip an options field (may contain quoted spaces)
    let mut in_q = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_q = !in_q,
            ' ' | '\t' if !in_q => return PublicKey::parse(&line[i + 1..]).ok(),
            _ => {}
        }
    }
    None
}

pub fn read(path: &Path) -> Vec<PublicKey> {
    fs::read_to_string(path).map(|s| s.lines().filter_map(parse_line).collect()).unwrap_or_default()
}

pub fn view(k: &PublicKey, source: &str) -> KeyView {
    KeyView { kind: k.short_kind().into(), comment: k.comment.clone(), fingerprint: k.fingerprint(), source: source.into() }
}

/// Every key root can log in with, card first, without duplicates.
pub fn list(paths: &Paths) -> Vec<KeyView> {
    let mut out: Vec<(PublicKey, &str)> = Vec::new();
    for (p, src) in [(&paths.card_keys, "card"), (&paths.home_keys, "home")] {
        for k in read(p) {
            if !out.iter().any(|(o, _)| o.same_key(&k)) {
                out.push((k, src));
            }
        }
    }
    out.iter().map(|(k, s)| view(k, s)).collect()
}

pub fn any(paths: &Paths) -> bool {
    !read(&paths.card_keys).is_empty() || !read(&paths.home_keys).is_empty()
}

/// Append `key` to the card's key file unless root can already use it.
/// Returns whether it was added.
pub fn add(paths: &Paths, key: &PublicKey) -> io::Result<bool> {
    if read(&paths.card_keys).iter().chain(read(&paths.home_keys).iter()).any(|k| k.same_key(key)) {
        return Ok(false);
    }
    let mut text = fs::read_to_string(&paths.card_keys).unwrap_or_default();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&key.line());
    text.push('\n');
    write_atomic(&paths.card_keys, text.as_bytes(), 0o600)?;
    Ok(true)
}

/// Remove every line (in both files) whose key has this fingerprint.
pub fn remove(paths: &Paths, fingerprint: &str) -> io::Result<usize> {
    let mut n = 0;
    for p in [&paths.card_keys, &paths.home_keys] {
        let Ok(text) = fs::read_to_string(p) else { continue };
        let mut kept = String::new();
        for line in text.lines() {
            if parse_line(line).is_some_and(|k| k.fingerprint() == fingerprint) {
                n += 1;
            } else {
                kept.push_str(line);
                kept.push('\n');
            }
        }
        if kept != text && kept.trim_end() != text.trim_end() {
            write_atomic(p, kept.as_bytes(), 0o600)?;
        }
    }
    Ok(n)
}

/// The Brick's SSH host keys, preferred (ed25519) first.
pub fn host_keys(dir: &Path) -> Vec<String> {
    ["ssh_host_ed25519_key.pub", "ssh_host_ecdsa_key.pub", "ssh_host_rsa_key.pub"]
        .iter()
        .filter_map(|f| fs::read_to_string(dir.join(f)).ok())
        .filter_map(|s| {
            let mut it = s.split_whitespace();
            Some(format!("{} {}", it.next()?, it.next()?))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    pub const K1: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl you@laptop";
    pub const K2: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIA2SX5sCWSkqrIKgZy8HTRtXwBDQd9vvOyR0x8wbGG4T you@desktop";

    #[test]
    fn add_list_remove() {
        let d = std::env::temp_dir().join(format!("mortar-setup-keys-{}", std::process::id()));
        let paths = Paths::under(&d);
        assert!(!any(&paths));
        let k1 = PublicKey::parse(K1).unwrap();
        assert!(add(&paths, &k1).unwrap());
        assert!(!add(&paths, &k1).unwrap(), "no duplicates");
        fs::create_dir_all(paths.home_keys.parent().unwrap()).unwrap();
        fs::write(&paths.home_keys, format!("# comment\nno-pty,from=\"10.0.0.1 x\" {K2}\n{K1}\n")).unwrap();
        let l = list(&paths);
        assert_eq!(l.len(), 2);
        assert_eq!((l[0].comment.as_str(), l[0].source.as_str()), ("you@laptop", "card"));
        assert_eq!((l[1].comment.as_str(), l[1].source.as_str()), ("you@desktop", "home"));
        assert_eq!(remove(&paths, &k1.fingerprint()).unwrap(), 2);
        assert_eq!(list(&paths).len(), 1);
        assert!(fs::read_to_string(&paths.home_keys).unwrap().contains("# comment"));
        fs::remove_dir_all(&d).unwrap();
    }
}
