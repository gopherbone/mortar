// SPDX-License-Identifier: GPL-2.0-or-later
//! Settings > About and the Home status widgets: the system's version and
//! generation, free storage, the battery's health and cycle count, the SoC
//! temperature. All plain file reads and one statvfs (cheap; the
//! compositor re-reads them with the status bar, every 2 s while a shell
//! screen shows). Anything the device does not report is None.

use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct About {
    /// "25.11.20261101.abcdef0" (the NixOS version of the running system).
    pub version: Option<String>,
    /// The system profile's generation number.
    pub generation: Option<u32>,
    pub kernel: Option<String>,
    /// Free and total bytes on /.
    pub storage: Option<(u64, u64)>,
    pub battery: Battery,
    /// The hottest thermal zone, degrees Celsius.
    pub temp_c: Option<f32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Battery {
    /// The gauge's word for it ("Good", "Overheat", ...).
    pub health: Option<String>,
    pub cycles: Option<u32>,
    /// Full charge now against the design capacity, percent.
    pub capacity_pct: Option<u8>,
}

impl About {
    pub fn read() -> About {
        About {
            version: read_trim("/run/current-system/nixos-version").or_else(os_release_version),
            generation: std::fs::read_link("/nix/var/nix/profiles/system").ok().and_then(|p| generation_of(&p.to_string_lossy())),
            kernel: read_trim("/proc/sys/kernel/osrelease"),
            storage: statvfs(Path::new("/")),
            battery: battery(Path::new("/sys/class/power_supply")),
            temp_c: temperature(Path::new("/sys/class/thermal")),
        }
    }

    /// Fixed values for previews and tests.
    pub fn demo() -> About {
        About {
            version: Some("25.11.20261101.7d5149e".into()),
            generation: Some(42),
            kernel: Some("7.3.0-mortar".into()),
            storage: Some((12_400_000_000, 29_300_000_000)),
            battery: Battery { health: Some("Good".into()), cycles: Some(212), capacity_pct: Some(94) },
            temp_c: Some(41.5),
        }
    }

    /// "12.4 GB free of 29.3 GB".
    pub fn storage_line(&self) -> Option<String> {
        self.storage.map(|(free, total)| format!("{} free of {}", gb(free), gb(total)))
    }
}

pub fn gb(b: u64) -> String {
    let g = b as f64 / 1e9;
    if g >= 100.0 { format!("{g:.0} GB") } else if g >= 1.0 { format!("{g:.1} GB") } else { format!("{:.0} MB", b as f64 / 1e6) }
}

fn read_trim(p: impl AsRef<Path>) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn os_release_version() -> Option<String> {
    let s = std::fs::read_to_string("/etc/os-release").ok()?;
    s.lines().find_map(|l| l.strip_prefix("VERSION_ID=")).map(|v| v.trim_matches('"').to_string())
}

/// "/nix/var/nix/profiles/system-42-link" or "system-42-link" -> 42.
pub fn generation_of(link: &str) -> Option<u32> {
    let name = link.rsplit('/').next()?;
    name.strip_prefix("system-")?.strip_suffix("-link")?.parse().ok()
}

fn statvfs(p: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(p.as_os_str().as_bytes()).ok()?;
    // SAFETY: a zeroed statvfs filled by the call; c is NUL-terminated
    unsafe {
        let mut st: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c.as_ptr(), &mut st) != 0 {
            return None;
        }
        let f = st.f_frsize as u64;
        Some((st.f_bavail as u64 * f, st.f_blocks as u64 * f))
    }
}

/// The first battery in the power-supply class.
pub fn battery(dir: &Path) -> Battery {
    let Ok(rd) = std::fs::read_dir(dir) else { return Battery::default() };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if read_trim(p.join("type")).as_deref() != Some("Battery") {
            continue;
        }
        let num = |f: &str| read_trim(p.join(f)).and_then(|v| v.parse::<u64>().ok());
        let full = num("charge_full").zip(num("charge_full_design")).or_else(|| num("energy_full").zip(num("energy_full_design")));
        return Battery {
            health: read_trim(p.join("health")).filter(|h| h != "Unknown"),
            cycles: num("cycle_count").map(|c| c as u32).filter(|c| *c > 0),
            capacity_pct: full.filter(|(_, d)| *d > 0).map(|(f, d)| ((f * 100) / d).min(100) as u8),
        };
    }
    Battery::default()
}

/// The hottest thermal zone (millidegrees in sysfs).
pub fn temperature(dir: &Path) -> Option<f32> {
    let rd = std::fs::read_dir(dir).ok()?;
    rd.flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("thermal_zone"))
        .filter_map(|e| read_trim(e.path().join("temp"))?.parse::<i64>().ok())
        .map(|m| m as f32 / 1000.0)
        .filter(|c| (-40.0..150.0).contains(c))
        .reduce(f32::max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_sysfs_shapes() {
        let d = std::env::temp_dir().join(format!("mortar-about-{}", std::process::id()));
        let bat = d.join("ps/axp-battery");
        std::fs::create_dir_all(&bat).unwrap();
        std::fs::create_dir_all(d.join("ps/usb")).unwrap();
        std::fs::write(d.join("ps/usb/type"), "USB\n").unwrap();
        std::fs::write(bat.join("type"), "Battery\n").unwrap();
        std::fs::write(bat.join("health"), "Good\n").unwrap();
        std::fs::write(bat.join("cycle_count"), "212\n").unwrap();
        std::fs::write(bat.join("charge_full"), "4700000\n").unwrap();
        std::fs::write(bat.join("charge_full_design"), "5000000\n").unwrap();
        let b = battery(&d.join("ps"));
        assert_eq!(b, Battery { health: Some("Good".into()), cycles: Some(212), capacity_pct: Some(94) });
        for (z, t) in [("thermal_zone0", "41500"), ("thermal_zone1", "47250"), ("cooling_device0", "99000")] {
            std::fs::create_dir_all(d.join("th").join(z)).unwrap();
            std::fs::write(d.join("th").join(z).join("temp"), t).unwrap();
        }
        assert_eq!(temperature(&d.join("th")), Some(47.25));
        assert_eq!(battery(&d.join("nothing")), Battery::default());
        std::fs::remove_dir_all(&d).ok();
        assert_eq!(generation_of("/nix/var/nix/profiles/system-42-link"), Some(42));
        assert_eq!(generation_of("system-7-link"), Some(7));
        assert_eq!(generation_of("/nix/store/abc-nixos-system"), None);
        assert_eq!(gb(12_400_000_000), "12.4 GB");
        assert_eq!(gb(512_000_000), "512 MB");
        assert!(statvfs(Path::new("/")).is_some_and(|(f, t)| t > 0 && f <= t));
    }
}
