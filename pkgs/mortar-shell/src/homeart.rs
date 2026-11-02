// SPDX-License-Identifier: GPL-2.0-or-later
//! Home's pattern: a small mosaic to the right of the big clock, in the
//! cross's language (patterns.rs: solid palette tiles on a 28 px grid with
//! 4 px of grout), but calm: a tile or two changes every few seconds, and
//! only for a while after the last press; then it holds still (no frames).
//! Its colours follow the time of day: yellow and lime by day, orange and
//! pink at dawn and dusk, lavender and ink at night.
//!
//! - Sun: a sun of tiles (core, ring, rays) that crosses the block over
//!   the day, rising and setting; at night a crescent moon and stars. A ray
//!   (or a star) blinks out or back each step.
//! - Truchet: the classic two-colour tiles; each step turns one a quarter.
//! - Life: Conway's Game of Life on the block (a small torus), a
//!   generation each step; a glider drops in when it dies out or repeats.
//!
//! Pure state: shellui steps it (`step`) and eases it (`ease`) and draws
//! it (`draw`) in full-screen coordinates.

use crate::art::Pal;
use crate::scene::{B, P};
use mortar_ui::gfx::Color;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Sun,
    Truchet,
    Life,
    Off,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Sun, Kind::Truchet, Kind::Life, Kind::Off];
    pub fn name(self) -> &'static str {
        match self {
            Kind::Sun => "Sun",
            Kind::Truchet => "Truchet",
            Kind::Life => "Life",
            Kind::Off => "Off",
        }
    }
}

/// The block: COLS x ROWS tiles of GRID px, its right edge at the
/// dashboard's margin, its top level with the clock's.
pub const COLS: usize = 15;
pub const ROWS: usize = 9;
const GRID: f32 = 28.0;
const GROUT: f32 = 4.0;
pub const X0: f32 = 1024.0 - 56.0 - COLS as f32 * GRID + GROUT;
pub const Y0: f32 = 92.0;
/// A step's change eases in over this long.
pub const EASE: f32 = 0.45;

/// The colours for the hour (0..24): three tones.
pub fn tones(pal: &Pal, hour: f32) -> [Color; 3] {
    if !(6.0..21.0).contains(&hour) {
        [pal.lavender, pal.ink, pal.lavender]
    } else if hour < 8.0 || hour >= 17.5 {
        [pal.orange, pal.pink, pal.yellow]
    } else {
        [pal.yellow, pal.lime, pal.lime]
    }
}

fn ease(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// One tile in cell (col, row), scaled by `s` about its centre.
fn tile(b: &mut B, col: f32, row: f32, s: f32, c: Color) {
    let size = (GRID - GROUT) * s;
    if size < 1.0 {
        return;
    }
    let (cx, cy) = (X0 + col * GRID + (GRID - GROUT) / 2.0, Y0 + row * GRID + (GRID - GROUT) / 2.0);
    b.rect(cx - size / 2.0, cy - size / 2.0, size, size, c);
}

#[derive(Clone, Debug)]
pub struct HomeArt {
    pub kind: Kind,
    /// Sun: which rays (or stars) show, before and after the last step.
    mask: u32,
    mask_old: u32,
    /// Truchet: each cell's quarter turns; the cell turning now.
    rot: Vec<u8>,
    turning: Option<usize>,
    /// Life: cells now and before the last step, and their ages.
    cells: Vec<bool>,
    old: Vec<bool>,
    age: Vec<u16>,
    hist: Vec<u64>,
    /// The last step's progress (0..1; 1: still).
    pub p: f32,
    rng: u32,
    /// Steps taken (tests, `ctl state`).
    pub steps: u64,
}

const TW: usize = 7;
const TH: usize = 4;
const RAYS: u32 = 12;
const STARS: [(f32, f32); 9] = [(1.0, 1.0), (4.0, 3.0), (2.0, 6.0), (7.0, 0.0), (8.0, 5.0), (5.0, 7.0), (0.0, 4.0), (6.0, 2.0), (3.0, 8.0)];

impl HomeArt {
    pub fn new(kind: Kind) -> HomeArt {
        let mut a = HomeArt {
            kind,
            mask: 0b1011_0110_1101,
            mask_old: 0b1011_0110_1101,
            rot: Vec::new(),
            turning: None,
            cells: vec![false; COLS * ROWS],
            old: vec![false; COLS * ROWS],
            age: vec![0; COLS * ROWS],
            hist: Vec::new(),
            p: 1.0,
            rng: 0x686f_6d65,
            steps: 0,
        };
        a.rot = (0..TW * TH).map(|_| (a.rand() & 1) as u8).collect();
        // Life: an R-pentomino and a glider, a few generations in
        for (x, y) in [(6, 3), (7, 3), (5, 4), (6, 4), (6, 5)] {
            a.cells[y * COLS + x] = true;
        }
        a.glider(1, 1);
        for _ in 0..6 {
            a.life_step();
        }
        a.old = a.cells.clone();
        a
    }

    fn rand(&mut self) -> u32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        self.rng
    }

    fn glider(&mut self, x: usize, y: usize) {
        for (dx, dy) in [(1, 0), (2, 1), (0, 2), (1, 2), (2, 2)] {
            self.cells[((y + dy) % ROWS) * COLS + (x + dx) % COLS] = true;
        }
    }

    fn life_step(&mut self) {
        let alive = |c: &[bool], x: usize, y: usize| c[(y % ROWS) * COLS + x % COLS];
        let mut next = vec![false; COLS * ROWS];
        for y in 0..ROWS {
            for x in 0..COLS {
                let mut n = 0;
                for dy in [ROWS - 1, 0, 1] {
                    for dx in [COLS - 1, 0, 1] {
                        if (dx, dy) != (0, 0) && alive(&self.cells, x + dx, y + dy) {
                            n += 1;
                        }
                    }
                }
                next[y * COLS + x] = matches!((alive(&self.cells, x, y), n), (true, 2) | (_, 3));
            }
        }
        for (a, on) in self.age.iter_mut().zip(&next) {
            *a = if *on { a.saturating_add(1) } else { 0 };
        }
        self.cells = next;
        let h = self.cells.iter().enumerate().fold(0xcbf2_9ce4_8422_2325u64, |h, (i, c)| if *c { (h ^ i as u64).wrapping_mul(0x100_0000_01b3) } else { h });
        if !self.cells.contains(&true) || self.hist.contains(&h) {
            let (x, y) = (self.rand() as usize % COLS, self.rand() as usize % ROWS);
            self.glider(x, y);
            self.hist.clear();
        } else {
            self.hist.push(h);
            if self.hist.len() > 16 {
                self.hist.remove(0);
            }
        }
    }

    /// One change: a ray or star blinks, a Truchet tile turns, a Life
    /// generation.
    pub fn step(&mut self) {
        self.steps += 1;
        self.p = 0.0;
        match self.kind {
            Kind::Sun => {
                self.mask_old = self.mask;
                let k = self.rand() % RAYS;
                self.mask ^= 1 << k;
                // never all out, never all on
                if self.mask.count_ones() < 5 || self.mask.count_ones() > 10 {
                    self.mask ^= 1 << k;
                    self.mask ^= 1 << ((k + 5) % RAYS);
                }
            }
            Kind::Truchet => {
                let i = self.rand() as usize % self.rot.len();
                self.turning = Some(i);
            }
            Kind::Life => {
                self.old = self.cells.clone();
                self.life_step();
            }
            Kind::Off => self.p = 1.0,
        }
    }

    /// Advance the change in progress.
    pub fn ease(&mut self, dt: f32) {
        if self.p >= 1.0 {
            return;
        }
        self.p = (self.p + dt / EASE).min(1.0);
        if self.p >= 1.0 {
            if let Some(i) = self.turning.take() {
                self.rot[i] = (self.rot[i] + 1) % 4;
            }
            self.mask_old = self.mask;
            self.old = self.cells.clone();
        }
    }

    /// A change is easing in (frames needed).
    pub fn moving(&self) -> bool {
        self.p < 1.0
    }

    /// Draw it for the hour (0..24).
    pub fn draw(&self, b: &mut B, pal: &Pal, hour: f32) {
        let t = tones(pal, hour);
        match self.kind {
            Kind::Sun => self.sun(b, &t, hour),
            Kind::Truchet => self.truchet(b, pal, &t),
            Kind::Life => {
                let e = ease(self.p);
                for i in 0..COLS * ROWS {
                    let (now, was) = (self.cells[i], self.old[i]);
                    let s = match (was, now) {
                        (true, true) => 1.0,
                        (false, true) => e,
                        (true, false) => 1.0 - e,
                        (false, false) => 0.0,
                    };
                    if s > 0.0 {
                        let c = if now && self.age[i] <= 1 { t[0] } else { t[1] };
                        tile(b, (i % COLS) as f32, (i / COLS) as f32, s, c);
                    }
                }
            }
            Kind::Off => {}
        }
    }

    /// The sun's centre (cells) for the hour: it crosses the block from
    /// left to right between 6 and 19 h, highest at midday.
    fn sun_at(hour: f32) -> (f32, f32) {
        let k = ((hour - 6.0) / 13.0).clamp(0.0, 1.0);
        let up = (k * std::f32::consts::PI).sin();
        (2.5 + k * (COLS as f32 - 5.0), ROWS as f32 + 1.0 - up * (ROWS as f32 - 3.5))
    }

    fn sun(&self, b: &mut B, t: &[Color; 3], hour: f32) {
        let e = ease(self.p);
        let shown = |bit: u32| match (self.mask_old >> bit & 1, self.mask >> bit & 1) {
            (1, 1) => 1.0,
            (0, 1) => e,
            (1, 0) => 1.0 - e,
            _ => 0.0,
        };
        let night = !(6.0..21.0).contains(&hour);
        if night {
            // a crescent moon (a disc with a bite) and stars
            let (cx, cy) = (COLS as f32 - 5.0, 4.5);
            for r in 0..ROWS {
                for c in 0..COLS {
                    let (x, y) = (c as f32 + 0.5, r as f32 + 0.5);
                    let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
                    let bite = ((x - cx - 1.6).powi(2) + (y - cy + 1.2).powi(2)).sqrt();
                    if d < 3.6 && bite > 3.0 {
                        tile(b, c as f32, r as f32, 1.0, t[0]);
                    }
                }
            }
            for (k, (x, y)) in STARS.iter().enumerate() {
                let s = shown(k as u32);
                if s > 0.0 {
                    tile(b, *x, *y, s * 0.55, t[1]);
                }
            }
            return;
        }
        let (cx, cy) = Self::sun_at(hour);
        for r in 0..ROWS {
            for c in 0..COLS {
                let (x, y) = (c as f32 + 0.5, r as f32 + 0.5);
                let (dx, dy) = (x - cx, y - cy);
                let d = (dx * dx + dy * dy).sqrt();
                if d < 2.5 {
                    tile(b, c as f32, r as f32, 1.0, t[0]);
                } else if d < 3.6 {
                    tile(b, c as f32, r as f32, 1.0, t[1]);
                } else if (4.4..6.4).contains(&d) {
                    // rays: every 30 degrees
                    let a = dy.atan2(dx).to_degrees().rem_euclid(360.0);
                    let k = (a / 30.0).round();
                    if (a - k * 30.0).abs() < 7.5 {
                        let s = shown(k as u32 % RAYS);
                        if s > 0.0 {
                            tile(b, c as f32, r as f32, s, t[2]);
                        }
                    }
                }
            }
        }
    }

    /// Truchet tiles on a 2-tile pitch: a quiet square (ground 2) with
    /// quarter discs in the hour's first tone at two opposite corners.
    fn truchet(&self, b: &mut B, pal: &Pal, t: &[Color; 3]) {
        let cell = GRID * 2.0;
        let side = cell - GROUT;
        let arc: [P; 9] = std::array::from_fn(|k| {
            let a = (k as f32 * 90.0 / 8.0).to_radians();
            (a.cos(), a.sin())
        });
        let x_off = (COLS as f32 * GRID - TW as f32 * cell) / 2.0;
        for j in 0..TH {
            for i in 0..TW {
                let n = j * TW + i;
                let turn = if self.turning == Some(n) { ease(self.p) } else { 0.0 };
                let ang = (self.rot[n] as f32 + turn) * std::f32::consts::FRAC_PI_2;
                let (x, y) = (X0 + x_off + i as f32 * cell, Y0 + j as f32 * cell);
                let (cx, cy) = (x + side / 2.0, y + side / 2.0);
                let (sa, ca) = ang.sin_cos();
                let rot = |p: P| {
                    let (dx, dy) = (p.0 - cx, p.1 - cy);
                    (cx + dx * ca - dy * sa, cy + dx * sa + dy * ca)
                };
                let h = side / 2.0;
                let sq = [(cx - h, cy - h), (cx + h, cy - h), (cx + h, cy + h), (cx - h, cy + h)];
                b.poly(&sq.map(rot), pal.ground_2);
                for (ox, oy, s) in [(cx - h, cy - h, 1.0f32), (cx + h, cy + h, -1.0)] {
                    let mut pts = [(0.0, 0.0); 10];
                    pts[0] = rot((ox, oy));
                    for (p, (c, sn)) in pts[1..].iter_mut().zip(arc) {
                        *p = rot((ox + s * h * c, oy + s * h * sn));
                    }
                    b.poly(&pts, t[0]);
                }
            }
        }
    }
}

/// "HH:MM" as hours (0..24).
pub fn hour_of(time: &str) -> f32 {
    let mut it = time.split(':');
    let h: f32 = it.next().and_then(|s| s.trim().parse().ok()).unwrap_or(12.0);
    let m: f32 = it.next().and_then(|s| s.trim().get(..2).and_then(|s| s.parse().ok())).unwrap_or(0.0);
    h + m / 60.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_change_a_little_and_settle() {
        for kind in [Kind::Sun, Kind::Truchet, Kind::Life] {
            let mut a = HomeArt::new(kind);
            assert!(!a.moving());
            a.step();
            assert!(a.moving());
            for _ in 0..30 {
                a.ease(1.0 / 30.0);
            }
            assert!(!a.moving(), "{kind:?} settles after a step");
        }
        // Life keeps going on its small torus
        let mut a = HomeArt::new(Kind::Life);
        for _ in 0..500 {
            a.step();
            a.ease(1.0);
            assert!(a.cells.contains(&true));
        }
        assert_eq!(hour_of("08:42"), 8.7);
        assert_eq!(hour_of("23:30"), 23.5);
        // the sun rises in the morning, is highest at midday
        assert!(HomeArt::sun_at(12.5).1 < HomeArt::sun_at(7.0).1);
    }
}
