// SPDX-License-Identifier: GPL-2.0-or-later
//! CPU renderer for `scene::Scene` (tiny-skia + fontdue): the headless test,
//! `preview`, screenshots, and the fallback when the GPU path is off.

use crate::scene::{Item, Scene, Src, TextRun, Typo, Xf};
use mortar_ui::gfx::{Color, Face};
use resvg::tiny_skia::{
    self, FillRule, FilterQuality, Paint, PathBuilder, Pattern, Pixmap, PixmapPaint, SpreadMode, Stroke, Transform,
};
use std::collections::HashMap;

/// Image sources the scene refers to (premultiplied RGBA pixmaps).
pub trait Images {
    fn get(&self, s: Src) -> Option<&Pixmap>;
}

type GlyphKey = (Face, u16, u32);

#[derive(Default)]
pub struct Raster {
    glyphs: HashMap<GlyphKey, (fontdue::Metrics, Vec<u8>)>,
    tinted: HashMap<(GlyphKey, Color), Pixmap>,
}

fn sk(m: &Xf) -> Transform {
    Transform::from_row(m.a, m.b, m.c, m.d, m.e, m.f)
}

fn paint(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(c.sk());
    p.anti_alias = true;
    p
}

fn path(pts: &[(f32, f32)], close: bool) -> Option<tiny_skia::Path> {
    let (first, rest) = pts.split_first()?;
    let mut pb = PathBuilder::new();
    pb.move_to(first.0, first.1);
    for p in rest {
        pb.line_to(p.0, p.1);
    }
    if close {
        pb.close();
    }
    pb.finish()
}

/// Axis-aligned rectangle on whole pixels: drawn without anti-aliasing so
/// 1 px borders stay crisp, as in the toolkit.
fn crisp_rect(pts: &[(f32, f32)]) -> Option<tiny_skia::Rect> {
    if pts.len() != 4 {
        return None;
    }
    let (x0, y0, x1, y1) = (pts[0].0, pts[0].1, pts[2].0, pts[2].1);
    let axis = pts[1] == (x1, y0) && pts[3] == (x0, y1);
    let whole = |v: f32| (v - v.round()).abs() < 0.01;
    if axis && whole(x0) && whole(y0) && whole(x1) && whole(y1) && x1 > x0 && y1 > y0 {
        tiny_skia::Rect::from_ltrb(x0, y0, x1, y1)
    } else {
        None
    }
}

impl Raster {
    pub fn render(&mut self, pm: &mut Pixmap, scene: &Scene, typo: &Typo, images: &dyn Images) {
        pm.fill(scene.clear.sk());
        let mut mask: Option<tiny_skia::Mask> = None;
        let mut clip_rect: Option<[f32; 4]> = None;
        for it in &scene.items {
            let m = mask.as_ref();
            match it {
                Item::Clip(r) => {
                    clip_rect = *r;
                    mask = r.and_then(|r| {
                        let mut mk = tiny_skia::Mask::new(pm.width(), pm.height())?;
                        let rect = tiny_skia::Rect::from_xywh(r[0], r[1], r[2].max(0.0), r[3].max(0.0))?;
                        mk.fill_path(&PathBuilder::from_rect(rect), FillRule::Winding, false, Transform::identity());
                        Some(mk)
                    });
                }
                Item::Fill { pts, color, .. } => {
                    if let Some(r) = crisp_rect(pts) {
                        let mut p = paint(*color);
                        p.anti_alias = false;
                        pm.fill_rect(r, &p, Transform::identity(), m);
                    } else if let Some(p) = path(pts, true) {
                        pm.fill_path(&p, &paint(*color), FillRule::Winding, Transform::identity(), m);
                    }
                }
                Item::Stroke { pts, closed, width, color } => {
                    if let Some(p) = path(pts, *closed) {
                        let s = Stroke { width: *width, line_join: tiny_skia::LineJoin::Round, line_cap: tiny_skia::LineCap::Round, ..Stroke::default() };
                        pm.stroke_path(&p, &paint(*color), &s, Transform::identity(), m);
                    }
                }
                Item::Shape { shape, xf, color } => {
                    let mut pb = PathBuilder::new();
                    for c in &shape.outline {
                        if let Some((f, rest)) = c.split_first() {
                            pb.move_to(f.0, f.1);
                            for p in rest {
                                pb.line_to(p.0, p.1);
                            }
                            pb.close();
                        }
                    }
                    if let Some(p) = pb.finish() {
                        pm.fill_path(&p, &paint(*color), FillRule::Winding, sk(xf), m);
                    }
                }
                Item::Text(run) => self.text(pm, typo, run, clip_rect, m),
                Item::Image { src, quad, uv, alpha } => {
                    if let Some(img) = images.get(*src) {
                        draw_image(pm, img, quad, uv, *alpha, m);
                    }
                }
            }
        }
    }

    fn glyph(&mut self, typo: &Typo, key: GlyphKey) -> &(fontdue::Metrics, Vec<u8>) {
        self.glyphs.entry(key).or_insert_with(|| typo.font(key.0).rasterize_indexed(key.1, key.2 as f32 / 64.0))
    }

    fn text(&mut self, pm: &mut Pixmap, typo: &Typo, run: &TextRun, clip: Option<[f32; 4]>, mask: Option<&tiny_skia::Mask>) {
        let skey = (run.size * 64.0).round() as u32;
        let layout = typo.layout(&run.s, run.face, run.size, run.tracking);
        let fast = run.xf.is_translate();
        for &(idx, pen) in layout.iter() {
            let key = (run.face, idx, skey);
            let m = self.glyph(typo, key).0;
            if m.width == 0 || m.height == 0 {
                continue;
            }
            let gx = run.x + pen + m.xmin as f32;
            let gy = run.base - m.height as f32 - m.ymin as f32;
            if fast {
                let (cov_m, cov) = self.glyph(typo, key);
                let x = (gx + run.xf.e).round() as i32;
                let y = (gy + run.xf.f).round() as i32;
                blit_coverage(pm, x, y, cov_m.width, cov_m.height, cov, run.color, clip);
            } else {
                let tinted = self.tinted.contains_key(&(key, run.color));
                if !tinted {
                    let (gm, cov) = self.glyph(typo, key);
                    let mut g = Pixmap::new(gm.width as u32, gm.height as u32).expect("glyph");
                    let c = run.color;
                    for (o, a) in g.data_mut().chunks_exact_mut(4).zip(cov.iter()) {
                        let a = *a as u32 * c.3 as u32 / 255;
                        o[0] = (c.0 as u32 * a / 255) as u8;
                        o[1] = (c.1 as u32 * a / 255) as u8;
                        o[2] = (c.2 as u32 * a / 255) as u8;
                        o[3] = a as u8;
                    }
                    self.tinted.insert((key, run.color), g);
                }
                let g = &self.tinted[&(key, run.color)];
                let t = sk(&run.xf.mul(&Xf::translate(gx, gy)));
                let pp = PixmapPaint { quality: FilterQuality::Bilinear, ..PixmapPaint::default() };
                pm.draw_pixmap(0, 0, g.as_ref(), &pp, t, mask);
            }
        }
        if self.tinted.len() > 4000 {
            self.tinted.clear();
        }
    }
}

/// Map the uv rectangle of `img` onto the parallelogram `quad`.
pub fn draw_image(pm: &mut Pixmap, img: &Pixmap, quad: &[(f32, f32); 4], uv: &[f32; 4], alpha: f32, mask: Option<&tiny_skia::Mask>) {
    let (iw, ih) = (img.width() as f32, img.height() as f32);
    let (u0, v0, u1, v1) = (uv[0] * iw, uv[1] * ih, uv[2] * iw, uv[3] * ih);
    let (sw, sh) = (u1 - u0, v1 - v0);
    if sw.abs() < 1e-3 || sh.abs() < 1e-3 {
        return;
    }
    let (tl, tr, bl) = (quad[0], quad[1], quad[3]);
    // image px -> screen: tl + (x - u0)/sw * (tr - tl) + (y - v0)/sh * (bl - tl)
    let a = (tr.0 - tl.0) / sw;
    let b = (tr.1 - tl.1) / sw;
    let c = (bl.0 - tl.0) / sh;
    let d = (bl.1 - tl.1) / sh;
    let e = tl.0 - a * u0 - c * v0;
    let f = tl.1 - b * u0 - d * v0;
    let t = Transform::from_row(a, b, c, d, e, f);
    let pat = Pattern::new(img.as_ref(), SpreadMode::Pad, FilterQuality::Bilinear, alpha.clamp(0.0, 1.0), t);
    let mut p = Paint::default();
    p.shader = pat;
    p.anti_alias = true;
    if let Some(path) = path(quad, true) {
        pm.fill_path(&path, &p, FillRule::Winding, Transform::identity(), mask);
    }
}

#[allow(clippy::too_many_arguments)]
fn blit_coverage(pm: &mut Pixmap, x: i32, y: i32, w: usize, h: usize, cov: &[u8], c: Color, clip: Option<[f32; 4]>) {
    let (pw, ph) = (pm.width() as i32, pm.height() as i32);
    let (cx0, cy0, cx1, cy1) = match clip {
        Some(r) => (r[0].round() as i32, r[1].round() as i32, (r[0] + r[2]).round() as i32, (r[1] + r[3]).round() as i32),
        None => (0, 0, pw, ph),
    };
    let data = pm.data_mut();
    let ca = c.3 as u32;
    for row in 0..h as i32 {
        let py = y + row;
        if py < cy0.max(0) || py >= cy1.min(ph) {
            continue;
        }
        for col in 0..w as i32 {
            let px = x + col;
            if px < cx0.max(0) || px >= cx1.min(pw) {
                continue;
            }
            let a = cov[(row as usize) * w + col as usize] as u32 * ca / 255;
            if a == 0 {
                continue;
            }
            let o = ((py * pw + px) * 4) as usize;
            let inv = 255 - a;
            data[o] = ((c.0 as u32 * a + data[o] as u32 * inv) / 255) as u8;
            data[o + 1] = ((c.1 as u32 * a + data[o + 1] as u32 * inv) / 255) as u8;
            data[o + 2] = ((c.2 as u32 * a + data[o + 2] as u32 * inv) / 255) as u8;
            data[o + 3] = (a + data[o + 3] as u32 * inv / 255) as u8;
        }
    }
}

/// The MENU backdrop on the CPU, matching the GPU's (gl.rs `blur`): a 4x4
/// box down to quarter size, then three rounds of a 7-wide box blur each
/// way (variance 12 texels², like three 9-tap gaussians of sigma 2). The
/// renderer scales the small result up with bilinear filtering.
/// Mean colour (sRGB 0..1) of an opaque pixmap (the blurred backdrop:
/// the MENU raises its veil over dark ones). Every 4th pixel suffices.
pub fn mean_rgb(pm: &Pixmap) -> [f32; 3] {
    mean_rgba_bytes(pm.data(), 4)
}

/// Mean of RGBA bytes, sampling every `step`th pixel.
pub fn mean_rgba_bytes(px: &[u8], step: usize) -> [f32; 3] {
    let mut s = [0f64; 3];
    let mut n = 0f64;
    for p in px.chunks_exact(4).step_by(step.max(1)) {
        for c in 0..3 {
            s[c] += p[c] as f64;
        }
        n += 1.0;
    }
    if n == 0.0 {
        return [1.0; 3];
    }
    s.map(|v| (v / n / 255.0) as f32)
}

pub fn blur_quarter(img: &Pixmap) -> Option<Pixmap> {
    let (sw, sh) = (img.width() as usize, img.height() as usize);
    let (w, h) = ((sw / 4).max(1), (sh / 4).max(1));
    let src = img.data();
    let mut buf = vec![0f32; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0f32; 4];
            let mut n: f32 = 0.0;
            for yy in y * 4..(y * 4 + 4).min(sh) {
                for xx in x * 4..(x * 4 + 4).min(sw) {
                    let o = (yy * sw + xx) * 4;
                    for c in 0..4 {
                        acc[c] += src[o + c] as f32;
                    }
                    n += 1.0;
                }
            }
            for c in 0..4 {
                buf[(y * w + x) * 4 + c] = acc[c] / n.max(1.0);
            }
        }
    }
    let mut tmp = vec![0f32; buf.len()];
    let r = 3i32;
    for _ in 0..3 {
        for (horiz, len, lines) in [(true, w, h), (false, h, w)] {
            for line in 0..lines {
                let at = |i: usize| if horiz { (line * w + i) * 4 } else { (i * w + line) * 4 };
                for i in 0..len {
                    let mut acc = [0f32; 4];
                    for k in -r..=r {
                        let j = (i as i32 + k).clamp(0, len as i32 - 1) as usize;
                        let o = at(j);
                        for c in 0..4 {
                            acc[c] += buf[o + c];
                        }
                    }
                    let o = at(i);
                    for c in 0..4 {
                        tmp[o + c] = acc[c] / (2 * r + 1) as f32;
                    }
                }
            }
            std::mem::swap(&mut buf, &mut tmp);
        }
    }
    let mut pm = Pixmap::new(w as u32, h as u32)?;
    for (d, s) in pm.data_mut().iter_mut().zip(&buf) {
        *d = s.round().clamp(0.0, 255.0) as u8;
    }
    Some(pm)
}

/// Opaque BGRX/BGRA rows (wl_shm XRGB/ARGB, dma-buf XR24) to a pixmap,
/// optionally downscaled by an integer factor (thumbnails).
pub fn pixmap_from_xrgb(src: &[u8], w: usize, h: usize, stride: usize, step: usize) -> Option<Pixmap> {
    let step = step.max(1);
    let (ow, oh) = (w / step, h / step);
    let mut pm = Pixmap::new(ow as u32, oh as u32)?;
    let dst = pm.data_mut();
    for y in 0..oh {
        let row = &src.get(y * step * stride..y * step * stride + w * 4)?;
        for x in 0..ow {
            let s = &row[x * step * 4..x * step * 4 + 4];
            let o = (y * ow + x) * 4;
            dst[o] = s[2];
            dst[o + 1] = s[1];
            dst[o + 2] = s[0];
            dst[o + 3] = 255;
        }
    }
    Some(pm)
}
