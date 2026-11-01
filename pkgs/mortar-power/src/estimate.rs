// SPDX-License-Identifier: GPL-2.0-or-later
//! Capacity when the fuel gauge has none: a voltage estimate.
//!
//! The AXP2202 gauge (programmed with TrimUI's battery model by our kernel
//! patch) is the primary source. Its capacity is unavailable only while the
//! model is not loaded (e.g. the kernel could not program it). Then we map
//! the battery voltage onto an open-circuit-voltage curve.
//!
//! TrimUI's device tree also carries `pmu_bat_para1..32`, an AXP803-style
//! OCV percentage table, but its 4.9 AXP2202 driver never reads it (the
//! AXP2202 gauge uses the binary battery model instead) and the voltage
//! points it is defined against are fixed in AXP803 silicon, not in the
//! table. So the curve below is a generic 4.2 V Li-ion one at low rate.
//! Under load the terminal voltage sags below OCV, so it is corrected by an
//! assumed typical current through the stock battery resistance
//! (pmu_battery_rdc, 147 mOhm), then smoothed and kept monotonic.

/// (mV, %) open-circuit voltage curve, 4.2 V Li-ion cell.
const OCV: &[(u32, u8)] = &[
    (3300, 0),
    (3450, 3),
    (3550, 6),
    (3630, 10),
    (3680, 15),
    (3720, 20),
    (3750, 25),
    (3770, 30),
    (3790, 35),
    (3810, 40),
    (3830, 45),
    (3850, 50),
    (3880, 55),
    (3910, 60),
    (3940, 65),
    (3980, 70),
    (4020, 75),
    (4060, 80),
    (4100, 85),
    (4140, 90),
    (4170, 95),
    (4195, 100),
];

/// pmu_battery_rdc in TrimUI's device tree.
pub const RDC_MOHM: u32 = 147;
/// Typical running draw from the battery (screen on, CPU mostly idle).
const LOAD_MA: u32 = 350;
/// While charging in constant current (960 mA runtime limit) the terminal
/// voltage sits above OCV by roughly I*R.
const CHARGE_MA: u32 = 900;

pub fn ocv_percent(mv: u32) -> f32 {
    let (first, last) = (OCV[0], OCV[OCV.len() - 1]);
    if mv <= first.0 {
        return first.1 as f32;
    }
    if mv >= last.0 {
        return last.1 as f32;
    }
    for w in OCV.windows(2) {
        let ((v0, p0), (v1, p1)) = (w[0], w[1]);
        if mv <= v1 {
            let t = (mv - v0) as f32 / (v1 - v0) as f32;
            return p0 as f32 + t * (p1 as f32 - p0 as f32);
        }
    }
    last.1 as f32
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Discharging,
    Charging,
    Idle,
}

/// Exponentially smoothed, direction-monotonic voltage estimate.
#[derive(Debug, Default)]
pub struct Estimator {
    ema: Option<f32>,
    shown: Option<f32>,
    flow: Option<Flow>,
}

impl Estimator {
    /// One sample (every ~10 s). Returns the percentage to show.
    pub fn update(&mut self, mv: u32, flow: Flow) -> u8 {
        let ocv = match flow {
            Flow::Discharging => mv + LOAD_MA * RDC_MOHM / 1000,
            Flow::Charging => mv.saturating_sub(CHARGE_MA * RDC_MOHM / 1000),
            Flow::Idle => mv,
        };
        let raw = ocv_percent(ocv);
        let ema = match self.ema {
            Some(e) if self.flow == Some(flow) => e + 0.2 * (raw - e),
            // a new direction: the correction changed, start from this sample
            _ => raw,
        };
        let shown = match (self.shown, flow) {
            (Some(s), Flow::Discharging) if self.flow == Some(flow) => s.min(ema),
            (Some(s), Flow::Charging) if self.flow == Some(flow) => s.max(ema),
            _ => ema,
        };
        self.ema = Some(ema);
        self.shown = Some(shown);
        self.flow = Some(flow);
        shown.round().clamp(0.0, 100.0) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_is_monotonic_and_bounded() {
        let mut prev = -1.0;
        for mv in (3000..4400).step_by(5) {
            let p = ocv_percent(mv);
            assert!((0.0..=100.0).contains(&p));
            assert!(p >= prev, "{mv}");
            prev = p;
        }
        assert_eq!(ocv_percent(3850), 50.0);
        assert_eq!(ocv_percent(2000), 0.0);
        assert_eq!(ocv_percent(4300), 100.0);
    }

    #[test]
    fn discharging_never_rises() {
        let mut e = Estimator::default();
        let first = e.update(3800, Flow::Discharging);
        // a load drop lets the voltage recover; the estimate must not jump up
        let after = e.update(3900, Flow::Discharging);
        assert!(after <= first);
        let lower = e.update(3700, Flow::Discharging);
        assert!(lower <= after);
    }

    #[test]
    fn charging_never_falls_and_is_compensated() {
        let mut e = Estimator::default();
        let a = e.update(4050, Flow::Charging);
        // 4.05 V while charging is well below 80 % at rest
        assert!(a < 75, "{a}");
        let b = e.update(4000, Flow::Charging);
        assert!(b >= a);
    }

    #[test]
    fn load_compensation_reads_higher_than_raw() {
        let mut e = Estimator::default();
        assert!(e.update(3700, Flow::Discharging) as f32 > ocv_percent(3700));
    }
}
