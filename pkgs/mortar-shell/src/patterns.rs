// SPDX-License-Identifier: GPL-2.0-or-later
//! The MENU's cross page background: an animated mosaic per category in a
//! strip at the right edge (clear of the list), like little screensavers.
//! Everything is solid tiles in full palette colours on a grid, with small
//! even gaps (grout) between them: no hairlines, no fades, no
//! translucency.
//!
//! - Apps: fixed, regular zig-zag paths (two tiles thick, evenly spaced)
//!   of which only a window of 12 columns shows; the window snakes along,
//!   its head tile growing in and its tail tile shrinking out a step at a
//!   time, each band out of phase, alternate bands the other way.
//! - Games: Conway's Game of Life on 28 px tiles, stepped 4 times a
//!   second, each tile growing in and shrinking out; newborn tiles yellow,
//!   older ones lime; a glider drops in when it dies out or repeats.
//! - Music: an oscilloscope as a mosaic: a Lissajous path quantised to a
//!   grid of square tiles, the head in ink, the newest stretch orange, the
//!   older stretch lavender stepping off tile by tile; the ratio drifts.
//! - Settings: Truchet tiles, the classic two-colour mosaic: each tile a
//!   pink square with lavender quarter discs at two opposite corners; a
//!   wave travels down the diagonal and turns each tile 90 degrees with an
//!   eased step, so the curves re-route.
//!
//! ▲▼ nudges it (`nudge`): the zig-zag windows hurry, one Life generation, the
//! scope's next ratio, the Truchet wave pushed along. Nothing runs while
//! the cross is not on screen; Reduce motion keeps it still. All of it is
//! a few hundred solid quads a frame.

use crate::art::Pal;
use crate::motion::Spring;
use crate::scene::{Xf, B, P};
use mortar_ui::gfx::Color;

/// The strip: x from STRIP_X to the right edge, the full height (clear of
/// the selected row's band and of every category's icon and name).
pub const STRIP_X: f32 = 772.0;
pub const STRIP_W: f32 = 1024.0 - STRIP_X;
const STRIP_H: f32 = 768.0;

/// The mosaic grid: a GRID px pitch, GROUT px between tiles.
const GRID: f32 = 28.0;
const GROUT: f32 = 4.0;
/// Grid rows start this far down (27 rows of 28 fill 756 of the 768).
const Y0: f32 = 6.0;
/// The patterns span the whole screen (the screensaver shows all of
/// them; the strip shows their right-hand end): 37 grid columns from
/// OX, so the strip's left edge falls on a column boundary.
const SCREEN_W: f32 = 1024.0;
const OX: f32 = -12.0;
const NCOL: i32 = 37;
/// The first column in the strip ((STRIP_X - OX) / GRID).
const STRIP_COL: usize = 28;

/// The zig-zags' drawing window: WIN columns of the path show; it moves
/// a column every STEP s, the new head tile growing in (and the tail tile
/// shrinking out) over the first HOP s of each step.
const WIN: i32 = 12;
const STEP: f32 = 0.2;
const HOP: f32 = 0.12;
/// The zig-zag: rows by column (period 4, amplitude 2), two tiles thick;
/// bands every BAND_GAP rows.
const ZIG: [f32; 4] = [0.0, 1.0, 2.0, 1.0];
const BAND_GAP: f32 = 5.0;
const BANDS: usize = 6;

/// Where band `band`'s window is at clock `t`: the head's index along the
/// path (0..path length) and how far its tile has grown (0..1). The path is
/// the screen's columns plus the window, so a window draws in, crosses
/// and leaves before it starts again.
fn window(t: f32, band: usize) -> (i32, f32) {
    let len = NCOL + WIN;
    // evenly spread phases (and a little uneven, so they never line up)
    let pos = t / STEP + band as f32 * (len as f32 / BANDS as f32 + 1.7);
    let k = pos.floor();
    let grow = ease((pos - k) * STEP / HOP);
    ((k as i64).rem_euclid(len as i64) as i32, grow)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Zigzag,
    Life,
    Scope,
    Truchet,
}

fn ease(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// The pattern's x range on screen (in its own units): what lies outside
/// is not drawn at all.
type Vis = (f32, f32);

/// One mosaic tile in grid cell (col, row), scaled by `s` (0..1) about its
/// centre, if it shows in `vis`.
fn tile(b: &mut B, vis: Vis, col: f32, row: f32, s: f32, c: Color) {
    let size = (GRID - GROUT) * s;
    if size < 1.0 || (col + 1.0) * GRID < vis.0 || col * GRID > vis.1 {
        return;
    }
    let (cx, cy) = (col * GRID + GRID / 2.0, Y0 + row * GRID + GRID / 2.0);
    b.rect(cx - size / 2.0, cy - size / 2.0, size, size, c);
}

// ------------------------------------------------------------------ life ----

const LW: usize = 37;
const LH: usize = 27;

/// Conway's Game of Life on a small torus, with a shown size per cell that
/// eases towards alive or dead, and each cell's age (newborns stand out).
#[derive(Clone, Debug)]
pub struct Life {
    cells: Vec<bool>,
    age: Vec<u16>,
    shown: Vec<f32>,
    hist: Vec<u64>,
    rng: u32,
    pub generation: u32,
    /// Generations the strip has been empty (it gets a glider at 12).
    quiet: u16,
}

impl Life {
    pub fn new() -> Life {
        let mut l = Life { cells: vec![false; LW * LH], age: vec![0; LW * LH], shown: vec![0.0; LW * LH], hist: Vec::new(), rng: 0x6d72_7461, generation: 0, quiet: 0 };
        // an R-pentomino (long-lived) and two gliders
        // seeded in the strip (what shows most of the time)
        let c = STRIP_COL;
        for (x, y) in [(c + 5, 6), (c + 6, 6), (c + 4, 7), (c + 5, 7), (c + 5, 8)] {
            l.set(x, y, true);
        }
        l.glider(c + 1, 15);
        l.glider(c + 5, 21);
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
        for (a, on) in self.age.iter_mut().zip(&next) {
            *a = if *on { a.saturating_add(1) } else { 0 };
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
        // the strip (usually all that shows) must not stay empty while
        // life goes on elsewhere
        let strip_alive = (0..LH).any(|y| (STRIP_COL..LW).any(|x| self.alive(x, y)));
        self.quiet = if strip_alive { 0 } else { self.quiet + 1 };
        if self.quiet >= 12 {
            let (x, y) = (STRIP_COL + (self.rand() as usize) % 6, (self.rand() as usize) % LH);
            self.glider(x, y);
            self.quiet = 0;
        }
    }

    /// Ease the shown sizes towards the cells (about 120 ms).
    fn ease(&mut self, dt: f32) {
        let k = (dt * 9.0).min(1.0);
        for i in 0..self.cells.len() {
            let t = if self.cells[i] { 1.0 } else { 0.0 };
            self.shown[i] += (t - self.shown[i]) * k;
            // within a twentieth of a pixel: there (still tiles are not
            // drawn again)
            if (t - self.shown[i]).abs() < 0.002 {
                self.shown[i] = t;
            }
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
    /// The zig-zags' clock: runs faster for a moment after a nudge.
    pub inch_t: f32,
    /// The scope's frequency ratio (shown, easing to `ratio_to`).
    ratio: f32,
    ratio_to: f32,
    /// The scope's path position, accumulated: `ratio * t` would speed up without
    /// limit as `t` grows, because any change in the ratio moves it by `t` times
    /// that change.
    scope_x: f32,
    scope_y: f32,
    boost: f32,
    /// The Truchet wave's push from ▲▼ (px along the diagonal), eased.
    pub nudge: Spring,
    pub life: Life,
    life_acc: f32,
}

impl Default for Patterns {
    fn default() -> Self {
        Patterns::new()
    }
}

/// How fast the scope's path runs, and the highest ratio ▲▼ reaches: its top speed
/// is about (SCOPE_RATIO_MAX + 0.5) * SCOPE_RATE radians a second.
const SCOPE_RATE: f32 = 0.6;
const SCOPE_RATIO_MAX: f32 = 4.0;

impl Patterns {
    pub fn new() -> Patterns {
        let mut nudge = Spring::new(0.0);
        nudge.k_mul = 0.4;
        // a nice still frame for Reduce motion: some time in
        Patterns { t: 7.0, inch_t: 7.0, ratio: 2.0, ratio_to: 2.0, scope_x: 18.9, scope_y: 12.6, boost: 0.0, nudge, life: Life::new(), life_acc: 0.0 }
    }

    /// Advance by `dt` seconds (only while the cross shows).
    pub fn step(&mut self, dt: f32) {
        let dt = dt.clamp(0.0, 0.05);
        self.t += dt;
        self.inch_t += dt * (1.0 + 2.0 * self.boost);
        self.ratio += (self.ratio_to - self.ratio) * (dt * 2.0).min(1.0);
        self.scope_x = (self.scope_x + 3.0 * SCOPE_RATE * dt) % std::f32::consts::TAU;
        self.scope_y = (self.scope_y + self.scope_ratio() * SCOPE_RATE * dt) % std::f32::consts::TAU;
        self.boost = (self.boost - dt * 1.2).max(0.0);
        self.life_acc += dt;
        while self.life_acc >= 0.25 {
            self.life_acc -= 0.25;
            self.life.step();
        }
        self.life.ease(dt);
    }

    /// The scope's ratio now: where ▲▼ left it, drifting slowly.
    fn scope_ratio(&self) -> f32 {
        self.ratio + 0.5 * (self.t * 0.06).sin()
    }

    /// ▲▼: a little push (`dir` +1 down, -1 up).
    pub fn nudge(&mut self, kind: Kind, dir: f32) {
        match kind {
            Kind::Life => {
                self.life.step();
                self.life_acc = 0.0;
            }
            Kind::Zigzag => self.boost = 1.0,
            Kind::Scope => self.ratio_to = (self.ratio_to.round() + dir.signum()).clamp(1.0, SCOPE_RATIO_MAX),
            Kind::Truchet => self.nudge.set(self.nudge.t + 120.0 * dir.signum()),
        }
    }

    /// Draw `kind` from screen x `x0` to the right edge (the strip, or more
    /// of the screen as the screensaver grows; `e` 0..1 is how far it has
    /// grown), moved `dx` sideways (transitions), in the current transform
    /// (the cross page). Only the part it still or already covers shows, so
    /// two never overlap mid-slide.
    pub fn draw(&self, b: &mut B, pal: &Pal, kind: Kind, dx: f32, x0: f32, e: f32) {
        let (lo, hi) = ((x0 + dx).max(x0), (SCREEN_W + dx).min(SCREEN_W));
        if hi - lo < 0.5 {
            return;
        }
        b.clip(Some([lo, 0.0, hi - lo, STRIP_H]));
        b.push(Xf::translate(OX + dx, 0.0));
        let vis = (lo - OX - dx, hi - OX - dx);
        match kind {
            Kind::Zigzag => self.zigzag(b, pal, vis),
            Kind::Life => self.life_tiles(b, pal, vis),
            Kind::Scope => self.scope(b, pal, e, vis),
            Kind::Truchet => self.truchet(b, pal, vis),
        }
        b.pop();
        b.clip(None);
    }

    /// Zig-zags: fixed, regular paths (the same shape for every band,
    /// evenly spaced) of which only a window of WIN columns shows; the
    /// window snakes along, drawing at its head and erasing at its tail, a
    /// tile at a time.
    fn zigzag(&self, b: &mut B, pal: &Pal, vis: Vis) {
        let cols = [pal.yellow, pal.lavender, pal.dark];
        for band in 0..BANDS {
            let base = band as f32 * BAND_GAP - 1.0;
            let col = cols[band % 3];
            let (head, grow) = window(self.inch_t, band);
            // from the tail being erased to the head being drawn
            for i in (head - WIN)..=head {
                if !(0..NCOL).contains(&i) {
                    continue;
                }
                let s = if i == head {
                    grow
                } else if i == head - WIN {
                    1.0 - grow
                } else {
                    1.0
                };
                // alternate bands run the other way along the screen
                let c = if band % 2 == 0 { i } else { NCOL - 1 - i };
                let row = base + ZIG[c.rem_euclid(4) as usize];
                tile(b, vis, c as f32, row, s, col);
                tile(b, vis, c as f32, row + 1.0, s, col);
            }
        }
    }

    fn life_tiles(&self, b: &mut B, pal: &Pal, vis: Vis) {
        for y in 0..LH {
            for x in 0..LW {
                let i = y * LW + x;
                let s = self.life.shown[i];
                if s < 0.04 {
                    continue;
                }
                let c = if self.life.cells[i] && self.life.age[i] <= 1 { pal.yellow } else { pal.lime };
                tile(b, vis, x as f32, y as f32, s, c);
            }
        }
    }

    /// The oscilloscope as a mosaic: the Lissajous path quantised to the
    /// grid; the newest stretch orange, the older lavender, the head ink.
    /// It is centred in the strip, and spreads across the screen as the
    /// screensaver grows (`e`).
    fn scope(&self, b: &mut B, pal: &Pal, e: f32, vis: Vis) {
        const CW: usize = NCOL as usize;
        const CH: usize = 27;
        let r = self.scope_ratio();
        let strip_cx = STRIP_COL as f32 * GRID + (SCREEN_W - STRIP_X) / 2.0;
        let full_cx = (SCREEN_W - OX) / 2.0;
        let (cx, cy) = (strip_cx + (full_cx - strip_cx) * e, Y0 + CH as f32 * GRID / 2.0);
        let (ax, ay) = (110.0 + 320.0 * e, 340.0);
        let breathe = 0.92 + 0.08 * (self.t * 0.7).sin();
        // the rank of the newest sample in each cell (0: none)
        let mut rank = [0u16; CW * CH];
        let n = 480;
        for k in (0..n).rev() {
            let back = k as f32 * 0.013;
            let (x, y) = (
                cx + ax * breathe * (self.scope_x - 3.0 * back + 0.6).sin(),
                cy + ay * breathe * (self.scope_y - r * back).sin(),
            );
            let (i, j) = ((x / GRID).floor(), ((y - Y0) / GRID).floor());
            if i < 0.0 || j < 0.0 || i as usize >= CW || j as usize >= CH {
                continue;
            }
            rank[j as usize * CW + i as usize] = (n - k) as u16;
        }
        let head = n as u16;
        for (idx, &rk) in rank.iter().enumerate() {
            if rk == 0 {
                continue;
            }
            let c = if rk + 3 >= head {
                pal.dark
            } else if rk as usize > n / 2 {
                pal.orange
            } else {
                pal.lavender
            };
            tile(b, vis, (idx % CW) as f32, (idx / CW) as f32, 1.0, c);
        }
    }

    /// Truchet tiles, the classic two-colour mosaic: a pink 56 px square
    /// with lavender quarter discs at two opposite corners; the wave turns
    /// each tile a quarter turn, eased, as it passes.
    fn truchet(&self, b: &mut B, pal: &Pal, vis: Vis) {
        let cell = GRID * 2.0;
        let side = cell - GROUT;
        let span = (SCREEN_W - OX) + STRIP_H + 160.0;
        let wave = self.t * 70.0 + self.nudge.x;
        let pass = (wave / span).floor() as i32;
        let front = wave.rem_euclid(span) - 80.0;
        let hash = |i: i32, j: i32| {
            let mut h = (i as u32).wrapping_mul(0x9e37_79b1) ^ (j as u32).wrapping_mul(0x85eb_ca6b);
            h ^= h >> 15;
            h = h.wrapping_mul(0x2c1b_3c6d);
            h ^ (h >> 12)
        };
        let (fill, disc) = (pal.pink, pal.lavender);
        // the quarter disc's arc, once (the bottom-right one is the same
        // turned half way round)
        let arc: [P; 9] = std::array::from_fn(|k| {
            let a = (k as f32 * 90.0 / 8.0).to_radians();
            (a.cos(), a.sin())
        });
        for j in 0..((STRIP_H - Y0) / cell) as i32 + 1 {
            for i in 0..((SCREEN_W - OX) / cell) as i32 + 1 {
                let (x, y) = (i as f32 * cell, Y0 + j as f32 * cell);
                // a turning square reaches a little out of its cell
                if x + cell + 10.0 < vis.0 || x - 10.0 > vis.1 {
                    continue;
                }
                let r = ease((front - (x + y)) / 60.0);
                let base = (hash(i, j) & 1) as i32 + pass;
                let ang = (base as f32 + r) * std::f32::consts::FRAC_PI_2;
                let (cx, cy) = (x + cell / 2.0, y + cell / 2.0);
                let (sa, ca) = ang.sin_cos();
                let rot = |p: P| {
                    let (dx, dy) = (p.0 - cx, p.1 - cy);
                    (cx + dx * ca - dy * sa, cy + dx * sa + dy * ca)
                };
                let h = side / 2.0;
                let sq = [(cx - h, cy - h), (cx + h, cy - h), (cx + h, cy + h), (cx - h, cy + h)];
                b.poly(&sq.map(rot), fill);
                // quarter discs at the top-left and bottom-right corners
                for (ox, oy, s) in [(cx - h, cy - h, 1.0f32), (cx + h, cy + h, -1.0)] {
                    let mut pts = [(0.0, 0.0); 10];
                    pts[0] = rot((ox, oy));
                    for (p, (c, sn)) in pts[1..].iter_mut().zip(arc) {
                        *p = rot((ox + s * h * c, oy + s * h * sn));
                    }
                    b.poly(&pts, disc);
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
        let mut l = Life { cells: vec![false; LW * LH], age: vec![0; LW * LH], shown: vec![0.0; LW * LH], hist: Vec::new(), rng: 1, generation: 0, quiet: 0 };
        // a blinker turns over every step
        for x in 4..7 {
            l.set(x, 10, true);
        }
        l.step();
        assert!(l.alive(5, 9) && l.alive(5, 10) && l.alive(5, 11) && !l.alive(4, 10));
        assert_eq!(l.age[10 * LW + 5], 1);

        l.step();
        assert!(l.alive(4, 10) && l.alive(6, 10));
        assert_eq!(l.age[10 * LW + 5], 2, "the centre has lived on");
        assert_eq!(l.age[10 * LW + 4], 1, "newborn");
    }

    #[test]
    fn life_never_dies_out() {
        let mut l = Life::new();
        for _ in 0..2000 {
            l.step();
            assert!(l.population() > 0, "generation {}", l.generation);
        }
        let before = l.cells.clone();
        let mut changed = false;
        for _ in 0..40 {
            l.step();
            changed |= l.cells != before;
        }
        assert!(changed);
    }

    #[test]
    fn zigzag_window_draws_and_erases_along_a_fixed_path() {
        // the head moves one column a step; its tile grows over HOP
        let (h0, g0) = window(0.0, 0);
        let (h1, g1) = window(STEP + HOP + 0.01, 0);
        assert_eq!(h1, (h0 + 1) % (NCOL + WIN));
        assert!(g0 < 0.01 && g1 > 0.99, "{g0} {g1}");
        // bands are out of phase: never all in the strip at once
        let in_strip = |t: f32| (0..BANDS).filter(|b| {
            let (h, _) = window(t, *b);
            (h - WIN + 1..=h).any(|i| {
                let c = if b % 2 == 0 { i } else { NCOL - 1 - i };
                (0..NCOL).contains(&i) && c as usize >= STRIP_COL
            })
        }).count();
        for k in 0..200 {
            let n = in_strip(k as f32 * 0.37);
            assert!(n < BANDS, "at {k}: {n} bands in the strip");
        }
        assert!((0..200).any(|k| in_strip(k as f32 * 0.37) >= 2));
        // a nudge hurries the windows
        let mut p = Patterns::new();
        p.step(0.05);
        let a = p.inch_t;
        p.nudge(Kind::Zigzag, 1.0);
        p.step(0.05);
        assert!(p.inch_t - a > 0.12, "a 50 ms step went {}", p.inch_t - a);
    }

    #[test]
    fn scope_speed_stays_bounded_however_long_it_runs() {
        let mut p = Patterns::new();
        for _ in 0..8 {
            p.nudge(Kind::Scope, 1.0);
        }
        assert_eq!(p.ratio_to, SCOPE_RATIO_MAX);
        let top = (SCOPE_RATIO_MAX + 0.5) * SCOPE_RATE;
        for _ in 0..(3600 * 30) {
            let y = p.scope_y;
            p.step(1.0 / 30.0);
            let moved = (p.scope_y - y).rem_euclid(std::f32::consts::TAU);
            assert!(moved * 30.0 <= top + 0.001, "{moved}");
        }
        assert!(p.scope_y.is_finite());
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
        for _ in 0..3 {
            p.nudge(Kind::Scope, -1.0);
        }
        assert_eq!(p.ratio_to, 1.0, "clamped");
        p.nudge(Kind::Truchet, 1.0);
        assert_eq!(p.nudge.t, 120.0, "the wave is pushed along");
    }
}
