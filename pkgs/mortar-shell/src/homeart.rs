// SPDX-License-Identifier: GPL-2.0-or-later
//! Home's pattern: a small mosaic to the right of the big clock, in the
//! cross's language (patterns.rs: solid palette tiles on a 28 px grid with
//! 4 px of grout). Sun and Truchet are calm, a tile or two every few seconds;
//! Life is lively, about two generations a second. All of them run only for a
//! while after the last press; then they hold still (no frames).
//! Its colours follow the time of day: yellow and lime by day, orange and
//! pink at dawn and dusk, lavender and ink at night.
//!
//! - Sun: a sun of tiles (core, ring, rays) that crosses the block over
//!   the day, rising and setting; at night a crescent moon and stars. A ray
//!   (or a star) blinks out or back each step.
//! - Truchet: the classic two-colour tiles; each step turns one a quarter.
//! - Life: Conway's Game of Life on the block (a small torus), a generation
//!   each step. A small torus settles fast, so an R-pentomino drops in when it
//!   thins out or repeats, and a glider every so often regardless.
//! - River: a landscape eroding. Rain falls on rough ground that drains to one
//!   outlet; water cuts channels in proportion to how much flows through them,
//!   so a branching river system forms by itself, and starts over on new ground.
//! - Boids: a small flock (separation, alignment, cohesion) that lights the tiles
//!   it crosses and leaves them fading behind it.
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
    River,
    Boids,
    Off,
}

impl Kind {
    pub const ALL: [Kind; 6] = [Kind::Sun, Kind::Truchet, Kind::Life, Kind::River, Kind::Boids, Kind::Off];
    /// Seconds between changes while it runs.
    pub fn every(self) -> f64 {
        match self {
            Kind::Life => 0.55,
            Kind::River => 0.3,
            Kind::Boids => 0.1,
            _ => 3.0,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Kind::Sun => "Sun",
            Kind::Truchet => "Truchet",
            Kind::Life => "Life",
            Kind::River => "River",
            Kind::Boids => "Boids",
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
/// Life's steps are close together, so each eases in faster.
const LIFE_EASE: f32 = 0.22;
/// Fewer live cells than this and Life gets a new seed.
const LIFE_THIN: usize = 14;
const LIFE_GLIDER_EVERY: u32 = 16;
/// River: how hard water cuts, how fast land rises, what counts as a channel, how
/// many raindrops, and how long a landscape lasts (steps) before new ground.
const RIVER_CUT: f32 = 0.05;
const RIVER_UPLIFT: f32 = 0.004;
const RIVER_CHANNEL: f32 = 3.0;
const RIVER_CARVE: f32 = 0.022;
const RIVER_DROPS: usize = 14;
const RIVER_LIFE: u32 = 280;
const FLOCK: usize = 13;

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
    /// River: ground height, where each cell drains (a neighbour; itself at the
    /// outlet), how much flows through it, the raindrops, and how long it has run.
    ground: Vec<f32>,
    drain: Vec<usize>,
    flow: Vec<f32>,
    outlet: usize,
    /// How deep the water has cut each cell: a channel shows once it passes 1, so
    /// the network grows back from the outlet into the hills, as real ones do.
    carved: Vec<f32>,
    drops: Vec<usize>,
    river_age: u32,
    /// River and Boids: each tile's size, shown and heading for.
    shown: Vec<f32>,
    goal: Vec<f32>,
    /// Boids: position and velocity, in cells.
    flock: Vec<[f32; 4]>,
    /// Life: generations since the last glider dropped in.
    since_glider: u32,
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
            since_glider: 0,
            ground: vec![0.0; COLS * ROWS],
            drain: (0..COLS * ROWS).collect(),
            flow: vec![1.0; COLS * ROWS],
            outlet: 0,
            carved: vec![0.0; COLS * ROWS],
            drops: Vec::new(),
            river_age: 0,
            shown: vec![0.0; COLS * ROWS],
            goal: vec![0.0; COLS * ROWS],
            flock: Vec::new(),
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
        match kind {
            Kind::River => {
                a.new_ground();
                // a stream already running, the rest still to come
                for _ in 0..6 {
                    a.river_step();
                }
                a.shown = a.goal.clone();
            }
            Kind::Boids => {
                for _ in 0..FLOCK {
                    let (x, y) = (a.unit() * COLS as f32, a.unit() * ROWS as f32);
                    let (vx, vy) = (a.unit() - 0.5, a.unit() - 0.5);
                    a.flock.push([x, y, vx, vy]);
                }
                for _ in 0..30 {
                    a.boids_step();
                }
                a.shown = a.goal.clone();
            }
            _ => {}
        }
        a
    }

    fn unit(&mut self) -> f32 {
        (self.rand() % 10_000) as f32 / 10_000.0
    }

    /// Rough ground, sloping gently to one outlet on the lower edge.
    fn new_ground(&mut self) {
        let ox = 2 + self.rand() as usize % (COLS - 4);
        self.outlet = (ROWS - 1) * COLS + ox;
        for i in 0..COLS * ROWS {
            let (dx, dy) = ((i % COLS) as f32 - ox as f32, (ROWS - 1 - i / COLS) as f32);
            self.ground[i] = 0.12 * (dx * dx + dy * dy).sqrt() + 1.4 * self.unit();
        }
        self.carved.iter_mut().for_each(|c| *c = 0.0);
        self.drops.clear();
        self.river_age = 0;
    }

    /// One step of erosion. Flood outward from the outlet so that every cell has a
    /// downhill way out (lakes fill until they spill), measure how much drains
    /// through each cell, then let the flow cut: more water, faster.
    fn river_step(&mut self) {
        let n = COLS * ROWS;
        let mut level = vec![f32::INFINITY; n];
        let mut done = vec![false; n];
        let mut order = Vec::with_capacity(n);
        level[self.outlet] = self.ground[self.outlet];
        self.drain[self.outlet] = self.outlet;
        for _ in 0..n {
            let Some(i) = (0..n).filter(|i| !done[*i]).min_by(|a, b| level[*a].total_cmp(&level[*b])) else {
                break;
            };
            done[i] = true;
            order.push(i);
            let (x, y) = ((i % COLS) as i32, (i / COLS) as i32);
            // Four neighbours: a channel's tiles always touch side by side.
            for (dx, dy) in [(0, -1), (-1, 0), (1, 0), (0, 1)] {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= COLS as i32 || ny >= ROWS as i32 {
                    continue;
                }
                let j = ny as usize * COLS + nx as usize;
                let through = self.ground[j].max(level[i] + 0.001);
                if !done[j] && through < level[j] {
                    level[j] = through;
                    self.drain[j] = i;
                }
            }
        }
        self.ground = level;
        self.flow.iter_mut().for_each(|f| *f = 1.0);
        for &i in order.iter().rev() {
            let to = self.drain[i];
            if to != i {
                self.flow[to] += self.flow[i];
            }
        }
        for &i in &order {
            let to = self.drain[i];
            if to != i {
                let drop = self.ground[i] - self.ground[to];
                let cut = (RIVER_CUT * self.flow[i].sqrt()).min(0.9) * drop;
                // the land keeps rising a little, so hills between channels stay
                self.ground[i] += RIVER_UPLIFT - cut;
            }
        }
        // Raindrops run down the channels, a cell a step, and fall again.
        for d in 0..self.drops.len() {
            self.drops[d] = self.drain[self.drops[d]];
        }
        while self.drops.len() < RIVER_DROPS {
            let i = self.rand() as usize % n;
            self.drops.push(i);
        }
        for d in 0..self.drops.len() {
            if self.drops[d] == self.outlet {
                self.drops[d] = self.rand() as usize % n;
            }
        }
        for i in 0..n {
            if self.flow[i] >= RIVER_CHANNEL {
                self.carved[i] += RIVER_CARVE * self.flow[i].sqrt();
            }
            self.goal[i] = if self.is_water(i) {
                (0.42 + 0.1 * self.flow[i].ln()).min(1.0)
            } else {
                (0.12 + 0.05 * self.ground[i]).clamp(0.12, 0.36)
            };
        }
        self.river_age += 1;
    }

    fn is_water(&self, i: usize) -> bool {
        self.carved[i] >= 1.0
    }

    /// Separation, alignment and cohesion, kept inside the block by a soft wall.
    fn boids_step(&mut self) {
        let now = self.flock.clone();
        for (i, b) in self.flock.iter_mut().enumerate() {
            let (mut sep, mut ali, mut coh, mut near) = ([0.0f32; 2], [0.0f32; 2], [0.0f32; 2], 0.0f32);
            for (j, o) in now.iter().enumerate() {
                let (dx, dy) = (o[0] - b[0], o[1] - b[1]);
                let d2 = dx * dx + dy * dy;
                if i == j || d2 > 20.0 {
                    continue;
                }
                near += 1.0;
                ali[0] += o[2];
                ali[1] += o[3];
                coh[0] += dx;
                coh[1] += dy;
                if d2 < 2.5 {
                    sep[0] -= dx / (d2 + 0.1);
                    sep[1] -= dy / (d2 + 0.1);
                }
            }
            if near > 0.0 {
                b[2] += 0.05 * ali[0] / near + 0.012 * coh[0] / near + 0.05 * sep[0];
                b[3] += 0.05 * ali[1] / near + 0.012 * coh[1] / near + 0.05 * sep[1];
            }
            let wall = |p: f32, max: f32| {
                if p < 1.5 {
                    0.07
                } else if p > max - 1.5 {
                    -0.07
                } else {
                    0.0
                }
            };
            b[2] += wall(b[0], COLS as f32);
            b[3] += wall(b[1], ROWS as f32);
            let speed = (b[2] * b[2] + b[3] * b[3]).sqrt().max(0.001);
            let want = speed.clamp(0.18, 0.42);
            b[2] *= want / speed;
            b[3] *= want / speed;
            // bounce off the edge rather than stall against it
            for (pos, vel, max) in [(0, 2, COLS as f32), (1, 3, ROWS as f32)] {
                if b[pos] + b[vel] < 0.0 || b[pos] + b[vel] >= max {
                    b[vel] = -b[vel];
                }
                b[pos] = (b[pos] + b[vel]).clamp(0.0, max - 0.01);
            }
        }
        for g in &mut self.goal {
            *g *= 0.86;
            if *g < 0.05 {
                *g = 0.0;
            }
        }
        for b in &self.flock {
            self.goal[b[1] as usize * COLS + b[0] as usize] = 1.0;
        }
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

    /// The classic long-lived seed: it boils for a good while from five cells.
    fn r_pentomino(&mut self, x: usize, y: usize) {
        for (dx, dy) in [(1, 0), (2, 0), (0, 1), (1, 1), (1, 2)] {
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
        let alive = self.cells.iter().filter(|c| **c).count();
        self.since_glider += 1;
        if alive < LIFE_THIN || self.hist.contains(&h) {
            let (x, y) = (self.rand() as usize % COLS, self.rand() as usize % ROWS);
            self.r_pentomino(x, y);
            self.hist.clear();
        } else {
            self.hist.push(h);
            if self.hist.len() > 16 {
                self.hist.remove(0);
            }
            if self.since_glider >= LIFE_GLIDER_EVERY {
                let (x, y) = (self.rand() as usize % COLS, self.rand() as usize % ROWS);
                self.glider(x, y);
                self.since_glider = 0;
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
            Kind::River => {
                if self.river_age >= RIVER_LIFE {
                    self.new_ground();
                }
                self.river_step();
            }
            Kind::Boids => self.boids_step(),
            Kind::Off => self.p = 1.0,
        }
    }

    /// Advance the change in progress.
    pub fn ease(&mut self, dt: f32) {
        if self.p >= 1.0 {
            return;
        }
        let ease = match self.kind {
            Kind::Life => LIFE_EASE,
            Kind::River => 0.28,
            Kind::Boids => 0.1,
            _ => EASE,
        };
        self.p = (self.p + dt / ease).min(1.0);
        if matches!(self.kind, Kind::River | Kind::Boids) {
            let k = (dt * if self.kind == Kind::River { 7.0 } else { 16.0 }).min(1.0);
            for (s, g) in self.shown.iter_mut().zip(&self.goal) {
                *s += (*g - *s) * k;
            }
            if self.p >= 1.0 {
                self.shown.clone_from(&self.goal);
            }
        }
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
            Kind::River => {
                let centre = |i: usize| {
                    (X0 + (i % COLS) as f32 * GRID + (GRID - GROUT) / 2.0, Y0 + (i / COLS) as f32 * GRID + (GRID - GROUT) / 2.0)
                };
                for i in 0..COLS * ROWS {
                    let water = self.is_water(i);
                    tile(b, (i % COLS) as f32, (i / COLS) as f32, self.shown[i], if water { pal.lavender } else { t[1] });
                    // a channel runs on into the next tile, across the grout
                    let to = self.drain[i];
                    if water && to != i {
                        let ((x0, y0), (x1, y1)) = (centre(i), centre(to));
                        let w = (GRID - GROUT) * self.shown[i].min(self.shown[to]);
                        let (x, y) = (x0.min(x1) - if y0 == y1 { 0.0 } else { w / 2.0 }, y0.min(y1) - if y0 == y1 { w / 2.0 } else { 0.0 });
                        let (dw, dh) = if y0 == y1 { ((x1 - x0).abs(), w) } else { (w, (y1 - y0).abs()) };
                        b.rect(x, y, dw, dh, pal.lavender);
                    }
                }
                // Rain only shows once it is running in a channel.
                for &d in &self.drops {
                    if self.is_water(d) {
                        tile(b, (d % COLS) as f32, (d / COLS) as f32, 0.5, t[0]);
                    }
                }
            }
            Kind::Boids => {
                for i in 0..COLS * ROWS {
                    let s = self.shown[i];
                    if s > 0.05 {
                        let c = if s > 0.82 { t[0] } else { t[1] };
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
    fn a_river_system_forms_and_starts_over() {
        let mut a = HomeArt::new(Kind::River);
        let early = (0..COLS * ROWS).filter(|i| a.is_water(*i)).count();
        for _ in 0..100 {
            a.step();
            a.ease(1.0);
        }
        let (n, biggest) = (COLS * ROWS, a.flow.iter().cloned().fold(0.0, f32::max));
        assert_eq!(biggest, n as f32, "everything drains to the outlet");
        assert_eq!(a.drain[a.outlet], a.outlet);
        for i in 0..n {
            // every cell's water reaches the outlet, without loops
            let (mut c, mut hops) = (i, 0);
            while c != a.outlet {
                c = a.drain[c];
                hops += 1;
                assert!(hops <= n, "cell {i} never drains");
            }
        }
        let channels = (0..n).filter(|i| a.is_water(*i)).count();
        let tributaries = (0..n).filter(|i| a.is_water(*i) && (0..n).filter(|j| a.drain[*j] == *i && *j != *i && a.is_water(*j)).count() >= 2).count();
        assert!(channels >= 12 && channels > early * 2, "{early} grew to {channels} channel cells");
        assert!(tributaries >= 1, "the channels branch");
        let outlet = a.outlet;
        let mut new_ground = false;
        for _ in 0..(RIVER_LIFE * 3) {
            a.step();
            a.ease(1.0);
            new_ground |= a.outlet != outlet;
            assert!(a.ground.iter().all(|h| h.is_finite()));
        }
        assert!(new_ground, "it starts over on new ground");
    }

    #[test]
    fn the_flock_stays_in_the_block_and_keeps_moving() {
        let mut a = HomeArt::new(Kind::Boids);
        let mut lit = 0;
        for _ in 0..600 {
            let was = a.flock.clone();
            a.step();
            a.ease(1.0);
            lit += a.goal.iter().filter(|g| **g >= 1.0).count();
            for (o, b) in was.iter().zip(&a.flock) {
                assert!(b[0] >= 0.0 && b[0] < COLS as f32 && b[1] >= 0.0 && b[1] < ROWS as f32);
                assert!((o[0] - b[0]).abs() + (o[1] - b[1]).abs() > 0.01);
            }
        }
        assert!(lit > 600, "{lit}");
    }

    #[test]
    fn steps_change_a_little_and_settle() {
        for kind in [Kind::Sun, Kind::Truchet, Kind::Life, Kind::River, Kind::Boids] {
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
        // and stays lively: most generations change plenty of cells
        let mut a = HomeArt::new(Kind::Life);
        let (mut changed, n) = (0usize, 400);
        for _ in 0..n {
            let was = a.cells.clone();
            a.step();
            a.ease(1.0);
            changed += was.iter().zip(&a.cells).filter(|(x, y)| x != y).count();
        }
        assert!(changed / n >= 8, "{} cells change per generation", changed / n);
        assert_eq!(hour_of("08:42"), 8.7);
        assert_eq!(hour_of("23:30"), 23.5);
        // the sun rises in the morning, is highest at midday
        assert!(HomeArt::sun_at(12.5).1 < HomeArt::sun_at(7.0).1);
    }
}
