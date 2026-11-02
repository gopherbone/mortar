// SPDX-License-Identifier: GPL-2.0-or-later
//! Redrawing only what changed. The GPU path renders into one of two
//! scanout buffers in turn, so the buffer it draws into still holds the
//! frame before last. Each item of a scene gets a fingerprint and the
//! screen area it covers (`Info`); comparing them with what that buffer
//! last got gives the areas that differ (`Grid`), as a few rectangles; the
//! renderer then draws only the items that touch them, scissored to them.
//! The cross's pattern strip is a quarter of the screen, and of it only
//! the tiles that move: most of a frame is never touched.
//!
//! Image contents are not in the fingerprints: whoever owns the textures
//! redraws everything when one of them changes.

use crate::scene::{Item, Scene, TextRun, P};
use std::hash::{Hash, Hasher};

/// An item's fingerprint (what it looks like, under the clip in force) and
/// the area it can touch: x0, y0, x1, y1 in scene units, clipped (empty
/// when x1 <= x0).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Info {
    pub hash: u64,
    pub bbox: [f32; 4],
}

/// Covered area is grown by this much (antialiasing, MSAA, pixel snapping).
const MARGIN: f32 = 2.0;

/// FNV-1a over 64-bit words (fast, and good enough to tell frames apart).
struct Fnv(u64);

impl Hasher for Fnv {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = (self.0 ^ *b as u64).wrapping_mul(0x100_0000_01b3);
        }
    }
    fn write_u32(&mut self, x: u32) {
        self.write_u64(x as u64);
    }
    fn write_u64(&mut self, x: u64) {
        self.0 = (self.0 ^ x).wrapping_mul(0x100_0000_01b3);
        self.0 ^= self.0 >> 29;
    }
}

impl Fnv {
    fn new(tag: u64) -> Fnv {
        let mut h = Fnv(0xcbf2_9ce4_8422_2325);
        h.write_u64(tag);
        h
    }
    fn f(&mut self, x: f32) {
        self.write_u32(x.to_bits());
    }
    fn pts(&mut self, p: &[P]) {
        self.write_u64(p.len() as u64);
        for q in p {
            self.write_u64(((q.0.to_bits() as u64) << 32) | q.1.to_bits() as u64);
        }
    }
    fn color(&mut self, c: mortar_ui::gfx::Color) {
        self.write_u32(u32::from_le_bytes([c.0, c.1, c.2, c.3]));
    }
}

fn bounds(p: &[P], grow: f32) -> [f32; 4] {
    let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for q in p {
        b = [b[0].min(q.0), b[1].min(q.1), b[2].max(q.0), b[3].max(q.1)];
    }
    [b[0] - grow, b[1] - grow, b[2] + grow, b[3] + grow]
}

fn meet(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])]
}

fn is_empty(b: &[f32; 4]) -> bool {
    !(b[2] > b[0] && b[3] > b[1])
}

/// Do `b` (x0, y0, x1, y1) and the rectangle `r` (x, y, w, h) overlap?
pub fn touches(b: &[f32; 4], r: &[f32; 4]) -> bool {
    b[0] < r[0] + r[2] && b[2] > r[0] && b[1] < r[1] + r[3] && b[3] > r[1]
}

/// Fingerprints and covered areas of `scene`'s items, into `out`. `ink`
/// gives a text run's inked box in its own space (x0, y0, x1, y1), None
/// when it draws nothing.
pub fn infos(scene: &Scene, ink: &mut dyn FnMut(&TextRun) -> Option<[f32; 4]>, out: &mut Vec<Info>) {
    out.clear();
    let screen = [0.0, 0.0, scene.w, scene.h];
    let mut clip = screen;
    let mut clip_h = 0u64;
    for it in &scene.items {
        let (mut h, bbox) = match it {
            Item::Clip(r) => {
                let mut h = Fnv::new(1);
                match r {
                    Some(r) => {
                        clip = meet(screen, [r[0], r[1], r[0] + r[2], r[1] + r[3]]);
                        r.iter().for_each(|x| h.f(*x));
                    }
                    None => clip = screen,
                }
                clip_h = h.finish();
                out.push(Info { hash: clip_h, bbox: [0.0; 4] });
                continue;
            }
            Item::Fill { pts, color, convex } => {
                let mut h = Fnv::new(2 + *convex as u64);
                h.pts(pts);
                h.color(*color);
                (h, bounds(pts, MARGIN))
            }
            Item::Stroke { pts, closed, width, color } => {
                let mut h = Fnv::new(4 + *closed as u64);
                h.pts(pts);
                h.f(*width);
                h.color(*color);
                // square joins reach half the width out along the segment
                (h, bounds(pts, width * 0.75 + MARGIN))
            }
            Item::Shape { shape, xf, color } => {
                let mut h = Fnv::new(6);
                h.write_u64(std::sync::Arc::as_ptr(shape) as usize as u64);
                [xf.a, xf.b, xf.c, xf.d, xf.e, xf.f].iter().for_each(|x| h.f(*x));
                h.color(*color);
                let b = bounds(&shape.tris, 0.0);
                let q = [(b[0], b[1]), (b[2], b[1]), (b[2], b[3]), (b[0], b[3])].map(|p| xf.apply(p));
                (h, bounds(&q, MARGIN))
            }
            Item::Text(r) => {
                let mut h = Fnv::new(7);
                r.s.hash(&mut h);
                r.face.hash(&mut h);
                [r.size, r.x, r.base, r.tracking, r.xf.a, r.xf.b, r.xf.c, r.xf.d, r.xf.e, r.xf.f].iter().for_each(|x| h.f(*x));
                h.color(r.color);
                let b = match ink(r) {
                    Some(b) => {
                        let q = [(b[0], b[1]), (b[2], b[1]), (b[2], b[3]), (b[0], b[3])].map(|p| r.xf.apply(p));
                        bounds(&q, MARGIN + 1.0)
                    }
                    None => [0.0; 4],
                };
                (h, b)
            }
            Item::Image { src, quad, uv, alpha } => {
                let mut h = Fnv::new(8);
                src.hash(&mut h);
                h.pts(quad);
                uv.iter().for_each(|x| h.f(*x));
                h.f(*alpha);
                (h, bounds(quad, MARGIN))
            }
        };
        h.write_u64(clip_h);
        let b = meet(bbox, clip);
        out.push(Info { hash: h.finish(), bbox: if is_empty(&b) { [0.0; 4] } else { b } });
    }
}

/// Where two frames differ, on a grid of CELL px cells.
pub struct Grid {
    cols: usize,
    rows: usize,
    w: f32,
    h: f32,
    on: Vec<bool>,
}

const CELL: f32 = 32.0;
/// At most this many rectangles (each is a scissored pass over the items).
const MAX_RECTS: usize = 6;
/// More of the screen than this: just draw it all.
const MOST: f32 = 0.6;

impl Grid {
    pub fn new(w: f32, h: f32) -> Grid {
        let (cols, rows) = ((w / CELL).ceil() as usize, (h / CELL).ceil() as usize);
        Grid { cols, rows, w, h, on: vec![false; cols * rows] }
    }

    /// Mark the cells `b` (x0, y0, x1, y1) touches.
    pub fn add(&mut self, b: &[f32; 4]) {
        if is_empty(b) {
            return;
        }
        let c0 = (b[0] / CELL).floor().max(0.0) as usize;
        let r0 = (b[1] / CELL).floor().max(0.0) as usize;
        let c1 = ((b[2] / CELL).ceil() as usize).min(self.cols);
        let r1 = ((b[3] / CELL).ceil() as usize).min(self.rows);
        for r in r0..r1 {
            for c in c0..c1 {
                self.on[r * self.cols + c] = true;
            }
        }
    }

    /// The differences between what a buffer shows (`old`) and the new
    /// frame (`new`). Items that kept their place in the list are compared
    /// one to one; otherwise the common start and end are skipped and the
    /// rest of both counts.
    pub fn diff(&mut self, old: &[Info], new: &[Info]) {
        if old.len() == new.len() {
            for (a, b) in old.iter().zip(new) {
                if a.hash != b.hash {
                    self.add(&a.bbox);
                    self.add(&b.bbox);
                }
            }
            return;
        }
        let pre = old.iter().zip(new).take_while(|(a, b)| a.hash == b.hash).count();
        let suf = old[pre..].iter().rev().zip(new[pre..].iter().rev()).take_while(|(a, b)| a.hash == b.hash).count();
        for i in &old[pre..old.len() - suf] {
            self.add(&i.bbox);
        }
        for i in &new[pre..new.len() - suf] {
            self.add(&i.bbox);
        }
    }

    /// The marked cells as at most MAX_RECTS rectangles (x, y, w, h);
    /// None when that is most of the screen (draw everything).
    pub fn rects(&self) -> Option<Vec<[f32; 4]>> {
        // runs of cells in each row, merged down while a run repeats
        let mut done: Vec<[usize; 4]> = Vec::new(); // c0, r0, c1, r1 (exclusive)
        let mut open: Vec<[usize; 4]> = Vec::new();
        for r in 0..self.rows {
            let mut runs = Vec::new();
            let mut c = 0;
            while c < self.cols {
                if self.on[r * self.cols + c] {
                    let s = c;
                    while c < self.cols && self.on[r * self.cols + c] {
                        c += 1;
                    }
                    runs.push((s, c));
                } else {
                    c += 1;
                }
            }
            let mut next = Vec::new();
            for (s, e) in runs {
                match open.iter().position(|o| o[0] == s && o[2] == e) {
                    Some(i) => {
                        let mut o = open.swap_remove(i);
                        o[3] = r + 1;
                        next.push(o);
                    }
                    None => next.push([s, r, e, r + 1]),
                }
            }
            done.append(&mut open);
            open = next;
        }
        done.append(&mut open);
        if done.len() > 64 {
            let b = done.iter().fold([usize::MAX, usize::MAX, 0, 0], |b, r| [b[0].min(r[0]), b[1].min(r[1]), b[2].max(r[2]), b[3].max(r[3])]);
            done = vec![b];
        }
        // too many: merge the pair that adds the least area
        let area = |r: &[usize; 4]| (r[2] - r[0]) * (r[3] - r[1]);
        let join = |a: &[usize; 4], b: &[usize; 4]| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])];
        while done.len() > MAX_RECTS {
            let mut best = (usize::MAX, 0, 0);
            for i in 0..done.len() {
                for j in i + 1..done.len() {
                    let cost = area(&join(&done[i], &done[j])) - area(&done[i]) - area(&done[j]);
                    if cost < best.0 {
                        best = (cost, i, j);
                    }
                }
            }
            let (_, i, j) = best;
            let b = done.swap_remove(j);
            done[i] = join(&done[i], &b);
        }
        // merged rectangles may overlap: count each cell once at most
        let total: usize = done.iter().map(area).sum();
        if total as f32 > MOST * (self.cols * self.rows) as f32 {
            return None;
        }
        Some(
            done.iter()
                .map(|r| {
                    let (x0, y0) = (r[0] as f32 * CELL, r[1] as f32 * CELL);
                    let (x1, y1) = ((r[2] as f32 * CELL).min(self.w), (r[3] as f32 * CELL).min(self.h));
                    [x0, y0, x1 - x0, y1 - y0]
                })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::Scene;
    use mortar_ui::gfx::Color;

    fn rect_item(x: f32, y: f32, w: f32, h: f32, c: Color) -> Item {
        Item::Fill { pts: vec![(x, y), (x + w, y), (x + w, y + h), (x, y + h)], color: c, convex: true }
    }

    fn scene(items: Vec<Item>) -> Scene {
        let mut s = Scene::new(1024.0, 768.0, Color(0, 0, 0, 255));
        s.items = items;
        s
    }

    fn info(s: &Scene) -> Vec<Info> {
        let mut v = Vec::new();
        infos(s, &mut |_| None, &mut v);
        v
    }

    #[test]
    fn a_moved_tile_damages_where_it_was_and_where_it_is() {
        let red = Color(255, 0, 0, 255);
        let a = scene(vec![rect_item(0.0, 0.0, 1024.0, 768.0, Color(9, 9, 9, 255)), rect_item(800.0, 100.0, 24.0, 24.0, red)]);
        let b = scene(vec![rect_item(0.0, 0.0, 1024.0, 768.0, Color(9, 9, 9, 255)), rect_item(900.0, 300.0, 24.0, 24.0, red)]);
        let mut g = Grid::new(1024.0, 768.0);
        g.diff(&info(&a), &info(&b));
        let r = g.rects().unwrap();
        assert_eq!(r.len(), 2, "{r:?}");
        assert!(r.iter().any(|r| touches(&[800.0, 100.0, 824.0, 124.0], r)));
        assert!(r.iter().any(|r| touches(&[900.0, 300.0, 924.0, 324.0], r)));
        assert!(r.iter().all(|r| r[2] * r[3] <= 64.0 * 64.0));
        // the same frame: nothing to draw
        let mut g = Grid::new(1024.0, 768.0);
        g.diff(&info(&a), &info(&a));
        assert!(g.rects().unwrap().is_empty());
    }

    #[test]
    fn clips_bound_the_damage_and_count_in_the_fingerprint() {
        let red = Color(255, 0, 0, 255);
        let a = scene(vec![Item::Clip(Some([772.0, 0.0, 252.0, 768.0])), rect_item(0.0, 0.0, 1024.0, 40.0, red), Item::Clip(None)]);
        let i = info(&a);
        assert_eq!(i[1].bbox[0], 772.0, "clipped");
        let b = scene(vec![Item::Clip(Some([700.0, 0.0, 324.0, 768.0])), rect_item(0.0, 0.0, 1024.0, 40.0, red), Item::Clip(None)]);
        assert_ne!(info(&b)[1].hash, i[1].hash, "the same rect under another clip looks different");
    }

    #[test]
    fn inserted_items_damage_only_themselves() {
        let c = Color(1, 2, 3, 255);
        let mut items: Vec<Item> = (0..20).map(|k| rect_item(k as f32 * 40.0, 0.0, 30.0, 30.0, c)).collect();
        let a = scene(items.clone());
        items.insert(10, rect_item(500.0, 500.0, 10.0, 10.0, c));
        let b = scene(items);
        let mut g = Grid::new(1024.0, 768.0);
        g.diff(&info(&a), &info(&b));
        let r = g.rects().unwrap();
        assert_eq!(r, vec![[480.0, 480.0, 32.0, 32.0]]);
    }

    #[test]
    fn scattered_changes_merge_into_a_few_rects_or_a_full_redraw() {
        let mut g = Grid::new(1024.0, 768.0);
        // a diagonal band of 40 small changes
        for k in 0..40 {
            let x = k as f32 * 25.0;
            g.add(&[x, x * 0.7, x + 20.0, x * 0.7 + 20.0]);
        }
        let r = g.rects().unwrap();
        assert!(r.len() <= MAX_RECTS);
        for k in 0..40 {
            let x = k as f32 * 25.0;
            assert!(r.iter().any(|r| touches(&[x, x * 0.7, x + 20.0, x * 0.7 + 20.0], r)), "change {k} is covered");
        }
        let mut g = Grid::new(1024.0, 768.0);
        g.add(&[0.0, 0.0, 1024.0, 600.0]);
        assert!(g.rects().is_none(), "most of the screen: draw it all");
    }
}
