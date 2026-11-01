// SPDX-License-Identifier: GPL-2.0-or-later
//! Wi-Fi and Bluetooth on/off for MENU > Quick: the kernel's rfkill
//! switches. State from /sys/class/rfkill, changes written to /dev/rfkill
//! (systemd gives the active seat's user access to it, the `uaccess` tag),
//! so no daemon is involved. iwd and bluetoothd follow the soft block on
//! their own (powering the adapter down and back up, reconnecting).

use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Radio {
    Wifi,
    Bluetooth,
}

impl Radio {
    /// rfkill's `type` file and the RFKILL_TYPE_* value.
    fn kind(self) -> (&'static str, u8) {
        match self {
            Radio::Wifi => ("wlan", 1),
            Radio::Bluetooth => ("bluetooth", 2),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Radio::Wifi => "Wi-Fi",
            Radio::Bluetooth => "Bluetooth",
        }
    }
}

/// Whether the radio is on (None: no such switch).
pub fn state(r: Radio) -> Option<bool> {
    state_in(Path::new("/sys/class/rfkill"), r)
}

fn state_in(dir: &Path, r: Radio) -> Option<bool> {
    let (kind, _) = r.kind();
    let mut found = None;
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        let read = |f: &str| std::fs::read_to_string(p.join(f)).ok().map(|s| s.trim().to_string());
        if read("type").as_deref() != Some(kind) {
            continue;
        }
        let blocked = read("soft").as_deref() == Some("1") || read("hard").as_deref() == Some("1");
        // on if any switch of that type is unblocked
        found = Some(found.unwrap_or(false) || !blocked);
    }
    found
}

/// Block or unblock every switch of the radio's type.
pub fn set(r: Radio, on: bool) -> Result<(), String> {
    use std::io::Write;
    let (_, ty) = r.kind();
    // struct rfkill_event { u32 idx; u8 type; u8 op; u8 soft; u8 hard; },
    // op 3 = RFKILL_OP_CHANGE_ALL
    let ev = [0, 0, 0, 0, ty, 3, u8::from(!on), 0];
    let mut f = std::fs::OpenOptions::new().write(true).open("/dev/rfkill").map_err(|e| format!("/dev/rfkill: {e}"))?;
    f.write_all(&ev).map_err(|e| format!("/dev/rfkill: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_switches() {
        let d = std::env::temp_dir().join(format!("mortar-rfkill-{}", std::process::id()));
        let mk = |n: &str, ty: &str, soft: &str| {
            let p = d.join(n);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("type"), format!("{ty}\n")).unwrap();
            std::fs::write(p.join("soft"), format!("{soft}\n")).unwrap();
            std::fs::write(p.join("hard"), "0\n").unwrap();
        };
        mk("rfkill0", "wlan", "0");
        mk("rfkill1", "bluetooth", "1");
        assert_eq!(state_in(&d, Radio::Wifi), Some(true));
        assert_eq!(state_in(&d, Radio::Bluetooth), Some(false));
        std::fs::remove_dir_all(&d).unwrap();
        assert_eq!(state_in(&d, Radio::Wifi), None);
    }
}
