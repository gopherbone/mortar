// SPDX-License-Identifier: GPL-2.0-or-later
//! Apps that need no system rebuild: each has its own Nix profile,
//! `<dir>/<id>` (`mortar.shell.appProfiles`), with a manifest at
//! `share/mortar/app.json`. See DESIGN.md, "Apps in their own profiles".
//!
//! This is the part that needs no compositor: reading manifests, scanning
//! the directory, and merging what it finds into the config's entries.
//! comp/appwatch.rs says when to look again.
//!
//! A bad manifest is a warning and no entry: nothing here can stop the shell.

use crate::config::{AppEntry, Category, GamepadMode};
use mortar_ui::icons::Icon;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

pub const MANIFEST: &str = "share/mortar/app.json";
/// The manifest format this shell reads.
pub const FORMAT: u64 = 1;
/// Far beyond any real manifest; anything larger is not one.
const MAX_MANIFEST: u64 = 64 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    #[allow(dead_code)] // checked before this is parsed
    format: u64,
    id: String,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    category: Option<Category>,
    #[serde(default = "default_icon")]
    icon: Icon,
    /// The program, relative to the profile, then its arguments as they are.
    run: Vec<String>,
    /// Start it through the device's mortar-sdl-run.
    #[serde(default)]
    gpu: bool,
    #[serde(default)]
    gamepad: GamepadMode,
    /// Absolute, or under "~/": the entry hides until these exist.
    #[serde(default)]
    requires: Vec<String>,
}

fn default_icon() -> Icon {
    Icon::Pad
}

/// What an app id may be. It is also the profile's name, and shell code uses
/// ids with a ':' for games.
pub fn valid_id(id: &str) -> bool {
    let mut c = id.chars();
    id.len() <= 64
        && c.next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && c.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        && !is_generation_link(id)
}

/// `<profile>-<N>-link`: a profile's generations live next to it, each with
/// the same manifest inside. Not an app of its own.
fn is_generation_link(name: &str) -> bool {
    name.strip_suffix("-link")
        .and_then(|n| n.rsplit_once('-'))
        .is_some_and(|(_, gen_no)| !gen_no.is_empty() && gen_no.bytes().all(|b| b.is_ascii_digit()))
}

/// Can this directory entry be a profile? Generations and the temporary
/// names nix uses while swapping a link are not, and are not worth a warning.
pub fn is_profile_name(name: &str) -> bool {
    valid_id(name)
}

/// A manifest's text to a launcher entry, without touching the disk.
pub fn parse(text: &str, profile: &Path, sdl_run: Option<&str>) -> Result<AppEntry, String> {
    // the format first: another format may well have other fields
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    match v.get("format").map(|f| f.as_u64()) {
        None => return Err("no \"format\"".into()),
        Some(Some(FORMAT)) => {}
        Some(_) => return Err(format!("format {}: this shell reads format {FORMAT} (a newer system may read it)", v["format"])),
    }
    let m: Manifest = serde_json::from_value(v).map_err(|e| e.to_string())?;
    if !valid_id(&m.id) {
        return Err(format!("bad id '{}'", m.id));
    }
    if m.name.trim().is_empty() {
        return Err("empty name".into());
    }
    let Some((prog, args)) = m.run.split_first() else {
        return Err("empty run".into());
    };
    let rel = Path::new(prog);
    if prog.is_empty() || !rel.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err(format!("run '{prog}' has to be a path inside the profile"));
    }
    let requires = m
        .requires
        .iter()
        .map(|r| if r.starts_with('/') || r.starts_with("~/") { Ok(crate::config::expand_home(r)) } else { Err(format!("requires '{r}' has to be absolute or start with ~/")) })
        .collect::<Result<Vec<_>, _>>()?;
    let mut command: Vec<String> = Vec::new();
    if m.gpu
        && let Some(sdl) = sdl_run
    {
        command.push(sdl.to_string());
    }
    // through the profile, not its store path: an update applies to the next start
    command.push(profile.join(rel).to_string_lossy().into_owned());
    command.extend(args.iter().cloned());
    Ok(AppEntry {
        id: m.id,
        name: m.name,
        description: m.description,
        icon: m.icon,
        command,
        env: BTreeMap::new(),
        gamepad: m.gamepad,
        single_instance: true,
        art: None,
        category: m.category,
        requires,
        profile: Some(profile.to_path_buf()),
    })
}

/// The entry for the profile `dir_name` in the apps directory.
fn load(profile: &Path, dir_name: &str, sdl_run: Option<&str>) -> Result<AppEntry, String> {
    let file = profile.join(MANIFEST);
    let meta = std::fs::metadata(&file).map_err(|e| format!("{MANIFEST}: {e}"))?;
    // not only size: reading a FIFO would hang the shell
    if !meta.is_file() || meta.len() > MAX_MANIFEST {
        return Err(format!("{MANIFEST}: not a manifest (a file up to {MAX_MANIFEST} bytes)"));
    }
    let text = std::fs::read_to_string(&file).map_err(|e| format!("{MANIFEST}: {e}"))?;
    let e = parse(&text, profile, sdl_run)?;
    if e.id != dir_name {
        return Err(format!("id '{}' is not the profile's name", e.id));
    }
    // the program itself: the first element of `run`, wherever sdl-run put it
    let prog = e.command.iter().find(|c| Path::new(c).starts_with(profile)).map(PathBuf::from).unwrap_or_default();
    let m = std::fs::metadata(&prog).map_err(|err| format!("run {}: {err}", prog.display()))?;
    if !m.is_file() || m.permissions().mode() & 0o111 == 0 {
        return Err(format!("run {}: not an executable file", prog.display()));
    }
    Ok(e)
}

pub struct Scanner {
    dir: PathBuf,
    sdl_run: Option<String>,
    /// What was last said about each profile: a rescan repeats nothing.
    said: BTreeMap<String, String>,
}

impl Scanner {
    pub fn new(dir: PathBuf, sdl_run: Option<String>) -> Scanner {
        Scanner { dir, sdl_run, said: BTreeMap::new() }
    }

    /// The directory's valid apps, by name. `current` is the shell's list:
    /// its config entries win over a manifest with the same id.
    pub fn scan(&mut self, current: &[AppEntry]) -> Vec<AppEntry> {
        let mut found = Vec::new();
        let mut said = BTreeMap::new();
        let names: Vec<String> = match std::fs::read_dir(&self.dir) {
            Ok(rd) => rd.flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|n| is_profile_name(n)).collect(),
            // not yet there: nothing has been installed
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                tracing::warn!("app profiles: {}: {e}", self.dir.display());
                Vec::new()
            }
        };
        for name in names {
            let profile = self.dir.join(&name);
            if current.iter().any(|a| a.profile.is_none() && a.id == name) {
                let msg = format!("app profile {}: the config's entry '{name}' wins over its manifest", profile.display());
                if self.said.get(&name) != Some(&msg) {
                    tracing::info!("{msg}");
                }
                said.insert(name, msg);
                continue;
            }
            match load(&profile, &name, self.sdl_run.as_deref()) {
                Ok(e) => found.push(e),
                Err(err) => {
                    let msg = format!("app profile {}: {err}; skipped", profile.display());
                    if self.said.get(&name) != Some(&msg) {
                        tracing::warn!("{msg}");
                    }
                    said.insert(name, msg);
                }
            }
        }
        self.said = said;
        found.sort_by_cached_key(|e| (e.name.to_lowercase(), e.id.clone()));
        found
    }
}

/// What a scan changed in the shell's list.
#[derive(Debug, Default, PartialEq)]
pub struct Change {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
}

impl Change {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

/// Put the discovered apps in the shell's list: after the config's entries,
/// which stay as they are.
pub fn apply(apps: &mut Vec<AppEntry>, found: Vec<AppEntry>) -> Change {
    let mut c = Change::default();
    let old = |id: &str| apps.iter().find(|a| a.profile.is_some() && a.id == id);
    for e in &found {
        match old(&e.id) {
            None => c.added.push(e.id.clone()),
            Some(o) if o != e => c.changed.push(e.id.clone()),
            Some(_) => {}
        }
    }
    c.removed = apps.iter().filter(|a| a.profile.is_some() && !found.iter().any(|e| e.id == a.id)).map(|a| a.id.clone()).collect();
    if !c.is_empty() || apps.iter().filter(|a| a.profile.is_some()).map(|a| &a.id).ne(found.iter().map(|e| &e.id)) {
        apps.retain(|a| a.profile.is_none());
        apps.extend(found);
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    const GOOD: &str = r#"{"format":1,"id":"xcom","name":"X-Com","description":"UFO Defense","category":"games","icon":"pad",
        "run":["bin/xcom","--flag"],"gpu":true,"gamepad":"keyboard","requires":["/data/WORLD.DAT","~/UFO/x"]}"#;

    /// A throwaway apps directory (cargo test also runs on macOS).
    struct Dir(PathBuf);
    impl Dir {
        fn new(tag: &str) -> Dir {
            let d = std::env::temp_dir().join(format!("mortar-discover-{}-{tag}", std::process::id()));
            let _ = fs::remove_dir_all(&d);
            fs::create_dir_all(&d).unwrap();
            Dir(d)
        }
        /// A store path stand-in with a program and this manifest.
        fn store(&self, tag: &str, manifest: &str) -> PathBuf {
            let s = self.0.join(".store").join(tag);
            fs::create_dir_all(s.join("bin")).unwrap();
            fs::create_dir_all(s.join("share/mortar")).unwrap();
            fs::write(s.join("bin/app"), "#!/bin/sh\n").unwrap();
            fs::set_permissions(s.join("bin/app"), fs::Permissions::from_mode(0o755)).unwrap();
            fs::write(s.join(MANIFEST), manifest).unwrap();
            s
        }
        /// What `nix-env -p <dir>/<id> --set <store>` leaves behind.
        fn install(&self, id: &str, generation: u32, store: &Path) {
            let link = format!("{id}-{generation}-link");
            symlink(store, self.0.join(&link)).unwrap();
            let tmp = self.0.join(format!("{id}.tmp"));
            symlink(&link, &tmp).unwrap();
            fs::rename(&tmp, self.0.join(id)).unwrap();
        }
        fn remove(&self, id: &str, generation: u32) {
            fs::remove_file(self.0.join(id)).unwrap();
            fs::remove_file(self.0.join(format!("{id}-{generation}-link"))).unwrap();
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn manifest(id: &str) -> String {
        format!(r#"{{"format":1,"id":"{id}","name":"{id} app","run":["bin/app"]}}"#)
    }
    fn config_entry(id: &str) -> AppEntry {
        serde_json::from_str(&format!(r#"{{"id":"{id}","name":"From config","command":["/bin/{id}"]}}"#)).unwrap()
    }
    fn ids(v: &[AppEntry]) -> Vec<&str> {
        v.iter().map(|a| a.id.as_str()).collect()
    }

    #[test]
    fn parses_every_field() {
        let e = parse(GOOD, Path::new("/p/xcom"), Some("/sdl/mortar-sdl-run")).unwrap();
        assert_eq!((e.id.as_str(), e.name.as_str(), e.description.as_str()), ("xcom", "X-Com", "UFO Defense"));
        assert_eq!(e.category, Some(Category::Games));
        assert_eq!(e.icon, Icon::Pad);
        assert_eq!(e.gamepad, GamepadMode::Keyboard);
        assert_eq!(e.command, ["/sdl/mortar-sdl-run", "/p/xcom/bin/xcom", "--flag"], "through sdl-run and the profile, arguments as they are");
        assert_eq!(e.requires, [PathBuf::from("/data/WORLD.DAT"), crate::config::expand_home("~/UFO/x")]);
        assert_eq!(e.profile.as_deref(), Some(Path::new("/p/xcom")));
        assert!(e.single_instance);
    }

    #[test]
    fn defaults_and_gpu() {
        let min = r#"{"format":1,"id":"a","name":"A","run":["bin/a"]}"#;
        let e = parse(min, Path::new("/p/a"), Some("/sdl")).unwrap();
        assert_eq!(e.command, ["/p/a/bin/a"], "no gpu: not through sdl-run");
        assert_eq!((e.icon, e.gamepad, e.category), (Icon::Pad, GamepadMode::Forward, None));
        assert!(e.requires.is_empty() && e.available());
        // gpu on a device with no sdl-run: it just runs
        let e = parse(r#"{"format":1,"id":"a","name":"A","run":["bin/a"],"gpu":true}"#, Path::new("/p/a"), None).unwrap();
        assert_eq!(e.command, ["/p/a/bin/a"]);
    }

    #[test]
    fn bad_manifests_say_why() {
        let bad = |m: &str, why: &str| {
            let e = parse(m, Path::new("/p/a"), None).expect_err(m);
            assert!(e.contains(why), "{m}: {e}");
        };
        bad("not json", "not JSON");
        bad("[1]", "format");
        bad(r#"{"id":"a","name":"A","run":["bin/a"]}"#, "no \"format\"");
        bad(r#"{"format":2,"id":"a","future":true}"#, "format 2");
        bad(r#"{"format":"1","id":"a","name":"A","run":["bin/a"]}"#, "format \"1\"");
        bad(r#"{"format":1,"name":"A","run":["bin/a"]}"#, "id");
        bad(r#"{"format":1,"id":"a","run":["bin/a"]}"#, "name");
        bad(r#"{"format":1,"id":"a","name":"A"}"#, "run");
        bad(r#"{"format":1,"id":"a","name":" ","run":["bin/a"]}"#, "empty name");
        bad(r#"{"format":1,"id":"a","name":"A","run":[]}"#, "empty run");
        bad(r#"{"format":1,"id":"a","name":"A","run":[""]}"#, "inside the profile");
        bad(r#"{"format":1,"id":"a","name":"A","run":["/bin/sh"]}"#, "inside the profile");
        bad(r#"{"format":1,"id":"a","name":"A","run":["../../bin/sh"]}"#, "inside the profile");
        bad(r#"{"format":1,"id":"a","name":"A","run":["bin/a"],"requires":["rel/file"]}"#, "requires");
        bad(r#"{"format":1,"id":"a","name":"A","run":["bin/a"],"category":"tools"}"#, "tools");
        bad(r#"{"format":1,"id":"a","name":"A","run":["bin/a"],"icon":"nope"}"#, "nope");
        bad(r#"{"format":1,"id":"a","name":"A","run":["bin/a"],"gamepad":"mouse"}"#, "mouse");
        bad(r#"{"format":1,"id":"a","name":"A","run":["bin/a"],"gpus":true}"#, "gpus");
        bad(r#"{"format":1,"id":"game:gbc:x","name":"A","run":["bin/a"]}"#, "bad id");
        bad(r#"{"format":1,"id":"A b","name":"A","run":["bin/a"]}"#, "bad id");
    }

    #[test]
    fn names_that_are_not_profiles() {
        for good in ["openxcom", "m8", "pico-8", "a_b", "9lives", "link", "x-link", "a-b-link"] {
            assert!(is_profile_name(good), "{good}");
        }
        for bad in ["openxcom-1-link", "a-b-12-link", "openxcom.tmp-123-4", ".hidden", "", "Up", &"a".repeat(65)] {
            assert!(!is_profile_name(bad), "{bad}");
        }
    }

    #[test]
    fn scan_skips_bad_profiles_and_generations() {
        let d = Dir::new("bad");
        let mut s = Scanner::new(d.0.clone(), None);
        // a good one, with its generation link next to it, and an older generation
        let good = d.store("good", &manifest("good"));
        d.install("good", 1, &good);
        symlink(&good, d.0.join("good-2-link")).unwrap();
        // not JSON
        d.install("junk", 1, &d.store("junk", "{ nope"));
        // says it is another app
        d.install("liar", 1, &d.store("liar", &manifest("good")));
        // a program that is not there
        d.install("noprog", 1, &d.store("noprog", r#"{"format":1,"id":"noprog","name":"N","run":["bin/gone"]}"#));
        // a program that does not execute
        let noexec = d.store("noexec", &manifest("noexec"));
        fs::set_permissions(noexec.join("bin/app"), fs::Permissions::from_mode(0o644)).unwrap();
        d.install("noexec", 1, &noexec);
        // no manifest at all; a dangling link; a plain file; a loop
        let none = d.store("none", "");
        fs::remove_file(none.join(MANIFEST)).unwrap();
        d.install("none", 1, &none);
        symlink("/nonexistent/store/path", d.0.join("dangling")).unwrap();
        fs::write(d.0.join("afile"), "x").unwrap();
        symlink("loop", d.0.join("loop")).unwrap();
        // far too big, and invalid UTF-8
        d.install("huge", 1, &d.store("huge", &" ".repeat(MAX_MANIFEST as usize + 1)));
        let binary = d.store("binary", "");
        fs::write(binary.join(MANIFEST), [0xff, 0xfe, 0x00]).unwrap();
        d.install("binary", 1, &binary);

        let found = s.scan(&[]);
        assert_eq!(ids(&found), ["good"]);
        assert_eq!(s.said.len(), 10, "each bad one was reported: {:?}", s.said);
        assert!(s.said["liar"].contains("not the profile's name"));
        assert!(s.said["noexec"].contains("not an executable"));
        assert!(s.said["huge"].contains("not a manifest"));
        // fixed: no longer reported
        d.remove("junk", 1);
        s.scan(&[]);
        assert!(!s.said.contains_key("junk"));
    }

    #[test]
    fn a_fifo_for_a_manifest_does_not_hang() {
        let d = Dir::new("fifo");
        let store = d.store("f", "");
        let file = store.join(MANIFEST);
        fs::remove_file(&file).unwrap();
        let c = std::ffi::CString::new(file.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
        d.install("f", 1, &store);
        let mut s = Scanner::new(d.0.clone(), None);
        assert!(s.scan(&[]).is_empty());
        assert!(s.said["f"].contains("not a manifest"));
    }

    #[test]
    fn a_missing_directory_is_no_apps() {
        let mut s = Scanner::new(PathBuf::from("/nonexistent/apps"), None);
        assert!(s.scan(&[]).is_empty());
        assert!(s.said.is_empty());
    }

    #[test]
    fn config_wins_over_a_manifest() {
        let d = Dir::new("over");
        d.install("xcom", 1, &d.store("xcom", &manifest("xcom")));
        d.install("other", 1, &d.store("other", &manifest("other")));
        let mut s = Scanner::new(d.0.clone(), None);
        let mut apps = vec![config_entry("xcom")];
        let found = s.scan(&apps);
        assert_eq!(ids(&found), ["other"], "the shadowed manifest is not offered");
        assert!(s.said["xcom"].contains("config's entry 'xcom' wins"), "and the log says so");
        let c = apply(&mut apps, found);
        assert_eq!(c.added, ["other"]);
        assert_eq!(ids(&apps), ["xcom", "other"]);
        assert_eq!(apps[0].name, "From config");
        assert_eq!(apps[0].command, ["/bin/xcom"]);
        // again: the discovered copy of a shadowing id is not mistaken for config
        let found = s.scan(&apps);
        assert!(apply(&mut apps, found).is_empty());
        assert_eq!(ids(&apps), ["xcom", "other"]);
    }

    #[test]
    fn add_change_remove() {
        let d = Dir::new("live");
        let mut s = Scanner::new(d.0.clone(), None);
        let mut apps = vec![config_entry("term")];
        let mut rescan = |apps: &mut Vec<AppEntry>| {
            let f = s.scan(apps);
            apply(apps, f)
        };
        assert!(rescan(&mut apps).is_empty(), "nothing installed, nothing changes");

        d.install("zed", 1, &d.store("zed1", &manifest("zed")));
        let c = rescan(&mut apps);
        assert_eq!((c.added.as_slice(), c.removed.len(), c.changed.len()), (["zed".to_string()].as_slice(), 0, 0));
        assert_eq!(ids(&apps), ["term", "zed"]);
        assert!(rescan(&mut apps).is_empty(), "a second look changes nothing");

        d.install("abc", 1, &d.store("abc1", &manifest("abc")));
        assert_eq!(rescan(&mut apps).added, ["abc"]);
        assert_eq!(ids(&apps), ["term", "abc", "zed"], "config first, then by name");

        // an update that only moves the store path: the entry is the same
        // (it runs through the profile link)
        d.install("zed", 2, &d.store("zed2", &manifest("zed")));
        assert!(rescan(&mut apps).is_empty());
        // one that changes the manifest
        d.install("zed", 3, &d.store("zed3", &manifest("zed").replace("zed app", "Zed 3")));
        assert_eq!(rescan(&mut apps).changed, ["zed"]);
        assert_eq!(apps.iter().find(|a| a.id == "zed").unwrap().name, "Zed 3");
        // an update that breaks it: warned about, and it goes
        d.install("zed", 4, &d.store("zed4", "{ broken"));
        assert_eq!(rescan(&mut apps).removed, ["zed"]);
        assert_eq!(ids(&apps), ["term", "abc"]);

        fs::remove_file(d.0.join("abc")).unwrap();
        assert_eq!(rescan(&mut apps).removed, ["abc"]);
        assert_eq!(ids(&apps), ["term"], "the config's entries stay");
    }

    #[test]
    fn a_running_app_outlives_its_profile() {
        use crate::apps::Apps;
        let d = Dir::new("running");
        let mut s = Scanner::new(d.0.clone(), None);
        d.install("zed", 1, &d.store("zed", &manifest("zed")));
        let mut list = Vec::new();
        let f = s.scan(&list);
        apply(&mut list, f);

        // running (as Apps sees it: its own copy of the entry)
        let mut apps: Apps<u8> = Apps::default();
        let e = list[0].clone();
        let uid = apps.add(Some(e.id.clone()), e.name.clone(), e.icon, e.gamepad).unwrap();
        apps.get_mut(uid).unwrap().spec = Some(e);
        apps.get_mut(uid).unwrap().windows.push(1);

        d.remove("zed", 1);
        let f = s.scan(&list);
        assert_eq!(apply(&mut list, f).removed, ["zed"]);
        assert!(list.is_empty());
        assert!(apps.tick().is_empty(), "removing the entry does not touch the app");
        assert_eq!(apps.by_entry("zed").map(|a| a.uid), Some(uid));
        assert!(apps.get(uid).unwrap().spec.as_ref().is_some_and(|s| s.profile.is_some()));
    }

    #[test]
    fn requires_hides_until_the_file_is_there() {
        let d = Dir::new("req");
        let data = d.0.join(".data/WORLD.DAT");
        let m = format!(r#"{{"format":1,"id":"a","name":"A","run":["bin/app"],"requires":["{}"]}}"#, data.display());
        d.install("a", 1, &d.store("a", &m));
        let mut s = Scanner::new(d.0.clone(), None);
        let found = s.scan(&[]);
        assert!(!found[0].available(), "listed, but not offered");
        fs::create_dir_all(data.parent().unwrap()).unwrap();
        fs::write(&data, "x").unwrap();
        assert!(found[0].available());
    }
}
