// SPDX-License-Identifier: GPL-2.0-or-later
//! What's using the space: a walk that adds up each child of a folder (never
//! following links, never leaving the disk), and the squarified treemap the
//! storage screen draws it with.

use crate::listing::{Kind, kind_of};
use mortar_ui::gfx::Rect;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileKind {
    Folder,
    File(Kind),
    Free,
    System,
    /// Many small things, gathered.
    Others(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub name: String,
    pub path: Option<PathBuf>,
    pub size: u64,
    pub kind: TileKind,
    /// What a folder is mostly full of, if it's mostly one thing.
    pub tint: Option<Kind>,
}

#[derive(Clone, Debug)]
pub enum ScanUpdate {
    /// So far, biggest first.
    Partial(Vec<Tile>),
    Done(Vec<Tile>),
    Failed(String),
}

static APPARENT: AtomicBool = AtomicBool::new(false);

/// Count a file's length instead of its blocks: the preview's sample files
/// are mostly holes.
pub fn apparent_sizes(on: bool) {
    APPARENT.store(on, Ordering::Relaxed);
}

const KINDS: [Kind; 9] = [Kind::Folder, Kind::Image, Kind::Video, Kind::Audio, Kind::Game, Kind::Save, Kind::Archive, Kind::Text, Kind::Other];

/// Bytes on disk under `p`, added to `tally` by kind of file.
pub fn usage(p: &Path, dev: u64, games: &[String], tally: &mut [u64; 9], stop: &dyn Fn() -> bool) -> u64 {
    let Ok(m) = fs::symlink_metadata(p) else { return 0 };
    if m.dev() != dev {
        return 0;
    }
    let mut n = if APPARENT.load(Ordering::Relaxed) { m.len() } else { m.blocks() * 512 };
    if m.is_dir() {
        if !stop()
            && let Ok(rd) = fs::read_dir(p)
        {
            for e in rd.flatten() {
                n += usage(&e.path(), dev, games, tally, stop);
            }
        }
    } else if let Some(name) = p.file_name() {
        let k = kind_of(name, games);
        if let Some(i) = KINDS.iter().position(|x| *x == k) {
            tally[i] += n;
        }
    }
    n
}

/// The kind that's more than half of it, if one is.
fn dominant(tally: &[u64; 9]) -> Option<Kind> {
    let total: u64 = tally.iter().sum();
    let (i, top) = tally.iter().enumerate().max_by_key(|(_, v)| **v)?;
    (total > 0 && *top * 2 > total).then_some(KINDS[i])
}

fn sort_tiles(v: &mut [Tile]) {
    v.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
}

/// Add up every child of `dir`, sending what's known every 300 ms.
pub fn scan_dir(dir: &Path, games: &[String], stop: &dyn Fn() -> bool, emit: &mut dyn FnMut(ScanUpdate)) {
    let rd = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) => return emit(ScanUpdate::Failed(crate::ops::plain(&e))),
    };
    let dev = fs::symlink_metadata(dir).map(|m| m.dev()).unwrap_or(0);
    let mut children: Vec<_> = rd.flatten().collect();
    // quick wins first: the files, then the folders
    children.sort_by_key(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false));
    let mut tiles = Vec::with_capacity(children.len());
    let mut last = Instant::now();
    for e in children {
        if stop() {
            return;
        }
        let p = e.path();
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let mut tally = [0u64; 9];
        let size = usage(&p, dev, games, &mut tally, stop);
        let name = crate::listing::display(&e.file_name());
        let kind = if is_dir { TileKind::Folder } else { TileKind::File(kind_of(&e.file_name(), games)) };
        tiles.push(Tile { name, path: Some(p), size, kind, tint: if is_dir { dominant(&tally) } else { None } });
        if last.elapsed() > Duration::from_millis(300) {
            last = Instant::now();
            let mut v = tiles.clone();
            sort_tiles(&mut v);
            emit(ScanUpdate::Partial(v));
        }
    }
    sort_tiles(&mut tiles);
    emit(ScanUpdate::Done(tiles));
}

/// The storage overview: your folders against the disk's total, with what's
/// left as "System" and "Free". Doesn't walk the Nix store.
pub fn scan_overview(roots: &[(String, PathBuf)], disk: &Path, games: &[String], stop: &dyn Fn() -> bool, emit: &mut dyn FnMut(ScanUpdate)) {
    let dev = fs::metadata(disk).map(|m| m.dev()).unwrap_or(0);
    let (free, total) = crate::ops::free_space(disk).unwrap_or((0, 0));
    let mut tiles = Vec::new();
    let mut known = 0;
    let mut seen: Vec<PathBuf> = Vec::new();
    for (name, p) in roots {
        if stop() {
            return;
        }
        // a root inside another root counts once
        if seen.iter().any(|s| p.starts_with(s)) {
            continue;
        }
        seen.push(p.clone());
        let same_disk = fs::metadata(p).map(|m| m.dev() == dev).unwrap_or(false);
        if !same_disk {
            continue;
        }
        let mut tally = [0u64; 9];
        let size = usage(p, dev, games, &mut tally, stop);
        known += size;
        tiles.push(Tile { name: name.clone(), path: Some(p.clone()), size, kind: TileKind::Folder, tint: dominant(&tally) });
        let mut v = tiles.clone();
        sort_tiles(&mut v);
        emit(ScanUpdate::Partial(v));
    }
    let used = total.saturating_sub(free);
    tiles.push(Tile { name: "System".into(), path: None, size: used.saturating_sub(known), kind: TileKind::System, tint: None });
    tiles.push(Tile { name: "Free".into(), path: None, size: free, kind: TileKind::Free, tint: None });
    sort_tiles(&mut tiles);
    emit(ScanUpdate::Done(tiles));
}

/// Gather what would be too small to see (under `min_share` of the total, or
/// past `max` tiles) into one "others" tile. Input sorted, biggest first.
pub fn gather(mut v: Vec<Tile>, min_share: f64, max: usize) -> Vec<Tile> {
    let total: u64 = v.iter().map(|t| t.size).sum();
    let cut = v.iter().position(|t| (t.size as f64) < total as f64 * min_share).unwrap_or(v.len()).min(max);
    if v.len() <= cut + 1 {
        return v;
    }
    let rest = v.split_off(cut);
    let n = rest.len();
    let size = rest.iter().map(|t| t.size).sum();
    v.push(Tile { name: format!("{n} more"), path: None, size, kind: TileKind::Others(n), tint: None });
    v
}

/// Squarified treemap (Bruls, Huizing, van Wijk): tiles as near square as the
/// sizes allow. `sizes` sorted, biggest first; one rect per size, in order.
pub fn squarify(sizes: &[f64], area: Rect) -> Vec<Rect> {
    let total: f64 = sizes.iter().sum();
    if sizes.is_empty() || total <= 0.0 || area.w <= 0.0 || area.h <= 0.0 {
        return vec![Rect::default(); sizes.len()];
    }
    let scale = (area.w as f64 * area.h as f64) / total;
    let scaled: Vec<f64> = sizes.iter().map(|s| s * scale).collect();
    let mut out = Vec::with_capacity(sizes.len());
    let (mut x, mut y, mut w, mut h) = (area.x as f64, area.y as f64, area.w as f64, area.h as f64);
    let worst = |row: &[f64], side: f64| -> f64 {
        let s: f64 = row.iter().sum();
        let (mx, mn) = row.iter().fold((0.0f64, f64::MAX), |(a, b), v| (a.max(*v), b.min(*v)));
        if mn <= 0.0 {
            return f64::MAX;
        }
        ((side * side * mx) / (s * s)).max((s * s) / (side * side * mn))
    };
    let mut i = 0;
    while i < scaled.len() {
        let side = w.min(h);
        let mut j = i + 1;
        while j < scaled.len() && worst(&scaled[i..=j], side) <= worst(&scaled[i..j], side) {
            j += 1;
        }
        let row = &scaled[i..j];
        let s: f64 = row.iter().sum();
        if w >= h {
            // a column at the left
            let cw = if h > 0.0 { s / h } else { 0.0 };
            let mut cy = y;
            for v in row {
                let ch = if cw > 0.0 { v / cw } else { 0.0 };
                out.push(Rect::new(x as f32, cy as f32, cw as f32, ch as f32));
                cy += ch;
            }
            x += cw;
            w -= cw;
        } else {
            // a row along the top
            let rh = if w > 0.0 { s / w } else { 0.0 };
            let mut cx = x;
            for v in row {
                let cw = if rh > 0.0 { v / rh } else { 0.0 };
                out.push(Rect::new(cx as f32, y as f32, cw as f32, rh as f32));
                cx += cw;
            }
            y += rh;
            h -= rh;
        }
        i = j;
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

/// The nearest rect that way from `from`: for the D-pad on a mosaic.
pub fn nearest(rects: &[Rect], from: usize, d: Dir) -> Option<usize> {
    let a = rects.get(from)?;
    let (ax, ay) = (a.x + a.w / 2.0, a.y + a.h / 2.0);
    let mut best: Option<(f32, usize)> = None;
    for (i, r) in rects.iter().enumerate() {
        if i == from || r.w <= 0.0 || r.h <= 0.0 {
            continue;
        }
        let (bx, by) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        // past the edge that way, and how far off the line it is
        let (along, across, overlap) = match d {
            Dir::Up => (a.y - r.bottom(), bx - ax, r.x < a.right() && r.right() > a.x),
            Dir::Down => (r.y - a.bottom(), bx - ax, r.x < a.right() && r.right() > a.x),
            Dir::Left => (a.x - r.right(), by - ay, r.y < a.bottom() && r.bottom() > a.y),
            Dir::Right => (r.x - a.right(), by - ay, r.y < a.bottom() && r.bottom() > a.y),
        };
        if along < -2.0 {
            continue;
        }
        let score = along.max(0.0) + across.abs() * if overlap { 0.25 } else { 1.5 } + if overlap { 0.0 } else { 300.0 };
        if best.is_none_or(|(s, _)| score < s) {
            best = Some((score, i));
        }
    }
    best.map(|b| b.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn treemap_fills_the_area_without_overlap() {
        let sizes = [600.0, 300.0, 120.0, 90.0, 40.0, 30.0, 12.0, 8.0];
        let area = Rect::new(0.0, 80.0, 1024.0, 600.0);
        let r = squarify(&sizes, area);
        let sum: f64 = r.iter().map(|r| (r.w * r.h) as f64).sum();
        assert!((sum - 1024.0 * 600.0).abs() < 2.0, "{sum}");
        for (i, a) in r.iter().enumerate() {
            assert!(a.x >= -0.5 && a.right() <= 1024.5 && a.y >= 79.5 && a.bottom() <= 680.5, "{a:?}");
            for b in &r[i + 1..] {
                let ov = (a.right().min(b.right()) - a.x.max(b.x)).max(0.0) * (a.bottom().min(b.bottom()) - a.y.max(b.y)).max(0.0);
                assert!(ov < 1.0, "{a:?} overlaps {b:?}");
            }
        }
        // sized by what's using the space
        let ratio = (r[0].w * r[0].h) / (r[1].w * r[1].h);
        assert!((ratio - 2.0).abs() < 0.01);
    }

    #[test]
    fn treemap_edge_cases() {
        assert!(squarify(&[], Rect::new(0.0, 0.0, 10.0, 10.0)).is_empty());
        let zero = squarify(&[0.0, 0.0], Rect::new(0.0, 0.0, 10.0, 10.0));
        assert_eq!(zero.len(), 2);
    }

    #[test]
    fn dpad_moves_to_the_neighbour() {
        // | 0 | 1 |
        // | 0 | 2 |
        let r = [Rect::new(0.0, 0.0, 100.0, 200.0), Rect::new(100.0, 0.0, 100.0, 100.0), Rect::new(100.0, 100.0, 100.0, 100.0)];
        assert_eq!(nearest(&r, 0, Dir::Right), Some(1));
        assert_eq!(nearest(&r, 1, Dir::Down), Some(2));
        assert_eq!(nearest(&r, 2, Dir::Up), Some(1));
        assert_eq!(nearest(&r, 2, Dir::Left), Some(0));
        assert_eq!(nearest(&r, 0, Dir::Left), None);
    }

    #[test]
    fn small_things_gather() {
        let t = |n: &str, s| Tile { name: n.into(), path: None, size: s, kind: TileKind::Folder, tint: None };
        let v = vec![t("a", 1000), t("b", 500), t("c", 1), t("d", 1), t("e", 1)];
        let g = gather(v, 0.01, 40);
        assert_eq!(g.len(), 3);
        assert_eq!(g[2].kind, TileKind::Others(3));
        assert_eq!(g[2].size, 3);
        // one small thing stays itself
        let g = gather(vec![t("a", 1000), t("c", 1)], 0.01, 40);
        assert_eq!(g[1].name, "c");
    }

    #[test]
    fn scan_counts_and_never_follows_links() {
        let d = crate::ops_tests::temp("scan");
        let outside = crate::ops_tests::temp("scan-outside");
        fs::write(outside.join("big"), vec![1u8; 1 << 20]).unwrap();
        fs::create_dir(d.join("folder")).unwrap();
        fs::write(d.join("folder/data"), vec![1u8; 200_000]).unwrap();
        std::os::unix::fs::symlink(&outside, d.join("link")).unwrap();
        fs::write(d.join("small.txt"), "hi").unwrap();
        let mut done = None;
        scan_dir(&d, &[], &|| false, &mut |u| {
            if let ScanUpdate::Done(v) = u {
                done = Some(v);
            }
        });
        let v = done.unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].name, "folder");
        assert_eq!(v[0].kind, TileKind::Folder);
        assert!(v[0].size >= 200_000);
        assert_eq!(v[0].tint, Some(Kind::Other), "mostly one kind of thing");
        let link = v.iter().find(|t| t.name == "link").unwrap();
        assert!(link.size < 1 << 20, "a link counts as a link, {}", link.size);
        fs::remove_dir_all(d).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }
}
