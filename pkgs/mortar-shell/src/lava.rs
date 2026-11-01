// SPDX-License-Identifier: GPL-2.0-or-later
//! The lava lamp behind the MENU's cross page (plan/mortar-cross-2.html,
//! its tuned constants): five wax pieces that heat at the bottom, rise,
//! cool and sink, a heavy pool along the bottom of the screen and a layer
//! hanging from the top, and a column joining a moving piece to the pool it
//! left, thinning until it pinches off.
//!
//! Physics (`Lamp::step`): mass grows with the square of the size, drag,
//! soft walls, cohesion (pieces repel when they overlap and pull together
//! when close), heat at the bottom and cooling at the top; ~15 s from
//! bottom to top. Navigation `kick`s it; each category has its own heat,
//! drag and buoyancy.
//!
//! Drawing: the pieces are not polygons but a signed distance field
//! (`Prims::dist`, a metaball-style union of every piece, pool and
//! column), which a renderer evaluates at 1/6 resolution over the lamp's
//! narrow region at the right edge and thresholds at full resolution: the
//! fill and a 2 px ink edge. The GL renderer runs the same maths in a
//! shader (comp/gl.rs `FS_LAVA_FIELD`); raster.rs calls `dist` on the CPU.
//!
//! The physics runs in the mockup's coordinates (a 340 px column centred
//! at x 800); `prims` maps it into the lamp: SCALE across, centred on CX,
//! so the lamp stays a quiet strip by the right edge, clear of the rows'
//! text.

/// The region the field covers, in lamp space (the cross page's own
/// coordinates): x from LX0, LW wide, the full screen height.
pub const LX0: f32 = 722.0;
pub const LW: f32 = 324.0;
pub const LH: f32 = 768.0;
/// Field texel size, px: a sixth of the screen's resolution.
pub const CELL: f32 = 6.0;
pub const FW: usize = (LW / CELL) as usize;
pub const FH: usize = (LH / CELL) as usize;
/// Ink edge, px.
pub const EDGE: f32 = 2.0;
/// Metaball union softness (px): gaps narrower than ~1.4x this close up.
pub const MERGE: f32 = 12.0;
/// Each primitive's distance is clamped to +-CLAMP before the union.
const CLAMP: f32 = 60.0;
/// The mockup's lamp mapped into ours: x' = CX + (x - 800) * SCALE.
pub const CX: f32 = 884.0;
pub const SCALE: f32 = 0.62;
/// Pool heights relative to the mockup's.
const POOL_H: f32 = 0.8;

/// Wax pieces: (x offset as a fraction of 260 px, phase, radius).
const BLOBS: [(f32, f32, f32); 5] = [(0.0, 0.0, 50.0), (0.32, 1.7, 38.0), (-0.3, 3.1, 44.0), (0.12, 4.4, 30.0), (-0.12, 5.6, 34.0)];
const TOP: f32 = 120.0;
const BOT: f32 = 650.0;
const HALF: f32 = 170.0;

/// A category's lamp: (centre x, heat speed, drag, buoyancy).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Feel {
    pub x: f32,
    pub heat: f32,
    pub drag: f32,
    pub buoy: f32,
}

pub const APPS: Feel = Feel { x: 800.0, heat: 0.4, drag: 0.9, buoy: 0.45 };
pub const GAMES: Feel = Feel { x: 770.0, heat: 0.55, drag: 0.7, buoy: 0.55 };
pub const MUSIC: Feel = Feel { x: 830.0, heat: 0.38, drag: 1.0, buoy: 0.45 };
pub const SETTINGS: Feel = Feel { x: 800.0, heat: 0.28, drag: 1.3, buoy: 0.35 };

#[derive(Clone, Copy, Debug, PartialEq)]
struct Blob {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    r: f32,
    m: f32,
    /// Temperature 0 (cold, sinks) .. 1 (hot, rises).
    temp: f32,
    ph: f32,
}

/// The lamp's state. Its springs (centre, sideways shift) are stepped by
/// the UI with the others (`springs`).
#[derive(Clone, Debug)]
pub struct Lamp {
    blobs: [Blob; 5],
    pub feel: Feel,
    /// Centre and the selection's sideways shift (springs, soft: k x0.35
    /// and x0.5).
    pub cx: crate::motion::Spring,
    pub sx: crate::motion::Spring,
    /// Wobble clock (seconds; frozen with Reduce motion).
    pub t: f32,
}

impl Lamp {
    pub fn new(feel: Feel) -> Lamp {
        let blobs = std::array::from_fn(|i| {
            let (fx, ph, r) = BLOBS[i];
            Blob { x: 800.0 + fx * 260.0, y: 420.0 + i as f32 * 50.0, vx: 0.0, vy: 0.0, r, m: (r / 36.0).powi(2), temp: if i % 2 == 1 { 0.8 } else { 0.2 }, ph }
        });
        let mut cx = crate::motion::Spring::new(feel.x);
        cx.k_mul = 0.35;
        let mut sx = crate::motion::Spring::new(0.0);
        sx.k_mul = 0.5;
        Lamp { blobs, feel, cx, sx, t: 0.0 }
    }

    /// A category and selection: the lamp drifts to the category's place
    /// and leans with the selected row.
    pub fn aim(&mut self, feel: Feel, sel: usize) {
        self.feel = feel;
        self.cx.set(feel.x);
        let k = sel as f32;
        self.sx.set(if sel % 2 == 1 { 14.0 } else { -14.0 } - k * 5.0);
    }

    /// A push from navigation (px/s, before the mass): lighter pieces move
    /// more.
    pub fn kick(&mut self, dx: f32, dy: f32) {
        for (i, o) in self.blobs.iter_mut().enumerate() {
            let f = (0.7 + 0.6 * ((i * 37) % 10) as f32 / 10.0) * 0.32 / o.m;
            o.vx += dx * f;
            o.vy += dy * f * 0.6;
        }
    }

    /// Fast enough that 30 fps would show steps (a recent kick).
    pub fn lively(&self) -> bool {
        self.blobs.iter().any(|o| o.vx.abs() > 45.0 || o.vy.abs() > 60.0) || !self.cx.rest() || !self.sx.rest()
    }

    /// Advance the wax by `dt` seconds (at most 33 ms at a time, in three
    /// slices, as the mockup).
    pub fn step(&mut self, dt: f32) {
        let dt = dt.clamp(0.0, 0.033);
        self.t += dt;
        let cx = self.cx.x + self.sx.x;
        let Feel { heat, drag, buoy, .. } = self.feel;
        let h = dt / 3.0;
        for _ in 0..3 {
            for i in 0..self.blobs.len() {
                let o = self.blobs[i];
                let mut temp = o.temp;
                if o.y > BOT - 110.0 {
                    temp = (temp + h * 0.35 * heat / o.m).min(1.0);
                } else if o.y < TOP + 110.0 {
                    temp = (temp - h * 0.35 * heat / o.m).max(0.0);
                } else {
                    temp += (0.5 - temp) * h * 0.02;
                }
                let mut fy = -(temp - 0.5) * 120.0 * buoy;
                let mut fx = 0.0;
                let (lo, hi) = (cx - HALF + o.r * 0.6, cx + HALF - o.r * 0.6);
                if o.x < lo {
                    fx += (lo - o.x) * 6.0;
                }
                if o.x > hi {
                    fx -= (o.x - hi) * 6.0;
                }
                fx += (cx - o.x) * 0.25;
                if o.y < TOP + o.r {
                    fy += (TOP + o.r - o.y) * 8.0;
                }
                if o.y > BOT - o.r {
                    fy -= (o.y - (BOT - o.r)) * 8.0;
                }
                for (j, q) in self.blobs.iter().enumerate() {
                    if j == i {
                        continue;
                    }
                    let (ddx, ddy) = (o.x - q.x, (o.y - q.y) / 1.8);
                    let d = (ddx * ddx + ddy * ddy).sqrt() + 0.01;
                    let rr = o.r + q.r;
                    if d < rr * 0.75 {
                        let pf = (rr * 0.75 - d) * 5.0;
                        fx += ddx / d * pf;
                        fy += ddy / d * pf;
                    } else if d < rr * 1.6 {
                        let af = (rr * 1.6 - d) * 0.5;
                        fx -= ddx / d * af;
                        fy -= ddy / d * af;
                    }
                }
                let o = &mut self.blobs[i];
                o.temp = temp;
                o.vx += fx / o.m * h;
                o.vy += fy / o.m * h;
                let k = (-drag * h).exp();
                o.vx *= k;
                o.vy *= k;
                o.x += o.vx * h;
                o.y += o.vy * h;
            }
        }
    }

    /// This frame's shapes.
    pub fn prims(&self) -> Prims {
        let cx = self.cx.x + self.sx.x;
        let t = self.t;
        let mut p = Prims::default();
        // the mockup's lamp, narrowed into ours by the right edge
        let m = |x: f32| CX + (x - 800.0) * SCALE;
        for (i, o) in self.blobs.iter().enumerate() {
            let vs = (o.vy.abs() / 90.0).min(0.5);
            let lean = (o.vx / 200.0).clamp(-0.35, 0.35);
            // moving up, the lower half trails (and the other way round)
            let up = o.vy < 0.0;
            let trail = |lower: bool| up == lower;
            let half = |lower: bool| {
                let tr = trail(lower);
                let sy = (1.8 + vs) * if tr { 1.0 + vs * 0.6 } else { 1.0 } * if lower { 1.05 } else { 0.95 };
                let sx = (0.82 - vs * 0.12) * if lower { 1.08 } else { 0.94 };
                let pinch = if tr { vs * 0.35 } else { 0.0 };
                [sx, sy, pinch]
            };
            let ph = [o.ph + t * 0.3, -o.ph * 1.7 + t * 0.2, t * 0.45 + i as f32];
            p.blobs[i] = BlobPrim {
                x: m(o.x),
                y: o.y,
                r: o.r * SCALE,
                lean,
                upper: half(false),
                lower: half(true),
                harm: ph.map(|a| (a.cos(), a.sin())),
            };
        }
        // the pools: a heavy mound along the bottom, a layer hanging from
        // the top; each swells under a piece that is near it
        let pool = |y_base: f32, w: f32, hgt: f32, up: f32| PoolPrim {
            x0: m(cx - w / 2.0),
            w: w * SCALE,
            hgt: hgt * POOL_H,
            y_base,
            up,
            phase: t * 0.35 + y_base,
            bumps: std::array::from_fn(|i| {
                let o = &self.blobs[i];
                let near = (1.0 - (o.y - y_base).abs() / 260.0).max(0.0);
                (m(o.x), 1.0 / (o.r * SCALE * 1.4), near * o.r * SCALE * 0.9)
            }),
        };
        p.pools = [pool(790.0, 360.0, 130.0, 1.0), pool(-24.0, 320.0, 118.0, -1.0)];
        // the columns: a rising piece stays joined to the bottom pool, a
        // sinking one trails a drip from the top, until it pinches off
        for (i, o) in self.blobs.iter().enumerate() {
            let up = o.vy < 0.0;
            let anchor = if up { BOT + 20.0 } else { TOP - 20.0 };
            let reach = if up { 300.0 } else { 230.0 };
            let k = (1.0 - (anchor - o.y).abs() / reach).max(0.0);
            let wn = o.r * 0.62 * k.powf(0.7);
            let mid = k.powf(1.4);
            p.necks[i] = if wn < 2.0 {
                NeckPrim::default()
            } else {
                NeckPrim { bx: m(o.x), by: o.y, ax: m(o.x + (cx - o.x) * 0.25), ay: anchor, w0: wn * SCALE, waist: wn * SCALE * (0.35 + 0.65 * mid) }
            };
        }
        p
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BlobPrim {
    pub x: f32,
    pub y: f32,
    pub r: f32,
    pub lean: f32,
    /// Per half: x scale, y scale, waist pinch (the trailing half).
    pub upper: [f32; 3],
    pub lower: [f32; 3],
    /// (cos, sin) of the 3θ, 2θ and 5θ wobble phases.
    pub harm: [(f32, f32); 3],
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PoolPrim {
    pub x0: f32,
    pub w: f32,
    pub hgt: f32,
    pub y_base: f32,
    /// 1: rises from the bottom; -1: hangs from the top.
    pub up: f32,
    pub phase: f32,
    /// Per piece: (x, 1 / width, height) of the swell under it.
    pub bumps: [(f32, f32, f32); 5],
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NeckPrim {
    pub bx: f32,
    pub by: f32,
    pub ax: f32,
    pub ay: f32,
    /// Half-width at the piece (0: no column); 1.6x at the pool.
    pub w0: f32,
    pub waist: f32,
}

/// Everything the field is made of, in lamp space.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Prims {
    pub blobs: [BlobPrim; 5],
    pub pools: [PoolPrim; 2],
    pub necks: [NeckPrim; 5],
}


impl BlobPrim {
    /// Signed distance (approximate, exact enough near the edge).
    pub fn dist(&self, px: f32, py: f32) -> f32 {
        let qy = py - self.y;
        let lx = px - self.x + qy * self.lean * 0.5;
        // the upper and lower halves differ; blend them across the middle
        // (no seam)
        let k = (qy / (self.r * 0.6) + 0.5).clamp(0.0, 1.0);
        let [sx, sy, pinch]: [f32; 3] = std::array::from_fn(|i| self.upper[i] + (self.lower[i] - self.upper[i]) * k);
        let (ux, uy) = (lx / sx, qy / sy);
        let l0 = (ux * ux + uy * uy).sqrt().max(1e-3);
        let s0 = uy / l0;
        // the trailing half narrows towards its tip
        let tf = 1.0 - pinch * s0.abs();
        let ux = ux / tf;
        let l = (ux * ux + uy * uy).sqrt().max(1e-3);
        let (c, s) = (ux / l, uy / l);
        let (c2, s2) = (c * c - s * s, 2.0 * s * c);
        let (c3, s3) = (4.0 * c * c * c - 3.0 * c, 3.0 * s - 4.0 * s * s * s);
        let (c5, s5) = (c3 * c2 - s3 * s2, s3 * c2 + c3 * s2);
        let [(a1c, a1s), (a2c, a2s), (a3c, a3s)] = self.harm;
        let wob = 0.1 * (s3 * a1c + c3 * a1s) + 0.07 * (s2 * a2c + c2 * a2s) + 0.03 * (s5 * a3c + c5 * a3s);
        let f = l - self.r * (1.0 + wob);
        let gx = c / (sx * tf);
        let gy = s / sy;
        f / (gx * gx + gy * gy).sqrt().max(1e-3)
    }
}

impl PoolPrim {
    pub fn dist(&self, px: f32, py: f32) -> f32 {
        let u = ((px - self.x0) / self.w).clamp(0.0, 1.0);
        let env = (std::f32::consts::PI * u).sin();
        let mut bump = 0.0;
        for (bx, iw, amp) in self.bumps {
            let d = (bx - px) * iw;
            bump += (-d * d).exp() * amp;
        }
        let hh = self.hgt * env * (1.0 + 0.12 * (u * 7.0 + self.phase).sin()) + bump * env;
        // the surface; inside is below it (bottom pool) or above (top)
        (self.y_base - self.up * hh - py) * self.up
    }
}

impl NeckPrim {
    pub fn dist(&self, px: f32, py: f32) -> f32 {
        if self.w0 <= 0.0 {
            return 1e4;
        }
        let (dx, dy) = (self.ax - self.bx, self.ay - self.by);
        let len2 = (dx * dx + dy * dy).max(1e-3);
        let t = (((px - self.bx) * dx + (py - self.by) * dy) / len2).clamp(0.0, 1.0);
        let (cx, cy) = (self.bx + dx * t, self.by + dy * t);
        let base = self.w0 + (self.w0 * 1.6 - self.w0) * t;
        let r = base + (self.waist - base) * 4.0 * t * (1.0 - t);
        // the threshold eats thin columns: they pinch off before they
        // vanish (the mockup's blur does the same)
        let r = r - 2.0;
        ((px - cx).powi(2) + (py - cy).powi(2)).sqrt() - r
    }
}

impl Prims {
    /// Signed distance to the wax (negative inside), px.
    ///
    /// The union is exponential (metaball-like, -k ln sum e^(-d/k)): every
    /// piece pulls on every other at once, so small gaps enclosed between
    /// three shapes fill in instead of leaving pinholes, which a pairwise
    /// smooth minimum did.
    pub fn dist(&self, px: f32, py: f32) -> f32 {
        let mut s = 0.0;
        let mut add = |d: f32| s += (-d.clamp(-CLAMP, CLAMP) / MERGE).exp();
        add(self.pools[0].dist(px, py));
        add(self.pools[1].dist(px, py));
        for b in &self.blobs {
            add(b.dist(px, py));
        }
        for n in &self.necks {
            if n.w0 > 0.0 {
                add(n.dist(px, py));
            }
        }
        -MERGE * s.ln()
    }

    /// The field over the lamp region at CELL px per texel, row by row
    /// (texel centres), for the CPU renderer.
    pub fn field(&self) -> Vec<f32> {
        let mut v = Vec::with_capacity(FW * FH);
        for j in 0..FH {
            let y = (j as f32 + 0.5) * CELL;
            for i in 0..FW {
                v.push(self.dist(LX0 + (i as f32 + 0.5) * CELL, y));
            }
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rises_and_sinks_slowly() {
        let mut l = Lamp::new(APPS);
        // the hot pieces rise: after a few seconds they are higher up,
        // after ~15 s one has been near the top
        let y0: Vec<f32> = l.blobs.iter().map(|b| b.y).collect();
        let mut top = f32::MAX;
        for _ in 0..(20 * 60) {
            l.step(1.0 / 60.0);
            top = top.min(l.blobs.iter().map(|b| b.y).fold(f32::MAX, f32::min));
        }
        assert!(top < 260.0, "something reached the top: {top}");
        assert!(l.blobs.iter().all(|b| b.y > TOP - 40.0 && b.y < BOT + 40.0), "walls hold: {:?}", l.blobs);
        let moved: f32 = l.blobs.iter().zip(&y0).map(|(b, y)| (b.y - y).abs()).sum();
        assert!(moved > 50.0);
        // calm: nothing flies about
        assert!(l.blobs.iter().all(|b| b.vy.abs() < 80.0), "{:?}", l.blobs.map(|b| b.vy));
    }

    #[test]
    fn kicks_move_light_pieces_more_and_settle() {
        let mut l = Lamp::new(SETTINGS);
        l.kick(460.0, 0.0);
        let v: Vec<f32> = l.blobs.iter().map(|b| b.vx).collect();
        // the biggest piece (r 50) is pushed least
        assert!(v[0] < v[3], "{v:?}");
        assert!(l.lively());
        for _ in 0..(5 * 60) {
            l.step(1.0 / 60.0);
        }
        assert!(l.blobs.iter().all(|b| b.vx.abs() < 30.0), "drag settles it");
    }

    #[test]
    fn field_has_inside_and_outside() {
        let l = Lamp::new(APPS);
        let p = l.prims();
        let b = &p.blobs[0];
        assert!(p.dist(b.x, b.y) < -12.0, "inside a piece");
        assert!(p.dist(CX, 766.0) < 0.0, "the bottom pool");
        assert!(p.dist(CX, 2.0) < 0.0, "the top layer");
        assert!(p.dist(LX0 + 4.0, 384.0) > 10.0, "outside the lamp");
        // the lamp keeps to the right edge, clear of the rows' text
        assert!((0..=128).all(|j| p.dist(700.0, j as f32 * 6.0) > EDGE), "nothing left of x 700");
        // the edge of a lone piece is about where its radius says
        let o = BlobPrim { x: 0.0, y: 0.0, r: 40.0, lean: 0.0, upper: [1.0, 1.0, 0.0], lower: [1.0, 1.0, 0.0], harm: [(1.0, 0.0); 3] };
        assert!(o.dist(50.0, 0.0) > 5.0 && o.dist(50.0, 0.0) < 15.0);
        assert_eq!(p.field().len(), FW * FH);
    }
}
