// SPDX-License-Identifier: GPL-2.0-or-later
//! The frontend's config: directories, hotkeys and the curated systems
//! (core, option presets, controls, scaling, BIOS). Written by
//! modules/mortar-play.nix to /etc/mortar-play/config.json; nothing is
//! configurable from the device itself by design.

use crate::input::{Button, GAME_BUTTONS, PadId};
use crate::video::{Filter, Fit, View};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    /// ROM roots; each has one folder per system (`<root>/<folder>/`).
    pub rom_dirs: Vec<String>,
    pub bios_dir: String,
    pub save_dir: String,
    pub state_dir: String,
    /// Recents, last-played, scratch space for extracted archives.
    pub data_dir: String,
    pub audio_latency_ms: u32,
    pub autosave_seconds: u32,
    /// Load the automatic state (saved on quit) when a game starts.
    pub auto_resume: bool,
    pub theme: String,
    pub hotkeys: Hotkeys,
    pub systems: Vec<System>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            rom_dirs: vec!["/var/lib/mortar-play/roms".into(), "~/Games".into()],
            bios_dir: "/var/lib/mortar-play/bios".into(),
            save_dir: "/var/lib/mortar-play/saves".into(),
            state_dir: "/var/lib/mortar-play/states".into(),
            data_dir: "/var/lib/mortar-play".into(),
            audio_latency_ms: 64,
            autosave_seconds: 10,
            auto_resume: true,
            theme: "light".into(),
            hotkeys: Hotkeys::default(),
            systems: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Hotkeys {
    /// Chords that open the in-game menu, e.g. "menu", "f1", "select+menu".
    /// Under mortar-shell a lone MENU belongs to the shell; MENU pressed
    /// while SELECT is held reaches the app.
    pub menu: Vec<String>,
    /// Held: fast-forward (per-system override in `System::fast_forward`).
    pub fast_forward: String,
    /// Fast-forward speed (x normal).
    pub fast_forward_speed: u32,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Hotkeys { menu: vec!["menu".into(), "f1".into(), "select+menu".into()], fast_forward: "r2".into(), fast_forward_speed: 3 }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Bios {
    /// Any one of these files satisfies the entry (case-insensitive).
    pub files: Vec<String>,
    pub required: bool,
    /// Shown on the BIOS screen, e.g. "PlayStation BIOS (scph5501.bin)".
    pub description: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct System {
    pub id: String,
    pub name: String,
    /// Short tag, e.g. "GBA".
    pub tag: String,
    /// Folder names under each ROM root.
    pub folders: Vec<String>,
    /// ROM extensions shown in the list (lowercase, no dot); archives
    /// (zip, 7z) are always shown.
    pub extensions: Vec<String>,
    /// Path to the libretro core (.so / .dylib).
    pub core: String,
    /// Core option presets (key -> value).
    pub options: BTreeMap<String, String>,
    /// The default picture (Display in the menus overrides it per system
    /// or per game; `crate::display`): smooth / sharp / sharp-bilinear /
    /// integer.
    pub scaling: Filter,
    /// fit (the display aspect ratio) or stretch.
    pub aspect: Fit,
    /// Brick button -> RetroPad button ("none" to unbind). Unlisted buttons
    /// map to the RetroPad button of the same name.
    pub controls: HashMap<String, String>,
    /// Overrides `Hotkeys::fast_forward` ("" = global, "none" = off).
    pub fast_forward: String,
    pub bios: Vec<Bios>,
    /// Pass archives to the core as-is (arcade sets) instead of extracting.
    pub keep_archives: bool,
    /// The core runs without a game (e.g. 2048): the home entry starts it.
    pub no_content: bool,
    pub order: i32,
}

impl Default for System {
    fn default() -> Self {
        System {
            id: String::new(),
            name: String::new(),
            tag: String::new(),
            folders: Vec::new(),
            extensions: Vec::new(),
            core: String::new(),
            options: BTreeMap::new(),
            scaling: Filter::Sharp,
            aspect: Fit::Fit,
            controls: HashMap::new(),
            fast_forward: String::new(),
            bios: Vec::new(),
            keep_archives: false,
            no_content: false,
            order: 100,
        }
    }
}

/// Expand a leading `~/` to $HOME.
pub fn expand(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return Path::new(&home).join(rest);
    }
    PathBuf::from(p)
}

impl Config {
    pub fn load(path: &Path) -> Result<Config, String> {
        let s = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut c: Config = serde_json::from_str(&s).map_err(|e| format!("{}: {e}", path.display()))?;
        c.systems.sort_by(|a, b| a.order.cmp(&b.order).then(a.id.cmp(&b.id)));
        Ok(c)
    }

    pub fn system(&self, id: &str) -> Option<&System> {
        self.systems.iter().find(|s| s.id == id)
    }

    pub fn bios_dir(&self) -> PathBuf {
        expand(&self.bios_dir)
    }
    pub fn data_dir(&self) -> PathBuf {
        expand(&self.data_dir)
    }
}

impl System {
    /// The default Display choice (before the user's saved ones).
    pub fn view(&self) -> View {
        View { filter: self.scaling, fit: self.aspect }
    }

    /// The RetroPad button for each Brick button, after this system's
    /// remaps and with the fast-forward button taken out.
    pub fn pad_map(&self, hot: &Hotkeys) -> HashMap<Button, PadId> {
        let mut m = HashMap::new();
        for b in GAME_BUTTONS {
            if let Some(p) = PadId::same_as(b) {
                m.insert(b, p);
            }
        }
        for (k, v) in &self.controls {
            let Some(b) = Button::parse(k) else {
                eprintln!("mortar-play: {}: unknown button '{k}' in controls", self.id);
                continue;
            };
            if v == "none" {
                m.remove(&b);
            } else if let Some(p) = PadId::parse(v) {
                m.insert(b, p);
            } else {
                eprintln!("mortar-play: {}: unknown RetroPad button '{v}'", self.id);
            }
        }
        if let Some(ff) = self.fast_forward_button(hot) {
            m.remove(&ff);
        }
        m
    }

    pub fn fast_forward_button(&self, hot: &Hotkeys) -> Option<Button> {
        let name = if self.fast_forward.is_empty() { &hot.fast_forward } else { &self.fast_forward };
        Button::parse(name)
    }

    /// Missing BIOS entries: (description, files, required).
    pub fn missing_bios(&self, bios_dir: &Path, rom_dir: Option<&Path>) -> Vec<&Bios> {
        let mut names: Vec<String> = Vec::new();
        for d in [Some(bios_dir), rom_dir].into_iter().flatten() {
            if let Ok(rd) = std::fs::read_dir(d) {
                names.extend(rd.flatten().map(|e| e.file_name().to_string_lossy().to_lowercase()));
            }
        }
        self.bios.iter().filter(|b| !b.files.iter().any(|f| names.contains(&f.to_lowercase()))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_map() {
        let j = r#"{
          "romDirs": ["/r"], "hotkeys": {"fastForward": "r2"},
          "systems": [
            {"id": "psx", "name": "PlayStation", "core": "x.so", "order": 5, "fastForward": "f2",
             "controls": {"l2": "l2", "y": "none"}, "scaling": "sharp-bilinear", "aspect": "stretch",
             "bios": [{"files": ["SCPH5501.BIN", "scph1001.bin"], "required": false}]},
            {"id": "gb", "name": "Game Boy", "core": "y.so", "order": 1, "controls": {"x": "a"}}
          ]}"#;
        let dir = std::env::temp_dir().join(format!("mp-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("c.json");
        std::fs::write(&p, j).unwrap();
        let c = Config::load(&p).unwrap();
        assert_eq!(c.systems[0].id, "gb");
        assert_eq!(c.audio_latency_ms, 64);
        let gb = c.system("gb").unwrap();
        let m = gb.pad_map(&c.hotkeys);
        assert_eq!(m[&Button::X], PadId::A);
        assert!(!m.contains_key(&Button::R2), "R2 is fast-forward");
        let psx = c.system("psx").unwrap();
        assert_eq!(psx.view(), View { filter: Filter::SharpBilinear, fit: Fit::Stretch });
        assert_eq!(gb.view(), View { filter: Filter::Sharp, fit: Fit::Fit });
        let m = psx.pad_map(&c.hotkeys);
        assert_eq!(m[&Button::R2], PadId::R2);
        assert!(!m.contains_key(&Button::Y));
        assert_eq!(psx.fast_forward_button(&c.hotkeys), Some(Button::F2));
        assert_eq!(psx.missing_bios(&dir, None).len(), 1);
        std::fs::write(dir.join("scph5501.bin"), b"x").unwrap();
        assert!(psx.missing_bios(&dir, None).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
