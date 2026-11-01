// SPDX-License-Identifier: GPL-2.0-or-later
//! Battery level and charging state from /sys/class/power_supply.

use std::fs;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Power {
    /// Battery percentage (None: no battery found, e.g. a VM).
    pub capacity: Option<u8>,
    pub charging: bool,
}

fn read(p: &Path) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

pub fn read_power(dir: &Path) -> Power {
    let mut p = Power { capacity: None, charging: false };
    let Ok(rd) = fs::read_dir(dir) else { return p };
    for e in rd.flatten() {
        let d = e.path();
        match read(&d.join("type")).as_deref() {
            Some("Battery") => {
                if p.capacity.is_none() {
                    p.capacity = read(&d.join("capacity")).and_then(|c| c.parse::<u8>().ok()).map(|c| c.min(100));
                }
                if matches!(read(&d.join("status")).as_deref(), Some("Charging" | "Full")) {
                    p.charging = true;
                }
            }
            // a charger: mains, or the USB port with VBUS
            Some("Mains" | "USB") if read(&d.join("online")).as_deref() == Some("1") => p.charging = true,
            _ => {}
        }
    }
    p
}

impl Power {
    /// Enough charge to download and stage an update?
    pub fn ok_for_update(&self, min: u8) -> bool {
        self.charging || self.capacity.is_none_or(|c| c >= min)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn supply(root: &Path, name: &str, files: &[(&str, &str)]) {
        let d = root.join(name);
        fs::create_dir_all(&d).unwrap();
        for (f, v) in files {
            fs::write(d.join(f), format!("{v}\n")).unwrap();
        }
    }

    #[test]
    fn levels() {
        let root = std::env::temp_dir().join(format!("mortar-update-power-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        assert_eq!(read_power(&root), Power { capacity: None, charging: false });
        assert!(read_power(&root).ok_for_update(30), "no battery: a VM or bench supply");
        supply(&root, "axp717-battery", &[("type", "Battery"), ("capacity", "22"), ("status", "Discharging")]);
        supply(&root, "axp717-usb", &[("type", "USB"), ("online", "0")]);
        let p = read_power(&root);
        assert_eq!(p, Power { capacity: Some(22), charging: false });
        assert!(!p.ok_for_update(30));
        supply(&root, "axp717-usb", &[("type", "USB"), ("online", "1")]);
        assert!(read_power(&root).ok_for_update(30));
        supply(&root, "axp717-usb", &[("type", "USB"), ("online", "0")]);
        supply(&root, "axp717-battery", &[("type", "Battery"), ("capacity", "30"), ("status", "Discharging")]);
        assert!(read_power(&root).ok_for_update(30));
        fs::remove_dir_all(&root).unwrap();
    }
}
