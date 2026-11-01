// SPDX-License-Identifier: GPL-2.0-or-later
//! A running game: the core, its saves (SRAM autosave, state slots with
//! thumbnails, the automatic resume state) and its input map.

use crate::config::{Config, System, expand};
use crate::input::{Button, Held, PadId};
use crate::library;
use crate::lr::core::{Core, Host};
use crate::lr::ffi;
use crate::lr::options::Options;
use crate::video::Frame;
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

pub const SLOTS: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    /// Written on quit, loaded on the next start (auto resume).
    Auto,
    N(u8),
}

impl Slot {
    fn suffix(self) -> String {
        match self {
            Slot::Auto => "auto".into(),
            Slot::N(n) => n.to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SavePaths {
    pub srm: PathBuf,
    pub rtc: PathBuf,
    states: PathBuf,
    base: String,
}

impl SavePaths {
    pub fn new(cfg: &Config, sys: &System, game: &Path) -> SavePaths {
        let base = game.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        let saves = expand(&cfg.save_dir).join(&sys.id);
        let states = expand(&cfg.state_dir).join(&sys.id);
        SavePaths { srm: saves.join(format!("{base}.srm")), rtc: saves.join(format!("{base}.rtc")), states, base }
    }
    pub fn state(&self, s: Slot) -> PathBuf {
        self.states.join(format!("{}.state.{}", self.base, s.suffix()))
    }
    pub fn thumb(&self, s: Slot) -> PathBuf {
        self.states.join(format!("{}.state.{}.png", self.base, s.suffix()))
    }
}

fn write_atomic(path: &Path, data: &[u8]) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

pub struct Session {
    pub core: Core,
    pub sys: System,
    /// The file the user picked (an archive stays the archive).
    pub game: Option<PathBuf>,
    pub saves: Option<SavePaths>,
    pad_map: HashMap<Button, PadId>,
    sram_copy: Vec<u8>,
    last_autosave: Instant,
    autosave_every: Duration,
    pub frames: u64,
    /// Set when the auto state was loaded at start.
    pub resumed: bool,
}

impl Session {
    /// Load `sys`'s core and `rom` (None: no content, e.g. 2048).
    pub fn start(cfg: &Config, sys: &System, rom: Option<&Path>) -> Result<Session, String> {
        let core_path = expand(&sys.core);
        let save_dir = expand(&cfg.save_dir).join(&sys.id);
        let bios = cfg.bios_dir();
        for d in [&save_dir, &bios] {
            let _ = std::fs::create_dir_all(d);
        }
        let presets = sys.options.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        let host = Host::new(Options::new(presets), &bios, &save_dir, &core_path);
        let mut core = Core::open(&core_path, host)?;
        let content = match rom {
            Some(p) => {
                let scratch = scratch_dir(cfg);
                Some(library::prepare(p, sys, &core.info.extensions, &scratch)?)
            }
            None => None,
        };
        core.load_game(content.as_deref())?;
        let saves = rom.map(|p| SavePaths::new(cfg, sys, p));
        let mut s = Session {
            core,
            sys: sys.clone(),
            game: rom.map(Path::to_path_buf),
            saves,
            pad_map: sys.pad_map(&cfg.hotkeys),
            sram_copy: Vec::new(),
            last_autosave: Instant::now(),
            autosave_every: Duration::from_secs(cfg.autosave_seconds.max(1) as u64),
            frames: 0,
            resumed: false,
        };
        s.load_sram();
        if cfg.auto_resume && s.has_state(Slot::Auto) {
            s.resumed = s.load_state(Slot::Auto).is_ok();
        }
        Ok(s)
    }

    pub fn host(&self) -> &Host {
        self.core.host_ref()
    }

    pub fn fps(&self) -> f64 {
        let f = self.core.av.timing.fps;
        if f > 1.0 { f } else { 60.0 }
    }

    pub fn sample_rate(&self) -> f64 {
        let r = self.core.av.timing.sample_rate;
        if r > 1000.0 { r } else { 44100.0 }
    }

    /// Display aspect ratio of `display_frame()`.
    pub fn aspect(&self) -> f32 {
        let g = self.core.av.geometry;
        let f = &self.host().frame;
        if g.aspect_ratio > 0.0 {
            g.aspect_ratio
        } else if self.host().rotation % 2 == 1 && f.w > 0 {
            f.h as f32 / f.w as f32
        } else if f.h > 0 {
            f.w as f32 / f.h as f32
        } else {
            4.0 / 3.0
        }
    }

    /// The last frame, rotated as the core asked.
    pub fn display_frame(&self) -> Cow<'_, Frame> {
        let h = self.host();
        if h.rotation == 0 { Cow::Borrowed(&h.frame) } else { Cow::Owned(h.frame.rotated(h.rotation)) }
    }

    /// Run one frame with `held` buttons. Returns true if the AV info changed.
    pub fn run(&mut self, held: &Held, fast_forward: bool, audio_occupancy: Option<u32>) -> bool {
        let mask = held.retropad(&self.pad_map);
        let h = self.core.host();
        h.pad[0] = mask;
        h.fast_forward = fast_forward;
        let changed = self.core.run(audio_occupancy).is_some();
        self.frames += 1;
        changed
    }

    pub fn audio(&self) -> &[i16] {
        &self.host().audio
    }

    pub fn reset(&mut self) {
        self.core.reset();
    }

    // ---------------------------------------------------------- SRAM ----

    fn load_sram(&mut self) {
        let Some(sp) = self.saves.clone() else { return };
        for (id, path) in [(ffi::MEMORY_SAVE_RAM, &sp.srm), (ffi::MEMORY_RTC, &sp.rtc)] {
            if let (Ok(data), Some(mem)) = (std::fs::read(path), self.core.memory(id)) {
                let n = data.len().min(mem.len());
                mem[..n].copy_from_slice(&data[..n]);
            }
        }
        self.sram_copy = self.core.memory(ffi::MEMORY_SAVE_RAM).map(|m| m.to_vec()).unwrap_or_default();
    }

    /// Write save RAM if it changed since the last write.
    pub fn save_sram(&mut self) -> Result<bool, String> {
        let Some(sp) = self.saves.clone() else { return Ok(false) };
        let Some(mem) = self.core.memory(ffi::MEMORY_SAVE_RAM) else { return Ok(false) };
        if mem == self.sram_copy.as_slice() && sp.srm.exists() {
            return Ok(false);
        }
        // never replace a real save with an all-zero/0xff fresh SRAM before the game wrote anything
        let blank = mem.iter().all(|&b| b == 0) || mem.iter().all(|&b| b == 0xff);
        if blank && !sp.srm.exists() {
            self.sram_copy = mem.to_vec();
            return Ok(false);
        }
        let data = mem.to_vec();
        write_atomic(&sp.srm, &data)?;
        if let Some(rtc) = self.core.memory(ffi::MEMORY_RTC) {
            let rtc = rtc.to_vec();
            write_atomic(&sp.rtc, &rtc)?;
        }
        self.sram_copy = data;
        Ok(true)
    }

    /// Call once per frame: writes SRAM every `autosave_seconds` if it changed.
    pub fn tick_autosave(&mut self) {
        if self.last_autosave.elapsed() >= self.autosave_every {
            self.last_autosave = Instant::now();
            if let Err(e) = self.save_sram() {
                eprintln!("mortar-play: autosave: {e}");
            }
        }
    }

    // -------------------------------------------------------- states ----

    pub fn has_state(&self, s: Slot) -> bool {
        self.saves.as_ref().is_some_and(|p| p.state(s).exists())
    }

    pub fn state_time(&self, s: Slot) -> Option<SystemTime> {
        let p = self.saves.as_ref()?.state(s);
        std::fs::metadata(p).ok()?.modified().ok()
    }

    pub fn state_thumb(&self, s: Slot) -> Option<Frame> {
        Frame::load_png(&self.saves.as_ref()?.thumb(s))
    }

    pub fn save_state(&mut self, s: Slot) -> Result<(), String> {
        let sp = self.saves.clone().ok_or("no game file to save for")?;
        let data = self.core.serialize().ok_or("this core can't save states")?;
        write_atomic(&sp.state(s), &data)?;
        let thumb = self.display_frame().thumbnail(320, 240);
        if !thumb.is_empty() {
            let tmp = sp.thumb(s).with_extension("png.tmp");
            thumb.save_png(&tmp)?;
            std::fs::rename(&tmp, sp.thumb(s)).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn load_state(&mut self, s: Slot) -> Result<(), String> {
        let sp = self.saves.clone().ok_or("no game file")?;
        let data = std::fs::read(sp.state(s)).map_err(|e| format!("{}: {e}", sp.state(s).display()))?;
        if self.core.unserialize(&data) { Ok(()) } else { Err("the core rejected the state".into()) }
    }

    /// Quit: auto state + SRAM. Errors are logged, not fatal. True when the
    /// resume state was written.
    pub fn finish(&mut self) -> bool {
        let mut saved = false;
        if self.game.is_some() {
            match self.save_state(Slot::Auto) {
                Ok(()) => saved = true,
                Err(e) => eprintln!("mortar-play: auto state: {e}"),
            }
        }
        if let Err(e) = self.save_sram() {
            eprintln!("mortar-play: save RAM: {e}");
        }
        saved
    }

    /// The state slot's file and thumbnail paths (None without a game file).
    pub fn slot_paths(&self, s: Slot) -> Option<(PathBuf, PathBuf)> {
        let sp = self.saves.as_ref()?;
        Some((sp.state(s), sp.thumb(s)))
    }

    /// The game's cleaned name and the system tag ("Pocket King", "GBC").
    pub fn title(&self) -> (String, String) {
        let name = match &self.game {
            Some(p) => library::display_name(&p.file_stem().unwrap_or_default().to_string_lossy()).0,
            None => self.sys.name.clone(),
        };
        (name, self.sys.tag.clone())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.save_sram();
    }
}

/// Where archives are extracted: RAM-backed if possible.
pub fn scratch_dir(cfg: &Config) -> PathBuf {
    if let Some(r) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(r).join("mortar-play");
    }
    cfg.data_dir().join("tmp")
}
