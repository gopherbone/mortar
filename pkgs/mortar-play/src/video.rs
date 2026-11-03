// SPDX-License-Identifier: GPL-2.0-or-later
//! Software video: the core's frame converted to XRGB8888, rotation, the
//! scaling layout (aspect / integer / fill) and a nearest-neighbour scaler
//! for presenters without a GPU (Wayland shm, headless screenshots).

use crate::lr::ffi;
use serde::{Deserialize, Serialize};

/// A frame in XRGB8888 (one u32 per pixel, top byte ignored), tightly packed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u32>,
}

impl Frame {
    pub fn new(w: u32, h: u32) -> Frame {
        Frame { w, h, px: vec![0; (w * h) as usize] }
    }

    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }

    /// Copy a core framebuffer (`pitch` bytes per row) in pixel `format`.
    ///
    /// # Safety
    /// `data` must point to `h` rows of `pitch` bytes.
    pub unsafe fn load(&mut self, data: *const u8, w: u32, h: u32, pitch: usize, format: u32) {
        self.w = w;
        self.h = h;
        self.px.resize((w * h) as usize, 0);
        for y in 0..h as usize {
            let row = unsafe { data.add(y * pitch) };
            let out = &mut self.px[y * w as usize..(y + 1) * w as usize];
            match format {
                ffi::PIXEL_FORMAT_XRGB8888 => {
                    let src = unsafe { std::slice::from_raw_parts(row as *const u32, w as usize) };
                    out.copy_from_slice(src);
                }
                ffi::PIXEL_FORMAT_RGB565 => {
                    let src = unsafe { std::slice::from_raw_parts(row as *const u16, w as usize) };
                    for (o, &p) in out.iter_mut().zip(src) {
                        *o = rgb565(p);
                    }
                }
                _ => {
                    let src = unsafe { std::slice::from_raw_parts(row as *const u16, w as usize) };
                    for (o, &p) in out.iter_mut().zip(src) {
                        *o = rgb1555(p);
                    }
                }
            }
        }
    }

    /// Rotated by `quarters` x 90 degrees counter-clockwise (libretro
    /// SET_ROTATION semantics).
    pub fn rotated(&self, quarters: u32) -> Frame {
        let (w, h) = (self.w as usize, self.h as usize);
        match quarters % 4 {
            0 => self.clone(),
            2 => Frame { w: self.w, h: self.h, px: self.px.iter().rev().copied().collect() },
            q => {
                let mut out = Frame::new(self.h, self.w);
                for y in 0..h {
                    for x in 0..w {
                        // CCW 90: (x, y) -> (y, w-1-x); CW 90 (q=3): (h-1-y, x)
                        let (nx, ny) = if q == 1 { (y, w - 1 - x) } else { (h - 1 - y, x) };
                        out.px[ny * h + nx] = self.px[y * w + x];
                    }
                }
                out
            }
        }
    }

    /// Bilinear-free box downscale to fit within `max_w` x `max_h` (thumbnails).
    pub fn thumbnail(&self, max_w: u32, max_h: u32) -> Frame {
        if self.is_empty() {
            return self.clone();
        }
        let s = (max_w as f32 / self.w as f32).min(max_h as f32 / self.h as f32).min(1.0);
        let (tw, th) = (((self.w as f32 * s) as u32).max(1), ((self.h as f32 * s) as u32).max(1));
        let mut out = Frame::new(tw, th);
        for ty in 0..th {
            let (y0, y1) = (ty * self.h / th, ((ty + 1) * self.h / th).max(ty * self.h / th + 1));
            for tx in 0..tw {
                let (x0, x1) = (tx * self.w / tw, ((tx + 1) * self.w / tw).max(tx * self.w / tw + 1));
                let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
                for y in y0..y1.min(self.h) {
                    for x in x0..x1.min(self.w) {
                        let p = self.px[(y * self.w + x) as usize];
                        r += (p >> 16) & 255;
                        g += (p >> 8) & 255;
                        b += p & 255;
                        n += 1;
                    }
                }
                let n = n.max(1);
                out.px[(ty * tw + tx) as usize] = (r / n) << 16 | (g / n) << 8 | (b / n);
            }
        }
        out
    }

    pub fn to_rgb(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(self.px.len() * 3);
        for p in &self.px {
            v.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
        }
        v
    }

    pub fn save_png(&self, path: &std::path::Path) -> Result<(), String> {
        let f = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut enc = png::Encoder::new(std::io::BufWriter::new(f), self.w, self.h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().map_err(|e| e.to_string())?;
        wr.write_image_data(&self.to_rgb()).map_err(|e| e.to_string())
    }

    pub fn load_png(path: &std::path::Path) -> Option<Frame> {
        let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).ok()?));
        let mut r = dec.read_info().ok()?;
        let mut buf = vec![0; r.output_buffer_size()?];
        let info = r.next_frame(&mut buf).ok()?;
        let ch = match info.color_type {
            png::ColorType::Rgb => 3,
            png::ColorType::Rgba => 4,
            _ => return None,
        };
        if info.bit_depth != png::BitDepth::Eight {
            return None;
        }
        let px = buf[..info.buffer_size()]
            .chunks_exact(ch)
            .map(|c| (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32)
            .collect();
        Some(Frame { w: info.width, h: info.height, px })
    }
}

#[inline]
pub fn rgb565(p: u16) -> u32 {
    let r = ((p >> 11) & 0x1f) as u32;
    let g = ((p >> 5) & 0x3f) as u32;
    let b = (p & 0x1f) as u32;
    (r << 3 | r >> 2) << 16 | (g << 2 | g >> 4) << 8 | (b << 3 | b >> 2)
}

#[inline]
pub fn rgb1555(p: u16) -> u32 {
    let r = ((p >> 10) & 0x1f) as u32;
    let g = ((p >> 5) & 0x1f) as u32;
    let b = (p & 0x1f) as u32;
    (r << 3 | r >> 2) << 16 | (g << 3 | g >> 2) << 8 | (b << 3 | b >> 2)
}

/// How the picture is scaled (the user picks it per system or per game;
/// the system's default is in modules/packs/emulation.nix).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Filter {
    /// Bilinear-like: inside the shell the display engine scales the frame
    /// (free); elsewhere the GPU's linear filter.
    Smooth,
    /// Nearest neighbour to the full size (crisp, uneven pixels at
    /// non-integer ratios).
    #[default]
    #[serde(alias = "aspect", alias = "fill")] // the old scaling names
    Sharp,
    /// Nearest to the largest whole multiple, then a smooth finish: crisp
    /// and even.
    SharpBilinear,
    /// Whole multiples only, centred, black borders: pixel perfect.
    #[serde(alias = "integer-aspect")]
    Integer,
}

/// The picture's shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Fit {
    /// The core's display aspect ratio.
    #[default]
    Fit,
    /// The whole screen (Integer: whole multiples on each axis).
    Stretch,
}

/// Filter + fit: what the Display setting chooses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct View {
    pub filter: Filter,
    pub fit: Fit,
}

impl View {
    /// The choices, in the order ◀▶ steps through them.
    pub const ALL: [View; 7] = [
        View { filter: Filter::SharpBilinear, fit: Fit::Fit },
        View { filter: Filter::Smooth, fit: Fit::Fit },
        View { filter: Filter::Sharp, fit: Fit::Fit },
        View { filter: Filter::Integer, fit: Fit::Fit },
        View { filter: Filter::SharpBilinear, fit: Fit::Stretch },
        View { filter: Filter::Smooth, fit: Fit::Stretch },
        View { filter: Filter::Sharp, fit: Fit::Stretch },
    ];

    pub fn label(self) -> String {
        let f = match self.filter {
            Filter::Smooth => "Smooth",
            Filter::Sharp => "Sharp",
            Filter::SharpBilinear => "Sharp bilinear",
            Filter::Integer => "Integer",
        };
        if self.fit == Fit::Stretch { format!("{f} · stretch") } else { f.to_string() }
    }

    /// "sharp-bilinear/fit" (the saved form).
    pub fn key(self) -> String {
        let f = match self.filter {
            Filter::Smooth => "smooth",
            Filter::Sharp => "sharp",
            Filter::SharpBilinear => "sharp-bilinear",
            Filter::Integer => "integer",
        };
        format!("{f}/{}", if self.fit == Fit::Stretch { "stretch" } else { "fit" })
    }

    pub fn parse(s: &str) -> Option<View> {
        let (f, a) = s.split_once('/').unwrap_or((s, "fit"));
        let filter = serde_json::from_value(serde_json::Value::String(f.into())).ok()?;
        let fit = serde_json::from_value(serde_json::Value::String(a.into())).ok()?;
        Some(View { filter, fit })
    }

    /// The next (`step` > 0) or previous choice.
    pub fn step(self, step: i32) -> View {
        let n = View::ALL.len() as i32;
        let i = View::ALL.iter().position(|v| *v == self).map_or(0, |i| i as i32);
        View::ALL[(i + step).rem_euclid(n) as usize]
    }
}

/// Destination rectangle on the screen (whole pixels).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dst {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Where a `fw` x `fh` frame with display aspect `aspect` (0 = square
/// pixels) goes on a `sw` x `sh` screen.
pub fn layout(view: View, fw: u32, fh: u32, aspect: f32, sw: u32, sh: u32) -> Dst {
    if fw == 0 || fh == 0 || sw == 0 || sh == 0 {
        return Dst::default();
    }
    let aspect = if aspect > 0.0 { aspect } else { fw as f32 / fh as f32 };
    let fit_aspect = |a: f32| -> (u32, u32) {
        let w = sw as f32;
        let h = w / a;
        if h <= sh as f32 { (sw, (h.round() as u32).min(sh)) } else { (((sh as f32 * a).round() as u32).min(sw), sh) }
    };
    let (w, h) = match (view.filter, view.fit) {
        (Filter::Integer, Fit::Fit) => {
            // whole multiples vertically (even scanlines), the width from
            // the aspect ratio (square-pixel systems: whole multiples too)
            let mut k = sh / fh;
            while k > 0 && (fh as f32 * k as f32 * aspect).round() as u32 > sw {
                k -= 1;
            }
            if k == 0 { fit_aspect(aspect) } else { (((fh * k) as f32 * aspect).round() as u32, fh * k) }
        }
        (Filter::Integer, Fit::Stretch) => {
            let (kx, ky) = (sw / fw, sh / fh);
            if kx == 0 || ky == 0 { (sw, sh) } else { (fw * kx, fh * ky) }
        }
        (_, Fit::Stretch) => (sw, sh),
        (_, Fit::Fit) => fit_aspect(aspect),
    };
    Dst { x: (sw - w) / 2, y: (sh - h) / 2, w, h }
}

/// Sharp bilinear: the whole multiple the frame is pre-scaled by (nearest)
/// before the smooth finish to `dst`; 1 means no pre-scale.
pub fn prescale(filter: Filter, fw: u32, fh: u32, dst: Dst) -> u32 {
    match filter {
        Filter::SharpBilinear if fw > 0 && fh > 0 => (dst.w / fw).min(dst.h / fh).max(1),
        _ => 1,
    }
}

/// Nearest-neighbour scaler into a 32-bit destination buffer (`stride` in
/// pixels). Each source row is gathered once into a cached row and copied to
/// every screen row that repeats it, so the destination is only ever
/// written, never read: it may be write-combined memory (a scanout dma-buf),
/// where reads are very slow.
#[derive(Default)]
pub struct Scaler {
    key: (u32, u32, Dst),
    xmap: Vec<u32>,
    ymap: Vec<u32>,
    row: Vec<u32>,
}

impl Scaler {
    pub fn scale(&mut self, f: &Frame, dst: Dst, out: &mut [u32], stride: usize) {
        if f.is_empty() || dst.w == 0 || dst.h == 0 {
            return;
        }
        if self.key != (f.w, f.h, dst) {
            self.key = (f.w, f.h, dst);
            // sample at pixel centres
            let map = |n: u32, src: u32| -> Vec<u32> {
                (0..n).map(|i| (((2 * i as u64 + 1) * src as u64) / (2 * n as u64)) as u32).map(|v| v.min(src - 1)).collect()
            };
            self.xmap = map(dst.w, f.w);
            self.ymap = map(dst.h, f.h);
            self.row = vec![0; dst.w as usize];
        }
        let mut prev = None;
        for (dy, &sy) in self.ymap.iter().enumerate() {
            if prev != Some(sy) {
                let src = &f.px[(sy * f.w) as usize..((sy + 1) * f.w) as usize];
                for (d, &sx) in self.row.iter_mut().zip(&self.xmap) {
                    *d = src[sx as usize];
                }
                prev = Some(sy);
            }
            let o = (dst.y as usize + dy) * stride + dst.x as usize;
            out[o..o + dst.w as usize].copy_from_slice(&self.row);
        }
    }
}

/// The whole screen for a frame: black borders plus the scaled frame.
pub fn compose(f: &Frame, view: View, aspect: f32, sw: u32, sh: u32) -> Frame {
    let mut out = Frame::new(sw, sh);
    let dst = layout(view, f.w, f.h, aspect, sw, sh);
    Scaler::default().scale(f, dst, &mut out.px, sw as usize);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_formats() {
        assert_eq!(rgb565(0xffff), 0xffffff);
        assert_eq!(rgb565(0xf800), 0xff0000);
        assert_eq!(rgb565(0x07e0), 0x00ff00);
        assert_eq!(rgb1555(0x7c00), 0xff0000);
        assert_eq!(rgb1555(0x001f), 0x0000ff);
        let src: [u16; 4] = [0xf800, 0x07e0, 0x001f, 0];
        let mut f = Frame::default();
        // 2x2 with a 6-byte pitch (one pixel of padding per row)
        let padded: [u16; 6] = [src[0], src[1], 0xdead, src[2], src[3], 0xbeef];
        unsafe { f.load(padded.as_ptr() as *const u8, 2, 2, 6, ffi::PIXEL_FORMAT_RGB565) };
        assert_eq!(f.px, vec![0xff0000, 0x00ff00, 0x0000ff, 0]);
    }

    fn v(filter: Filter, fit: Fit) -> View {
        View { filter, fit }
    }

    #[test]
    fn layouts() {
        let int = v(Filter::Integer, Fit::Fit);
        // GB 160x144 on 1024x768: integer 5x
        assert_eq!(layout(int, 160, 144, 0.0, 1024, 768), Dst { x: 112, y: 24, w: 800, h: 720 });
        // GBA 240x160 (3:2): 4x
        assert_eq!(layout(int, 240, 160, 1.5, 1024, 768), Dst { x: 32, y: 64, w: 960, h: 640 });
        // SNES 256x224 at 4:3: full screen
        assert_eq!(layout(v(Filter::Sharp, Fit::Fit), 256, 224, 4.0 / 3.0, 1024, 768), Dst { x: 0, y: 0, w: 1024, h: 768 });
        // GBA aspect 3:2: full width
        assert_eq!(layout(v(Filter::Smooth, Fit::Fit), 240, 160, 1.5, 1024, 768), Dst { x: 0, y: 42, w: 1024, h: 683 });
        // NES 256x240 integer at 4:3: 3x vertical (720), 960 wide
        assert_eq!(layout(int, 256, 240, 4.0 / 3.0, 1024, 768), Dst { x: 32, y: 24, w: 960, h: 720 });
        // PS1 320x240: 3x
        assert_eq!(layout(int, 320, 240, 4.0 / 3.0, 1024, 768), Dst { x: 32, y: 24, w: 960, h: 720 });
        // integer stretch: whole multiples on each axis
        assert_eq!(layout(v(Filter::Integer, Fit::Stretch), 256, 240, 4.0 / 3.0, 1024, 768), Dst { x: 0, y: 24, w: 1024, h: 720 });
        // vertical arcade 224x288 at 3:4
        let d = layout(v(Filter::Smooth, Fit::Fit), 224, 288, 0.75, 1024, 768);
        assert_eq!((d.w, d.h), (576, 768));
        assert_eq!(layout(v(Filter::Smooth, Fit::Stretch), 10, 10, 1.0, 1024, 768).w, 1024);
        // larger than the screen: falls back to fitting
        assert_eq!(layout(int, 2000, 1000, 2.0, 1024, 768).w, 1024);
    }

    #[test]
    fn views() {
        assert_eq!(View::parse("sharp-bilinear/fit"), Some(v(Filter::SharpBilinear, Fit::Fit)));
        assert_eq!(View::parse("smooth"), Some(v(Filter::Smooth, Fit::Fit)));
        assert_eq!(View::parse("bogus/fit"), None);
        for x in View::ALL {
            assert_eq!(View::parse(&x.key()), Some(x));
        }
        let a = View::ALL[0];
        assert_eq!(a.step(1).step(-1), a);
        assert_eq!(a.step(-1), View::ALL[View::ALL.len() - 1]);
        assert_eq!(v(Filter::SharpBilinear, Fit::Stretch).label(), "Sharp bilinear · stretch");
        // sharp bilinear pre-scales PS1 320x240 by 3 for 1024x768, 640x480 not at all
        let full = Dst { x: 0, y: 0, w: 1024, h: 768 };
        assert_eq!(prescale(Filter::SharpBilinear, 320, 240, full), 3);
        assert_eq!(prescale(Filter::SharpBilinear, 640, 480, full), 1);
        assert_eq!(prescale(Filter::Smooth, 320, 240, full), 1);
    }

    #[test]
    fn scaler_nearest() {
        let f = Frame { w: 2, h: 2, px: vec![1, 2, 3, 4] };
        let int = v(Filter::Integer, Fit::Fit);
        let out = compose(&f, int, 0.0, 4, 4);
        assert_eq!(out.px, vec![1, 1, 2, 2, 1, 1, 2, 2, 3, 3, 4, 4, 3, 3, 4, 4]);
        let out = compose(&f, int, 0.0, 6, 4);
        assert_eq!(&out.px[0..6], &[0, 1, 1, 2, 2, 0]);
    }

    #[test]
    fn rotation() {
        // 3x2:  1 2 3 / 4 5 6
        let f = Frame { w: 3, h: 2, px: vec![1, 2, 3, 4, 5, 6] };
        let ccw = f.rotated(1);
        assert_eq!((ccw.w, ccw.h), (2, 3));
        assert_eq!(ccw.px, vec![3, 6, 2, 5, 1, 4]);
        let cw = f.rotated(3);
        assert_eq!(cw.px, vec![4, 1, 5, 2, 6, 3]);
        assert_eq!(f.rotated(2).px, vec![6, 5, 4, 3, 2, 1]);
    }

    #[test]
    fn thumb() {
        let f = Frame { w: 4, h: 2, px: vec![0xff0000; 8] };
        let t = f.thumbnail(2, 2);
        assert_eq!((t.w, t.h), (2, 1));
        assert_eq!(t.px[0], 0xff0000);
    }
}
