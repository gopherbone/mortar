// SPDX-License-Identifier: GPL-2.0-or-later
//! `mortar-rumble [MS] [COUNT]`: buzz the Brick's rumble motor (the
//! `gpio-vibrator` force-feedback input device) COUNT times for MS each.
//! Used to get the attention of whoever is holding the Brick.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::process::exit;
use std::thread::sleep;
use std::time::Duration;

const EV_FF: u16 = 0x15;
const FF_RUMBLE: u16 = 0x50;

/// struct ff_effect with the rumble variant of the union (64-bit layout:
/// the union is 8-aligned at offset 16; total size 48).
#[repr(C)]
struct FfEffect {
    kind: u16,
    id: i16,
    direction: u16,
    trigger_button: u16,
    trigger_interval: u16,
    replay_length: u16,
    replay_delay: u16,
    _pad: u16,
    strong_magnitude: u16,
    weak_magnitude: u16,
    _union_rest: [u8; 28],
}

#[repr(C)]
struct InputEvent {
    tv_sec: libc::time_t,
    tv_usec: libc::suseconds_t,
    kind: u16,
    code: u16,
    value: i32,
}

// _IOW('E', 0x80, struct ff_effect)
const EVIOCSFF: libc::c_ulong =
    (1 << 30) | ((std::mem::size_of::<FfEffect>() as libc::c_ulong) << 16) | (0x45 << 8) | 0x80;

fn find_device() -> Option<String> {
    for entry in fs::read_dir("/sys/class/input").ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("event") {
            continue;
        }
        let dev = entry.path().join("device/name");
        if fs::read_to_string(dev).map(|n| n.trim() == "gpio-vibrator").unwrap_or(false) {
            return Some(format!("/dev/input/{name}"));
        }
    }
    None
}

fn play(f: &mut File, id: i16, on: bool) -> std::io::Result<()> {
    let ev = InputEvent { tv_sec: 0, tv_usec: 0, kind: EV_FF, code: id as u16, value: on as i32 };
    let bytes = unsafe {
        std::slice::from_raw_parts(&ev as *const _ as *const u8, std::mem::size_of::<InputEvent>())
    };
    f.write_all(bytes)
}

fn main() {
    assert_eq!(std::mem::size_of::<FfEffect>(), 48);
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ms: u16 = args.first().and_then(|a| a.parse().ok()).unwrap_or(250);
    let count: u32 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(2);

    let path = find_device().unwrap_or_else(|| {
        eprintln!("mortar-rumble: no gpio-vibrator input device");
        exit(1)
    });
    let mut f = OpenOptions::new().read(true).write(true).open(&path).unwrap_or_else(|e| {
        eprintln!("mortar-rumble: {path}: {e}");
        exit(1)
    });

    let mut eff = FfEffect {
        kind: FF_RUMBLE,
        id: -1,
        direction: 0,
        trigger_button: 0,
        trigger_interval: 0,
        replay_length: ms,
        replay_delay: 0,
        _pad: 0,
        strong_magnitude: 0xffff,
        weak_magnitude: 0xffff,
        _union_rest: [0; 28],
    };
    if unsafe { libc::ioctl(f.as_raw_fd(), EVIOCSFF, &mut eff as *mut FfEffect) } < 0 {
        eprintln!("mortar-rumble: upload effect: {}", std::io::Error::last_os_error());
        exit(1)
    }
    for i in 0..count {
        if let Err(e) = play(&mut f, eff.id, true) {
            eprintln!("mortar-rumble: play: {e}");
            exit(1)
        }
        sleep(Duration::from_millis(ms as u64));
        let _ = play(&mut f, eff.id, false);
        if i + 1 < count {
            sleep(Duration::from_millis(150));
        }
    }
}
