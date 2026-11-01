// SPDX-License-Identifier: GPL-2.0-or-later
//! The decisions, without I/O (unit tested): USB input current, and the
//! low-battery warnings and power-off.

use std::collections::VecDeque;

// ------------------------------------------------------------------ USB ----

/// Stock (TrimUI 4.9 axp2202_usb_power) input current policy, as
/// jkingston's brick-usb-current reimplements it: start every attach at the
/// USB 2.0 PC limit; if no USB host configures the gadget within a few
/// seconds it is a charger, so raise the limit to the adapter current. A
/// data connection keeps the PC limit until the cable is pulled.
#[derive(Debug, Default)]
pub struct UsbPolicy {
    observed: bool,
    attached_ms: Option<u64>,
    delay_ms: u64,
    configured: bool,
}

pub const ATTACH_DELAY_MS: u64 = 5_000;
/// At boot the gadget needs longer to come up and enumerate.
pub const BOOT_DELAY_MS: u64 = 20_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsbLimit {
    Host,
    Adapter,
}

impl UsbPolicy {
    pub fn update(&mut self, online: bool, configured: bool, now_ms: u64) -> UsbLimit {
        if !online {
            *self = UsbPolicy { observed: true, ..UsbPolicy::default() };
            return UsbLimit::Host;
        }
        let attached = *self.attached_ms.get_or_insert_with(|| {
            self.delay_ms = if self.observed { ATTACH_DELAY_MS } else { BOOT_DELAY_MS };
            now_ms
        });
        self.observed = true;
        self.configured |= configured;
        if self.configured || now_ms.saturating_sub(attached) < self.delay_ms {
            UsbLimit::Host
        } else {
            UsbLimit::Adapter
        }
    }
}

// -------------------------------------------------------------- battery ----

#[derive(Clone, Copy, Debug)]
pub struct Thresholds {
    /// Warn at these capacities, highest first (15 and 5).
    pub low: u8,
    pub critical: u8,
    /// Power off at or below this capacity...
    pub off: u8,
    /// ...but only if the battery also reads below this (a confused gauge
    /// at a healthy voltage must not switch the Brick off).
    pub off_confirm_mv: u32,
    /// Power off below this loaded voltage (median of 3), whatever the gauge.
    pub floor_mv: u32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds { low: 15, critical: 5, off: 3, off_confirm_mv: 3650, floor_mv: 3300 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Ok,
    Low,
    Critical,
    /// Power-off pending.
    Shutdown,
}

impl Level {
    pub fn name(self) -> &'static str {
        match self {
            Level::Ok => "ok",
            Level::Low => "low",
            Level::Critical => "critical",
            Level::Shutdown => "shutdown",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Low(u8),
    Critical(u8),
    /// Start the power-off countdown.
    PowerOff,
    /// External power came back during the countdown.
    Cancelled,
    Plugged,
}

/// One battery reading (every ~10 s).
#[derive(Clone, Copy, Debug, Default)]
pub struct Reading {
    pub capacity: Option<u8>,
    pub voltage_mv: Option<u32>,
    /// Discharging and no external power.
    pub on_battery: bool,
}

#[derive(Debug)]
pub struct Battery {
    t: Thresholds,
    warned_low: bool,
    warned_critical: bool,
    voltages: VecDeque<u32>,
    /// Consecutive readings that met the power-off condition.
    off_votes: u32,
    pub level: Level,
    was_on_battery: Option<bool>,
}

impl Battery {
    pub fn new(t: Thresholds) -> Battery {
        Battery {
            t,
            warned_low: false,
            warned_critical: false,
            voltages: VecDeque::new(),
            off_votes: 0,
            level: Level::Ok,
            was_on_battery: None,
        }
    }

    fn median_mv(&self) -> Option<u32> {
        if self.voltages.len() < 3 {
            return None;
        }
        let mut v: Vec<u32> = self.voltages.iter().copied().collect();
        v.sort_unstable();
        Some(v[v.len() / 2])
    }

    pub fn update(&mut self, r: Reading) -> Vec<Event> {
        let mut ev = Vec::new();
        let was = self.was_on_battery.replace(r.on_battery);
        if !r.on_battery {
            if self.level == Level::Shutdown {
                ev.push(Event::Cancelled);
            } else if was == Some(true) {
                ev.push(Event::Plugged);
            }
            self.warned_low = false;
            self.warned_critical = false;
            self.voltages.clear();
            self.off_votes = 0;
            self.level = Level::Ok;
            return ev;
        }
        if let Some(mv) = r.voltage_mv {
            self.voltages.push_back(mv);
            if self.voltages.len() > 3 {
                self.voltages.pop_front();
            }
        }
        // re-arm a warning once the level is clearly above it again (a
        // recalibrating gauge can step up a few percent)
        if let Some(c) = r.capacity {
            if c >= self.t.low + 3 {
                self.warned_low = false;
            }
            if c >= self.t.critical + 3 {
                self.warned_critical = false;
            }
        }

        let median = self.median_mv();
        let by_capacity = r.capacity.is_some_and(|c| c <= self.t.off)
            && r.voltage_mv.is_some_and(|v| v <= self.t.off_confirm_mv);
        let by_voltage = median.is_some_and(|m| m <= self.t.floor_mv);
        if by_capacity || by_voltage {
            self.off_votes += 1;
        } else {
            self.off_votes = 0;
        }
        if self.level == Level::Shutdown {
            return ev;
        }
        // two readings in a row (~10 s apart): one bad sample never powers off
        if self.off_votes >= 2 {
            self.level = Level::Shutdown;
            ev.push(Event::PowerOff);
            return ev;
        }

        let c = r.capacity;
        self.level = match c {
            Some(c) if c <= self.t.critical => Level::Critical,
            Some(c) if c <= self.t.low => Level::Low,
            _ if by_voltage || median.is_some_and(|m| m <= self.t.floor_mv + 100) => Level::Critical,
            _ => Level::Ok,
        };
        match (self.level, c) {
            (Level::Critical, _) if !self.warned_critical => {
                self.warned_critical = true;
                self.warned_low = true;
                ev.push(Event::Critical(c.unwrap_or(0)));
            }
            (Level::Low, Some(c)) if !self.warned_low => {
                self.warned_low = true;
                ev.push(Event::Low(c));
            }
            _ => {}
        }
        ev
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(c: u8, mv: u32) -> Reading {
        Reading { capacity: Some(c), voltage_mv: Some(mv), on_battery: true }
    }

    #[test]
    fn usb_charger_gets_adapter_current_after_delay() {
        let mut p = UsbPolicy::default();
        assert_eq!(p.update(false, false, 0), UsbLimit::Host);
        assert_eq!(p.update(true, false, 1000), UsbLimit::Host);
        assert_eq!(p.update(true, false, 5_999), UsbLimit::Host);
        assert_eq!(p.update(true, false, 6_000), UsbLimit::Adapter);
        // unplug resets
        assert_eq!(p.update(false, false, 7_000), UsbLimit::Host);
        assert_eq!(p.update(true, false, 8_000), UsbLimit::Host);
    }

    #[test]
    fn usb_host_keeps_pc_limit_until_unplugged() {
        let mut p = UsbPolicy::default();
        p.update(false, false, 0);
        p.update(true, false, 0);
        assert_eq!(p.update(true, true, 1000), UsbLimit::Host);
        // a configured host that suspends the bus stays a host
        assert_eq!(p.update(true, false, 60_000), UsbLimit::Host);
    }

    #[test]
    fn usb_plugged_at_boot_waits_longer() {
        let mut p = UsbPolicy::default();
        assert_eq!(p.update(true, false, 0), UsbLimit::Host);
        assert_eq!(p.update(true, false, 19_000), UsbLimit::Host);
        assert_eq!(p.update(true, false, 20_000), UsbLimit::Adapter);
    }

    #[test]
    fn warns_once_at_each_level() {
        let mut b = Battery::new(Thresholds::default());
        assert!(b.update(on(40, 3800)).is_empty());
        assert_eq!(b.update(on(15, 3700)), vec![Event::Low(15)]);
        assert!(b.update(on(14, 3690)).is_empty());
        assert!(b.update(on(16, 3700)).is_empty());
        assert_eq!(b.update(on(5, 3600)), vec![Event::Critical(5)]);
        assert!(b.update(on(4, 3600)).is_empty());
        assert_eq!(b.level, Level::Critical);
    }

    #[test]
    fn powers_off_after_two_confirming_readings() {
        let mut b = Battery::new(Thresholds::default());
        b.update(on(4, 3600));
        assert!(!b.update(on(3, 3580)).contains(&Event::PowerOff));
        assert_eq!(b.update(on(3, 3570)), vec![Event::PowerOff]);
        assert_eq!(b.level, Level::Shutdown);
        // no repeats
        assert!(b.update(on(2, 3560)).is_empty());
    }

    #[test]
    fn confused_gauge_at_healthy_voltage_does_not_power_off() {
        let mut b = Battery::new(Thresholds::default());
        for _ in 0..5 {
            assert!(!b.update(on(0, 3850)).contains(&Event::PowerOff));
        }
    }

    #[test]
    fn voltage_floor_powers_off_without_gauge() {
        let mut b = Battery::new(Thresholds::default());
        let r = |mv| Reading { capacity: None, voltage_mv: Some(mv), on_battery: true };
        b.update(r(3700));
        b.update(r(3290));
        // one sagging sample is not enough: the median still reads high
        assert!(!b.update(r(3700)).contains(&Event::PowerOff));
        b.update(r(3280));
        let mut off = false;
        for _ in 0..3 {
            off |= b.update(r(3270)).contains(&Event::PowerOff);
        }
        assert!(off);
    }

    #[test]
    fn plugging_in_cancels_and_rearms() {
        let mut b = Battery::new(Thresholds::default());
        b.update(on(3, 3500));
        assert_eq!(b.update(on(3, 3500)), vec![Event::PowerOff]);
        let plugged = Reading { on_battery: false, ..on(3, 3900) };
        assert_eq!(b.update(plugged), vec![Event::Cancelled]);
        assert_eq!(b.level, Level::Ok);
        assert_eq!(b.update(on(14, 3700)), vec![Event::Low(14)]);
        assert_eq!(b.update(plugged), vec![Event::Plugged]);
    }

    #[test]
    fn starting_low_warns_immediately() {
        let mut b = Battery::new(Thresholds::default());
        assert_eq!(b.update(on(9, 3650)), vec![Event::Low(9)]);
    }
}
