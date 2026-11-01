// SPDX-License-Identifier: GPL-2.0-or-later
//! Per-app virtual gamepads (uinput).
//!
//! The shell grabs every physical pad. Each running app in "forward" mode
//! gets its own uinput clone of every physical pad (same buttons and axes,
//! so SDL builds the same automatic mapping), and only the visible app's
//! clones are fed. Clones have vendor 0x6d72 and product 0x4d00 | slot<<4 |
//! pad, and each app is started with SDL's joystick blacklist naming the
//! physical pads and every other slot's clones, so an SDL app opens only its
//! own copy (see DESIGN.md for why this, not EVIOCREVOKE).

use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, BusType, Device, EventType, InputEvent, InputId, KeyCode, UinputAbsSetup};
use std::collections::HashMap;

pub const VENDOR: u16 = 0x6d72;
/// Clones per slot named in the blacklist (pads beyond this still work,
/// they just are not hidden from other apps).
const PADS_IN_BLACKLIST: u16 = 4;
/// gpio-keys' default id: the Brick's built-in pad (and its volume keys).
const GPIO_KEYS: (u16, u16) = (0x0001, 0x0001);

pub fn product(slot: u8, pad: u16) -> u16 {
    0x4d00 | ((slot as u16) << 4) | (pad & 0xf)
}

pub struct PhysPad {
    pub id: u16,
    pub name: String,
    pub input_id: InputId,
    keys: AttributeSet<KeyCode>,
    abs: Vec<UinputAbsSetup>,
}

#[derive(Default)]
pub struct Pads {
    phys: Vec<PhysPad>,
    virt: HashMap<(u8, u16), VirtualDevice>,
    /// Slots that want clones (running forward-mode apps).
    slots: Vec<u8>,
    /// uinput unavailable: forwarding disabled, logged once.
    broken: bool,
}

impl Pads {
    /// Register a physical pad; returns its pad id. Clones are made for
    /// every active slot.
    pub fn add_phys(&mut self, dev: &Device) -> u16 {
        let id = (0..16).find(|i| !self.phys.iter().any(|p| p.id == *i)).unwrap_or(15);
        let mut keys = AttributeSet::<KeyCode>::new();
        if let Some(k) = dev.supported_keys() {
            for c in k.iter() {
                keys.insert(c);
            }
        }
        let abs = dev
            .get_absinfo()
            .map(|it| it.map(|(code, info)| UinputAbsSetup::new(code, info)).collect())
            .unwrap_or_default();
        self.phys.push(PhysPad {
            id,
            name: dev.name().unwrap_or("gamepad").to_string(),
            input_id: dev.input_id(),
            keys,
            abs,
        });
        for s in self.slots.clone() {
            self.make(s, id);
        }
        id
    }

    pub fn remove_phys(&mut self, id: u16) {
        self.phys.retain(|p| p.id != id);
        self.virt.retain(|(_, p), _| *p != id);
    }

    fn make(&mut self, slot: u8, pad: u16) {
        if self.broken || self.virt.contains_key(&(slot, pad)) {
            return;
        }
        let Some(p) = self.phys.iter().find(|p| p.id == pad) else { return };
        let name = format!("{} (Mortar {})", p.name, slot);
        let id = InputId::new(BusType::BUS_VIRTUAL, VENDOR, product(slot, pad), p.input_id.version());
        let build = || -> std::io::Result<VirtualDevice> {
            let mut b = VirtualDevice::builder()?.name(&name).input_id(id).with_keys(&p.keys)?;
            for a in &p.abs {
                b = b.with_absolute_axis(a)?;
            }
            b.build()
        };
        match build() {
            Ok(v) => {
                tracing::info!(slot, pad, %name, "virtual pad created");
                self.virt.insert((slot, pad), v);
            }
            Err(e) => {
                tracing::warn!("uinput: {e}; apps will not get gamepad input (need /dev/uinput)");
                self.broken = true;
            }
        }
    }

    /// Make clones for an app slot (before launching it).
    pub fn add_slot(&mut self, slot: u8) {
        if !self.slots.contains(&slot) {
            self.slots.push(slot);
        }
        let ids: Vec<u16> = self.phys.iter().map(|p| p.id).collect();
        for id in ids {
            self.make(slot, id);
        }
    }

    pub fn remove_slot(&mut self, slot: u8) {
        self.slots.retain(|s| *s != slot);
        self.virt.retain(|(s, _), _| *s != slot);
    }

    pub fn emit(&mut self, slot: u8, pad: u16, ty: EventType, code: u16, value: i32) {
        if let Some(v) = self.virt.get_mut(&(slot, pad))
            && let Err(e) = v.emit(&[InputEvent::new(ty.0, code, value)])
        {
            tracing::warn!("uinput emit: {e}");
        }
    }

    /// Release `codes` on all of a slot's clones (that have the key), and
    /// centre their hats.
    pub fn release_all(&mut self, slot: u8, codes: &[u16]) {
        let pads: Vec<(u16, bool, bool)> = self
            .phys
            .iter()
            .map(|p| (p.id, true, p.abs.iter().any(|a| a.code() == 0x10 || a.code() == 0x11)))
            .collect();
        for (pad, _, hat) in pads {
            let has = |c: u16| self.phys.iter().any(|p| p.id == pad && p.keys.contains(KeyCode::new(c)));
            let codes: Vec<u16> = codes.iter().copied().filter(|c| has(*c)).collect();
            for c in codes {
                self.emit(slot, pad, EventType::KEY, c, 0);
            }
            if hat {
                self.emit(slot, pad, EventType::ABSOLUTE, 0x10, 0);
                self.emit(slot, pad, EventType::ABSOLUTE, 0x11, 0);
            }
        }
    }

    /// SDL_JOYSTICK_BLACKLIST_DEVICES for an app in `slot`: the physical
    /// pads, gpio-keys, and every other slot's clones.
    pub fn blacklist(&self, slot: u8) -> String {
        let mut ids: Vec<(u16, u16)> = vec![GPIO_KEYS];
        for p in &self.phys {
            let id = (p.input_id.vendor(), p.input_id.product());
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        for s in 0..crate::apps::MAX_SLOTS {
            if s != slot {
                for pad in 0..PADS_IN_BLACKLIST {
                    ids.push((VENDOR, product(s, pad)));
                }
            }
        }
        ids.iter().map(|(v, p)| format!("0x{v:04x}/0x{p:04x}")).collect::<Vec<_>>().join(",")
    }

    pub fn phys_names(&self) -> Vec<String> {
        self.phys.iter().map(|p| p.name.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blacklist_names_other_slots_only() {
        let p = Pads::default();
        let b = p.blacklist(2);
        assert!(b.starts_with("0x0001/0x0001,"));
        assert!(b.contains(&format!("0x6d72/0x{:04x}", product(0, 0))));
        assert!(!b.contains(&format!("0x6d72/0x{:04x}", product(2, 0))));
        assert_eq!(product(3, 1), 0x4d31);
    }
}
