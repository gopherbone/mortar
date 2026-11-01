// SPDX-License-Identifier: GPL-2.0-or-later
//! The MENU's cross page background: a quiet animated pattern per
//! category in a strip at the right edge, made of the home screen's flat
//! shapes (flat fills mixed toward the ground, thin ink edges, no
//! gradients), like little screensavers:
//!
//! - Apps: zig-zag chevron stripes drifting slowly on the diagonal.
//! - Games: Conway's Game of Life on 24 px cells, stepped 4 times a
//!   second, each cell growing in and shrinking out; reseeded with a
//!   glider when it settles.
//! - Music: orbits: small circles going round two centres on ellipses at
//!   their own speeds, faint rings.
//! - Settings: a sliding checkerboard whose squares flip as a wave passes.
//!
//! ▲▼ nudges it (`nudge`): a scroll step, one Life generation, a burst of
//! orbit speed. Nothing runs while the cross is not on screen; Reduce
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Zigzag,
    Life,
    Orbits,
    Checker,
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
    /// Orbit time: runs faster for a moment after a nudge.
    orbit_t: f32,
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
        Patterns { t: 7.0, orbit_t: 7.0, boost: 0.0, nudge, life: Life::new(), life_acc: 0.0 }
    }

    /// Advance by `dt` seconds (only while the cross shows).
    pub fn step(&mut self, dt: f32) {
        let dt = dt.clamp(0.0, 0.05);
        self.t += dt;
        self.orbit_t += dt * (1.0 + 2.5 * self.boost);
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
            Kind::Orbits => self.boost = 1.0,
            _ => self.nudge.set(self.nudge.t + 28.0 * dir),
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
            Kind::Orbits => self.orbits(b, soft(pal.orange, pal.ground), soft(pal.lavender, pal.ground), pal.ink, edge),
            Kind::Checker => self.checker(b, soft(pal.pink, pal.ground), edge),
        }
        b.pop();
        b.clip(None);
    }

    fn zigzag(&self, b: &mut B, fill: Color, edge: Color) {
        let (period, amp, thick, space) = (64.0, 12.0, 18.0, 58.0);
        let ox = (self.t * 14.0 + self.nudge.x).rem_euclid(period);
        let oy = (self.t * 9.0 + self.nudge.x * 0.6).rem_euclid(space);
        let n = (STRIP_W / (period / 2.0)) as i32 + 4;
        let mut k = -1.0;
        while k * space - oy < STRIP_H + space {
            let base = k * space - oy;
            let pts: Vec<P> = (0..n).map(|i| (i as f32 * period / 2.0 - ox - period / 2.0, base + if i % 2 == 0 { -amp } else { amp })).collect();
            for w in pts.windows(2) {
                let (a, c) = (w[0], w[1]);
                b.poly(&[a, c, (c.0, c.1 + thick), (a.0, a.1 + thick)], fill);
            }
            b.stroke(&pts, false, 1.5, edge);
            let low: Vec<P> = pts.iter().map(|p| (p.0, p.1 + thick)).collect();
            b.stroke(&low, false, 1.5, edge);
            k += 1.0;
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

    fn orbits(&self, b: &mut B, a: Color, c: Color, ink: Color, edge: Color) {
        let ring = with_a(ink, 0.16);
        let t = self.orbit_t;
        // (centre, rings: (rx, bodies: (radius, speed rad/s, phase, colour)))
        let systems: [(P, f32); 2] = [((150.0, 250.0), 0.0), ((130.0, 560.0), 1.3)];
        for (si, ((cx, cy), ph)) in systems.iter().enumerate() {
            b.ellipse(*cx, *cy, 9.0, 9.0, a);
            b.stroke(&crate::scene::ellipse_pts(*cx, *cy, 9.0, 9.0, 24), true, 1.5, edge);
            for (ri, rx) in [52.0f32, 88.0, 124.0].iter().enumerate() {
                let ry = rx * 0.52;
                b.stroke(&crate::scene::ellipse_pts(*cx, *cy, *rx, ry, 56), true, 1.0, ring);
                let bodies = if ri == 1 { 2 } else { 1 };
                for k in 0..bodies {
                    let speed = (0.55 - ri as f32 * 0.14) * if si == 1 { -1.0 } else { 1.0 };
                    let ang = ph + t * speed + k as f32 * std::f32::consts::PI + ri as f32 * 2.1;
                    let (x, y) = (cx + rx * ang.cos(), cy + ry * ang.sin());
                    // a touch smaller behind the centre
                    let r = (14.0 - ri as f32 * 2.5) * (0.85 + 0.15 * ang.sin());
                    let col = if (ri + k + si) % 2 == 0 { a } else { c };
                    b.ellipse(x, y, r, r, col);
                    b.stroke(&crate::scene::ellipse_pts(x, y, r, r, 24), true, 1.5, edge);
                }
            }
        }
    }

    fn checker(&self, b: &mut B, fill: Color, edge: Color) {
        let cell = 44.0;
        let ox = (self.t * 7.0 + self.nudge.x).rem_euclid(cell * 2.0);
        let oy = (self.t * 4.5).rem_euclid(cell * 2.0);
        // a flip wave sweeps down the diagonal; each pass swaps the board
        let span = STRIP_W + STRIP_H + 200.0;
        let wave = self.t * 60.0;
        let pass = (wave / span).floor() as i32;
        let front = wave.rem_euclid(span) - 100.0;
        let cols = (STRIP_W / cell) as i32 + 3;
        let rows = (STRIP_H / cell) as i32 + 3;
        for j in -1..rows {
            for i in -1..cols {
                let (x, y) = (i as f32 * cell - ox + cell, j as f32 * cell - oy + cell);
                let d = x + y;
                let p = ((front - d) / 90.0).clamp(0.0, 1.0);
                let flipped = p >= 0.5;
                let base = (i + j).rem_euclid(2) == pass.rem_euclid(2);
                let on = base != flipped;
                // the card turns: thin at the middle of its flip
                let sx = (p * std::f32::consts::PI).cos().abs();
                if !on || sx < 0.06 {
                    continue;
                }
                let w = (cell - 8.0) * sx;
                let (l, t) = (x + (cell - w) / 2.0, y + 4.0);
                b.rect(l, t, w, cell - 8.0, fill);
                b.frame(l, t, w, cell - 8.0, 1.5, edge);
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
    fn soft_mixes_toward_the_ground() {
        let c = soft(Color(255, 0, 0, 255), Color(0, 0, 255, 255));
        assert_eq!((c.0, c.2), (166, 89));
    }
}
