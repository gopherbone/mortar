// SPDX-License-Identifier: GPL-2.0-or-later
//! The picture language: solid tiles on a grid with even grout, the same as
//! the shell's patterns (mortar-shell/src/patterns.rs). A glyph for each kind
//! of file, a mosaic of a folder's contents, a file's first bytes as tiles,
//! and the motifs on the Places tiles. Everything here is static: it's drawn
//! once when something changes.

use crate::listing::Kind;
use mortar_ui::gfx::{Color, Rect, Ui};
use mortar_ui::theme;

pub const PINK: Color = Color::hex(0xff77a8);
pub const DARK: Color = Color::hex(0x1d1d1d);
pub const LIGHT: Color = Color::hex(0xe1ebf7);

/// A kind's colour, and the ink that reads on it.
pub fn kind_colors(k: Kind) -> (Color, Color) {
    let t = theme::get();
    match k {
        Kind::Folder => (t.lavender, DARK),
        Kind::Image | Kind::Video => (PINK, DARK),
        Kind::Game => (t.lime, DARK),
        Kind::Save => (t.yellow, DARK),
        Kind::Audio | Kind::Archive => (t.orange, DARK),
        Kind::Text | Kind::Other => (t.ground_2, t.line),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Kind(Kind),
    Link,
    Lock,
    Check,
    Broken,
}

/// 6 x 6 bitmaps, read as text so they can be edited by eye.
fn bitmap(g: Glyph) -> [&'static str; 6] {
    match g {
        Glyph::Kind(Kind::Folder) => ["......", "###...", "######", "######", "######", "######"],
        Glyph::Kind(Kind::Image) => ["....##", "....##", ".#....", "###.#.", "######", "######"],
        Glyph::Kind(Kind::Video) => ["##....", "####..", "######", "######", "####..", "##...."],
        Glyph::Kind(Kind::Audio) => ["....#.", "..#.#.", "..#.#.", "#.#.#.", "#.#.#.", "#.#.#."],
        Glyph::Kind(Kind::Game) => ["..##..", "..##..", "######", "######", "..##..", "..##.."],
        Glyph::Kind(Kind::Save) => ["..##..", ".####.", "######", "######", ".####.", "..##.."],
        Glyph::Kind(Kind::Archive) => ["##..##", "##..##", "..##..", "..##..", "##..##", "##..##"],
        Glyph::Kind(Kind::Text) => ["######", "......", "#####.", "......", "######", "###..."],
        Glyph::Kind(Kind::Other) => ["####..", "#####.", "######", "######", "######", "######"],
        Glyph::Link => ["..####", "....##", "...#.#", "..#..#", ".#....", "#....."],
        Glyph::Lock => ["..##..", ".#..#.", ".#..#.", "######", "######", "######"],
        Glyph::Check => [".....#", "....##", "#..##.", "####..", ".##...", "......"],
        Glyph::Broken => ["##..##", ".####.", "..##..", "..##..", ".####.", "##..##"],
    }
}

/// A glyph of `cell` px squares, `grout` apart, at (x, y).
pub fn glyph(ui: &mut Ui, g: Glyph, x: f32, y: f32, cell: f32, grout: f32, c: Color) {
    for (r, row) in bitmap(g).iter().enumerate() {
        for (col, ch) in row.bytes().enumerate() {
            if ch == b'#' {
                ui.fill_block(Rect::new(x + col as f32 * (cell + grout), y + r as f32 * (cell + grout), cell, cell), c);
            }
        }
    }
}

/// Width of a glyph at this size.
pub fn glyph_w(cell: f32, grout: f32) -> f32 {
    6.0 * cell + 5.0 * grout
}

/// A tiny repeatable random number generator, seeded by a name: a folder gets
/// the same picture every time.
#[derive(Clone, Copy)]
pub struct Rng(u32);

impl Rng {
    pub fn seed(b: &[u8]) -> Rng {
        let mut h = 0x811c_9dc5u32;
        for c in b {
            h = (h ^ *c as u32).wrapping_mul(0x0100_0193);
        }
        Rng(h | 1)
    }
    pub fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
}

/// The cells of an n x n grid in a square spiral from the middle.
pub fn spiral(n: i32, turn: u32) -> Vec<(i32, i32)> {
    let (mut x, mut y) = (n / 2, n / 2);
    let mut out = Vec::with_capacity((n * n) as usize);
    let dirs = [(1, 0), (0, 1), (-1, 0), (0, -1)];
    let mut d = (turn % 4) as usize;
    let (mut run, mut left, mut flips) = (1, 1, 0);
    while out.len() < (n * n) as usize {
        if x >= 0 && y >= 0 && x < n && y < n {
            out.push((x, y));
        }
        x += dirs[d].0;
        y += dirs[d].1;
        left -= 1;
        if left == 0 {
            d = (d + 1) % 4;
            flips += 1;
            if flips % 2 == 0 {
                run += 1;
            }
            left = run;
        }
    }
    out
}

/// One tile per thing in the folder, up to the grid; more than that and each
/// tile stands for several. Kinds stay in proportion.
pub fn folder_cells(by_kind: &[(Kind, usize)], cells: usize) -> Vec<Kind> {
    let total: usize = by_kind.iter().map(|k| k.1).sum();
    if total == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (i, (k, n)) in by_kind.iter().enumerate() {
        let share = if total <= cells {
            *n
        } else {
            // at least one, so nothing that's there goes missing
            let left = cells.saturating_sub(out.len());
            let rest_kinds = by_kind.len() - i - 1;
            ((*n as f32 * cells as f32 / total as f32).round() as usize).max(1).min(left.saturating_sub(rest_kinds).max(1))
        };
        for _ in 0..share {
            out.push(*k);
        }
    }
    out.truncate(cells);
    out
}

/// A folder as a picture of what's inside: its things in a spiral from the
/// middle, kinds grouped, turned by the folder's name.
pub fn folder_mosaic(ui: &mut Ui, r: Rect, by_kind: &[(Kind, usize)], name: &[u8]) {
    let back = DARK;
    let n = ((r.w + 4.0) / 28.0).floor() as i32;
    let pitch = (r.w + 4.0) / n as f32;
    let cells = spiral(n, Rng::seed(name).next() >> 7);
    let list = folder_cells(by_kind, (n * n) as usize);
    ui.fill_block(r, back);
    for (i, (cx, cy)) in cells.iter().enumerate() {
        let c = match list.get(i) {
            Some(k) => {
                let (bg, ink) = kind_colors(*k);
                // neutral things in a light colour, on the dark picture
                let _ = ink;
                if *k == Kind::Text || *k == Kind::Other { LIGHT } else { bg }
            }
            None => continue,
        };
        ui.fill_block(Rect::new(r.x + *cx as f32 * pitch, r.y + *cy as f32 * pitch, pitch - 4.0, pitch - 4.0), c);
    }
}

/// A byte's colour: what kind of byte it is. Text comes out one calm colour,
/// compressed data as even confetti, a zeroed header as a gap.
pub fn byte_color(b: u8) -> Option<Color> {
    let t = theme::get();
    Some(match b {
        0 => return None,
        0xff => LIGHT,
        0x01..=0x1f | 0x7f => t.lavender,
        0x20..=0x7e => t.lime,
        0x80..=0xa9 => PINK,
        0xaa..=0xd4 => t.orange,
        _ => t.yellow,
    })
}

/// The file's first bytes, a tile each, on dark.
pub fn byte_mosaic(ui: &mut Ui, r: Rect, bytes: &[u8]) {
    let back = DARK;
    let n = ((r.w + 4.0) / 28.0).floor() as usize;
    let pitch = (r.w + 4.0) / n as f32;
    ui.fill_block(r, back);
    for (i, b) in bytes.iter().take(n * n).enumerate() {
        if let Some(c) = byte_color(*b) {
            ui.fill_block(Rect::new(r.x + (i % n) as f32 * pitch, r.y + (i / n) as f32 * pitch, pitch - 4.0, pitch - 4.0), c);
        }
    }
}

/// A quarter disc filling a square from one corner (0..4 clockwise from top left).
fn quarter(ui: &mut Ui, x: f32, y: f32, s: f32, corner: u8, c: Color) {
    let (cx, cy, a0) = match corner % 4 {
        0 => (x, y, 0.0f32),
        1 => (x + s, y, 90.0),
        2 => (x + s, y + s, 180.0),
        _ => (x, y + s, 270.0),
    };
    let mut pts = vec![(cx, cy)];
    for i in 0..=12 {
        let a = (a0 + i as f32 * 7.5).to_radians();
        pts.push((cx + s * a.cos(), cy + s * a.sin()));
    }
    ui.fill_poly(&pts, c);
}

/// Places: motifs. Each fills `r` on a grid, in `c` (and `c2`), on the tile's
/// own colour, cut to whole tiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motif {
    /// A sun of tiles rising from the bottom edge.
    Sun,
    /// The shell's zig-zag, two tiles thick.
    Zigzag,
    /// Truchet quarter discs.
    Truchet,
    /// A checker with some tiles missing.
    Chips,
    /// Stairs: a column for each depth.
    Stairs,
    /// A small treemap.
    Blocks,
    /// Tiles falling away.
    Crumble,
    /// Teeth along the bottom edge: a card's contacts.
    Card,
}

pub fn motif(ui: &mut Ui, m: Motif, r: Rect, c: Color, c2: Color, seed: &[u8]) {
    // the shell's two pitches: 28 for big tiles, 20 for small ones
    let (g, tile) = if r.h >= 150.0 { (28.0f32, 24.0f32) } else { (20.0f32, 16.0f32) };
    let cols = (r.w / g).floor() as i32;
    let rows = (r.h / g).floor() as i32;
    let x0 = r.x + r.w - cols as f32 * g + 4.0;
    let y0 = r.y + r.h - rows as f32 * g + 4.0;
    let cell = |ui: &mut Ui, cx: i32, cy: i32, col: Color| {
        if cx >= 0 && cy >= 0 && cx < cols && cy < rows {
            ui.fill_block(Rect::new(x0 + cx as f32 * g, y0 + cy as f32 * g, tile, tile), col);
        }
    };
    let mut rng = Rng::seed(seed);
    match m {
        Motif::Sun => {
            let (cx, cy) = (cols as f32 * 0.62, rows as f32 - 1.0);
            let rad = (rows as f32 * 0.72).min(cols as f32 * 0.3);
            for y in 0..rows {
                for x in 0..cols {
                    let d = (((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)) as f32).sqrt();
                    if d <= rad {
                        cell(ui, x, y, c);
                    } else if d <= rad + 1.4 {
                        cell(ui, x, y, c2);
                    } else if d <= rad + 3.2 && (x + y) % 2 == 0 && rng.next() % 3 == 0 {
                        cell(ui, x, y, c2);
                    }
                }
            }
        }
        Motif::Zigzag => {
            const Z: [i32; 4] = [0, 1, 2, 1];
            let mut band = 0;
            let mut top = 0;
            while top < rows {
                for x in 0..cols {
                    let y = top + Z[(x as usize + band * 2) % 4];
                    cell(ui, x, y, if band % 2 == 0 { c } else { c2 });
                    cell(ui, x, y + 1, if band % 2 == 0 { c } else { c2 });
                }
                top += 5;
                band += 1;
            }
        }
        Motif::Truchet => {
            let s = if r.h >= 150.0 { 52.0f32 } else { 40.0 };
            let (ncol, nrow) = ((r.w / (s + 4.0)).floor() as i32, (r.h / (s + 4.0)).floor() as i32);
            let (xs, ys) = (r.x + r.w - ncol as f32 * (s + 4.0) + 4.0, r.y + r.h - nrow as f32 * (s + 4.0) + 4.0);
            for y in 0..nrow {
                for x in 0..ncol {
                    let (px, py) = (xs + x as f32 * (s + 4.0), ys + y as f32 * (s + 4.0));
                    ui.fill_block(Rect::new(px, py, s, s), c);
                    let rad = s * 0.5;
                    if rng.next() % 2 == 0 {
                        quarter(ui, px, py, rad, 0, c2);
                        quarter(ui, px + s - rad, py + s - rad, rad, 2, c2);
                    } else {
                        quarter(ui, px + s - rad, py, rad, 1, c2);
                        quarter(ui, px, py + s - rad, rad, 3, c2);
                    }
                }
            }
        }
        Motif::Chips => {
            for y in 0..rows {
                for x in 0..cols {
                    if (x + y) % 2 == 0 && rng.next() % 5 != 0 {
                        cell(ui, x, y, if rng.next() % 6 == 0 { c2 } else { c });
                    }
                }
            }
        }
        Motif::Stairs => {
            for x in 0..cols {
                let h = (x + 1).min(rows);
                for y in 0..h {
                    cell(ui, x, rows - 1 - y, if y == h - 1 { c2 } else { c });
                }
            }
        }
        Motif::Blocks => {
            let inner = Rect::new(r.x + r.w - cols as f32 * g + 4.0, r.y + r.h - rows as f32 * g + 4.0, cols as f32 * g - 4.0, rows as f32 * g - 4.0);
            let rects = crate::storage::squarify(&[8.0, 5.0, 3.0, 2.0, 1.0, 1.0], inner);
            for (i, b) in rects.iter().enumerate() {
                let col = if i % 2 == 0 { c } else { c2 };
                ui.fill_block(Rect::new(b.x, b.y, (b.w - 4.0).max(1.0), (b.h - 4.0).max(1.0)), col);
            }
        }
        Motif::Crumble => {
            for x in 0..cols {
                for y in 0..rows {
                    let depth = rows - y;
                    let keep = depth as f32 <= 1.0 + rows as f32 * (x as f32 + 1.0) / cols as f32;
                    if keep && (y >= rows - 2 || rng.next() % 3 != 0) {
                        cell(ui, x, y, if rng.next() % 5 == 0 { c2 } else { c });
                    }
                }
            }
        }
        Motif::Card => {
            for x in 0..cols {
                cell(ui, x, rows - 1, if x % 2 == 0 { c } else { c2 });
                if x % 2 == 0 {
                    cell(ui, x, rows - 2, c);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spiral_visits_every_cell_once() {
        for n in [1, 2, 5, 11, 12] {
            for turn in 0..4 {
                let mut s = spiral(n, turn);
                assert_eq!(s.len() as i32, n * n);
                s.sort();
                s.dedup();
                assert_eq!(s.len() as i32, n * n);
            }
        }
    }

    #[test]
    fn folder_cells_keep_every_kind() {
        assert!(folder_cells(&[], 121).is_empty());
        assert_eq!(folder_cells(&[(Kind::Game, 3), (Kind::Folder, 1)], 121).len(), 4);
        let v = folder_cells(&[(Kind::Game, 100_000), (Kind::Folder, 2), (Kind::Save, 1)], 121);
        assert_eq!(v.len(), 121);
        assert!(v.contains(&Kind::Folder) && v.contains(&Kind::Save));
        assert!(v.iter().filter(|k| **k == Kind::Game).count() > 100);
    }

    #[test]
    fn a_name_always_gets_the_same_picture() {
        assert_eq!(Rng::seed(b"roms").next(), Rng::seed(b"roms").next());
        assert_ne!(Rng::seed(b"roms").next(), Rng::seed(b"saves").next());
    }
}
