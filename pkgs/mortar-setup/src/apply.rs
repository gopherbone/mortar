// SPDX-License-Identifier: GPL-2.0-or-later
//! `mortar-setup apply`: the boot-time step that turns `mortar.toml` (and
//! `ssh-keys/*.pub`) into Brick settings. Runs before iwd, sshd, Avahi and
//! networkd.
//!
//! The file is applied when it changed since last time (hash in the state
//! file), so settings changed later on the Brick stick. Host name, Wi-Fi
//! country and the USB-network switch are re-asserted on every boot.

use crate::control::SetupSummary;
use crate::keys;
use crate::setupfile::{scrub_password, SetupFile, Wifi};
use crate::state::{write_atomic, Paths, State};
use crate::sys;
use mortar_pair::PublicKey;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Options {
    /// Change the running system (host name, `iw reg`, networkd). Off in tests.
    pub system: bool,
}

/// iwd's file name for a network: the SSID itself if it is plain,
/// otherwise `=` + hex (iwd.network(5)).
pub fn iwd_file_name(ssid: &str, open: bool) -> String {
    let plain = !ssid.is_empty() && ssid.chars().all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '_' || c == '-');
    let base = if plain {
        ssid.to_string()
    } else {
        let hex: String = ssid.bytes().map(|b| format!("{b:02x}")).collect();
        format!("={hex}")
    };
    format!("{base}.{}", if open { "open" } else { "psk" })
}

/// Store a network for iwd (it watches the directory and autoconnects to
/// known networks).
pub fn write_iwd_network(dir: &Path, w: &Wifi) -> std::io::Result<PathBuf> {
    let open = w.password.is_empty();
    let path = dir.join(iwd_file_name(&w.ssid, open));
    let mut s = String::new();
    if !open {
        s.push_str("[Security]\n");
        s.push_str(&format!("Passphrase={}\n", w.password));
    }
    if w.hidden {
        s.push_str("[Settings]\nHidden=true\n");
    }
    write_atomic(&path, s.as_bytes(), 0o600)?;
    Ok(path)
}

fn pub_files(dir: &Path) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_name().to_string_lossy().ends_with(".pub"))
                .filter_map(|e| Some((e.file_name().to_string_lossy().into_owned(), fs::read_to_string(e.path()).ok()?)))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn hash(text: &str, pubs: &[(String, String)]) -> String {
    let mut h = Sha256::new();
    h.update(text.as_bytes());
    for (n, c) in pubs {
        h.update([0]);
        h.update(n.as_bytes());
        h.update([0]);
        h.update(c.as_bytes());
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Turn usb0 off/on through a networkd drop-in (`/run`, re-made each boot).
pub fn set_usb_network(paths: &Paths, on: bool, system: bool) {
    let f = paths.usb_dropin_dir.join("mortar-setup.conf");
    let changed = if on {
        fs::remove_file(&f).is_ok()
    } else {
        fs::create_dir_all(&paths.usb_dropin_dir).is_ok()
            && fs::write(&f, "# mortar-setup: USB network switched off\n[Link]\nActivationPolicy=always-down\n").is_ok()
    };
    if system && changed {
        sys::run(&["networkctl", "reload"]);
        sys::run(&["networkctl", if on { "up" } else { "down" }, "usb0"]);
    }
}

pub fn apply(paths: &Paths, opts: &Options) -> SetupSummary {
    let mut state = State::load(&paths.state_file());
    let mut sum = SetupSummary { path: paths.setup_file.display().to_string(), ..Default::default() };
    let text = fs::read_to_string(&paths.setup_file).ok();
    let pubs = pub_files(&paths.card_keys_dir);
    let parsed = match text.as_deref().map(SetupFile::parse) {
        Some(Ok(f)) => Some(f),
        Some(Err(e)) => {
            sum.warnings.push(format!("mortar.toml: {}", e.lines().next().unwrap_or(&e)));
            None
        }
        None => None,
    };

    if let Some(f) = &parsed {
        sum.has_content = f.has_content();
        sum.hostname = f.hostname.clone();
        sum.wifi_ssid = f.wifi.as_ref().map(|w| w.ssid.clone()).filter(|s| !s.is_empty());
        sum.password_login = f.password_login;
        sum.usb_network = f.usb_network;
        sum.warnings.extend(f.warnings.iter().cloned());
    }
    sum.has_content |= !pubs.is_empty();

    let h = hash(text.as_deref().unwrap_or(""), &pubs);
    let fresh = state.applied_hash.as_deref() != Some(h.as_str());

    // key lines from the file and from ssh-keys/*.pub
    let mut key_lines: Vec<(String, String)> = Vec::new();
    if let Some(f) = &parsed {
        key_lines.extend(f.authorized_keys.iter().map(|k| ("mortar.toml".to_string(), k.clone())));
    }
    for (name, content) in &pubs {
        key_lines.extend(content.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')).map(|l| (format!("ssh-keys/{name}"), l.to_string())));
    }
    for (src, line) in &key_lines {
        match PublicKey::parse(line) {
            Ok(k) => {
                sum.keys.push(if k.comment.is_empty() { k.fingerprint() } else { k.comment.clone() });
                if fresh && let Err(e) = keys::add(paths, &k) {
                    sum.warnings.push(format!("could not store a key from {src}: {e}"));
                }
            }
            Err(e) => sum.warnings.push(format!("{src}: {e}")),
        }
    }

    if fresh && let Some(f) = &parsed {
        if let Some(w) = &f.wifi {
            if !w.ssid.is_empty() {
                match write_iwd_network(&paths.iwd_dir, w) {
                    Ok(_) => {
                        if let Some(t) = &text
                            && let Some(scrubbed) = scrub_password(t)
                        {
                            match write_atomic(&paths.setup_file, scrubbed.as_bytes(), 0o600) {
                                Ok(()) => sum.wifi_password_scrubbed = true,
                                Err(e) => sum.warnings.push(format!("could not remove the Wi-Fi password from mortar.toml: {e}")),
                            }
                        }
                    }
                    Err(e) => sum.warnings.push(format!("could not store Wi-Fi '{}': {e}", w.ssid)),
                }
            }
            if !w.country.is_empty() {
                state.country = Some(w.country.clone());
            }
        }
        if let Some(hn) = &f.hostname {
            state.hostname = Some(hn.clone());
        }
        if let Some(p) = f.password_login {
            state.password_login = p;
        }
        if let Some(u) = f.usb_network {
            state.usb_network = u;
        }
        // hash what is on the card now (the scrubbed text)
        let now = fs::read_to_string(&paths.setup_file).unwrap_or_default();
        state.applied_hash = Some(hash(&now, &pubs));
    } else if fresh {
        state.applied_hash = Some(h);
    }
    // a key on the card means the first login is no longer needed
    if fresh && keys::any(paths) && parsed.as_ref().is_none_or(|f| f.password_login.is_none()) {
        state.password_login = false;
    }
    if let Err(e) = state.save(&paths.state_file()) {
        sum.warnings.push(format!("could not save setup state: {e}"));
    }

    // every boot
    if opts.system {
        if let Some(hn) = &state.hostname
            && let Err(e) = sys::set_hostname(hn)
        {
            sum.warnings.push(format!("host name: {e}"));
        }
        if let Some(c) = &state.country {
            sys::run(&["iw", "reg", "set", c]);
        }
    }
    set_usb_network(paths, state.usb_network, opts.system);

    let _ = fs::create_dir_all(&paths.run_dir);
    if let Ok(j) = serde_json::to_string_pretty(&sum) {
        let _ = fs::write(paths.summary_file(), j);
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mortar-setup-apply-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn iwd_names() {
        assert_eq!(iwd_file_name("Gloria_5G", false), "Gloria_5G.psk");
        assert_eq!(iwd_file_name("My Net", true), "My Net.open");
        assert_eq!(iwd_file_name("café", false), "=636166c3a9.psk");
        assert_eq!(iwd_file_name("a/b", false), "=612f62.psk");
    }

    #[test]
    fn apply_mockup_file() {
        let d = tmp("mockup");
        let paths = Paths::under(&d);
        fs::create_dir_all(&paths.card_keys_dir).unwrap();
        fs::write(
            &paths.setup_file,
            "hostname = \"my-brick\"\n[wifi]\nssid = \"Gloria_5G\"\npassword = \"correct horse battery\" # removed after first boot\ncountry = \"US\"\n[ssh]\nauthorized_keys = [\"ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl you@laptop\", \"garbage\"]\npassword_login = false\n[usb]\nnetwork = true\n",
        )
        .unwrap();
        fs::write(paths.card_keys_dir.join("desk.pub"), "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIA2SX5sCWSkqrIKgZy8HTRtXwBDQd9vvOyR0x8wbGG4T you@desktop\n").unwrap();
        let s = apply(&paths, &Options { system: false });
        assert!(s.has_content);
        assert_eq!(s.hostname.as_deref(), Some("my-brick"));
        assert_eq!(s.wifi_ssid.as_deref(), Some("Gloria_5G"));
        assert!(s.wifi_password_scrubbed);
        assert_eq!(s.keys, vec!["you@laptop", "you@desktop"]);
        assert_eq!(s.warnings.len(), 1, "{:?}", s.warnings);
        let psk = fs::read_to_string(paths.iwd_dir.join("Gloria_5G.psk")).unwrap();
        assert_eq!(psk, "[Security]\nPassphrase=correct horse battery\n");
        assert!(!fs::read_to_string(&paths.setup_file).unwrap().contains("correct horse"));
        assert_eq!(keys::list(&paths).len(), 2);
        let st = State::load(&paths.state_file());
        assert_eq!((st.hostname.as_deref(), st.country.as_deref(), st.password_login, st.usb_network), (Some("my-brick"), Some("US"), false, true));

        // second boot: unchanged file, a key removed on the Brick stays removed
        let fp = keys::list(&paths)[0].fingerprint.clone();
        keys::remove(&paths, &fp).unwrap();
        let s2 = apply(&paths, &Options { system: false });
        assert!(!s2.wifi_password_scrubbed);
        assert_eq!(keys::list(&paths).len(), 1);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn blank_template_changes_nothing() {
        let d = tmp("blank");
        let paths = Paths::under(&d);
        fs::create_dir_all(&paths.boot).unwrap();
        fs::write(&paths.setup_file, crate::setupfile::TEMPLATE).unwrap();
        let s = apply(&paths, &Options { system: false });
        assert!(!s.has_content);
        assert!(s.warnings.is_empty(), "{:?}", s.warnings);
        assert!(!paths.iwd_dir.exists());
        assert_eq!(fs::read_to_string(&paths.setup_file).unwrap(), crate::setupfile::TEMPLATE);
        let st = State::load(&paths.state_file());
        assert!(st.password_login && st.usb_network && !st.done);
        // usb network off via the file -> drop-in appears
        fs::write(&paths.setup_file, "[usb]\nnetwork = false\n").unwrap();
        apply(&paths, &Options { system: false });
        assert!(paths.usb_dropin_dir.join("mortar-setup.conf").exists());
        fs::remove_dir_all(&d).unwrap();
    }
}
