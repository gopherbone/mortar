// SPDX-License-Identifier: GPL-2.0-or-later
//! A retained-for-one-frame display list for the shell's own UI.
//!
//! The UI (shellui.rs) builds a `Scene` every frame it animates; two
//! renderers draw the same list:
//!
//! - `raster` (CPU, tiny-skia): tests, previews, screenshots, and the
//!   fallback when the GPU path is off or failed.
//! - `comp::gl` (GLES 2 on the PowerVR): the device, at 60 fps.
//!
//! Shapes are flattened to polygons in screen space at build time (so the
//! GL side only triangulates convex fans, or uses a cached triangulation
//! for the few concave shapes); text and images keep a transform.

use mortar_ui::gfx::{Color, Face, Fonts, TextStyle};
use std::sync::Arc;

pub type P = (f32, f32);

/// 2D affine transform, canvas order: x' = a x + c y + e, y' = b x + d y + f.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xf {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

impl Xf {
    pub const ID: Xf = Xf { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };
    pub fn translate(x: f32, y: f32) -> Xf {
        Xf { e: x, f: y, ..Xf::ID }
    }
    pub fn scale(sx: f32, sy: f32) -> Xf {
        Xf { a: sx, d: sy, ..Xf::ID }
    }
    pub fn rotate(rad: f32) -> Xf {
        let (s, c) = rad.sin_cos();
        Xf { a: c, b: s, c: -s, d: c, e: 0.0, f: 0.0 }
    }
    /// `self` after `o` (o is applied first).
    pub fn mul(&self, o: &Xf) -> Xf {
        Xf {
            a: self.a * o.a + self.c * o.b,
            b: self.b * o.a + self.d * o.b,
            c: self.a * o.c + self.c * o.d,
            d: self.b * o.c + self.d * o.d,
            e: self.a * o.e + self.c * o.f + self.e,
            f: self.b * o.e + self.d * o.f + self.f,
        }
    }
    /// `m` about the point (cx, cy) (CSS transform-origin).
    pub fn about(cx: f32, cy: f32, m: Xf) -> Xf {
        Xf::translate(cx, cy).mul(&m).mul(&Xf::translate(-cx, -cy))
    }
    pub fn apply(&self, p: P) -> P {
        (self.a * p.0 + self.c * p.1 + self.e, self.b * p.0 + self.d * p.1 + self.f)
    }
    /// Uniform scale factor (for stroke widths).
    pub fn scale_factor(&self) -> f32 {
        (self.a * self.d - self.b * self.c).abs().sqrt()
    }
    /// Only a translation (text can be blitted, not resampled).
    pub fn is_translate(&self) -> bool {
        (self.a - 1.0).abs() < 1e-4 && (self.d - 1.0).abs() < 1e-4 && self.b.abs() < 1e-4 && self.c.abs() < 1e-4
    }
}

/// Where an image's pixels come from; resolved by the renderer's owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Src {
    /// The newest buffer of a running app's window (by app uid).
    App(u32),
    /// The same buffer blurred (the MENU's backdrop): renderers keep a
    /// small blurred copy, refreshed when the app commits (at most ~15/s).
    Blur(u32),
    /// A CPU-drawn layer (mortar-ui toolkit screens).
    Layer(Layer),
    /// The home screen blurred (the MENU's backdrop when no app is in
    /// front): rendered once when it is first needed, blurred like
    /// `Blur`, kept while frames keep using it.
    HomeBlur,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Layer {
    /// The on-screen keyboard strip.
    Osk,
    /// A full-screen setup page (Wi-Fi, Remote access).
    Page,
}

/// A concave shape with a cached triangulation (in local units).
#[derive(Debug)]
pub struct Shape {
    pub outline: Vec<Vec<P>>,
    pub tris: Vec<P>,
}

impl Shape {
    pub fn from_path(d: &str) -> Shape {
        let outline = parse_path(d);
        let tris = outline.iter().flat_map(|c| triangulate(c)).collect();
        Shape { outline, tris }
    }
}

#[derive(Clone, Debug)]
pub struct TextRun {
    pub s: String,
    pub face: Face,
    pub size: f32,
    /// Pen start and baseline, in the run's local space.
    pub x: f32,
    pub base: f32,
    pub tracking: f32,
    pub color: Color,
    pub xf: Xf,
}

#[derive(Clone, Debug)]
pub enum Item {
    /// A filled polygon in screen space. `convex` polygons are fanned.
    Fill { pts: Vec<P>, color: Color, convex: bool },
    /// A polyline of `width` px in screen space.
    Stroke { pts: Vec<P>, closed: bool, width: f32, color: Color },
    /// A cached concave shape under a transform.
    Shape { shape: Arc<Shape>, xf: Xf, color: Color },
    Text(TextRun),
    /// An image mapped onto a quad (TL, TR, BR, BL); uv = (u0, v0, u1, v1).
    Image { src: Src, quad: [P; 4], uv: [f32; 4], alpha: f32 },
    /// Clip what follows to a screen rectangle (x, y, w, h), or stop clipping.
    Clip(Option<[f32; 4]>),
}

#[derive(Clone, Debug)]
pub struct Scene {
    #[allow(dead_code)] // for renderers that size themselves from the scene
    pub w: f32,
    #[allow(dead_code)]
    pub h: f32,
    pub clear: Color,
    pub items: Vec<Item>,
}

impl Scene {
    pub fn new(w: f32, h: f32, clear: Color) -> Scene {
        Scene { w, h, clear, items: Vec::new() }
    }
    /// The app images this frame samples, sharp or blurred (their commits
    /// need a redraw).
    pub fn apps(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Image { src: Src::App(u) | Src::Blur(u), .. } => Some(*u),
                _ => None,
            })
            .collect();
        v.sort();
        v.dedup();
        v
    }

    /// The frame shows the blurred home screen.
    pub fn home_blur(&self) -> bool {
        self.items.iter().any(|i| matches!(i, Item::Image { src: Src::HomeBlur, .. }))
    }

    /// The apps this frame shows blurred.
    pub fn blurs(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Image { src: Src::Blur(u), .. } => Some(*u),
                _ => None,
            })
            .collect();
        v.sort();
        v.dedup();
        v
    }
}

// ------------------------------------------------------------ typography ----

/// Fonts plus the text metrics both renderers and the layout agree on.
pub struct Typo {
    pub fonts: Fonts,
}

impl Typo {
    pub fn new(fonts: Fonts) -> Typo {
        Typo { fonts }
    }
    pub fn font(&self, f: Face) -> &fontdue::Font {
        self.fonts.face(f)
    }
    /// Glyph indices and pen positions of one line.
    pub fn layout(&self, s: &str, face: Face, size: f32, tracking: f32) -> Vec<(u16, f32)> {
        let font = self.font(face);
        let mut out = Vec::with_capacity(s.len());
        let mut pen = 0.0;
        let mut prev: Option<u16> = None;
        for ch in s.chars() {
            let idx = font.lookup_glyph_index(ch);
            if let Some(p) = prev {
                pen += font.horizontal_kern_indexed(p, idx, size).unwrap_or(0.0) + tracking * size;
            }
            out.push((idx, pen));
            pen += font.metrics_indexed(idx, size).advance_width;
            prev = Some(idx);
        }
        out
    }
    pub fn measure(&self, s: &str, face: Face, size: f32, tracking: f32) -> f32 {
        let font = self.font(face);
        match self.layout(s, face, size, tracking).last() {
            Some((idx, pen)) => pen + font.metrics_indexed(*idx, size).advance_width,
            None => 0.0,
        }
    }
    /// Baseline below the top of a CSS line box of `lh` x size.
    pub fn base_in_line(&self, face: Face, size: f32, lh: f32) -> f32 {
        let lm = self.font(face).horizontal_line_metrics(size).expect("horizontal font");
        (size * lh - (lm.ascent - lm.descent)) / 2.0 + lm.ascent
    }
}

// --------------------------------------------------------------- builder ----

pub struct B<'a> {
    pub typo: &'a Typo,
    pub scene: Scene,
    xfs: Vec<Xf>,
    alphas: Vec<f32>,
}

/// Text style shorthand: (face, px, colour) with CSS line-height 1.2.
pub fn ts(face: Face, size: f32, color: Color) -> TextStyle {
    TextStyle::new(face, size, color).line_height(1.2)
}

impl<'a> B<'a> {
    pub fn new(typo: &'a Typo, w: f32, h: f32, clear: Color) -> B<'a> {
        B { typo, scene: Scene::new(w, h, clear), xfs: vec![Xf::ID], alphas: vec![1.0] }
    }
    pub fn finish(self) -> Scene {
        self.scene
    }
    pub fn xf(&self) -> Xf {
        *self.xfs.last().unwrap()
    }
    pub fn alpha(&self) -> f32 {
        *self.alphas.last().unwrap()
    }
    /// Apply `m` (in the current space) to what follows, until `pop`.
    pub fn push(&mut self, m: Xf) {
        let x = self.xf().mul(&m);
        self.xfs.push(x);
    }
    pub fn pop(&mut self) {
        if self.xfs.len() > 1 {
            self.xfs.pop();
        }
    }
    pub fn push_alpha(&mut self, a: f32) {
        let x = self.alpha() * a.clamp(0.0, 1.0);
        self.alphas.push(x);
    }
    pub fn pop_alpha(&mut self) {
        if self.alphas.len() > 1 {
            self.alphas.pop();
        }
    }
    fn col(&self, c: Color) -> Color {
        let a = self.alpha();
        if a >= 1.0 { c } else { Color(c.0, c.1, c.2, (c.3 as f32 * a).round() as u8) }
    }
    fn visible(&self, c: Color) -> bool {
        self.col(c).3 > 0
    }

    pub fn poly(&mut self, pts: &[P], color: Color) {
        if !self.visible(color) || pts.len() < 3 {
            return;
        }
        let x = self.xf();
        let pts = pts.iter().map(|p| x.apply(*p)).collect();
        let color = self.col(color);
        self.scene.items.push(Item::Fill { pts, color, convex: true });
    }
    pub fn poly_concave(&mut self, pts: &[P], color: Color) {
        if !self.visible(color) || pts.len() < 3 {
            return;
        }
        let x = self.xf();
        let pts = pts.iter().map(|p| x.apply(*p)).collect();
        let color = self.col(color);
        self.scene.items.push(Item::Fill { pts, color, convex: false });
    }
    pub fn stroke(&mut self, pts: &[P], closed: bool, width: f32, color: Color) {
        if !self.visible(color) || pts.len() < 2 {
            return;
        }
        let x = self.xf();
        let pts = pts.iter().map(|p| x.apply(*p)).collect();
        let color = self.col(color);
        self.scene.items.push(Item::Stroke { pts, closed, width: width * x.scale_factor(), color });
    }
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Color) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        self.poly(&[(x, y), (x + w, y), (x + w, y + h), (x, y + h)], color);
    }
    /// A border `bw` wide inside the box (CSS border-box).
    pub fn frame(&mut self, x: f32, y: f32, w: f32, h: f32, bw: f32, color: Color) {
        self.rect(x, y, w, bw, color);
        self.rect(x, y + h - bw, w, bw, color);
        self.rect(x, y + bw, bw, h - 2.0 * bw, color);
        self.rect(x + w - bw, y + bw, bw, h - 2.0 * bw, color);
    }
    /// A filled box with a 1 px border (the style's basic block).
    pub fn block(&mut self, x: f32, y: f32, w: f32, h: f32, fill: Color, line: Color) {
        self.rect(x, y, w, h, fill);
        self.frame(x, y, w, h, 1.0, line);
    }
    pub fn ellipse(&mut self, cx: f32, cy: f32, rx: f32, ry: f32, color: Color) {
        let pts = ellipse_pts(cx, cy, rx, ry, 56);
        self.poly(&pts, color);
    }
    pub fn rrect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, color: Color) {
        let pts = rrect_pts(x, y, w, h, r);
        self.poly(&pts, color);
    }
    pub fn shape(&mut self, shape: &Arc<Shape>, m: Xf, color: Color) {
        if !self.visible(color) {
            return;
        }
        let xf = self.xf().mul(&m);
        let color = self.col(color);
        self.scene.items.push(Item::Shape { shape: shape.clone(), xf, color });
    }

    // ------------------------------------------------------------ text ----

    pub fn measure(&self, s: &str, st: &TextStyle) -> f32 {
        self.typo.measure(s, st.face, st.size, st.tracking)
    }
    /// Text on a baseline; returns the advance.
    pub fn text_base(&mut self, x: f32, base: f32, s: &str, st: &TextStyle) -> f32 {
        let w = self.measure(s, st);
        if s.is_empty() || !self.visible(st.color) {
            return w;
        }
        let run = TextRun {
            s: s.to_string(),
            face: st.face,
            size: st.size,
            x,
            base,
            tracking: st.tracking,
            color: self.col(st.color),
            xf: self.xf(),
        };
        self.scene.items.push(Item::Text(run));
        w
    }
    /// Text whose CSS line box (height size x line_height) starts at `top`.
    pub fn text(&mut self, x: f32, top: f32, s: &str, st: &TextStyle) -> f32 {
        let base = top + self.typo.base_in_line(st.face, st.size, st.line_height);
        self.text_base(x, base, s, st)
    }
    pub fn text_right(&mut self, right: f32, top: f32, s: &str, st: &TextStyle) -> f32 {
        let w = self.measure(s, st);
        self.text(right - w, top, s, st);
        w
    }
    pub fn text_center(&mut self, cx: f32, top: f32, s: &str, st: &TextStyle) -> f32 {
        let w = self.measure(s, st);
        self.text(cx - w / 2.0, top, s, st);
        w
    }
    /// Truncate with an ellipsis to fit `max_w`.
    pub fn fit(&self, s: &str, st: &TextStyle, max_w: f32) -> String {
        if self.measure(s, st) <= max_w {
            return s.to_string();
        }
        let chars: Vec<char> = s.chars().collect();
        for n in (0..chars.len()).rev() {
            let t: String = chars[..n].iter().collect::<String>().trim_end().to_string() + "…";
            if self.measure(&t, st) <= max_w {
                return t;
            }
        }
        String::new()
    }

    /// Clip to a rectangle in the current space (translate/scale only).
    pub fn clip(&mut self, r: Option<[f32; 4]>) {
        let m = self.xf();
        let r = r.map(|r| {
            let (x0, y0) = m.apply((r[0], r[1]));
            let (x1, y1) = m.apply((r[0] + r[2], r[1] + r[3]));
            [x0.min(x1), y0.min(y1), (x1 - x0).abs(), (y1 - y0).abs()]
        });
        self.scene.items.push(Item::Clip(r));
    }

    // ----------------------------------------------------------- images ----

    pub fn image(&mut self, src: Src, x: f32, y: f32, w: f32, h: f32, uv: [f32; 4]) {
        let a = self.alpha();
        if a <= 0.0 || w <= 0.0 || h <= 0.0 {
            return;
        }
        let m = self.xf();
        let quad = [m.apply((x, y)), m.apply((x + w, y)), m.apply((x + w, y + h)), m.apply((x, y + h))];
        self.scene.items.push(Item::Image { src, quad, uv, alpha: a });
    }
}

// -------------------------------------------------------------- geometry ----

pub fn ellipse_pts(cx: f32, cy: f32, rx: f32, ry: f32, n: usize) -> Vec<P> {
    (0..n)
        .map(|i| {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            (cx + rx * a.cos(), cy + ry * a.sin())
        })
        .collect()
}

pub fn rrect_pts(x: f32, y: f32, w: f32, h: f32, r: f32) -> Vec<P> {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    if r <= 0.0 {
        return vec![(x, y), (x + w, y), (x + w, y + h), (x, y + h)];
    }
    let mut v = Vec::with_capacity(40);
    let corners = [(x + w - r, y + r, -90.0f32), (x + w - r, y + h - r, 0.0), (x + r, y + h - r, 90.0), (x + r, y + r, 180.0)];
    for (cx, cy, a0) in corners {
        for i in 0..=8 {
            let a = (a0 + i as f32 * 90.0 / 8.0).to_radians();
            v.push((cx + r * a.cos(), cy + r * a.sin()));
        }
    }
    v
}

/// SVG path data (M/L/H/V/C/Z, absolute and relative) to closed contours,
/// cubics flattened.
pub fn parse_path(d: &str) -> Vec<Vec<P>> {
    let mut toks: Vec<String> = Vec::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, toks: &mut Vec<String>| {
        if !cur.is_empty() {
            toks.push(std::mem::take(cur));
        }
    };
    for ch in d.chars() {
        if ch.is_ascii_alphabetic() && ch != 'e' && ch != 'E' {
            flush(&mut cur, &mut toks);
            toks.push(ch.to_string());
        } else if ch == ',' || ch.is_whitespace() {
            flush(&mut cur, &mut toks);
        } else if ch == '-' && !cur.is_empty() && !cur.ends_with('e') {
            flush(&mut cur, &mut toks);
            cur.push(ch);
        } else if ch == '.' && cur.contains('.') {
            flush(&mut cur, &mut toks);
            cur.push(ch);
        } else {
            cur.push(ch);
        }
    }
    flush(&mut cur, &mut toks);
    let mut out: Vec<Vec<P>> = Vec::new();
    let mut c: Vec<P> = Vec::new();
    let (mut x, mut y) = (0.0f32, 0.0f32);
    let mut cmd = 'M';
    let mut i = 0;
    let num = |i: &mut usize| -> f32 {
        let v = toks.get(*i).and_then(|t| t.parse::<f32>().ok()).unwrap_or(0.0);
        *i += 1;
        v
    };
    while i < toks.len() {
        if toks[i].len() == 1 && toks[i].chars().next().unwrap().is_ascii_alphabetic() {
            cmd = toks[i].chars().next().unwrap();
            i += 1;
            if cmd == 'Z' || cmd == 'z' {
                if c.len() > 2 {
                    out.push(std::mem::take(&mut c));
                }
                c.clear();
                continue;
            }
        }
        let rel = cmd.is_ascii_lowercase();
        let (ox, oy) = if rel { (x, y) } else { (0.0, 0.0) };
        match cmd.to_ascii_uppercase() {
            'M' => {
                if c.len() > 2 {
                    out.push(std::mem::take(&mut c));
                }
                c.clear();
                x = ox + num(&mut i);
                y = oy + num(&mut i);
                c.push((x, y));
                cmd = if rel { 'l' } else { 'L' };
            }
            'L' => {
                x = ox + num(&mut i);
                y = oy + num(&mut i);
                c.push((x, y));
            }
            'H' => {
                x = ox + num(&mut i);
                c.push((x, y));
            }
            'V' => {
                y = oy + num(&mut i);
                c.push((x, y));
            }
            'C' => {
                let p1 = (ox + num(&mut i), oy + num(&mut i));
                let p2 = (ox + num(&mut i), oy + num(&mut i));
                let p3 = (ox + num(&mut i), oy + num(&mut i));
                let p0 = (x, y);
                for k in 1..=8 {
                    let t = k as f32 / 8.0;
                    let u = 1.0 - t;
                    let b = |a: f32, b: f32, c2: f32, d2: f32| u * u * u * a + 3.0 * u * u * t * b + 3.0 * u * t * t * c2 + t * t * t * d2;
                    c.push((b(p0.0, p1.0, p2.0, p3.0), b(p0.1, p1.1, p2.1, p3.1)));
                }
                x = p3.0;
                y = p3.1;
            }
            _ => i += 1, // unsupported: skip a token
        }
    }
    if c.len() > 2 {
        out.push(c);
    }
    // drop the closing duplicate of the first point
    for c in &mut out {
        while c.len() > 3 && dist2(c[0], *c.last().unwrap()) < 1e-6 {
            c.pop();
        }
    }
    out
}

fn dist2(a: P, b: P) -> f32 {
    (a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)
}

fn cross(o: P, a: P, b: P) -> f32 {
    (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
}

/// Ear-clipping triangulation of a simple polygon; triangles as a flat
/// list of points.
pub fn triangulate(poly: &[P]) -> Vec<P> {
    let mut idx: Vec<usize> = Vec::with_capacity(poly.len());
    for (i, p) in poly.iter().enumerate() {
        if idx.last().is_none_or(|&j| dist2(poly[j], *p) > 1e-8) {
            idx.push(i);
        }
    }
    if idx.len() < 3 {
        return Vec::new();
    }
    let area: f32 = (0..idx.len()).map(|i| {
        let (a, b) = (poly[idx[i]], poly[idx[(i + 1) % idx.len()]]);
        a.0 * b.1 - b.0 * a.1
    }).sum();
    if area < 0.0 {
        idx.reverse();
    }
    let mut out = Vec::with_capacity((idx.len() - 2) * 3);
    let mut guard = 0;
    while idx.len() > 3 && guard < 100_000 {
        guard += 1;
        let n = idx.len();
        let mut clipped = false;
        for i in 0..n {
            let (ia, ib, ic) = (idx[(i + n - 1) % n], idx[i], idx[(i + 1) % n]);
            let (a, b, c) = (poly[ia], poly[ib], poly[ic]);
            let cr = cross(a, b, c);
            if cr <= 1e-9 {
                if cr.abs() <= 1e-9 {
                    // collinear: drop the middle point
                    idx.remove(i);
                    clipped = true;
                    break;
                }
                continue; // reflex
            }
            let inside = idx.iter().any(|&j| {
                if j == ia || j == ib || j == ic {
                    return false;
                }
                let p = poly[j];
                p != a && p != b && p != c && cross(a, b, p) >= -1e-6 && cross(b, c, p) >= -1e-6 && cross(c, a, p) >= -1e-6
            });
            if inside {
                continue;
            }
            out.extend_from_slice(&[a, b, c]);
            idx.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            break; // degenerate; fan the rest
        }
    }
    for i in 1..idx.len().saturating_sub(1) {
        out.extend_from_slice(&[poly[idx[0]], poly[idx[i]], poly[idx[i + 1]]]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri_area(t: &[P]) -> f32 {
        t.chunks(3).map(|c| cross(c[0], c[1], c[2]).abs() / 2.0).sum()
    }

    #[test]
    fn triangulates_concave() {
        // an L shape, area 3
        let l = [(0.0, 0.0), (2.0, 0.0), (2.0, 1.0), (1.0, 1.0), (1.0, 2.0), (0.0, 2.0)];
        let t = triangulate(&l);
        assert_eq!(t.len(), 12);
        assert!((tri_area(&t) - 3.0).abs() < 1e-4, "{t:?} {}", tri_area(&t));
    }

    #[test]
    fn spark_path_parses() {
        let s = Shape::from_path(mortar_ui::icons::CLAUDE_SPARK_PATH);
        assert_eq!(s.outline.len(), 1);
        let n = s.outline[0].len();
        assert!(n > 100, "{n} points");
        for p in &s.outline[0] {
            assert!(p.0 >= 0.0 && p.0 <= 94.0 && p.1 >= 0.0 && p.1 <= 94.0, "{p:?}");
        }
        // the triangulation covers the polygon's area
        let poly_area: f32 = (0..n).map(|i| {
            let (a, b) = (s.outline[0][i], s.outline[0][(i + 1) % n]);
            a.0 * b.1 - b.0 * a.1
        }).sum::<f32>().abs() / 2.0;
        assert!((tri_area(&s.tris) - poly_area).abs() / poly_area < 0.01, "{} vs {poly_area}", tri_area(&s.tris));
    }

    #[test]
    fn transforms_compose() {
        let m = Xf::about(10.0, 10.0, Xf::rotate(std::f32::consts::FRAC_PI_2));
        let p = m.apply((20.0, 10.0));
        assert!((p.0 - 10.0).abs() < 1e-4 && (p.1 - 20.0).abs() < 1e-4, "{p:?}");
        let t = Xf::translate(5.0, 0.0).mul(&Xf::scale(2.0, 2.0));
        assert_eq!(t.apply((1.0, 1.0)), (7.0, 2.0));
    }
}
