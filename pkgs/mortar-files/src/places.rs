// SPDX-License-Identifier: GPL-2.0-or-later
//! Where you start from: home, the Brick's game folders, removable disks,
//! the whole filesystem, the Trash and the storage view. What doesn't exist
//! isn't listed. Also what the emulator config says about game files.

use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PlaceKind {
    Home,
    Games,
    Saves,
    Bios,
    Removable,
    Root,
    Trash,
    Storage,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub kind: PlaceKind,
    pub name: String,
    pub path: PathBuf,
}

/// The inputs to `discover`, so tests and previews can use made-up places.
#[derive(Clone, Debug)]
pub struct PlaceEnv {
    pub home: PathBuf,
    /// /var/lib/mortar-play
    pub play: PathBuf,
    /// /proc/self/mountinfo
    pub mountinfo: PathBuf,
    pub root: PathBuf,
    pub trash: PathBuf,
}

impl PlaceEnv {
    pub fn system(home: PathBuf, play: PathBuf) -> PlaceEnv {
        let trash = home.join(".local/share/Trash");
        PlaceEnv { home, play, mountinfo: "/proc/self/mountinfo".into(), root: "/".into(), trash }
    }
}

/// Undo mountinfo's octal escapes (`\040` for a space).
fn unescape(s: &str) -> PathBuf {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..i + 4].iter().all(u8::is_ascii_digit) {
            out.push((b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0'));
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    PathBuf::from(std::ffi::OsString::from(String::from_utf8_lossy(&out).into_owned()))
}

/// Mounted disks that are someone's to browse: what's under the usual
/// removable folders, and FAT/exFAT anywhere but the firmware's.
pub fn removable(mountinfo: &str) -> Vec<PathBuf> {
    let mut v = Vec::new();
    for l in mountinfo.lines() {
        let Some((a, b)) = l.split_once(" - ") else { continue };
        let Some(point) = a.split(' ').nth(4) else { continue };
        let Some(fstype) = b.split(' ').next() else { continue };
        let p = unescape(point);
        let under = ["/run/media/", "/media/", "/mnt/"].iter().any(|r| p.starts_with(r) && p != Path::new(r.trim_end_matches('/')));
        let fat = matches!(fstype, "vfat" | "exfat" | "msdos") && !p.starts_with("/boot");
        if (under || fat) && !v.contains(&p) {
            v.push(p);
        }
    }
    v
}

pub fn discover(env: &PlaceEnv) -> Vec<Place> {
    let mut out = vec![Place { kind: PlaceKind::Home, name: "Home".into(), path: env.home.clone() }];
    for (kind, name, sub) in [(PlaceKind::Games, "Games", "roms"), (PlaceKind::Saves, "Saves", "saves"), (PlaceKind::Bios, "BIOS", "bios")] {
        let p = env.play.join(sub);
        if p.is_dir() {
            out.push(Place { kind, name: name.into(), path: p });
        }
    }
    if let Ok(s) = fs::read_to_string(&env.mountinfo) {
        for p in removable(&s).into_iter().take(2) {
            if p.is_dir() {
                let name = p.file_name().map(|n| crate::listing::display(n)).unwrap_or_else(|| "Card".into());
                out.push(Place { kind: PlaceKind::Removable, name, path: p });
            }
        }
    }
    out.push(Place { kind: PlaceKind::Trash, name: "Trash".into(), path: env.trash.join("files") });
    out.push(Place { kind: PlaceKind::Root, name: "Everything".into(), path: env.root.clone() });
    out.push(Place { kind: PlaceKind::Storage, name: "Storage".into(), path: env.root.clone() });
    out
}

// ------------------------------------------------------- the emulator's ----

/// The parts of /etc/mortar-play/config.json this app cares about.
#[derive(Clone, Debug, Default)]
pub struct PlayConfig {
    pub data_dir: Option<PathBuf>,
    pub rom_dirs: Vec<PathBuf>,
    pub systems: Vec<PlaySystem>,
}

#[derive(Clone, Debug, Default)]
pub struct PlaySystem {
    pub id: String,
    pub folders: Vec<String>,
    pub extensions: Vec<String>,
}

impl PlayConfig {
    pub fn parse(s: &str, home: &Path) -> Option<PlayConfig> {
        let v: serde_json::Value = serde_json::from_str(s).ok()?;
        let strings = |x: &serde_json::Value| -> Vec<String> { x.as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect()).unwrap_or_default() };
        let rom_dirs = strings(&v["romDirs"])
            .into_iter()
            .map(|d| match d.strip_prefix("~/") {
                Some(r) => home.join(r),
                None => PathBuf::from(d),
            })
            .collect();
        let systems = v["systems"]
            .as_array()?
            .iter()
            .filter(|s| !s["noContent"].as_bool().unwrap_or(false))
            .map(|s| PlaySystem { id: s["id"].as_str().unwrap_or("").into(), folders: strings(&s["folders"]), extensions: strings(&s["extensions"]) })
            .collect();
        let data_dir = v["dataDir"].as_str().map(PathBuf::from);
        Some(PlayConfig { data_dir, rom_dirs, systems })
    }

    pub fn load(path: &Path, home: &Path) -> Option<PlayConfig> {
        Self::parse(&fs::read_to_string(path).ok()?, home)
    }

    /// Every extension some system plays, for the look of a file. Not ".md",
    /// which is far more often notes than a Mega Drive game, nor ".bin" and
    /// ".txt" (a game in its system's folder still opens as a game: `system_for`).
    pub fn extensions(&self) -> Vec<String> {
        let mut v: Vec<String> = self.systems.iter().flat_map(|s| s.extensions.iter().cloned()).filter(|e| !matches!(e.as_str(), "md" | "bin" | "txt")).collect();
        v.sort();
        v.dedup();
        v
    }

    /// In a system's folder under a ROM root, with that system's extension.
    pub fn in_rom_folder(&self, file: &Path) -> bool {
        let ext = file.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        self.rom_dirs.iter().any(|root| {
            file.strip_prefix(root).ok().and_then(|r| r.components().next()).is_some_and(|f| self.systems.iter().any(|s| s.folders.iter().any(|x| std::ffi::OsStr::new(x) == f.as_os_str()) && s.extensions.contains(&ext)))
        })
    }

    /// Which system plays this file: by the folder it's in under a ROM root,
    /// else by an extension only one system claims.
    pub fn system_for(&self, file: &Path) -> Option<&str> {
        for root in &self.rom_dirs {
            if let Ok(rest) = file.strip_prefix(root)
                && let Some(folder) = rest.components().next()
                && let Some(s) = self.systems.iter().find(|s| s.folders.iter().any(|f| std::ffi::OsStr::new(f) == folder.as_os_str()))
            {
                return Some(&s.id);
            }
        }
        let ext = file.extension()?.to_string_lossy().to_lowercase();
        let mut hits = self.systems.iter().filter(|s| s.extensions.iter().any(|e| *e == ext));
        match (hits.next(), hits.next()) {
            (Some(s), None) => Some(&s.id),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removable_mounts() {
        let info = "\
22 1 179:2 / / rw,relatime shared:1 - ext4 /dev/mmcblk0p2 rw
23 22 179:1 / /boot rw - vfat /dev/mmcblk0p1 rw
24 22 0:5 / /nix/store ro - ext4 /dev/mmcblk0p2 ro
25 22 179:33 / /run/media/brick/GAME\\040CARD rw - exfat /dev/mmcblk1p1 rw
26 22 179:34 / /mnt/data rw - ext4 /dev/mmcblk1p2 rw
27 22 179:35 / /srv/fat rw - vfat /dev/sda1 rw
28 22 0:40 / /mnt rw - tmpfs tmpfs rw
";
        assert_eq!(removable(info), [PathBuf::from("/run/media/brick/GAME CARD"), "/mnt/data".into(), "/srv/fat".into()]);
    }

    #[test]
    fn only_what_exists_is_listed() {
        let d = crate::ops_tests::temp("places");
        let env = PlaceEnv { home: d.join("home"), play: d.join("play"), mountinfo: d.join("none"), root: "/".into(), trash: d.join("trash") };
        let names: Vec<_> = discover(&env).into_iter().map(|p| p.kind).collect();
        assert_eq!(names, [PlaceKind::Home, PlaceKind::Trash, PlaceKind::Root, PlaceKind::Storage]);
        fs::create_dir_all(d.join("play/roms")).unwrap();
        let names: Vec<_> = discover(&env).into_iter().map(|p| p.kind).collect();
        assert!(names.contains(&PlaceKind::Games));
        assert!(!names.contains(&PlaceKind::Bios));
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn which_system_plays_it() {
        let c = PlayConfig::parse(r#"{"romDirs":["/roms","~/Games"],"systems":[
            {"id":"gbc","folders":["gbc"],"extensions":["gbc"]},
            {"id":"md","folders":["md","genesis"],"extensions":["md","bin"]},
            {"id":"psx","folders":["psx"],"extensions":["cue","bin"]},
            {"id":"2048","noContent":true,"folders":[],"extensions":["x"]}]}"#, Path::new("/home/b")).unwrap();
        assert_eq!(c.system_for(Path::new("/roms/gbc/a.gbc")), Some("gbc"));
        assert_eq!(c.system_for(Path::new("/home/b/Games/genesis/deep/a.bin")), Some("md"));
        assert_eq!(c.system_for(Path::new("/tmp/a.gbc")), Some("gbc"));
        assert_eq!(c.system_for(Path::new("/tmp/a.bin")), None, "two systems claim it");
        assert_eq!(c.system_for(Path::new("/tmp/a.x")), None, "no-content systems don't count");
        assert!(c.extensions().contains(&"cue".to_string()));
        assert!(!c.extensions().contains(&"md".to_string()));
        assert!(c.in_rom_folder(Path::new("/roms/md/a.md")));
        assert!(!c.in_rom_folder(Path::new("/tmp/a.md")));
    }
}
