// SPDX-License-Identifier: GPL-2.0-or-later
//! Where things live on the Brick, and the small persistent state of setup.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Paths {
    /// The FAT boot partition.
    pub boot: PathBuf,
    pub setup_file: PathBuf,
    /// `<boot>/ssh-keys`: `root` (the authorized keys sshd reads) and any
    /// `*.pub` files people dropped in.
    pub card_keys_dir: PathBuf,
    pub card_keys: PathBuf,
    /// `~root/.ssh/authorized_keys` (where ssh-copy-id puts keys).
    pub home_keys: PathBuf,
    pub state_dir: PathBuf,
    pub run_dir: PathBuf,
    pub iwd_dir: PathBuf,
    pub host_key_dir: PathBuf,
    /// networkd drop-in directory for usb0's .network file.
    pub usb_dropin_dir: PathBuf,
}

impl Paths {
    pub fn system() -> Paths {
        Paths::with(Path::new("/boot"), Path::new("/"))
    }

    /// Everything under `root` (tests, dev).
    pub fn under(root: &Path) -> Paths {
        Paths::with(&root.join("boot"), root)
    }

    fn with(boot: &Path, root: &Path) -> Paths {
        Paths {
            boot: boot.to_path_buf(),
            setup_file: boot.join("mortar.toml"),
            card_keys_dir: boot.join("ssh-keys"),
            card_keys: boot.join("ssh-keys/root"),
            home_keys: root.join("root/.ssh/authorized_keys"),
            state_dir: root.join("var/lib/mortar-setup"),
            run_dir: root.join("run/mortar-setup"),
            iwd_dir: root.join("var/lib/iwd"),
            host_key_dir: root.join("etc/ssh"),
            usb_dropin_dir: root.join("run/systemd/network/10-mortar-usb0.network.d"),
        }
    }

    pub fn state_file(&self) -> PathBuf {
        self.state_dir.join("state.json")
    }
    pub fn summary_file(&self) -> PathBuf {
        self.run_dir.join("setup-file.json")
    }
    pub fn sshd_dir(&self) -> PathBuf {
        self.run_dir.join("sshd")
    }
    pub fn socket(&self) -> PathBuf {
        self.run_dir.join("control.sock")
    }
}

/// Persistent setup state (`/var/lib/mortar-setup/state.json`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// First-boot setup finished (or skipped).
    pub done: bool,
    /// The pairing code also works as root's SSH password over USB.
    pub password_login: bool,
    pub usb_network: bool,
    pub ssh_server: bool,
    pub hostname: Option<String>,
    pub country: Option<String>,
    /// Hash of the setup file (+ ssh-keys/*.pub) last applied, so settings
    /// changed on the Brick are not overwritten on every boot.
    pub applied_hash: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        State {
            done: false,
            password_login: true,
            usb_network: true,
            ssh_server: true,
            hostname: None,
            country: None,
            applied_hash: None,
        }
    }
}

impl State {
    pub fn load(path: &Path) -> State {
        fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }
    /// Save; also keeps a `done` marker next to the file, which systemd
    /// can test (ConditionPathExists) without parsing JSON.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        write_atomic(path, serde_json::to_string_pretty(self)?.as_bytes(), 0o600)?;
        let marker = path.with_file_name("done");
        if self.done {
            fs::write(marker, "")?;
        } else if marker.exists() {
            fs::remove_file(marker)?;
        }
        Ok(())
    }
}

/// Write a file via a temp file + fsync + rename, creating parent dirs
/// (0700). Works on the FAT partition too (mode bits are then ignored).
pub fn write_atomic(path: &Path, data: &[u8], mode: u32) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    if !dir.exists() {
        fs::create_dir_all(dir)?;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    let tmp = dir.join(format!(".{}.tmp", path.file_name().and_then(|n| n.to_str()).unwrap_or("file")));
    {
        let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(mode).open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn state_roundtrip_and_defaults() {
        let d = std::env::temp_dir().join(format!("mortar-setup-state-{}", std::process::id()));
        let p = d.join("x/state.json");
        assert_eq!(State::load(&p), State::default());
        let s = State { done: true, hostname: Some("b".into()), ..Default::default() };
        s.save(&p).unwrap();
        assert_eq!(State::load(&p), s);
        assert!(p.with_file_name("done").exists());
        // unknown/missing fields keep defaults
        fs::write(&p, "{\"done\":true}").unwrap();
        let l = State::load(&p);
        assert!(l.done && l.usb_network && l.password_login);
        fs::remove_dir_all(&d).unwrap();
    }
}
