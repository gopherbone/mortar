// SPDX-License-Identifier: GPL-2.0-or-later
//! The MENU's cross page background: a quiet animated pattern per
//! category in a strip at the right edge, made of the home screen's flat
//! shapes (flat fills mixed toward the ground, thin ink edges, no
//! gradients), like little screensavers:
//!
//! - Apps: zig-zag lines of marching ants drifting slowly on the
//!   diagonal: ink dashes running along a soft yellow line, neighbouring
//!   lines marching opposite ways, each line drawing itself in from the
//!   left and then erasing, out of step with its neighbours.
//! - Games: Conway's Game of Life on 24 px cells, stepped 4 times a
//!   second, each cell growing in and shrinking out; reseeded with a
//!   glider when it settles.
//! - Music: an oscilloscope: a Lissajous trace with a fading tail, its
//!   frequency ratio drifting slowly (generative: the shell has no cheap
//!   way to read the audio's level).
//! - Settings: Truchet tiles: quarter-circle arcs on a grid, the tiles
//!   turning 90 degrees as a wave travels down the diagonal, so the curves
//!   keep re-routing.
//!
//! ▲▼ nudges it (`nudge`): a burst of marching speed, one Life generation,
//! the scope's next ratio, the Truchet wave pushed along. Nothing runs while the cross is not on screen; Reduce
//! motion keeps it still. Everything is drawn with the UI's ordinary
//! polygons and strokes, so it costs a few hundred small fills a frame.

use crate::art::Pal;
use crate::motion::Spring;
use crate::scene::{Xf, B, P};
use mortar_ui::gfx::Color;

/// The strip: x from STRIP_X to the right edge, the full height (the
/// selected row's band ends left of it).
pub const STRIP_X: f32 = 744.0;
pub const STRIP_W: f32 = 1024.0 - STRIP_X;
const STRIP_H: f32 = 768.0;

/// Marching ants: dash and gap (px), speed (px/s), draw-and-erase cycle (s).
const ANT_ON: f32 = 9.0;
const ANT_OFF: f32 = 7.0;
const ANT_SPEED: f32 = 22.0;
const ANT_CYCLE: f32 = 7.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Zigzag,
    Life,
    Scope,
    Truchet,
}

/// A pattern colour: the accent mixed 35% toward the ground, so the menu's
/// text stays dominant.
pub fn soft(c: Color, ground: Color) -> Color {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * 0.35).round() as u8;
    Color(m(c.0, ground.0), m(c.1, ground.1), m(c.2, ground.2), 255)
}

fn with_a(c: Color, a: f32) -> Color {
    Color(c.0, c.1, c.2, (a.clamp(0.0, 1.0) * c.3 as f32).round() as u8)
}

// ------------------------------------------------------------------ life ----

const LW: usize = 12;
const LH: usize = 32;
const CELL: f32 = 24.0;

/// Conway's Game of Life on a small torus, with a shown size per cell that
/// eases towards alive or dead.
#[derive(Clone, Debug)]
pub struct Life {
    cells: Vec<bool>,
    shown: Vec<f32>,
    hist: Vec<u64>,
    rng: u32,
    pub generation: u32,
}

impl Life {
    pub fn new() -> Life {
        let mut l = Life { cells: vec![false; LW * LH], shown: vec![0.0; LW * LH], hist: Vec::new(), rng: 0x6d72_7461, generation: 0 };
        // an R-pentomino (long-lived) and two gliders
        for (x, y) in [(5, 6), (6, 6), (4, 7), (5, 7), (5, 8)] {
            l.set(x, y, true);
        }
        l.glider(1, 16);
        l.glider(7, 24);
        // start on an interesting frame (also what Reduce motion shows)
        for _ in 0..12 {
            l.step();
        }
        for i in 0..l.cells.len() {
            l.shown[i] = if l.cells[i] { 1.0 } else { 0.0 };
        }
        l
    }

    fn set(&mut self, x: usize, y: usize, on: bool) {
        self.cells[(y % LH) * LW + x % LW] = on;
    }

    pub fn alive(&self, x: usize, y: usize) -> bool {
        self.cells[(y % LH) * LW + x % LW]
    }

    pub fn population(&self) -> usize {
        self.cells.iter().filter(|c| **c).count()
    }

    fn glider(&mut self, x: usize, y: usize) {
        for (dx, dy) in [(1, 0), (2, 1), (0, 2), (1, 2), (2, 2)] {
            self.set(x + dx, y + dy, true);
        }
    }

    fn rand(&mut self) -> u32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        self.rng
    }

    /// One generation; a gentle reseed (a glider somewhere) when it died
    /// out or repeats itself.
    pub fn step(&mut self) {
        let mut next = vec![false; LW * LH];
        for y in 0..LH {
            for x in 0..LW {
                let mut n = 0;
                for dy in [LH - 1, 0, 1] {
                    for dx in [LW - 1, 0, 1] {
                        if (dx, dy) != (0, 0) && self.alive(x + dx, y + dy) {
                            n += 1;
                        }
                    }
                }
                let on = self.alive(x, y);
                next[y * LW + x] = matches!((on, n), (true, 2) | (_, 3));
            }
        }
        self.cells = next;
        self.generation += 1;
        let h = self.cells.iter().enumerate().fold(0xcbf2_9ce4_8422_2325u64, |h, (i, c)| if *c { (h ^ i as u64).wrapping_mul(0x100_0000_01b3) } else { h });
        if self.population() == 0 || self.hist.contains(&h) {
            let (x, y) = ((self.rand() as usize) % LW, (self.rand() as usize) % LH);
            self.glider(x, y);
            self.hist.clear();
        } else {
            self.hist.push(h);
            if self.hist.len() > 12 {
                self.hist.remove(0);
            }
        }
    }

    /// Ease the shown sizes towards the cells (about 120 ms).
    fn ease(&mut self, dt: f32) {
        let k = (dt * 9.0).min(1.0);
        for i in 0..self.cells.len() {
            let t = if self.cells[i] { 1.0 } else { 0.0 };
            self.shown[i] += (t - self.shown[i]) * k;
        }
    }
}

impl Default for Life {
    fn default() -> Self {
        Life::new()
    }
}

// ------------------------------------------------------------- patterns ----

/// The patterns' shared clock and state.
#[derive(Clone, Debug)]
pub struct Patterns {
    /// Pattern time (seconds; still with Reduce motion).
    pub t: f32,
    /// The scope's frequency ratio (shown, easing to `ratio_to`).
    ratio: f32,
    ratio_to: f32,
    /// How far the zig-zags' ants have marched (px); faster after a nudge.
    pub march: f32,
    boost: f32,
    /// A scroll offset (px) that ▲▼ push, eased.
    pub nudge: Spring,
    pub life: Life,
    life_acc: f32,
}

impl Default for Patterns {
    fn default() -> Self {
        Patterns::new()
    }
}

impl Patterns {
    pub fn new() -> Patterns {
        let mut nudge = Spring::new(0.0);
        nudge.k_mul = 0.4;
        // a nice still frame for Reduce motion: some time in
        Patterns { t: 7.0, ratio: 2.0, ratio_to: 2.0, march: 0.0, boost: 0.0, nudge, life: Life::new(), life_acc: 0.0 }
    }

    /// Advance by `dt` seconds (only while the cross shows).
    pub fn step(&mut self, dt: f32) {
        let dt = dt.clamp(0.0, 0.05);
        self.t += dt;
        self.ratio += (self.ratio_to - self.ratio) * (dt * 2.0).min(1.0);
        self.march += dt * ANT_SPEED * (1.0 + 3.0 * self.boost);
        self.boost = (self.boost - dt * 1.2).max(0.0);
        self.life_acc += dt;
        while self.life_acc >= 0.25 {
            self.life_acc -= 0.25;
            self.life.step();
        }
        self.life.ease(dt);
    }

    /// ▲▼: a little push (`dir` +1 down, -1 up).
    pub fn nudge(&mut self, kind: Kind, dir: f32) {
        match kind {
            Kind::Life => {
                self.life.step();
                self.life_acc = 0.0;
            }
            Kind::Zigzag => self.boost = 1.0,
            Kind::Scope => self.ratio_to = (self.ratio_to.round() + dir.signum()).clamp(1.0, 6.0),
            Kind::Truchet => self.nudge.set(self.nudge.t + 120.0 * dir.signum()),
        }
    }

    /// Draw `kind` in the strip, moved `dx` sideways (transitions), clipped
    /// to the strip, in the current transform (the cross page).
    /// (`dx` moves the pattern; only the part of the strip it still or
    /// already covers shows, so two never overlap mid-slide.)
    pub fn draw(&self, b: &mut B, pal: &Pal, kind: Kind, dx: f32) {
        let (x0, x1) = ((STRIP_X + dx).max(STRIP_X), (STRIP_X + dx + STRIP_W).min(STRIP_X + STRIP_W));
        if x1 - x0 < 0.5 {
            return;
        }
        b.clip(Some([x0, 0.0, x1 - x0, STRIP_H]));
        b.push(Xf::translate(STRIP_X + dx, 0.0));
        let edge = with_a(pal.ink, 0.7);
        match kind {
            Kind::Zigzag => self.zigzag(b, soft(pal.yellow, pal.ground), edge),
            Kind::Life => self.life_cells(b, soft(pal.lime, pal.ground), edge),
            Kind::Scope => self.scope(b, soft(pal.orange, pal.ground), pal.ink),
            Kind::Truchet => self.truchet(b, soft(pal.pink, pal.ground), edge),
        }
        b.pop();
        b.clip(None);
    }

    /// Marching ants on zig-zag lines.
    fn zigzag(&self, b: &mut B, fill: Color, edge: Color) {
        use crate::scene::{polyline_len, sub_polyline};
        let (period, amp, space) = (64.0, 13.0, 46.0);
        let drift = self.t * 9.0;
        let ox = (self.t * 14.0).rem_euclid(period);
        let oy = drift.rem_euclid(space);
        // lines keep their parity as they drift (so their direction too)
        let shift = (drift / space).floor() as i32;
        let n = (STRIP_W / (period / 2.0)) as i32 + 4;
        let mut k = -1;
        while k as f32 * space - oy < STRIP_H + space {
            let base = k as f32 * space - oy;
            let odd = (k + shift).rem_euclid(2) == 1;
            let pts: Vec<P> = (0..n).map(|i| (i as f32 * period / 2.0 - ox - period / 2.0, base + if i % 2 == 0 { -amp } else { amp })).collect();
            let len = polyline_len(&pts);
            // draw in from the left, then erase from the left; odd lines
            // half a cycle behind (one draws while its neighbour erases)
            let c = (self.t / ANT_CYCLE + if odd { 0.5 } else { 0.0 }).fract();
            let ease = |x: f32| x * x * (3.0 - 2.0 * x);
            let (w0, w1) = if c < 0.5 { (0.0, ease(c * 2.0) * len) } else { (ease(c * 2.0 - 1.0) * len, len) };
            let seg = sub_polyline(&pts, w0, w1);
            if seg.len() >= 2 {
                b.stroke(&seg, false, 7.0, fill);
                // the ants: dashes anchored to the line, marching one way
                // on even lines and the other on odd ones
                let phase = if odd { -self.march } else { self.march } + w0;
                b.dashed(&seg, 2.5, edge, ANT_ON, ANT_OFF, phase);
            }
            k += 1;
        }
    }

    fn life_cells(&self, b: &mut B, fill: Color, edge: Color) {
        let x0 = (STRIP_W - LW as f32 * CELL) / 2.0;
        for y in 0..LH {
            for x in 0..LW {
                let s = self.life.shown[y * LW + x];
                if s < 0.04 {
                    continue;
                }
                let size = (CELL - 4.0) * s;
                let (cx, cy) = (x0 + x as f32 * CELL + CELL / 2.0, y as f32 * CELL + CELL / 2.0);
                let (l, t) = (cx - size / 2.0, cy - size / 2.0);
                b.rect(l, t, size, size, fill);
                if size > 4.0 {
                    b.frame(l, t, size, size, 1.5, edge);
                }
            }
        }
    }

    /// An oscilloscope: a Lissajous figure (x at 3x, y at the drifting
    /// ratio) traced with a tail that thins and fades.
    fn scope(&self, b: &mut B, col: Color, ink: Color) {
        let r = self.ratio + 0.5 * (self.t * 0.06).sin();
        let (cx, cy, ax, ay) = (STRIP_W / 2.0, STRIP_H / 2.0, 110.0, 300.0);
        // a slow breath in the amplitude
        let breathe = 0.92 + 0.08 * (self.t * 0.7).sin();
        let pts: Vec<P> = (0..360)
            .map(|k| {
                let s = self.t * 0.9 - k as f32 * 0.016;
                (cx + ax * breathe * (3.0 * s + 0.6).sin(), cy + ay * breathe * (r * s).sin())
            })
            .collect();
        for (n, ch) in pts.chunks(12).enumerate() {
            let a = 1.0 - n as f32 / 30.0;
            let mut seg = ch.to_vec();
            if let Some(next) = pts.get((n + 1) * 12) {
                seg.push(*next);
            }
            b.stroke(&seg, false, 10.0 * a.max(0.3), with_a(col, a));
            b.stroke(&seg, false, 2.5, with_a(ink, a * 0.8));
        }
        let (hx, hy) = pts[0];
        b.ellipse(hx, hy, 6.0, 6.0, ink);
    }

    /// Truchet tiles: two quarter-circle arcs per 40 px tile; a wave runs
    /// down the diagonal and turns each tile 90 degrees as it passes.
    fn truchet(&self, b: &mut B, col: Color, edge: Color) {
        let cell = 40.0;
        let span = STRIP_W + STRIP_H + 160.0;
        let wave = self.t * 70.0 + self.nudge.x;
        let pass = (wave / span).floor() as i32;
        let front = wave.rem_euclid(span) - 80.0;
        let hash = |i: i32, j: i32| {
            let mut h = (i as u32).wrapping_mul(0x9e37_79b1) ^ (j as u32).wrapping_mul(0x85eb_ca6b);
            h ^= h >> 15;
            h = h.wrapping_mul(0x2c1b_3c6d);
            h ^ (h >> 12)
        };
        for j in 0..(STRIP_H / cell) as i32 + 1 {
            for i in 0..(STRIP_W / cell) as i32 + 1 {
                let (x, y) = (i as f32 * cell, j as f32 * cell);
                let r = ((front - (x + y)) / 50.0).clamp(0.0, 1.0);
                let base = (hash(i, j) & 1) as i32 + pass;
                let ang = (base as f32 + r) * std::f32::consts::FRAC_PI_2;
                let (cx, cy) = (x + cell / 2.0, y + cell / 2.0);
                let (sa, ca) = ang.sin_cos();
                let rot = |p: P| {
                    let (dx, dy) = (p.0 - cx, p.1 - cy);
                    (cx + dx * ca - dy * sa, cy + dx * sa + dy * ca)
                };
                for (ox, oy, a0) in [(x, y, 0.0f32), (x + cell, y + cell, 180.0)] {
                    let pts: Vec<P> = (0..=8)
                        .map(|k| {
                            let a = (a0 + k as f32 * 90.0 / 8.0).to_radians();
                            rot((ox + cell / 2.0 * a.cos(), oy + cell / 2.0 * a.sin()))
                        })
                        .collect();
                    b.stroke(&pts, false, 8.0, col);
                    b.stroke(&pts, false, 1.5, edge);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn life_follows_the_rules() {
        let mut l = Life { cells: vec![false; LW * LH], shown: vec![0.0; LW * LH], hist: Vec::new(), rng: 1, generation: 0 };
        // a blinker turns over every step
        for x in 4..7 {
            l.set(x, 10, true);
        }
        l.step();
        assert!(l.alive(5, 9) && l.alive(5, 10) && l.alive(5, 11) && !l.alive(4, 10));
        l.step();
        assert!(l.alive(4, 10) && l.alive(6, 10));
    }

    #[test]
    fn life_never_dies_out() {
        let mut l = Life::new();
        for _ in 0..2000 {
            l.step();
            assert!(l.population() > 0, "generation {}", l.generation);
        }
        // a still life or a cycle gets a glider: it keeps changing
        let before = l.cells.clone();
        let mut changed = false;
        for _ in 0..40 {
            l.step();
            changed |= l.cells != before;
        }
        assert!(changed);
    }

    #[test]
    fn ants_march_and_a_nudge_hurries_them() {
        let mut p = Patterns::new();
        p.step(1.0 / 30.0);
        let m0 = p.march;
        for _ in 0..30 {
            p.step(1.0 / 30.0);
        }
        let calm = p.march - m0;
        assert!((calm - ANT_SPEED).abs() < 1.0, "{calm} px in a second");
        p.nudge(Kind::Zigzag, 1.0);
        let m1 = p.march;
        p.step(1.0 / 30.0);
        assert!(p.march - m1 > ANT_SPEED / 30.0 * 3.0, "kicked");
    }

    #[test]
    fn scope_and_truchet_take_nudges() {
        let mut p = Patterns::new();
        p.nudge(Kind::Scope, 1.0);
        assert_eq!(p.ratio_to, 3.0);
        for _ in 0..90 {
            p.step(1.0 / 30.0);
        }
        assert!((p.ratio - 3.0).abs() < 0.05, "eases to the new ratio: {}", p.ratio);
        p.nudge(Kind::Scope, -1.0);
        p.nudge(Kind::Scope, -1.0);
        p.nudge(Kind::Scope, -1.0);
        assert_eq!(p.ratio_to, 1.0, "clamped");
        p.nudge(Kind::Truchet, 1.0);
        assert_eq!(p.nudge.t, 120.0, "the wave is pushed along");
    }

    #[test]
    fn soft_mixes_toward_the_ground() {
        let c = soft(Color(255, 0, 0, 255), Color(0, 0, 255, 255));
        assert_eq!((c.0, c.2), (166, 89));
    }
}
