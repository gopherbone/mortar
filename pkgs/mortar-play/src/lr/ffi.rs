// SPDX-License-Identifier: GPL-2.0-or-later
//! The part of libretro.h (API version 1) this frontend speaks. Values and
//! layouts follow libretro-common/include/libretro.h.
#![allow(non_camel_case_types, dead_code)]

use std::ffi::{c_char, c_uint, c_void};

pub const RETRO_API_VERSION: c_uint = 1;

pub const EXPERIMENTAL: c_uint = 0x10000;

// environment commands (full values: the EXPERIMENTAL bit is part of them;
// 44 alone is SET_SERIALIZATION_QUIRKS, 44|EXP is SET_HW_SHARED_CONTEXT)
pub const ENV_SET_ROTATION: c_uint = 1;
pub const ENV_GET_OVERSCAN: c_uint = 2;
pub const ENV_GET_CAN_DUPE: c_uint = 3;
pub const ENV_SET_MESSAGE: c_uint = 6;
pub const ENV_SHUTDOWN: c_uint = 7;
pub const ENV_SET_PERFORMANCE_LEVEL: c_uint = 8;
pub const ENV_GET_SYSTEM_DIRECTORY: c_uint = 9;
pub const ENV_SET_PIXEL_FORMAT: c_uint = 10;
pub const ENV_SET_INPUT_DESCRIPTORS: c_uint = 11;
pub const ENV_SET_KEYBOARD_CALLBACK: c_uint = 12;
pub const ENV_SET_DISK_CONTROL_INTERFACE: c_uint = 13;
pub const ENV_SET_HW_RENDER: c_uint = 14;
pub const ENV_GET_VARIABLE: c_uint = 15;
pub const ENV_SET_VARIABLES: c_uint = 16;
pub const ENV_GET_VARIABLE_UPDATE: c_uint = 17;
pub const ENV_SET_SUPPORT_NO_GAME: c_uint = 18;
pub const ENV_GET_LIBRETRO_PATH: c_uint = 19;
pub const ENV_SET_FRAME_TIME_CALLBACK: c_uint = 21;
pub const ENV_SET_AUDIO_CALLBACK: c_uint = 22;
pub const ENV_GET_RUMBLE_INTERFACE: c_uint = 23;
pub const ENV_GET_INPUT_DEVICE_CAPABILITIES: c_uint = 24;
pub const ENV_GET_LOG_INTERFACE: c_uint = 27;
pub const ENV_GET_PERF_INTERFACE: c_uint = 28;
pub const ENV_GET_CORE_ASSETS_DIRECTORY: c_uint = 30;
pub const ENV_GET_SAVE_DIRECTORY: c_uint = 31;
pub const ENV_SET_SYSTEM_AV_INFO: c_uint = 32;
pub const ENV_SET_PROC_ADDRESS_CALLBACK: c_uint = 33;
pub const ENV_SET_SUBSYSTEM_INFO: c_uint = 34;
pub const ENV_SET_CONTROLLER_INFO: c_uint = 35;
pub const ENV_SET_MEMORY_MAPS: c_uint = 36 | EXPERIMENTAL;
pub const ENV_SET_GEOMETRY: c_uint = 37;
pub const ENV_GET_USERNAME: c_uint = 38;
pub const ENV_GET_LANGUAGE: c_uint = 39;
pub const ENV_GET_CURRENT_SOFTWARE_FRAMEBUFFER: c_uint = 40 | EXPERIMENTAL;
pub const ENV_GET_HW_RENDER_INTERFACE: c_uint = 41 | EXPERIMENTAL;
pub const ENV_SET_SUPPORT_ACHIEVEMENTS: c_uint = 42 | EXPERIMENTAL;
pub const ENV_SET_HW_RENDER_CONTEXT_NEGOTIATION_INTERFACE: c_uint = 43 | EXPERIMENTAL;
pub const ENV_SET_SERIALIZATION_QUIRKS: c_uint = 44;
pub const ENV_SET_HW_SHARED_CONTEXT: c_uint = 44 | EXPERIMENTAL;
pub const ENV_GET_VFS_INTERFACE: c_uint = 45 | EXPERIMENTAL;
pub const ENV_GET_LED_INTERFACE: c_uint = 46 | EXPERIMENTAL;
pub const ENV_GET_AUDIO_VIDEO_ENABLE: c_uint = 47 | EXPERIMENTAL;
pub const ENV_GET_MIDI_INTERFACE: c_uint = 48 | EXPERIMENTAL;
pub const ENV_GET_FASTFORWARDING: c_uint = 49 | EXPERIMENTAL;
pub const ENV_GET_TARGET_REFRESH_RATE: c_uint = 50 | EXPERIMENTAL;
pub const ENV_GET_INPUT_BITMASKS: c_uint = 51 | EXPERIMENTAL;
pub const ENV_GET_CORE_OPTIONS_VERSION: c_uint = 52;
pub const ENV_SET_CORE_OPTIONS: c_uint = 53;
pub const ENV_SET_CORE_OPTIONS_INTL: c_uint = 54;
pub const ENV_SET_CORE_OPTIONS_DISPLAY: c_uint = 55;
pub const ENV_GET_PREFERRED_HW_RENDER: c_uint = 56;
pub const ENV_GET_DISK_CONTROL_INTERFACE_VERSION: c_uint = 57;
pub const ENV_SET_DISK_CONTROL_EXT_INTERFACE: c_uint = 58;
pub const ENV_GET_MESSAGE_INTERFACE_VERSION: c_uint = 59;
pub const ENV_SET_MESSAGE_EXT: c_uint = 60;
pub const ENV_GET_INPUT_MAX_USERS: c_uint = 61;
pub const ENV_SET_AUDIO_BUFFER_STATUS_CALLBACK: c_uint = 62;
pub const ENV_SET_MINIMUM_AUDIO_LATENCY: c_uint = 63;
pub const ENV_SET_FASTFORWARDING_OVERRIDE: c_uint = 64;
pub const ENV_SET_CONTENT_INFO_OVERRIDE: c_uint = 65;
pub const ENV_GET_GAME_INFO_EXT: c_uint = 66;
pub const ENV_SET_CORE_OPTIONS_V2: c_uint = 67;
pub const ENV_SET_CORE_OPTIONS_V2_INTL: c_uint = 68;
pub const ENV_SET_CORE_OPTIONS_UPDATE_DISPLAY_CALLBACK: c_uint = 69;
pub const ENV_SET_VARIABLE: c_uint = 70;
pub const ENV_GET_THROTTLE_STATE: c_uint = 71 | EXPERIMENTAL;
pub const ENV_GET_SAVESTATE_CONTEXT: c_uint = 72 | EXPERIMENTAL;
pub const ENV_GET_JIT_CAPABLE: c_uint = 74;

// pixel formats
pub const PIXEL_FORMAT_0RGB1555: c_uint = 0;
pub const PIXEL_FORMAT_XRGB8888: c_uint = 1;
pub const PIXEL_FORMAT_RGB565: c_uint = 2;

// input
pub const DEVICE_NONE: c_uint = 0;
pub const DEVICE_JOYPAD: c_uint = 1;
pub const DEVICE_MOUSE: c_uint = 2;
pub const DEVICE_KEYBOARD: c_uint = 3;
pub const DEVICE_LIGHTGUN: c_uint = 4;
pub const DEVICE_ANALOG: c_uint = 5;
pub const DEVICE_POINTER: c_uint = 6;
pub const DEVICE_MASK: c_uint = 0xff;

pub const JOYPAD_B: c_uint = 0;
pub const JOYPAD_Y: c_uint = 1;
pub const JOYPAD_SELECT: c_uint = 2;
pub const JOYPAD_START: c_uint = 3;
pub const JOYPAD_UP: c_uint = 4;
pub const JOYPAD_DOWN: c_uint = 5;
pub const JOYPAD_LEFT: c_uint = 6;
pub const JOYPAD_RIGHT: c_uint = 7;
pub const JOYPAD_A: c_uint = 8;
pub const JOYPAD_X: c_uint = 9;
pub const JOYPAD_L: c_uint = 10;
pub const JOYPAD_R: c_uint = 11;
pub const JOYPAD_L2: c_uint = 12;
pub const JOYPAD_R2: c_uint = 13;
pub const JOYPAD_L3: c_uint = 14;
pub const JOYPAD_R3: c_uint = 15;
pub const JOYPAD_MASK: c_uint = 256;

pub const MEMORY_SAVE_RAM: c_uint = 0;
pub const MEMORY_RTC: c_uint = 1;

pub const HW_FRAME_BUFFER_VALID: *const c_void = usize::MAX as *const c_void;

pub const LOG_DEBUG: i32 = 0;
pub const LOG_INFO: i32 = 1;
pub const LOG_WARN: i32 = 2;
pub const LOG_ERROR: i32 = 3;

pub const NUM_CORE_OPTION_VALUES_MAX: usize = 128;

pub type retro_environment_t = unsafe extern "C" fn(cmd: c_uint, data: *mut c_void) -> bool;
pub type retro_video_refresh_t = unsafe extern "C" fn(data: *const c_void, width: c_uint, height: c_uint, pitch: usize);
pub type retro_audio_sample_t = unsafe extern "C" fn(left: i16, right: i16);
pub type retro_audio_sample_batch_t = unsafe extern "C" fn(data: *const i16, frames: usize) -> usize;
pub type retro_input_poll_t = unsafe extern "C" fn();
pub type retro_input_state_t = unsafe extern "C" fn(port: c_uint, device: c_uint, index: c_uint, id: c_uint) -> i16;
pub type retro_log_printf_t = unsafe extern "C" fn(level: i32, fmt: *const c_char, ...);
pub type retro_frame_time_callback_t = unsafe extern "C" fn(usec: i64);
pub type retro_audio_buffer_status_callback_t = unsafe extern "C" fn(active: bool, occupancy: c_uint, underrun_likely: bool);
pub type retro_proc_address_t = Option<unsafe extern "C" fn()>;
pub type retro_hw_context_reset_t = Option<unsafe extern "C" fn()>;
pub type retro_hw_get_current_framebuffer_t = unsafe extern "C" fn() -> usize;
pub type retro_hw_get_proc_address_t = unsafe extern "C" fn(sym: *const c_char) -> retro_proc_address_t;

#[repr(C)]
pub struct retro_system_info {
    pub library_name: *const c_char,
    pub library_version: *const c_char,
    pub valid_extensions: *const c_char,
    pub need_fullpath: bool,
    pub block_extract: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct retro_game_geometry {
    pub base_width: c_uint,
    pub base_height: c_uint,
    pub max_width: c_uint,
    pub max_height: c_uint,
    pub aspect_ratio: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct retro_system_timing {
    pub fps: f64,
    pub sample_rate: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct retro_system_av_info {
    pub geometry: retro_game_geometry,
    pub timing: retro_system_timing,
}

#[repr(C)]
pub struct retro_game_info {
    pub path: *const c_char,
    pub data: *const c_void,
    pub size: usize,
    pub meta: *const c_char,
}

#[repr(C)]
pub struct retro_variable {
    pub key: *const c_char,
    pub value: *const c_char,
}

#[repr(C)]
pub struct retro_message {
    pub msg: *const c_char,
    pub frames: c_uint,
}

#[repr(C)]
pub struct retro_message_ext {
    pub msg: *const c_char,
    pub duration: c_uint,
    pub priority: c_uint,
    pub level: i32,
    pub target: i32,
    pub type_: i32,
    pub progress: i8,
}

#[repr(C)]
pub struct retro_input_descriptor {
    pub port: c_uint,
    pub device: c_uint,
    pub index: c_uint,
    pub id: c_uint,
    pub description: *const c_char,
}

#[repr(C)]
pub struct retro_log_callback {
    pub log: retro_log_printf_t,
}

#[repr(C)]
pub struct retro_frame_time_callback {
    pub callback: Option<retro_frame_time_callback_t>,
    pub reference: i64,
}

#[repr(C)]
pub struct retro_audio_buffer_status_callback {
    pub callback: Option<retro_audio_buffer_status_callback_t>,
}

#[repr(C)]
pub struct retro_core_option_value {
    pub value: *const c_char,
    pub label: *const c_char,
}

#[repr(C)]
pub struct retro_core_option_definition {
    pub key: *const c_char,
    pub desc: *const c_char,
    pub info: *const c_char,
    pub values: [retro_core_option_value; NUM_CORE_OPTION_VALUES_MAX],
    pub default_value: *const c_char,
}

#[repr(C)]
pub struct retro_core_options_intl {
    pub us: *const retro_core_option_definition,
    pub local: *const retro_core_option_definition,
}

#[repr(C)]
pub struct retro_core_option_v2_definition {
    pub key: *const c_char,
    pub desc: *const c_char,
    pub desc_categorized: *const c_char,
    pub info: *const c_char,
    pub info_categorized: *const c_char,
    pub category_key: *const c_char,
    pub values: [retro_core_option_value; NUM_CORE_OPTION_VALUES_MAX],
    pub default_value: *const c_char,
}

#[repr(C)]
pub struct retro_core_options_v2 {
    pub categories: *const c_void,
    pub definitions: *const retro_core_option_v2_definition,
}

#[repr(C)]
pub struct retro_core_options_v2_intl {
    pub us: *const retro_core_options_v2,
    pub local: *const retro_core_options_v2,
}

#[repr(C)]
pub struct retro_perf_counter {
    pub ident: *const c_char,
    pub start: u64,
    pub total: u64,
    pub call_cnt: u64,
    pub registered: bool,
}

#[repr(C)]
pub struct retro_perf_callback {
    pub get_time_usec: unsafe extern "C" fn() -> i64,
    pub get_cpu_features: unsafe extern "C" fn() -> u64,
    pub get_perf_counter: unsafe extern "C" fn() -> u64,
    pub perf_register: unsafe extern "C" fn(*mut retro_perf_counter),
    pub perf_start: unsafe extern "C" fn(*mut retro_perf_counter),
    pub perf_stop: unsafe extern "C" fn(*mut retro_perf_counter),
    pub perf_log: unsafe extern "C" fn(),
}

/// Disk control, v0 (SET_DISK_CONTROL_INTERFACE) is the first seven fields of
/// the ext (v1) interface; we only call those.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct retro_disk_control_callback {
    pub set_eject_state: Option<unsafe extern "C" fn(bool) -> bool>,
    pub get_eject_state: Option<unsafe extern "C" fn() -> bool>,
    pub get_image_index: Option<unsafe extern "C" fn() -> c_uint>,
    pub set_image_index: Option<unsafe extern "C" fn(c_uint) -> bool>,
    pub get_num_images: Option<unsafe extern "C" fn() -> c_uint>,
    pub replace_image_index: *const c_void,
    pub add_image_index: *const c_void,
}

#[repr(C)]
pub struct retro_hw_render_callback {
    pub context_type: c_uint,
    pub context_reset: retro_hw_context_reset_t,
    pub get_current_framebuffer: Option<retro_hw_get_current_framebuffer_t>,
    pub get_proc_address: Option<retro_hw_get_proc_address_t>,
    pub depth: bool,
    pub stencil: bool,
    pub bottom_left_origin: bool,
    pub version_major: c_uint,
    pub version_minor: c_uint,
    pub cache_context: bool,
    pub context_destroy: retro_hw_context_reset_t,
    pub debug_context: bool,
}

pub const HW_CONTEXT_NONE: c_uint = 0;
pub const HW_CONTEXT_OPENGL: c_uint = 1;
pub const HW_CONTEXT_OPENGLES2: c_uint = 2;
pub const HW_CONTEXT_OPENGL_CORE: c_uint = 3;
pub const HW_CONTEXT_OPENGLES3: c_uint = 4;
