// SPDX-License-Identifier: GPL-2.0-or-later
//! The game library: ROM folders per system, display names, archives,
//! recently played.

use crate::config::{Config, System, expand};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const ARCHIVES: [&str; 2] = ["zip", "7z"];

#[derive(Clone, Debug, PartialEq)]
pub struct Game {
    pub system: String,
    pub path: PathBuf,
    pub name: String,
    /// "(USA)", "[!]" groups from the file name, without brackets.
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Shelf {
    pub games: Vec<Game>,
    /// The folders that were looked in (shown when empty).
    pub dirs: Vec<PathBuf>,
}

pub fn ext_of(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// "Super Mario World (USA) [!]" -> ("Super Mario World", ["USA", "!"]).
pub fn display_name(stem: &str) -> (String, Vec<String>) {
    let mut name = String::new();
    let mut tags = Vec::new();
    let mut depth = 0;
    let mut cur = String::new();
    for ch in stem.chars() {
        match ch {
            '(' | '[' => {
                if depth == 0 {
                    cur.clear();
                } else {
                    cur.push(ch);
                }
                depth += 1;
            }
            ')' | ']' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    let t = cur.trim().to_string();
                    if !t.is_empty() {
                        tags.push(t);
                    }
                } else {
                    cur.push(ch);
                }
            }
            _ if depth > 0 => cur.push(ch),
            _ => name.push(ch),
        }
    }
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let name = if name.is_empty() { stem.to_string() } else { name.replace('_', " ") };
    (name, tags)
}

/// Files a playlist (.m3u) or cue sheet (.cue) refers to: hidden from the
/// list so a disc shows up once.
fn referenced(p: &Path) -> Vec<String> {
    let Ok(s) = std::fs::read_to_string(p) else { return Vec::new() };
    let mut out = Vec::new();
    match ext_of(p).as_str() {
        "cue" => {
            for l in s.lines() {
                let l = l.trim();
                if l.len() > 5 && l[..5].eq_ignore_ascii_case("FILE ") {
                    let rest = &l[5..];
                    let name = if let Some(r) = rest.strip_prefix('"') { r.split('"').next().unwrap_or("") } else { rest.split_whitespace().next().unwrap_or("") };
                    out.push(name.to_lowercase());
                }
            }
        }
        "m3u" => {
            for l in s.lines() {
                let l = l.trim();
                if !l.is_empty() && !l.starts_with('#') {
                    out.push(Path::new(l).file_name().map(|f| f.to_string_lossy().to_lowercase()).unwrap_or_default());
                }
            }
        }
        _ => {}
    }
    out
}

fn walk(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        if p.is_dir() {
            if depth > 0 {
                walk(&p, depth - 1, out);
            }
        } else {
            out.push(p);
        }
    }
}

/// The ROM folders of a system: every root x every folder name.
pub fn system_dirs(cfg: &Config, sys: &System) -> Vec<PathBuf> {
    let mut v = Vec::new();
    for root in &cfg.rom_dirs {
        for f in &sys.folders {
            v.push(expand(root).join(f));
        }
    }
    v
}

pub fn scan_system(cfg: &Config, sys: &System) -> Shelf {
    let dirs = system_dirs(cfg, sys);
    let mut files = Vec::new();
    for d in &dirs {
        walk(d, 2, &mut files);
    }
    let hidden: HashSet<String> = files
        .iter()
        .filter(|p| matches!(ext_of(p).as_str(), "cue" | "m3u"))
        .flat_map(|p| {
            // a .cue listed in an .m3u is hidden too; the .m3u stays
            referenced(p)
        })
        .collect();
    let mut games: Vec<Game> = files
        .into_iter()
        .filter(|p| {
            let e = ext_of(p);
            (sys.extensions.contains(&e) || ARCHIVES.contains(&e.as_str()))
                && !hidden.contains(&p.file_name().unwrap_or_default().to_string_lossy().to_lowercase())
        })
        .map(|p| {
            let (name, tags) = display_name(&p.file_stem().unwrap_or_default().to_string_lossy());
            Game { system: sys.id.clone(), path: p, name, tags }
        })
        .collect();
    games.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.path.cmp(&b.path)));
    games.dedup_by(|a, b| a.path == b.path);
    Shelf { games, dirs }
}

pub fn scan(cfg: &Config) -> Vec<Shelf> {
    cfg.systems.iter().map(|s| scan_system(cfg, s)).collect()
}

// --------------------------------------------------------------- recents ----

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Recent {
    pub system: String,
    pub path: PathBuf,
}

pub struct Recents {
    file: PathBuf,
    pub list: Vec<Recent>,
}

impl Recents {
    pub fn load(data_dir: &Path) -> Recents {
        let file = data_dir.join("recent.json");
        let list = std::fs::read_to_string(&file).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        Recents { file, list }
    }

    pub fn touch(&mut self, system: &str, path: &Path) {
        self.list.retain(|r| r.path != path);
        self.list.insert(0, Recent { system: system.into(), path: path.into() });
        self.list.truncate(24);
        self.save();
    }

    /// Drop entries whose file is gone.
    pub fn prune(&mut self) {
        let n = self.list.len();
        self.list.retain(|r| r.path.exists());
        if self.list.len() != n {
            self.save();
        }
    }

    fn save(&self) {
        if let Some(d) = self.file.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let tmp = self.file.with_extension("json.tmp");
        if std::fs::write(&tmp, serde_json::to_string_pretty(&self.list).unwrap_or_default()).is_ok() {
            let _ = std::fs::rename(&tmp, &self.file);
        }
    }
}

// -------------------------------------------------------------- archives ----

/// The file to hand the core: archives go as-is when the core reads them
/// (or the system keeps them, e.g. arcade sets), else the first entry with
/// a usable extension is extracted into `scratch`.
pub fn prepare(path: &Path, sys: &System, core_exts: &[String], scratch: &Path) -> Result<PathBuf, String> {
    let ext = ext_of(path);
    if !ARCHIVES.contains(&ext.as_str()) || sys.keep_archives || core_exts.contains(&ext) {
        return Ok(path.to_path_buf());
    }
    let usable = |name: &str| {
        let e = ext_of(Path::new(name));
        !e.is_empty() && (core_exts.contains(&e) || sys.extensions.contains(&e)) && !ARCHIVES.contains(&e.as_str())
    };
    let _ = std::fs::remove_dir_all(scratch);
    std::fs::create_dir_all(scratch).map_err(|e| format!("{}: {e}", scratch.display()))?;
    let err = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    match ext.as_str() {
        "zip" => {
            let f = std::fs::File::open(path).map_err(|e| err(&e))?;
            let mut z = zip::ZipArchive::new(f).map_err(|e| err(&e))?;
            for i in 0..z.len() {
                let mut entry = z.by_index(i).map_err(|e| err(&e))?;
                let name = entry.name().map_err(|e| err(&e))?.to_string();
                if entry.is_dir() || !usable(&name) {
                    continue;
                }
                let out = scratch.join(Path::new(&name).file_name().unwrap_or_default());
                let mut w = std::fs::File::create(&out).map_err(|e| err(&e))?;
                std::io::copy(&mut entry, &mut w).map_err(|e| err(&e))?;
                return Ok(out);
            }
            Err(format!("{}: nothing this system can play inside", path.display()))
        }
        _ => {
            let mut found: Option<PathBuf> = None;
            let mut r = sevenz_rust2::ArchiveReader::open(path, sevenz_rust2::Password::empty()).map_err(|e| err(&e))?;
            r.for_each_entries(|entry, reader| {
                if found.is_some() || entry.is_directory() || !usable(entry.name()) {
                    // drain so the stream stays in sync
                    std::io::copy(reader, &mut std::io::sink())?;
                    return Ok(true);
                }
                let out = scratch.join(Path::new(entry.name()).file_name().unwrap_or_default());
                let mut w = std::fs::File::create(&out)?;
                std::io::copy(reader, &mut w)?;
                found = Some(out);
                Ok(true)
            })
            .map_err(|e| err(&e))?;
            found.ok_or_else(|| format!("{}: nothing this system can play inside", path.display()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(display_name("Super Mario World (USA) [!]"), ("Super Mario World".into(), vec!["USA".into(), "!".into()]));
        assert_eq!(display_name("Pokemon - Red Version (USA, Europe) (SGB Enhanced)").0, "Pokemon - Red Version");
        assert_eq!(display_name("my_homebrew").0, "my homebrew");
        assert_eq!(display_name("(Only tags)").0, "(Only tags)");
        assert_eq!(display_name("Nested (a (b) c) end"), ("Nested end".into(), vec!["a (b) c".into()]));
    }

    #[test]
    fn scan_hides_cue_tracks_and_unzips() {
        let root = std::env::temp_dir().join(format!("mp-lib-{}", std::process::id()));
        let d = root.join("roms/psx");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("Game (USA).cue"), "FILE \"Game (USA) (Track 1).bin\" BINARY\nFILE \"Game (USA) (Track 2).bin\" BINARY\n").unwrap();
        std::fs::write(d.join("Game (USA) (Track 1).bin"), b"x").unwrap();
        std::fs::write(d.join("Game (USA) (Track 2).bin"), b"x").unwrap();
        std::fs::write(d.join("Other.bin"), b"x").unwrap();
        std::fs::write(d.join(".hidden.bin"), b"x").unwrap();
        std::fs::write(d.join("readme.txt"), b"x").unwrap();
        let cfg = Config { rom_dirs: vec![root.join("roms").to_string_lossy().into()], ..Default::default() };
        let sys = System { id: "psx".into(), folders: vec!["psx".into()], extensions: vec!["cue".into(), "bin".into()], ..Default::default() };
        let shelf = scan_system(&cfg, &sys);
        let names: Vec<_> = shelf.games.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, vec!["Game", "Other"]);

        // a zip with one ROM gets extracted when the core can't read zips
        let zp = d.join("z.zip");
        {
            let f = std::fs::File::create(&zp).unwrap();
            let mut w = zip::ZipWriter::new(f);
            w.start_file("inner.bin", zip::write::SimpleFileOptions::default()).unwrap();
            std::io::Write::write_all(&mut w, b"ROMDATA").unwrap();
            w.finish().unwrap();
        }
        let scratch = root.join("scratch");
        let out = prepare(&zp, &sys, &["bin".into()], &scratch).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"ROMDATA");
        // and passed through when it can
        assert_eq!(prepare(&zp, &sys, &["zip".into()], &scratch).unwrap(), zp);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn recents_order() {
        let root = std::env::temp_dir().join(format!("mp-rec-{}", std::process::id()));
        let mut r = Recents::load(&root);
        r.touch("nes", Path::new("/a.nes"));
        r.touch("gb", Path::new("/b.gb"));
        r.touch("nes", Path::new("/a.nes"));
        let r2 = Recents::load(&root);
        assert_eq!(r2.list.iter().map(|x| x.path.to_str().unwrap()).collect::<Vec<_>>(), vec!["/a.nes", "/b.gb"]);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
