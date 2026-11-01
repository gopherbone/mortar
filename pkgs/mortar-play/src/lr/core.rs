// SPDX-License-Identifier: GPL-2.0-or-later
//! A libretro core loaded with dlopen, and the frontend side of its
//! callbacks.
//!
//! libretro callbacks carry no user pointer, so the frontend state they
//! touch (`Host`) is reached through one process-wide pointer. Only one core
//! runs at a time; every callback except `log` happens on the thread that
//! calls into the core, inside `retro_run`/`retro_load_game`/etc.

use super::ffi::{self, *};
use super::options::Options;
use crate::video::Frame;
use std::ffi::{CStr, CString, c_char, c_uint, c_void};
use std::path::Path;
use std::sync::atomic::{AtomicPtr, AtomicI32, Ordering};
use std::time::Instant;

/// Messages the core asked us to show.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct Message {
    pub text: String,
    pub frames: u32,
}

/// Everything the core's callbacks read or write.
pub struct Host {
    pub pixel_format: c_uint,
    pub frame: Frame,
    /// A new (non-dupe) frame arrived during the last run.
    pub frame_new: bool,
    /// Core samples of the last run (interleaved stereo).
    pub audio: Vec<i16>,
    /// RetroPad bitmask per port.
    pub pad: [u16; 2],
    pub options: Options,
    pub system_dir: CString,
    pub save_dir: CString,
    pub core_path: CString,
    pub username: CString,
    /// AV info from SET_SYSTEM_AV_INFO / SET_GEOMETRY, picked up by the
    /// session after a run.
    pub av: retro_system_av_info,
    pub av_changed: bool,
    pub rotation: u32,
    pub frame_time: Option<(retro_frame_time_callback_t, i64)>,
    pub audio_status: Option<retro_audio_buffer_status_callback_t>,
    pub disk: Option<retro_disk_control_callback>,
    pub hw_render: Option<HwRequest>,
    pub messages: Vec<Message>,
    pub shutdown: bool,
    pub support_no_game: bool,
    pub descriptors: Vec<(u32, u32, String)>,
    pub fast_forward: bool,
    pub can_dupe: bool,
}

/// What a hardware-rendered core asked for (SET_HW_RENDER). Not served yet;
/// see DESIGN.md "Hardware rendering".
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
pub struct HwRequest {
    pub context_type: c_uint,
    pub major: c_uint,
    pub minor: c_uint,
}

impl Host {
    pub fn new(options: Options, system_dir: &Path, save_dir: &Path, core_path: &Path) -> Host {
        let c = |p: &Path| CString::new(p.to_string_lossy().as_bytes()).unwrap_or_default();
        Host {
            pixel_format: PIXEL_FORMAT_0RGB1555,
            frame: Frame::default(),
            frame_new: false,
            audio: Vec::with_capacity(4096),
            pad: [0; 2],
            options,
            system_dir: c(system_dir),
            save_dir: c(save_dir),
            core_path: c(core_path),
            username: CString::new("player").unwrap(),
            av: retro_system_av_info::default(),
            av_changed: false,
            rotation: 0,
            frame_time: None,
            audio_status: None,
            disk: None,
            hw_render: None,
            messages: Vec::new(),
            shutdown: false,
            support_no_game: false,
            descriptors: Vec::new(),
            fast_forward: false,
            can_dupe: true,
        }
    }
}

static HOST: AtomicPtr<Host> = AtomicPtr::new(std::ptr::null_mut());
/// Lowest core log level printed (MORTAR_PLAY_LOG=debug|info|warn|error).
static LOG_LEVEL: AtomicI32 = AtomicI32::new(LOG_WARN);

/// The current host. Callbacks only run while a `Core` holds one.
#[allow(clippy::mut_from_ref)]
fn host() -> Option<&'static mut Host> {
    let p = HOST.load(Ordering::Acquire);
    if p.is_null() { None } else { Some(unsafe { &mut *p }) }
}

unsafe fn cstr<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() { None } else { unsafe { CStr::from_ptr(p) }.to_str().ok() }
}

// --------------------------------------------------------------- logging ----

unsafe extern "C" {
    fn mortar_play_log_printf(level: i32, fmt: *const c_char, ...);
}

#[unsafe(no_mangle)]
pub extern "C" fn mortar_play_log(level: i32, msg: *const c_char) {
    if level < LOG_LEVEL.load(Ordering::Relaxed) {
        return;
    }
    let m = unsafe { cstr(msg) }.unwrap_or("?").trim_end();
    let l = ["debug", "info", "warn", "error"].get(level as usize).copied().unwrap_or("?");
    eprintln!("core[{l}]: {m}");
}

pub fn set_log_level_from_env() {
    let l = match std::env::var("MORTAR_PLAY_LOG").as_deref() {
        Ok("debug") => LOG_DEBUG,
        Ok("info") => LOG_INFO,
        Ok("error") => LOG_ERROR,
        _ => LOG_WARN,
    };
    LOG_LEVEL.store(l, Ordering::Relaxed);
}

// ------------------------------------------------------------------ perf ----

static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

unsafe extern "C" fn perf_time_usec() -> i64 {
    EPOCH.get_or_init(Instant::now).elapsed().as_micros() as i64
}
unsafe extern "C" fn perf_cpu_features() -> u64 {
    // RETRO_SIMD_NEON (1 << 5) on aarch64, SSE/SSE2 (1|2) on x86_64
    if cfg!(target_arch = "aarch64") { 1 << 5 } else if cfg!(target_arch = "x86_64") { 1 | 2 } else { 0 }
}
unsafe extern "C" fn perf_counter() -> u64 {
    EPOCH.get_or_init(Instant::now).elapsed().as_nanos() as u64
}
unsafe extern "C" fn perf_register(c: *mut retro_perf_counter) {
    if !c.is_null() {
        unsafe { (*c).registered = true };
    }
}
unsafe extern "C" fn perf_start(c: *mut retro_perf_counter) {
    if !c.is_null() {
        unsafe {
            (*c).call_cnt += 1;
            (*c).start = perf_counter();
        }
    }
}
unsafe extern "C" fn perf_stop(c: *mut retro_perf_counter) {
    if !c.is_null() {
        unsafe { (*c).total += perf_counter() - (*c).start };
    }
}
unsafe extern "C" fn perf_log() {}

// ----------------------------------------------------------- callbacks ----

unsafe extern "C" fn environment(cmd: c_uint, data: *mut c_void) -> bool {
    let Some(h) = host() else { return false };
    unsafe { env(h, cmd, data) }
}

macro_rules! out {
    ($data:expr, $t:ty, $v:expr) => {{
        if !$data.is_null() {
            *($data as *mut $t) = $v;
        }
        true
    }};
}

unsafe fn env(h: &mut Host, cmd: c_uint, data: *mut c_void) -> bool {
    unsafe {
        match cmd {
            ENV_SET_ROTATION => {
                h.rotation = *(data as *const c_uint) % 4;
                true
            }
            ENV_GET_OVERSCAN => out!(data, bool, false),
            ENV_GET_CAN_DUPE => out!(data, bool, h.can_dupe),
            ENV_SET_MESSAGE => {
                let m = &*(data as *const retro_message);
                if let Some(t) = cstr(m.msg) {
                    h.messages.push(Message { text: t.to_string(), frames: m.frames });
                }
                true
            }
            ENV_SET_MESSAGE_EXT => {
                let m = &*(data as *const retro_message_ext);
                // target 0 = all, 1 = OSD, 2 = log
                if let Some(t) = cstr(m.msg) {
                    if m.target != 2 {
                        h.messages.push(Message { text: t.to_string(), frames: (m.duration * 60 / 1000).max(60) });
                    }
                    mortar_play_log(m.level, m.msg);
                }
                true
            }
            ENV_SHUTDOWN => {
                h.shutdown = true;
                true
            }
            ENV_SET_PERFORMANCE_LEVEL => true,
            ENV_GET_SYSTEM_DIRECTORY | ENV_GET_CORE_ASSETS_DIRECTORY => out!(data, *const c_char, h.system_dir.as_ptr()),
            ENV_GET_SAVE_DIRECTORY => out!(data, *const c_char, h.save_dir.as_ptr()),
            ENV_GET_LIBRETRO_PATH => out!(data, *const c_char, h.core_path.as_ptr()),
            ENV_GET_USERNAME => out!(data, *const c_char, h.username.as_ptr()),
            ENV_GET_LANGUAGE => out!(data, c_uint, 0),
            ENV_SET_PIXEL_FORMAT => {
                let f = *(data as *const c_uint);
                if f <= PIXEL_FORMAT_RGB565 {
                    h.pixel_format = f;
                    true
                } else {
                    false
                }
            }
            ENV_SET_INPUT_DESCRIPTORS => {
                let mut d = data as *const retro_input_descriptor;
                h.descriptors.clear();
                while !d.is_null() && !(*d).description.is_null() {
                    if (*d).device == DEVICE_JOYPAD {
                        h.descriptors.push(((*d).port, (*d).id, cstr((*d).description).unwrap_or("").to_string()));
                    }
                    d = d.add(1);
                }
                true
            }
            ENV_SET_DISK_CONTROL_INTERFACE | ENV_SET_DISK_CONTROL_EXT_INTERFACE => {
                h.disk = Some(*(data as *const retro_disk_control_callback));
                true
            }
            ENV_GET_DISK_CONTROL_INTERFACE_VERSION => out!(data, c_uint, 1),
            ENV_SET_HW_RENDER => {
                let r = &*(data as *const retro_hw_render_callback);
                h.hw_render = Some(HwRequest { context_type: r.context_type, major: r.version_major, minor: r.version_minor });
                eprintln!(
                    "mortar-play: the core wants hardware rendering (context {} {}.{}); not supported yet",
                    r.context_type, r.version_major, r.version_minor
                );
                false
            }
            ENV_GET_PREFERRED_HW_RENDER => false,
            ENV_GET_VARIABLE => {
                let v = &mut *(data as *mut retro_variable);
                match cstr(v.key) {
                    Some(k) => {
                        v.value = h.options.get_ptr(k);
                        !v.value.is_null()
                    }
                    None => false,
                }
            }
            ENV_SET_VARIABLES => {
                h.options.set_legacy(data as *const retro_variable);
                true
            }
            ENV_SET_VARIABLE => {
                if data.is_null() {
                    return true; // "is SET_VARIABLE supported?"
                }
                let v = &*(data as *const retro_variable);
                if let (Some(k), Some(val)) = (cstr(v.key), cstr(v.value)) {
                    h.options.set_value(k, val);
                }
                true
            }
            ENV_GET_VARIABLE_UPDATE => out!(data, bool, h.options.take_updated()),
            ENV_GET_CORE_OPTIONS_VERSION => out!(data, c_uint, 2),
            ENV_SET_CORE_OPTIONS => {
                h.options.set_v1(data as *const retro_core_option_definition);
                true
            }
            ENV_SET_CORE_OPTIONS_INTL => {
                let i = &*(data as *const retro_core_options_intl);
                h.options.set_v1(i.us);
                true
            }
            ENV_SET_CORE_OPTIONS_V2 => {
                h.options.set_v2(data as *const retro_core_options_v2);
                true
            }
            ENV_SET_CORE_OPTIONS_V2_INTL => {
                let i = &*(data as *const retro_core_options_v2_intl);
                h.options.set_v2(i.us);
                true
            }
            ENV_SET_CORE_OPTIONS_DISPLAY | ENV_SET_CORE_OPTIONS_UPDATE_DISPLAY_CALLBACK => true,
            ENV_SET_SUPPORT_NO_GAME => {
                h.support_no_game = *(data as *const bool);
                true
            }
            ENV_SET_FRAME_TIME_CALLBACK => {
                let f = &*(data as *const retro_frame_time_callback);
                h.frame_time = f.callback.map(|cb| (cb, f.reference));
                true
            }
            ENV_SET_AUDIO_BUFFER_STATUS_CALLBACK => {
                h.audio_status = if data.is_null() { None } else { (*(data as *const retro_audio_buffer_status_callback)).callback };
                true
            }
            ENV_SET_MINIMUM_AUDIO_LATENCY => true,
            ENV_GET_LOG_INTERFACE => {
                let f: unsafe extern "C" fn(i32, *const c_char, ...) = mortar_play_log_printf;
                out!(data, retro_log_callback, retro_log_callback { log: f })
            }
            ENV_GET_PERF_INTERFACE => out!(
                data,
                retro_perf_callback,
                retro_perf_callback {
                    get_time_usec: perf_time_usec,
                    get_cpu_features: perf_cpu_features,
                    get_perf_counter: perf_counter,
                    perf_register,
                    perf_start,
                    perf_stop,
                    perf_log,
                }
            ),
            ENV_SET_SYSTEM_AV_INFO => {
                h.av = *(data as *const retro_system_av_info);
                h.av_changed = true;
                true
            }
            ENV_SET_GEOMETRY => {
                h.av.geometry = *(data as *const retro_game_geometry);
                h.av_changed = true;
                true
            }
            ENV_GET_INPUT_BITMASKS => true,
            ENV_GET_INPUT_DEVICE_CAPABILITIES => out!(data, u64, (1 << DEVICE_JOYPAD) | (1 << DEVICE_ANALOG)),
            ENV_GET_INPUT_MAX_USERS => out!(data, c_uint, 1),
            ENV_GET_AUDIO_VIDEO_ENABLE => out!(data, i32, 3),
            ENV_GET_FASTFORWARDING => out!(data, bool, h.fast_forward),
            ENV_GET_TARGET_REFRESH_RATE => out!(data, f32, 60.0),
            ENV_GET_MESSAGE_INTERFACE_VERSION => out!(data, c_uint, 1),
            ENV_GET_JIT_CAPABLE => out!(data, bool, true),
            ENV_SET_CONTROLLER_INFO
            | ENV_SET_SUBSYSTEM_INFO
            | ENV_SET_MEMORY_MAPS
            | ENV_SET_SUPPORT_ACHIEVEMENTS
            | ENV_SET_SERIALIZATION_QUIRKS
            | ENV_SET_CONTENT_INFO_OVERRIDE
            | ENV_SET_FASTFORWARDING_OVERRIDE
            | ENV_SET_PROC_ADDRESS_CALLBACK
            | ENV_SET_KEYBOARD_CALLBACK => true,
            // Not offered: VFS, LED, rumble, sensors, camera, MIDI, game
            // info ext, throttle state, savestate context, HW contexts.
            _ => {
                if LOG_LEVEL.load(Ordering::Relaxed) <= LOG_DEBUG {
                    eprintln!("mortar-play: unhandled environment call {cmd} (0x{cmd:x})");
                }
                false
            }
        }
    }
}

unsafe extern "C" fn video_refresh(data: *const c_void, w: c_uint, h: c_uint, pitch: usize) {
    let Some(host) = host() else { return };
    if data.is_null() || data == HW_FRAME_BUFFER_VALID {
        return; // dupe (or a HW frame, which we don't serve)
    }
    unsafe { host.frame.load(data as *const u8, w, h, pitch, host.pixel_format) };
    host.frame_new = true;
}

unsafe extern "C" fn audio_sample(l: i16, r: i16) {
    if let Some(h) = host() {
        h.audio.extend_from_slice(&[l, r]);
    }
}

unsafe extern "C" fn audio_batch(data: *const i16, frames: usize) -> usize {
    if let Some(h) = host()
        && !data.is_null()
    {
        h.audio.extend_from_slice(unsafe { std::slice::from_raw_parts(data, frames * 2) });
    }
    frames
}

unsafe extern "C" fn input_poll() {}

unsafe extern "C" fn input_state(port: c_uint, device: c_uint, _index: c_uint, id: c_uint) -> i16 {
    let Some(h) = host() else { return 0 };
    let Some(&mask) = h.pad.get(port as usize) else { return 0 };
    match device & DEVICE_MASK {
        DEVICE_JOYPAD if id == JOYPAD_MASK => mask as i16,
        DEVICE_JOYPAD if id < 16 => ((mask >> id) & 1) as i16,
        _ => 0,
    }
}

// ------------------------------------------------------------------ core ----

type Fn0 = unsafe extern "C" fn();

#[derive(Clone, Copy)]
struct Api {
    init: Fn0,
    deinit: Fn0,
    api_version: unsafe extern "C" fn() -> c_uint,
    get_system_info: unsafe extern "C" fn(*mut retro_system_info),
    get_system_av_info: unsafe extern "C" fn(*mut retro_system_av_info),
    set_controller_port_device: unsafe extern "C" fn(c_uint, c_uint),
    reset: Fn0,
    run: Fn0,
    serialize_size: unsafe extern "C" fn() -> usize,
    serialize: unsafe extern "C" fn(*mut c_void, usize) -> bool,
    unserialize: unsafe extern "C" fn(*const c_void, usize) -> bool,
    load_game: unsafe extern "C" fn(*const retro_game_info) -> bool,
    unload_game: Fn0,
    get_memory_data: unsafe extern "C" fn(c_uint) -> *mut c_void,
    get_memory_size: unsafe extern "C" fn(c_uint) -> usize,
    set_environment: unsafe extern "C" fn(retro_environment_t),
    set_video_refresh: unsafe extern "C" fn(retro_video_refresh_t),
    set_audio_sample: unsafe extern "C" fn(retro_audio_sample_t),
    set_audio_sample_batch: unsafe extern "C" fn(retro_audio_sample_batch_t),
    set_input_poll: unsafe extern "C" fn(retro_input_poll_t),
    set_input_state: unsafe extern "C" fn(retro_input_state_t),
}

#[derive(Clone, Debug, Default)]
#[allow(dead_code)]
pub struct SystemInfo {
    pub name: String,
    pub version: String,
    pub extensions: Vec<String>,
    pub need_fullpath: bool,
    pub block_extract: bool,
}

/// A loaded core with a game. Dropping it unloads the game, deinits the
/// core and dlcloses it.
pub struct Core {
    api: Api,
    pub info: SystemInfo,
    pub av: retro_system_av_info,
    host: Box<Host>,
    game_loaded: bool,
    /// ROM bytes for cores that load from memory (kept for the core's sake).
    _data: Option<Vec<u8>>,
    _path: Option<CString>,
    _lib: libloading::Library,
}

impl Core {
    /// dlopen `path`, install callbacks and retro_init. Only one Core may
    /// exist at a time.
    pub fn open(path: &Path, host: Host) -> Result<Core, String> {
        if !HOST.load(Ordering::Acquire).is_null() {
            return Err("another core is already running".into());
        }
        set_log_level_from_env();
        let lib = unsafe { libloading::Library::new(path) }.map_err(|e| format!("{}: {e}", path.display()))?;
        macro_rules! sym {
            ($name:literal) => {
                *unsafe { lib.get($name) }.map_err(|e| format!("{}: {}: {e}", path.display(), String::from_utf8_lossy($name)))?
            };
        }
        let api = Api {
            init: sym!(b"retro_init"),
            deinit: sym!(b"retro_deinit"),
            api_version: sym!(b"retro_api_version"),
            get_system_info: sym!(b"retro_get_system_info"),
            get_system_av_info: sym!(b"retro_get_system_av_info"),
            set_controller_port_device: sym!(b"retro_set_controller_port_device"),
            reset: sym!(b"retro_reset"),
            run: sym!(b"retro_run"),
            serialize_size: sym!(b"retro_serialize_size"),
            serialize: sym!(b"retro_serialize"),
            unserialize: sym!(b"retro_unserialize"),
            load_game: sym!(b"retro_load_game"),
            unload_game: sym!(b"retro_unload_game"),
            get_memory_data: sym!(b"retro_get_memory_data"),
            get_memory_size: sym!(b"retro_get_memory_size"),
            set_environment: sym!(b"retro_set_environment"),
            set_video_refresh: sym!(b"retro_set_video_refresh"),
            set_audio_sample: sym!(b"retro_set_audio_sample"),
            set_audio_sample_batch: sym!(b"retro_set_audio_sample_batch"),
            set_input_poll: sym!(b"retro_set_input_poll"),
            set_input_state: sym!(b"retro_set_input_state"),
        };
        let v = unsafe { (api.api_version)() };
        if v != RETRO_API_VERSION {
            return Err(format!("{}: libretro API version {v}, want {RETRO_API_VERSION}", path.display()));
        }
        let mut host = Box::new(host);
        HOST.store(&mut *host as *mut Host, Ordering::Release);
        let mut si = retro_system_info {
            library_name: std::ptr::null(),
            library_version: std::ptr::null(),
            valid_extensions: std::ptr::null(),
            need_fullpath: false,
            block_extract: false,
        };
        let info = unsafe {
            (api.get_system_info)(&mut si);
            SystemInfo {
                name: cstr(si.library_name).unwrap_or("?").to_string(),
                version: cstr(si.library_version).unwrap_or("").to_string(),
                extensions: cstr(si.valid_extensions).unwrap_or("").split('|').filter(|s| !s.is_empty()).map(|s| s.to_lowercase()).collect(),
                need_fullpath: si.need_fullpath,
                block_extract: si.block_extract,
            }
        };
        unsafe {
            (api.set_environment)(environment);
            (api.set_video_refresh)(video_refresh);
            (api.set_audio_sample)(audio_sample);
            (api.set_audio_sample_batch)(audio_batch);
            (api.set_input_poll)(input_poll);
            (api.set_input_state)(input_state);
            (api.init)();
        }
        Ok(Core { api, info, av: retro_system_av_info::default(), host, game_loaded: false, _data: None, _path: None, _lib: lib })
    }

    pub fn host(&mut self) -> &mut Host {
        &mut self.host
    }
    pub fn host_ref(&self) -> &Host {
        &self.host
    }

    /// Load a game file (None: start without content, for cores that
    /// support it).
    pub fn load_game(&mut self, rom: Option<&Path>) -> Result<(), String> {
        let ok = match rom {
            None => unsafe { (self.api.load_game)(std::ptr::null()) },
            Some(p) => {
                let cpath = CString::new(p.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
                let data = if self.info.need_fullpath { None } else { Some(std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?) };
                let gi = retro_game_info {
                    path: cpath.as_ptr(),
                    data: data.as_ref().map_or(std::ptr::null(), |d| d.as_ptr() as *const c_void),
                    size: data.as_ref().map_or(0, |d| d.len()),
                    meta: std::ptr::null(),
                };
                let ok = unsafe { (self.api.load_game)(&gi) };
                self._data = data;
                self._path = Some(cpath);
                ok
            }
        };
        if !ok {
            return Err(match rom {
                Some(p) => format!("the core could not load {}", p.file_name().unwrap_or_default().to_string_lossy()),
                None => "the core needs a game".into(),
            });
        }
        self.game_loaded = true;
        let mut av = retro_system_av_info::default();
        unsafe { (self.api.get_system_av_info)(&mut av) };
        self.av = av;
        self.host.av = av;
        self.host.av_changed = false;
        unsafe { (self.api.set_controller_port_device)(0, DEVICE_JOYPAD) };
        Ok(())
    }

    /// One frame. Returns the AV info if the core changed it.
    pub fn run(&mut self, audio_occupancy: Option<u32>) -> Option<retro_system_av_info> {
        self.host.audio.clear();
        self.host.frame_new = false;
        if let Some((cb, reference)) = self.host.frame_time {
            unsafe { cb(reference) };
        }
        if let (Some(cb), Some(occ)) = (self.host.audio_status, audio_occupancy) {
            unsafe { cb(true, occ, occ < 25) };
        }
        unsafe { (self.api.run)() };
        if self.host.av_changed {
            self.host.av_changed = false;
            // SET_GEOMETRY keeps timing; SET_SYSTEM_AV_INFO may change both
            if self.host.av.timing.fps <= 0.0 {
                self.host.av.timing = self.av.timing;
            }
            self.av = self.host.av;
            return Some(self.av);
        }
        None
    }

    pub fn reset(&mut self) {
        unsafe { (self.api.reset)() }
    }

    pub fn serialize(&mut self) -> Option<Vec<u8>> {
        let n = unsafe { (self.api.serialize_size)() };
        if n == 0 {
            return None;
        }
        let mut v = vec![0u8; n];
        if unsafe { (self.api.serialize)(v.as_mut_ptr() as *mut c_void, n) } { Some(v) } else { None }
    }

    pub fn unserialize(&mut self, data: &[u8]) -> bool {
        unsafe { (self.api.unserialize)(data.as_ptr() as *const c_void, data.len()) }
    }

    /// A core memory region (save RAM, RTC) as a mutable slice.
    pub fn memory(&mut self, id: c_uint) -> Option<&mut [u8]> {
        let n = unsafe { (self.api.get_memory_size)(id) };
        let p = unsafe { (self.api.get_memory_data)(id) } as *mut u8;
        if n == 0 || p.is_null() { None } else { Some(unsafe { std::slice::from_raw_parts_mut(p, n) }) }
    }

    // disk control (multi-disc PS1 games)

    pub fn disk_count(&self) -> Option<(u32, u32)> {
        let d = self.host.disk?;
        let n = unsafe { (d.get_num_images?)() };
        let i = unsafe { (d.get_image_index?)() };
        Some((i, n))
    }

    /// Eject, select the next image, close the tray. Returns the new index.
    pub fn disk_next(&mut self) -> Option<u32> {
        let d = self.host.disk?;
        let (i, n) = self.disk_count()?;
        if n < 2 {
            return None;
        }
        let next = (i + 1) % n;
        unsafe {
            (d.set_eject_state?)(true);
            (d.set_image_index?)(next);
            (d.set_eject_state?)(false);
        }
        Some(next)
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        unsafe {
            if self.game_loaded {
                (self.api.unload_game)();
            }
            (self.api.deinit)();
        }
        HOST.store(std::ptr::null_mut(), Ordering::Release);
    }
}


#[allow(dead_code)]
fn _assert_ffi_types() {
    let _: ffi::retro_input_state_t = input_state;
}
