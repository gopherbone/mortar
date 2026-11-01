// SPDX-License-Identifier: GPL-2.0-or-later
//! System status for the status bar and quick menu, read from /sys and /proc
//! (no daemons, no D-Bus). Each probe degrades to "unknown" off-device.

use crate::toolkit::StatusInfo;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default)]
pub struct Sys {
    pub time: String,
    pub battery: Option<u8>,
    pub wifi: bool,
    pub usb_net: bool,
    pub ssh_sessions: u32,
    pub sshd_listening: bool,
    pub brightness: Option<u8>,
}

impl Sys {
    pub fn read() -> Sys {
        Sys {
            time: clock(),
            battery: battery(),
            wifi: wifi_up(),
            usb_net: iface_up("usb0"),
            ssh_sessions: ssh_sessions(),
            sshd_listening: listening(22),
            brightness: Backlight::find().and_then(|b| b.get()),
        }
    }
    /// Fixed values matching the plan mockups (screenshots, dev mode).
    pub fn demo(time: Option<String>) -> Sys {
        Sys {
            time: time.unwrap_or_else(clock),
            battery: Some(74),
            wifi: true,
            usb_net: false,
            ssh_sessions: 0,
            sshd_listening: true,
            brightness: Some(60),
        }
    }
    pub fn bar(&self) -> StatusInfo {
        StatusInfo {
            left: self.time.clone(),
            wifi: self.wifi,
            battery: self.battery,
            ssh_sessions: self.ssh_sessions,
            ssh_badge_always: false,
            note: if self.usb_net { Some("USB".into()) } else { None },
        }
    }
}

fn read_trim(p: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

pub fn clock() -> String {
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return String::new();
        }
        format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
    }
}

fn battery() -> Option<u8> {
    for e in fs::read_dir("/sys/class/power_supply").ok()?.flatten() {
        let p = e.path();
        if read_trim(p.join("type")).as_deref() == Some("Battery")
            && let Some(c) = read_trim(p.join("capacity")).and_then(|c| c.parse::<u8>().ok()) {
                return Some(c.min(100));
            }
    }
    None
}

fn iface_up(name: &str) -> bool {
    read_trim(format!("/sys/class/net/{name}/operstate")).as_deref() == Some("up")
}

fn wifi_up() -> bool {
    let Ok(rd) = fs::read_dir("/sys/class/net") else { return false };
    rd.flatten().any(|e| e.path().join("wireless").exists() && iface_up(&e.file_name().to_string_lossy()))
}

/// Logged-in SSH connections: per-connection sshd processes retitle
/// themselves "sshd-session: user@pts/0" (OpenSSH >= 9.8) or "sshd: user@…".
fn ssh_sessions() -> u32 {
    let Ok(rd) = fs::read_dir("/proc") else { return 0 };
    rd.flatten()
        .filter(|e| e.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit()))
        .filter_map(|e| fs::read(e.path().join("cmdline")).ok())
        .filter(|c| {
            let s = String::from_utf8_lossy(c);
            (s.starts_with("sshd-session: ") || s.starts_with("sshd: ")) && s.contains('@')
        })
        .count() as u32
}

fn listening(port: u16) -> bool {
    let want = format!(":{port:04X}");
    ["/proc/net/tcp", "/proc/net/tcp6"].iter().any(|f| {
        fs::read_to_string(f).is_ok_and(|s| {
            s.lines().skip(1).any(|l| {
                let c: Vec<&str> = l.split_whitespace().collect();
                c.len() > 3 && c[1].ends_with(&want) && c[3] == "0A"
            })
        })
    })
}

/// The panel backlight (/sys/class/backlight/*). Writing needs the udev rule
/// from the mortar-greeter module (group video, g+w).
pub struct Backlight {
    dir: PathBuf,
    max: u32,
}

impl Backlight {
    pub fn find() -> Option<Backlight> {
        let e = fs::read_dir("/sys/class/backlight").ok()?.flatten().next()?;
        let max = read_trim(e.path().join("max_brightness"))?.parse().ok()?;
        Some(Backlight { dir: e.path(), max })
    }
    pub fn get(&self) -> Option<u8> {
        let v: u32 = read_trim(self.dir.join("brightness"))?.parse().ok()?;
        Some(((v * 100 + self.max / 2) / self.max.max(1)) as u8)
    }
    pub fn set(&self, pct: u8) -> Result<(), String> {
        let v = (pct.clamp(5, 100) as u32 * self.max).div_ceil(100);
        fs::write(self.dir.join("brightness"), v.to_string()).map_err(|e| e.to_string())
    }
}
