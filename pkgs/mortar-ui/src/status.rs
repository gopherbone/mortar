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
    /// On external power and the charger is charging the battery.
    pub charging: bool,
    /// On external power (charging, full or paused).
    pub plugged: bool,
    pub wifi: bool,
    pub usb_net: bool,
    pub ssh_sessions: u32,
    pub sshd_listening: bool,
    pub brightness: Option<u8>,
}

impl Sys {
    pub fn read() -> Sys {
        let power = Power::read();
        Sys {
            time: clock(),
            battery: power.as_ref().and_then(|p| p.capacity),
            charging: power.as_ref().is_some_and(|p| p.charging),
            plugged: power.as_ref().is_some_and(|p| p.plugged),
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
            charging: false,
            plugged: false,
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
            charging: self.charging,
            ssh_sessions: self.ssh_sessions,
            ssh_badge_always: false,
            note: if self.usb_net { Some("USB".into()) } else { None },
        }
    }
}

fn read_trim(p: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

unsafe extern "C" {
    fn tzset();
}

/// Re-read the time zone. glibc's localtime_r never looks at /etc/localtime
/// again by itself; tzset (TZ unset) does when the file changed (e.g. after
/// `timedatectl set-timezone`). Cheap: one stat.
pub fn reload_timezone() {
    // SAFETY: no arguments; nothing here calls setenv("TZ")
    unsafe { tzset() }
}

pub fn clock() -> String {
    reload_timezone();
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return String::new();
        }
        format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
    }
}

/// Battery state: from mortar-power's state file (its capacity is the
/// gauge's, or a smoothed voltage estimate when the gauge has no data), or
/// straight from the power-supply class when that service is not running.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Power {
    pub capacity: Option<u8>,
    pub charging: bool,
    pub plugged: bool,
    /// "ok", "low", "critical" or "shutdown" (mortar-power only).
    pub level: String,
    /// The capacity is a voltage estimate, not the fuel gauge.
    pub estimated: bool,
    pub voltage_mv: Option<u32>,
    pub time_to_empty_s: Option<u32>,
    pub time_to_full_s: Option<u32>,
}

/// mortar-power's state file (key=value lines, rewritten atomically).
pub const POWER_STATE: &str = "/run/mortar-power/state";

impl Power {
    pub fn read() -> Option<Power> {
        Power::from_state_file(Path::new(POWER_STATE)).or_else(Power::from_sysfs)
    }

    /// The state file, if mortar-power updated it in the last minute.
    pub fn from_state_file(path: &Path) -> Option<Power> {
        let meta = fs::metadata(path).ok()?;
        let age = meta.modified().ok()?.elapsed().unwrap_or_default();
        if age > std::time::Duration::from_secs(60) {
            return None;
        }
        Some(Power::parse(&fs::read_to_string(path).ok()?))
    }

    pub fn parse(text: &str) -> Power {
        let mut p = Power::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim();
            match k.trim() {
                "capacity" => p.capacity = v.parse::<u8>().ok().map(|c| c.min(100)),
                "charging" => p.charging = v == "1",
                "plugged" => p.plugged = v == "1",
                "level" => p.level = v.to_string(),
                "source" => p.estimated = v == "voltage",
                "voltage_mv" => p.voltage_mv = v.parse().ok(),
                "time_to_empty_s" => p.time_to_empty_s = v.parse().ok(),
                "time_to_full_s" => p.time_to_full_s = v.parse().ok(),
                _ => {}
            }
        }
        p
    }

    fn from_sysfs() -> Option<Power> {
        let mut out: Option<Power> = None;
        let mut plugged = false;
        for e in fs::read_dir("/sys/class/power_supply").ok()?.flatten() {
            let p = e.path();
            match read_trim(p.join("type")).as_deref() {
                Some("Battery") if out.is_none() => {
                    if read_trim(p.join("present")).as_deref() == Some("0") {
                        continue;
                    }
                    let status = read_trim(p.join("status")).unwrap_or_default();
                    out = Some(Power {
                        capacity: read_trim(p.join("capacity")).and_then(|c| c.parse::<u8>().ok()).map(|c| c.min(100)),
                        charging: status == "Charging",
                        plugged: status == "Charging" || status == "Full",
                        voltage_mv: read_trim(p.join("voltage_now")).and_then(|v| v.parse::<u32>().ok()).map(|uv| uv / 1000),
                        ..Power::default()
                    });
                }
                Some("USB") | Some("Mains") => plugged |= read_trim(p.join("online")).as_deref() == Some("1"),
                _ => {}
            }
        }
        let mut out = out?;
        out.plugged |= plugged;
        Some(out)
    }
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
    /// Switch the backlight off or on without touching the level
    /// (bl_power: 0 on, 4 = FB_BLANK_POWERDOWN).
    pub fn power(&self, on: bool) -> Result<(), String> {
        fs::write(self.dir.join("bl_power"), if on { "0" } else { "4" }).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::Power;

    #[test]
    fn parses_mortar_power_state() {
        let p = Power::parse("capacity=64\nsource=voltage\nstatus=Charging\ncharging=1\nplugged=1\nvoltage_mv=3987\ntime_to_empty_s=\ntime_to_full_s=4200\nlevel=ok\n");
        assert_eq!(p.capacity, Some(64));
        assert!(p.charging && p.plugged && p.estimated);
        assert_eq!((p.voltage_mv, p.time_to_empty_s, p.time_to_full_s), (Some(3987), None, Some(4200)));
        assert_eq!(p.level, "ok");
        assert_eq!(Power::parse("capacity=\n").capacity, None);
    }
}
