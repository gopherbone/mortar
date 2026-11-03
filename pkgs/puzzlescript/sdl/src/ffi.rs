// SPDX-License-Identifier: MIT
//! Hand-written bindings for ../src/ps.h (the C engine core) plus thin safe wrappers.
//!
//! Only the structs the shell reads are mirrored, field for field; `check_layout` compares
//! them against offsets the C compiler reports (csrc/layout.c), so a ps.h change that isn't
//! mirrored here fails loudly at startup and in `cargo test` instead of corrupting memory.
#![allow(non_snake_case, non_camel_case_types, dead_code)]

use std::ffi::{c_char, c_double, c_int, c_void, CStr, CString};
use std::ptr;

pub const PS_COLOR_TRANSPARENT: u32 = 0xFF00_0000;
pub const PS_SFX_EVENT_COUNT: usize = 21;
pub const PS_TERMINAL_WIDTH: usize = 34;
pub const PS_TERMINAL_HEIGHT: usize = 13;

// keys as checkKey() sees them
pub const KEY_UP: c_int = 0;
pub const KEY_LEFT: c_int = 1;
pub const KEY_DOWN: c_int = 2;
pub const KEY_RIGHT: c_int = 3;
pub const KEY_ACTION: c_int = 4;
pub const KEY_UNDO: c_int = 5;
pub const KEY_RESTART: c_int = 6;
pub const KEY_QUIT: c_int = 7;

#[repr(C)]
pub struct PSMetadata {
    pub title: *mut c_char,
    pub author: *mut c_char,
    pub homepage: *mut c_char,
    pub background_color_str: *mut c_char,
    pub text_color_str: *mut c_char,
    pub background_color: u32,
    pub text_color: u32,
    pub again_interval: c_double,
    pub realtime_interval: c_double,
    pub key_repeat_interval: c_double,
    pub noaction: bool,
    pub noundo: bool,
    pub norestart: bool,
    pub norepeat_action: bool,
    pub require_player_movement: bool,
    pub run_rules_on_level_start: bool,
    pub throttle_movement: bool,
    pub verbose_logging: bool,
    pub has_flickscreen: bool,
    pub has_zoomscreen: bool,
    pub flickscreen: [c_int; 2],
    pub zoomscreen: [c_int; 2],
    pub color_palette: *mut c_char,
    pub text_controls: *mut c_char,
    pub text_message_continue: *mut c_char,
    pub scanline: bool,
}

#[repr(C)]
pub struct PSObject {
    pub name: *mut c_char,
    pub id: c_int,
    pub layer: c_int,
    pub lineNumber: c_int,
    pub ncolors: c_int,
    pub colors: *mut u32,
    pub spriteW: c_int,
    pub spriteH: c_int,
    pub sprite: *mut i8,
}

#[repr(C)]
pub struct PSLevelDef {
    pub isMessage: bool,
    pub message: *mut c_char,
    pub lineNumber: c_int,
    pub width: c_int,
    pub height: c_int,
    pub objects: *mut u32,
}

#[repr(C)]
pub struct PSSfxMovementMask {
    pub objectMask: *mut u32,
    pub directionMask: *mut u32,
    pub seed: i64,
}

#[repr(C)]
pub struct PSSfxObjectMask {
    pub objectMask: *mut u32,
    pub seed: i64,
}

#[repr(C)]
pub struct PSGame {
    pub metadata: PSMetadata,
    pub strideObj: c_int,
    pub strideMov: c_int,
    pub layerCount: c_int,
    pub objectCount: c_int,
    pub objects: *mut PSObject,
    pub layerMasks: *mut *mut u32,
    pub playerMask: *mut u32,
    pub playerMaskAggregate: bool,
    pub backgroundid: c_int,
    pub backgroundlayer: c_int,
    pub ruleGroupCount: c_int,
    pub rules: *mut c_void,
    pub loopPoint: *mut c_int,
    pub lateRuleGroupCount: c_int,
    pub lateRules: *mut c_void,
    pub lateLoopPoint: *mut c_int,
    pub rigid: bool,
    pub rigidGroupCount: c_int,
    pub rigidGroupIndex_to_GroupIndex: *mut c_int,
    pub groupNumber_to_RigidGroupIndex: *mut c_int,
    pub groupNumberCount: c_int,
    pub winconditionCount: c_int,
    pub winconditions: *mut c_void,
    pub levelCount: c_int,
    pub levels: *mut PSLevelDef,
    pub sfx_Events: [i64; PS_SFX_EVENT_COUNT],
    pub sfx_CreationMaskCount: c_int,
    pub sfx_DestructionMaskCount: c_int,
    pub sfx_MovementFailureMaskCount: c_int,
    pub sfx_CreationMasks: *mut PSSfxObjectMask,
    pub sfx_DestructionMasks: *mut PSSfxObjectMask,
    pub sfx_MovementFailureMasks: *mut PSSfxMovementMask,
    pub sfx_MovementMaskCounts: *mut c_int,
    pub sfx_MovementMasks: *mut *mut PSSfxMovementMask,
    pub hasSounds: bool,
    pub errorCount: c_int,
    pub errorStrings: *mut *mut c_char,
    pub errorStringCount: c_int,
    pub priv_: *mut c_void,
}

/// Prefix of PSLevel: the shell only reads through a pointer, so the tail is left out.
#[repr(C)]
pub struct PSLevel {
    pub lineNumber: c_int,
    pub width: c_int,
    pub height: c_int,
    pub n_tiles: c_int,
    pub layerCount: c_int,
    pub objects: *mut u32,
}

#[repr(C)]
pub struct PSStorage {
    pub ctx: *mut c_void,
    pub get: Option<unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_char>,
    pub set: Option<unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char)>,
    pub remove: Option<unsafe extern "C" fn(*mut c_void, *const c_char)>,
}

#[repr(C)]
pub struct PSGlyph {
    pub w: c_int,
    pub h: c_int,
    pub rows: *const u8,
}

#[repr(C)]
pub struct PSSession {
    _opaque: [u8; 0],
}

extern "C" {
    pub static mut ps_log_hook: Option<unsafe extern "C" fn(*const c_char)>;
    pub static mut ps_over_budget_hook: Option<unsafe extern "C" fn() -> bool>;
    static ps_ffi_layout: [usize; 23];

    pub fn ps_malloc(n: usize) -> *mut c_void;
    pub fn ps_free(p: *mut c_void);
    pub fn ps_rng_set_entropy(e: u32);

    pub fn ps_compile(src: *const c_char, len: usize) -> *mut PSGame;
    pub fn ps_game_free(g: *mut PSGame);

    pub fn ps_session_new(g: *const PSGame, st: *const PSStorage, seed: *const c_char) -> *mut PSSession;
    pub fn ps_session_free(s: *mut PSSession);
    pub fn ps_key_down(s: *mut PSSession, key: c_int);
    pub fn ps_key_up(s: *mut PSSession, key: c_int);
    pub fn ps_update(s: *mut PSSession, dt: c_double);
    pub fn ps_in_text_mode(s: *const PSSession) -> bool;
    pub fn ps_on_title_screen(s: *const PSSession) -> bool;
    pub fn ps_text_lines(s: *const PSSession) -> *const *const c_char;
    pub fn ps_current_level(s: *const PSSession) -> *const PSLevel;
    pub fn ps_current_level_index(s: *const PSSession) -> c_int;
    pub fn ps_view_rect(s: *const PSSession, x0: *mut c_int, y0: *mut c_int, x1: *mut c_int, y1: *mut c_int);
    pub fn ps_take_dirty(s: *mut PSSession) -> bool;
    pub fn ps_take_sounds(s: *mut PSSession, out: *mut i64, max: c_int) -> c_int;
    pub fn ps_goto_level(s: *mut PSSession, level: c_int);
    pub fn ps_goto_title(s: *mut PSSession);
    pub fn ps_is_winning(s: *const PSSession) -> bool;

    pub fn ps_sfx_generate(seed: i64, nsamples: *mut c_int, rate: *mut c_int) -> *mut i16;
    pub fn ps_font_glyph(cp: u32) -> *const PSGlyph;
}

macro_rules! off {
    ($t:ty, $f:ident) => {
        std::mem::offset_of!($t, $f)
    };
}

/// Compares the Rust struct layouts with the C compiler's. Returns a description of the first
/// mismatch.
pub fn check_layout() -> Result<(), String> {
    let rust: [(&str, usize); 23] = [
        ("sizeof PSMetadata", std::mem::size_of::<PSMetadata>()),
        ("PSMetadata.scanline", off!(PSMetadata, scanline)),
        ("sizeof PSObject", std::mem::size_of::<PSObject>()),
        ("PSObject.sprite", off!(PSObject, sprite)),
        ("sizeof PSLevelDef", std::mem::size_of::<PSLevelDef>()),
        ("PSLevelDef.objects", off!(PSLevelDef, objects)),
        ("sizeof PSGame", std::mem::size_of::<PSGame>()),
        ("PSGame.objects", off!(PSGame, objects)),
        ("PSGame.backgroundid", off!(PSGame, backgroundid)),
        ("PSGame.levelCount", off!(PSGame, levelCount)),
        ("PSGame.levels", off!(PSGame, levels)),
        ("PSGame.sfx_Events", off!(PSGame, sfx_Events)),
        ("PSGame.sfx_MovementMasks", off!(PSGame, sfx_MovementMasks)),
        ("PSGame.errorCount", off!(PSGame, errorCount)),
        ("PSGame.errorStringCount", off!(PSGame, errorStringCount)),
        ("PSGame.priv", off!(PSGame, priv_)),
        ("PSLevel.width", off!(PSLevel, width)),
        ("PSLevel.height", off!(PSLevel, height)),
        ("PSLevel.objects", off!(PSLevel, objects)),
        ("sizeof PSSfxObjectMask", std::mem::size_of::<PSSfxObjectMask>()),
        ("sizeof PSSfxMovementMask", std::mem::size_of::<PSSfxMovementMask>()),
        ("sizeof PSStorage", std::mem::size_of::<PSStorage>()),
        ("terminator", 0),
    ];
    let c = unsafe { ptr::addr_of!(ps_ffi_layout).read() };
    for (i, (name, v)) in rust.iter().enumerate() {
        if c[i] != *v {
            return Err(format!("FFI layout mismatch: {name}: C {} vs Rust {v}", c[i]));
        }
    }
    Ok(())
}

pub fn cstr(p: *const c_char) -> Option<String> {
    if p.is_null() {
        None
    } else {
        Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
    }
}

/// Copies a Rust string into memory the engine may ps_free.
pub fn ps_string(s: &str) -> *mut c_char {
    let bytes = s.as_bytes();
    unsafe {
        let p = ps_malloc(bytes.len() + 1) as *mut u8;
        if p.is_null() {
            return ptr::null_mut();
        }
        ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        *p.add(bytes.len()) = 0;
        p as *mut c_char
    }
}

// ---- compiled game ----------------------------------------------------------------------------

pub struct Game {
    raw: *mut PSGame,
}

impl Game {
    pub fn compile(src: &[u8]) -> Game {
        // the compiler wants a NUL-terminated buffer
        let mut buf = Vec::with_capacity(src.len() + 1);
        buf.extend_from_slice(src);
        buf.push(0);
        let raw = unsafe { ps_compile(buf.as_ptr() as *const c_char, src.len()) };
        assert!(!raw.is_null(), "ps_compile returned NULL");
        Game { raw }
    }
    pub fn raw(&self) -> &PSGame {
        unsafe { &*self.raw }
    }
    pub fn ptr(&self) -> *const PSGame {
        self.raw
    }
    pub fn error_count(&self) -> i32 {
        self.raw().errorCount
    }
    pub fn errors(&self) -> Vec<String> {
        let g = self.raw();
        (0..g.errorStringCount.max(0) as usize)
            .filter_map(|i| cstr(unsafe { *g.errorStrings.add(i) }))
            .collect()
    }
    pub fn objects(&self) -> &[PSObject] {
        let g = self.raw();
        if g.objects.is_null() || g.objectCount <= 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(g.objects, g.objectCount as usize) }
        }
    }
    pub fn levels(&self) -> &[PSLevelDef] {
        let g = self.raw();
        if g.levels.is_null() || g.levelCount <= 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(g.levels, g.levelCount as usize) }
        }
    }
    pub fn title(&self) -> Option<String> {
        cstr(self.raw().metadata.title)
    }
    /// Every sound seed the game declares, most likely first (events, then object sounds).
    pub fn sound_seeds(&self) -> Vec<i64> {
        let g = self.raw();
        let mut v: Vec<i64> = g.sfx_Events.to_vec();
        unsafe {
            let push_obj = |v: &mut Vec<i64>, p: *mut PSSfxObjectMask, n: c_int| {
                for i in 0..n.max(0) as usize {
                    v.push((*p.add(i)).seed);
                }
            };
            for i in 0..g.sfx_MovementFailureMaskCount.max(0) as usize {
                v.push((*g.sfx_MovementFailureMasks.add(i)).seed);
            }
            if !g.sfx_MovementMaskCounts.is_null() && !g.sfx_MovementMasks.is_null() {
                for l in 0..g.layerCount.max(0) as usize {
                    let n = *g.sfx_MovementMaskCounts.add(l);
                    let arr = *g.sfx_MovementMasks.add(l);
                    for i in 0..n.max(0) as usize {
                        v.push((*arr.add(i)).seed);
                    }
                }
            }
            push_obj(&mut v, g.sfx_CreationMasks, g.sfx_CreationMaskCount);
            push_obj(&mut v, g.sfx_DestructionMasks, g.sfx_DestructionMaskCount);
        }
        let mut seen = std::collections::HashSet::new();
        v.retain(|&s| s >= 0 && seen.insert(s));
        v
    }
}

impl Drop for Game {
    fn drop(&mut self) {
        unsafe { ps_game_free(self.raw) }
    }
}

// ---- play session -----------------------------------------------------------------------------

/// A play session. Borrows the game and the storage block by raw pointer, so `Session` must be
/// dropped before either (the shell keeps all three in one struct, session first).
pub struct Session {
    raw: *mut PSSession,
}

pub struct LevelView<'a> {
    pub width: usize,
    pub height: usize,
    pub objects: &'a [u32],
}

impl Session {
    /// # Safety
    /// `game` and `storage` must outlive the session.
    pub unsafe fn new(game: &Game, storage: *const PSStorage, seed: Option<&str>) -> Session {
        let seed_c = seed.map(|s| CString::new(s).unwrap());
        let raw = ps_session_new(game.ptr(), storage, seed_c.as_ref().map_or(ptr::null(), |c| c.as_ptr()));
        assert!(!raw.is_null());
        Session { raw }
    }
    pub fn key_down(&mut self, k: c_int) {
        unsafe { ps_key_down(self.raw, k) }
    }
    pub fn key_up(&mut self, k: c_int) {
        unsafe { ps_key_up(self.raw, k) }
    }
    pub fn tap(&mut self, k: c_int) {
        self.key_down(k);
        self.key_up(k);
    }
    pub fn update(&mut self, dt: f64) {
        unsafe { ps_update(self.raw, dt) }
    }
    pub fn in_text_mode(&self) -> bool {
        unsafe { ps_in_text_mode(self.raw) }
    }
    pub fn on_title_screen(&self) -> bool {
        unsafe { ps_on_title_screen(self.raw) }
    }
    pub fn text_lines(&self) -> Vec<String> {
        let p = unsafe { ps_text_lines(self.raw) };
        if p.is_null() {
            return Vec::new();
        }
        (0..PS_TERMINAL_HEIGHT).map(|i| cstr(unsafe { *p.add(i) }).unwrap_or_default()).collect()
    }
    pub fn level(&self, stride: usize) -> Option<LevelView<'_>> {
        let l = unsafe { ps_current_level(self.raw) };
        if l.is_null() {
            return None;
        }
        let l = unsafe { &*l };
        if l.objects.is_null() || l.width <= 0 || l.height <= 0 {
            return None;
        }
        let (w, h) = (l.width as usize, l.height as usize);
        Some(LevelView { width: w, height: h, objects: unsafe { std::slice::from_raw_parts(l.objects, w * h * stride) } })
    }
    pub fn level_index(&self) -> i32 {
        unsafe { ps_current_level_index(self.raw) }
    }
    pub fn view_rect(&self) -> (i32, i32, i32, i32) {
        let (mut a, mut b, mut c, mut d) = (0, 0, 0, 0);
        unsafe { ps_view_rect(self.raw, &mut a, &mut b, &mut c, &mut d) };
        (a, b, c, d)
    }
    pub fn take_dirty(&mut self) -> bool {
        unsafe { ps_take_dirty(self.raw) }
    }
    pub fn take_sounds(&mut self) -> Vec<i64> {
        let mut buf = [0i64; 32];
        let n = unsafe { ps_take_sounds(self.raw, buf.as_mut_ptr(), 32) };
        buf[..n.clamp(0, 32) as usize].to_vec()
    }
    pub fn goto_level(&mut self, i: usize) {
        unsafe { ps_goto_level(self.raw, i as c_int) }
    }
    pub fn goto_title(&mut self) {
        unsafe { ps_goto_title(self.raw) }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        unsafe { ps_session_free(self.raw) }
    }
}

// ---- sfx / font -------------------------------------------------------------------------------

/// sfxr PCM for a seed: (mono i16 samples, sample rate).
pub fn sfx_generate(seed: i64) -> Option<(Vec<i16>, i32)> {
    let (mut n, mut rate) = (0, 0);
    let p = unsafe { ps_sfx_generate(seed, &mut n, &mut rate) };
    if p.is_null() {
        return None;
    }
    let v = if n > 0 && rate > 0 { Some((unsafe { std::slice::from_raw_parts(p, n as usize) }.to_vec(), rate)) } else { None };
    unsafe { ps_free(p as *mut c_void) };
    v
}

/// PuzzleScript font glyph (5x12, rows top to bottom, bit 4 = leftmost pixel).
pub fn glyph(cp: u32) -> Option<[u8; 12]> {
    let g = unsafe { ps_font_glyph(cp) };
    if g.is_null() {
        return None;
    }
    let g = unsafe { &*g };
    let mut rows = [0u8; 12];
    let h = (g.h.max(0) as usize).min(12);
    rows[..h].copy_from_slice(unsafe { std::slice::from_raw_parts(g.rows, h) });
    Some(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_c() {
        check_layout().unwrap();
    }

    #[test]
    fn compiles_and_plays_the_fixture() {
        let src = include_bytes!("../testdata/fixture.txt").to_vec();
        let g = Game::compile(&src);
        assert_eq!(g.error_count(), 0, "{:?}", g.errors());
        assert!(g.levels().len() > 5);
        let mut s = unsafe { Session::new(&g, ptr::null(), Some("test")) };
        assert!(s.on_title_screen());
        s.tap(KEY_ACTION);
        for _ in 0..60 {
            s.update(1.0 / 30.0);
        }
        // title -> (message ->) first level
        while s.in_text_mode() {
            s.tap(KEY_ACTION);
            for _ in 0..30 {
                s.update(1.0 / 30.0);
            }
        }
        let stride = g.raw().strideObj as usize;
        let lv = s.level(stride).unwrap();
        assert!(lv.width > 0 && lv.height > 0);
        assert!(sfx_generate(3_295_707).is_some());
        assert!(glyph('A' as u32).is_some());
    }
}
