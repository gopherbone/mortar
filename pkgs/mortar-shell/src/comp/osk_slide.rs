// SPDX-License-Identifier: GPL-2.0-or-later
//! The on-screen keyboard slides up when it opens and down when it closes
//! (a critically damped spring, about 180 ms to settle visually; Reduce
//! motion: at once), on both the pixman and the GLES paths.
//!
//! `osk.visible` is the logical state (input, the app's size); the slide
//! is only what is drawn. The app is reconfigured at the start of either
//! slide, so its resize happens under the moving keyboard. Until it
//! commits at the new size, the old buffer stays and any strip below it is
//! filled with the app's own edge colour (backend.rs), never black.

use super::state::Shell;
use crate::motion::{step_all, Params, Spring};
use std::time::Instant;

/// No overshoot: the keyboard must not lift off the bottom edge.
const SPRING: Params = Params { k: 600.0, z: 1.0 };

pub struct OskSlide {
    spring: Spring,
    last: Instant,
}

impl Default for OskSlide {
    fn default() -> Self {
        OskSlide { spring: Spring::new(0.0), last: Instant::now() }
    }
}

impl OskSlide {
    /// Fraction of the keyboard on screen, 0..1.
    pub fn frac(&self) -> f32 {
        self.spring.x.clamp(0.0, 1.0)
    }

    /// Advance by `dt` s towards `target`; true while it moves.
    fn advance(&mut self, dt: f32, target: f32, reduced: bool, full_px: f32) -> bool {
        self.spring.set(target);
        let before = self.spring.x;
        if reduced {
            self.spring.snap(target);
        } else if dt > 0.0 {
            step_all(&mut [&mut self.spring], dt.min(1.0), SPRING);
            // settle once it is within half a pixel
            if (self.spring.x - target).abs() * full_px < 0.5 && self.spring.v.abs() * full_px < 30.0 {
                self.spring.snap(target);
            }
        }
        self.spring.x != before || self.spring.x != target
    }
}

impl Shell {
    /// The keyboard is (partly) on screen.
    pub fn osk_shown(&self) -> bool {
        self.osk.visible || self.osk_slide.frac() > 0.0
    }

    /// The keyboard is moving (frames needed).
    pub fn osk_sliding(&self) -> bool {
        let target = if self.osk.visible { 1.0 } else { 0.0 };
        self.osk_slide.frac() != target
    }

    /// How much of the keyboard is on screen, in pixels.
    pub fn osk_shown_h(&self) -> i32 {
        (self.osk_height() as f32 * self.osk_slide.frac()).round() as i32
    }

    /// Step the slide on the real clock (not while UI time is manual).
    /// True when a frame is needed.
    pub fn osk_slide_tick(&mut self) -> bool {
        let now = Instant::now();
        let dt = now.duration_since(self.osk_slide.last).as_secs_f32();
        self.osk_slide.last = now;
        self.osk_slide_step(if self.manual_time { 0.0 } else { dt })
    }

    /// Step the slide by `dt` seconds (also `ctl time +MS`).
    pub fn osk_slide_step(&mut self, dt: f32) -> bool {
        let target = if self.osk.visible { 1.0 } else { 0.0 };
        // another app in front (or the shell's screens): no slide over it
        if !self.osk.visible && !self.keyboard_app() {
            let moved = self.osk_slide.frac() > 0.0;
            self.osk_slide.spring.snap(0.0);
            return moved;
        }
        let reduced = self.sui.prefs.reduce_motion;
        let full = self.osk_height() as f32;
        self.osk_slide.advance(dt, target, reduced, full)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(s: &mut OskSlide, target: f32, ms: u32) -> Vec<f32> {
        let mut out = Vec::new();
        for _ in 0..ms / 16 {
            s.advance(0.016, target, false, 400.0);
            out.push(s.frac());
        }
        out
    }

    #[test]
    fn slides_up_and_down_without_overshoot() {
        let mut s = OskSlide::default();
        let up = run(&mut s, 1.0, 400);
        assert!(up[0] > 0.0 && up[0] < 0.3, "starts moving at once: {}", up[0]);
        assert!(up.windows(2).all(|w| w[1] >= w[0]), "monotonic");
        assert!(up[180 / 16] > 0.9, "mostly up after ~180 ms: {}", up[180 / 16]);
        assert_eq!(*up.last().unwrap(), 1.0, "settles exactly");
        let down = run(&mut s, 0.0, 400);
        assert!(down.windows(2).all(|w| w[1] <= w[0]));
        assert_eq!(*down.last().unwrap(), 0.0);
    }

    #[test]
    fn reduced_motion_is_instant() {
        let mut s = OskSlide::default();
        assert!(s.advance(0.016, 1.0, true, 400.0));
        assert_eq!(s.frac(), 1.0);
        assert!(!s.advance(0.016, 1.0, true, 400.0), "at rest: no more frames");
    }
}
