// SPDX-License-Identifier: GPL-2.0-or-later
//! Motion: damped springs, as in the approved prototype (plan/mortar-motion):
//! semi-implicit Euler, stepped in slices of at most 1/240 s, with the
//! stiffness k and damping ratio zeta of the selected preset.
//!
//!   Bouncy  k=320 zeta=0.62   (default: overshoots and settles)
//!   Calm    k=260 zeta=1.0    (critically damped, no bounce)
//!   Snappy  k=620 zeta=0.78
//!
//! Reduced motion (a setting) uses k=420 zeta=1.0 whatever the preset, and the
//! UI also drops the ambient drift.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Preset {
    #[default]
    Bouncy,
    Calm,
    Snappy,
}

impl Preset {
    pub const ALL: [Preset; 3] = [Preset::Bouncy, Preset::Calm, Preset::Snappy];
    pub fn name(self) -> &'static str {
        match self {
            Preset::Bouncy => "Bouncy",
            Preset::Calm => "Calm",
            Preset::Snappy => "Snappy",
        }
    }
    pub fn params(self) -> Params {
        match self {
            Preset::Bouncy => Params { k: 320.0, z: 0.62 },
            Preset::Calm => Params { k: 260.0, z: 1.0 },
            Preset::Snappy => Params { k: 620.0, z: 0.78 },
        }
    }
    pub fn parse(s: &str) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| p.name().eq_ignore_ascii_case(s))
    }
}

/// Spring constants: stiffness and damping ratio (1 = no overshoot).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub k: f32,
    pub z: f32,
}

impl Params {
    pub fn new(preset: Preset, reduced: bool) -> Params {
        if reduced { Params { k: 420.0, z: 1.0 } } else { preset.params() }
    }
}

/// One animated value chasing its target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring {
    pub x: f32,
    pub t: f32,
    pub v: f32,
    /// Stiffness multiplier (some springs are faster than the preset).
    pub k_mul: f32,
}

impl Spring {
    pub const fn new(v: f32) -> Spring {
        Spring { x: v, t: v, v: 0.0, k_mul: 1.0 }
    }
    pub fn set(&mut self, t: f32) {
        self.t = t;
    }
    pub fn snap(&mut self, t: f32) {
        self.x = t;
        self.t = t;
        self.v = 0.0;
    }
    /// One integration step of `dt` seconds (keep dt <= 1/240).
    pub fn step(&mut self, dt: f32, p: Params) {
        let k = p.k * self.k_mul;
        let c = 2.0 * p.z * k.sqrt();
        let a = -k * (self.x - self.t) - c * self.v;
        self.v += a * dt;
        self.x += self.v * dt;
    }
    /// Close enough to the target to stop animating (prototype thresholds;
    /// for targets of hundreds of pixels the velocity bound grows with the
    /// target, or f32 rounding keeps the spring jittering by an ulp).
    pub fn rest(&self) -> bool {
        (self.x - self.t).abs() < 0.0005 && self.v.abs() < 0.002f32.max(self.t.abs() * 2e-5)
    }
    /// Settle exactly once at rest, so the next frame is identical.
    pub fn settle(&mut self) {
        if self.rest() {
            self.x = self.t;
            self.v = 0.0;
        }
    }
}

/// Advance a set of springs by `dt` seconds in 1/240 s slices.
pub fn step_all(springs: &mut [&mut Spring], dt: f32, p: Params) {
    let steps = ((dt / (1.0 / 240.0)).ceil() as usize).max(1);
    let h = dt / steps as f32;
    for _ in 0..steps {
        for s in springs.iter_mut() {
            s.step(h, p);
        }
    }
    for s in springs.iter_mut() {
        s.settle();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(p: Params) -> (f32, f32) {
        let mut s = Spring::new(0.0);
        s.set(1.0);
        let mut peak: f32 = 0.0;
        let mut t = 0.0;
        while t < 3.0 {
            step_all(&mut [&mut s], 1.0 / 60.0, p);
            peak = peak.max(s.x);
            t += 1.0 / 60.0;
            if s.rest() {
                break;
            }
        }
        (peak, t)
    }

    #[test]
    fn presets_behave() {
        let (bouncy_peak, bouncy_t) = run(Preset::Bouncy.params());
        assert!(bouncy_peak > 1.05, "bouncy overshoots: {bouncy_peak}");
        assert!(bouncy_t < 1.5, "settles: {bouncy_t}");
        let (calm_peak, _) = run(Preset::Calm.params());
        assert!(calm_peak <= 1.0005, "calm does not overshoot: {calm_peak}");
        let (snappy_peak, snappy_t) = run(Preset::Snappy.params());
        assert!(snappy_peak > 1.0 && snappy_t < bouncy_t);
        let (reduced_peak, _) = run(Params::new(Preset::Bouncy, true));
        assert!(reduced_peak <= 1.0005);
    }

    #[test]
    fn parse() {
        assert_eq!(Preset::parse("snappy"), Some(Preset::Snappy));
        assert_eq!(serde_json::from_str::<Preset>("\"calm\"").unwrap(), Preset::Calm);
    }
}
