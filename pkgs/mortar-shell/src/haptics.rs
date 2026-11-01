// SPDX-License-Identifier: GPL-2.0-or-later
//! Haptic ticks on the Brick's rumble motor: the `gpio-vibrator`
//! force-feedback input device (see pkgs/mortar-rumble). The motor is on/off,
//! so the feel comes from pulse length alone:
//!
//!   tick    one short pulse (default 8 ms): moves, tabs, the drawer
//!   double  two pulses (18 ms, 72 ms apart): launch, switch, close
//!   long    one long pulse (60 ms): screenshot
//!
//! A thread plays them (an FF_RUMBLE effect uploaded once with no length,
//! started and stopped by hand), so timing never blocks the compositor.
//! Pulses that arrive while one plays are dropped, not queued: a held D-pad
//! must not leave the motor buzzing after release.

use serde::Deserialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Haptic {
    Tick,
    Double,
    Long,
}

/// Pulse lengths in ms (config `ui.hapticMs`).
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Pulses {
    pub tick: u32,
    pub double: u32,
    pub gap: u32,
    pub long: u32,
}

impl Default for Pulses {
    fn default() -> Self {
        Pulses { tick: 8, double: 18, gap: 72, long: 60 }
    }
}

impl Pulses {
    /// (on, off) pairs for a pattern.
    pub fn pattern(&self, h: Haptic) -> Vec<(u32, u32)> {
        match h {
            Haptic::Tick => vec![(self.tick, 0)],
            Haptic::Double => vec![(self.double, self.gap), (self.double, 0)],
            Haptic::Long => vec![(self.long, 0)],
        }
    }
}

pub struct Haptics {
    tx: Option<std::sync::mpsc::SyncSender<Haptic>>,
}

impl Haptics {
    pub fn off() -> Haptics {
        Haptics { tx: None }
    }

    /// Find the rumble device and start the thread (None if there is none).
    pub fn start(pulses: Pulses) -> Haptics {
        #[cfg(target_os = "linux")]
        {
            match dev::open() {
                Ok((path, dev)) => {
                    tracing::info!(path = %path, "haptics: rumble device");
                    // capacity 1: a pattern waiting while one plays is enough
                    let (tx, rx) = std::sync::mpsc::sync_channel::<Haptic>(1);
                    let r = std::thread::Builder::new().name("mortar-haptics".into()).spawn(move || dev::run(dev, rx, pulses));
                    if let Err(e) = r {
                        tracing::warn!("haptics thread: {e}");
                        return Haptics::off();
                    }
                    Haptics { tx: Some(tx) }
                }
                Err(e) => {
                    tracing::info!("haptics off: {e}");
                    Haptics::off()
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = pulses;
            Haptics::off()
        }
    }

    pub fn play(&self, h: Haptic) {
        if let Some(tx) = &self.tx {
            let _ = tx.try_send(h); // full: a pattern is pending, drop this one
        }
    }
}

#[cfg(target_os = "linux")]
mod dev {
    use super::{Haptic, Pulses};
    use std::fs::{self, File, OpenOptions};
    use std::io::Write;
    use std::os::fd::AsRawFd;
    use std::sync::mpsc::Receiver;
    use std::time::Duration;

    const EV_FF: u16 = 0x15;
    const FF_RUMBLE: u16 = 0x50;

    /// struct ff_effect, rumble variant (64-bit layout, 48 bytes).
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

    const EVIOCSFF: libc::c_ulong = (1 << 30) | ((std::mem::size_of::<FfEffect>() as libc::c_ulong) << 16) | (0x45 << 8) | 0x80;

    pub struct Dev {
        f: File,
        id: i16,
    }

    fn find() -> Option<String> {
        for e in fs::read_dir("/sys/class/input").ok()?.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with("event")
                && fs::read_to_string(e.path().join("device/name")).is_ok_and(|n| n.trim() == "gpio-vibrator")
            {
                return Some(format!("/dev/input/{name}"));
            }
        }
        None
    }

    pub fn open() -> Result<(String, Dev), String> {
        assert_eq!(std::mem::size_of::<FfEffect>(), 48);
        let path = std::env::var("MORTAR_SHELL_RUMBLE").ok().or_else(find).ok_or("no gpio-vibrator device")?;
        let f = OpenOptions::new().read(true).write(true).open(&path).map_err(|e| format!("{path}: {e}"))?;
        let mut eff = FfEffect {
            kind: FF_RUMBLE,
            id: -1,
            direction: 0,
            trigger_button: 0,
            trigger_interval: 0,
            replay_length: 0, // until stopped
            replay_delay: 0,
            _pad: 0,
            strong_magnitude: 0xffff,
            weak_magnitude: 0xffff,
            _union_rest: [0; 28],
        };
        // SAFETY: EVIOCSFF with a properly laid out ff_effect
        if unsafe { libc::ioctl(f.as_raw_fd(), EVIOCSFF, &mut eff as *mut FfEffect) } < 0 {
            return Err(format!("{path}: upload effect: {}", std::io::Error::last_os_error()));
        }
        Ok((path, Dev { f, id: eff.id }))
    }

    impl Dev {
        fn set(&mut self, on: bool) -> std::io::Result<()> {
            let ev = InputEvent { tv_sec: 0, tv_usec: 0, kind: EV_FF, code: self.id as u16, value: on as i32 };
            // SAFETY: plain struct as bytes
            let bytes = unsafe { std::slice::from_raw_parts(&ev as *const _ as *const u8, std::mem::size_of::<InputEvent>()) };
            self.f.write_all(bytes)
        }
    }

    pub fn run(mut dev: Dev, rx: Receiver<Haptic>, pulses: Pulses) {
        while let Ok(h) = rx.recv() {
            for (on, off) in pulses.pattern(h) {
                if dev.set(true).is_err() {
                    tracing::warn!("haptics: write failed; stopping");
                    return;
                }
                std::thread::sleep(Duration::from_millis(on as u64));
                let _ = dev.set(false);
                if off > 0 {
                    std::thread::sleep(Duration::from_millis(off as u64));
                }
            }
            // a short rest so ticks stay distinct on a slow motor
            std::thread::sleep(Duration::from_millis(12));
        }
        let _ = dev.set(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn patterns() {
        let p = Pulses::default();
        assert_eq!(p.pattern(Haptic::Tick), vec![(8, 0)]);
        assert_eq!(p.pattern(Haptic::Double), vec![(18, 72), (18, 0)]);
        assert_eq!(p.pattern(Haptic::Long), vec![(60, 0)]);
    }
}
