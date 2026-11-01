// SPDX-License-Identifier: GPL-2.0-or-later
//! Software rendering: an RGBA canvas (tiny-skia), text (fontdue) and the
//! mockup's line icons (resvg). Everything draws into `Ui::pm`; a backend then
//! copies that pixmap to the screen, a window or a PNG.

use resvg::tiny_skia::{
    self, Color as SkColor, FillRule, Paint, PathBuilder, Pixmap, PixmapPaint, Rect as SkRect,
    Stroke, Transform,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::icons::Icon;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Color(pub u8, pub u8, pub u8, pub u8);

impl Color {
    pub const fn hex(v: u32) -> Color {
        Color((v >> 16) as u8, (v >> 8) as u8, v as u8, 255)
    }
    pub fn with_alpha(self, a: f32) -> Color {
        Color(self.0, self.1, self.2, (a.clamp(0.0, 1.0) * 255.0) as u8)
    }
    /// Like CSS color-mix(in srgb, self p, other).
    pub fn mix(self, other: Color, p: f32) -> Color {
        let m = |a: u8, b: u8| (a as f32 * p + b as f32 * (1.0 - p)).round() as u8;
        Color(m(self.0, other.0), m(self.1, other.1), m(self.2, other.2), 255)
    }
    pub fn sk(self) -> SkColor {
        SkColor::from_rgba8(self.0, self.1, self.2, self.3)
    }
    fn css(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn inset(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, (self.w - 2.0 * dx).max(0.0), (self.h - 2.0 * dy).max(0.0))
    }
    /// Split off the top `h` pixels; returns (top, rest).
    pub fn take_top(&self, h: f32) -> (Rect, Rect) {
        (Rect::new(self.x, self.y, self.w, h), Rect::new(self.x, self.y + h, self.w, (self.h - h).max(0.0)))
    }
    /// Round to whole pixels (crisp square blocks and 1 px outlines).
    pub fn snap(&self) -> Rect {
        let (x0, y0) = (self.x.round(), self.y.round());
        Rect::new(x0, y0, (self.right().round() - x0).max(0.0), (self.bottom().round() - y0).max(0.0))
    }
    pub fn take_bottom(&self, h: f32) -> (Rect, Rect) {
        (Rect::new(self.x, self.y, self.w, (self.h - h).max(0.0)), Rect::new(self.x, self.bottom() - h, self.w, h))
    }
}

// ---------------------------------------------------------------- fonts ----

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Face {
    /// Instrument Serif: display, used sparingly (titles, names, clock).
    Serif,
    SerifItalic,
    /// Onest: UI text.
    Body400,
    Body500,
    Body600,
    Mono400,
    Mono600,
}

const FACE_FILES: [(Face, &str); 7] = [
    (Face::Serif, "InstrumentSerif-Regular.ttf"),
    (Face::SerifItalic, "InstrumentSerif-Italic.ttf"),
    (Face::Body400, "Onest-Regular.ttf"),
    (Face::Body500, "Onest-Medium.ttf"),
    (Face::Body600, "Onest-SemiBold.ttf"),
    (Face::Mono400, "GeistMono-Regular.ttf"),
    (Face::Mono600, "GeistMono-SemiBold.ttf"),
];

/// A text style: face, pixel size, colour, CSS-like letter-spacing (em) and
/// line-height (multiple of size) used for vertical centring.
#[derive(Clone, Copy, Debug)]
pub struct TextStyle {
    pub face: Face,
    pub size: f32,
    pub color: Color,
    pub tracking: f32,
    pub line_height: f32,
}

impl TextStyle {
    pub fn new(face: Face, size: f32, color: Color) -> TextStyle {
        TextStyle { face, size, color, tracking: 0.0, line_height: 1.35 }
    }
    pub fn tracking(mut self, em: f32) -> Self {
        self.tracking = em;
        self
    }
    pub fn line_height(mut self, lh: f32) -> Self {
        self.line_height = lh;
        self
    }
    pub fn color(mut self, c: Color) -> Self {
        self.color = c;
        self
    }
    pub fn line_px(&self) -> f32 {
        self.size * self.line_height
    }
}

/// Where to find the font files: $MORTAR_UI_FONTS, then the directory baked in
/// at build time by Nix, then ./fonts.
pub fn font_dir() -> PathBuf {
    if let Ok(d) = std::env::var("MORTAR_UI_FONTS") {
        return PathBuf::from(d);
    }
    if let Some(d) = option_env!("MORTAR_UI_FONT_DIR") {
        return PathBuf::from(d);
    }
    PathBuf::from("fonts")
}

pub struct Fonts {
    faces: HashMap<Face, fontdue::Font>,
}

impl Fonts {
    pub fn load(dir: &Path) -> Result<Fonts, String> {
        let mut faces = HashMap::new();
        for (face, file) in FACE_FILES {
            let p = dir.join(file);
            let data = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            let font = fontdue::Font::from_bytes(data, fontdue::FontSettings::default())
                .map_err(|e| format!("{}: {e}", p.display()))?;
            faces.insert(face, font);
        }
        Ok(Fonts { faces })
    }
    /// The font for a face (for programs that lay out text themselves).
    pub fn face(&self, f: Face) -> &fontdue::Font {
        &self.faces[&f]
    }
}

// ------------------------------------------------------------------- ui ----

type GlyphKey = (Face, u16, u32);

pub struct Ui {
    pub pm: Pixmap,
    fonts: Fonts,
    glyphs: HashMap<GlyphKey, (fontdue::Metrics, Vec<u8>)>,
    icons: HashMap<(Icon, u32, Color, u32), Pixmap>,
}

impl Ui {
    pub fn new(w: u32, h: u32, fonts: Fonts) -> Ui {
        Ui { pm: Pixmap::new(w, h).expect("canvas size"), fonts, glyphs: HashMap::new(), icons: HashMap::new() }
    }
    pub fn width(&self) -> f32 {
        self.pm.width() as f32
    }
    pub fn height(&self) -> f32 {
        self.pm.height() as f32
    }
    /// CSS container-query width unit: 1cqw = 1% of the screen width. The
    /// mockups are written in cqw, so layouts use this to match them exactly.
    pub fn cq(&self, v: f32) -> f32 {
        v * self.width() / 100.0
    }
    pub fn bounds(&self) -> Rect {
        Rect::new(0.0, 0.0, self.width(), self.height())
    }

    pub fn clear(&mut self, c: Color) {
        self.pm.fill(c.sk());
    }

    fn paint(c: Color) -> Paint<'static> {
        let mut p = Paint::default();
        p.set_color(c.sk());
        p.anti_alias = true;
        p
    }

    pub fn fill_rect(&mut self, r: Rect, c: Color) {
        if let Some(sr) = SkRect::from_xywh(r.x, r.y, r.w, r.h) {
            self.pm.fill_rect(sr, &Self::paint(c), Transform::identity(), None);
        }
    }

    /// A square block on whole pixels (no anti-aliased edges).
    pub fn fill_block(&mut self, r: Rect, c: Color) {
        let r = r.snap();
        if let Some(sr) = SkRect::from_xywh(r.x, r.y, r.w, r.h) {
            let mut p = Self::paint(c);
            p.anti_alias = false;
            self.pm.fill_rect(sr, &p, Transform::identity(), None);
        }
    }

    /// A crisp outline `w` px wide inside `r` (CSS border, box-sizing:
    /// border-box), drawn as four blocks so 1 px stays 1 px.
    pub fn outline(&mut self, r: Rect, w: f32, c: Color) {
        let r = r.snap();
        let w = w.round().max(1.0);
        self.fill_block(Rect::new(r.x, r.y, r.w, w), c);
        self.fill_block(Rect::new(r.x, r.bottom() - w, r.w, w), c);
        self.fill_block(Rect::new(r.x, r.y + w, w, r.h - 2.0 * w), c);
        self.fill_block(Rect::new(r.right() - w, r.y + w, w, r.h - 2.0 * w), c);
    }

    /// A rounded (pill) shape filled and outlined, for toggles and pills.
    pub fn pill_shape(&mut self, r: Rect, fill: Color, line: Color, w: f32) {
        let rad = r.h / 2.0;
        self.fill_rrect(r, rad, line);
        self.fill_rrect(r.inset(w, w), rad - w, fill);
    }

    fn poly_path(pts: &[(f32, f32)], close: bool) -> Option<tiny_skia::Path> {
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

    /// Fill a polygon (illustrations).
    pub fn fill_poly(&mut self, pts: &[(f32, f32)], c: Color) {
        if let Some(p) = Self::poly_path(pts, true) {
            self.pm.fill_path(&p, &Self::paint(c), FillRule::Winding, Transform::identity(), None);
        }
    }

    /// Stroke a polyline (illustrations: 1 px ink line art).
    pub fn stroke_poly(&mut self, pts: &[(f32, f32)], close: bool, width: f32, c: Color) {
        if let Some(p) = Self::poly_path(pts, close) {
            let s = Stroke { width, line_join: tiny_skia::LineJoin::Round, line_cap: tiny_skia::LineCap::Round, ..Stroke::default() };
            self.pm.stroke_path(&p, &Self::paint(c), &s, Transform::identity(), None);
        }
    }

    fn rrect_path(r: Rect, rad: f32) -> Option<tiny_skia::Path> {
        let rad = rad.min(r.w / 2.0).min(r.h / 2.0).max(0.0);
        if rad <= 0.0 {
            return SkRect::from_xywh(r.x, r.y, r.w, r.h).map(PathBuilder::from_rect);
        }
        let k = 0.552_284_8 * rad;
        let (x0, y0, x1, y1) = (r.x, r.y, r.right(), r.bottom());
        let mut pb = PathBuilder::new();
        pb.move_to(x0 + rad, y0);
        pb.line_to(x1 - rad, y0);
        pb.cubic_to(x1 - rad + k, y0, x1, y0 + rad - k, x1, y0 + rad);
        pb.line_to(x1, y1 - rad);
        pb.cubic_to(x1, y1 - rad + k, x1 - rad + k, y1, x1 - rad, y1);
        pb.line_to(x0 + rad, y1);
        pb.cubic_to(x0 + rad - k, y1, x0, y1 - rad + k, x0, y1 - rad);
        pb.line_to(x0, y0 + rad);
        pb.cubic_to(x0, y0 + rad - k, x0 + rad - k, y0, x0 + rad, y0);
        pb.close();
        pb.finish()
    }

    pub fn fill_rrect(&mut self, r: Rect, rad: f32, c: Color) {
        if let Some(p) = Self::rrect_path(r, rad) {
            self.pm.fill_path(&p, &Self::paint(c), FillRule::Winding, Transform::identity(), None);
        }
    }

    /// A border drawn inside `r` (like CSS border with box-sizing: border-box).
    pub fn stroke_rrect(&mut self, r: Rect, rad: f32, width: f32, c: Color) {
        let h = width / 2.0;
        if let Some(p) = Self::rrect_path(r.inset(h, h), (rad - h).max(0.0)) {
            let s = Stroke { width, ..Stroke::default() };
            self.pm.stroke_path(&p, &Self::paint(c), &s, Transform::identity(), None);
        }
    }

    pub fn fill_circle(&mut self, cx: f32, cy: f32, r: f32, c: Color) {
        if let Some(p) = PathBuilder::from_circle(cx, cy, r) {
            self.pm.fill_path(&p, &Self::paint(c), FillRule::Winding, Transform::identity(), None);
        }
    }

    pub fn stroke_circle(&mut self, cx: f32, cy: f32, r: f32, width: f32, c: Color) {
        if let Some(p) = PathBuilder::from_circle(cx, cy, r - width / 2.0) {
            let s = Stroke { width, ..Stroke::default() };
            self.pm.stroke_path(&p, &Self::paint(c), &s, Transform::identity(), None);
        }
    }

    // --------------------------------------------------------- text ----

    /// Width of a single line of text.
    pub fn measure(&self, s: &str, st: &TextStyle) -> f32 {
        let font = &self.fonts.faces[&st.face];
        let mut w = 0.0;
        let mut prev: Option<u16> = None;
        let n = s.chars().count();
        for (i, ch) in s.chars().enumerate() {
            let idx = font.lookup_glyph_index(ch);
            if let Some(p) = prev {
                w += font.horizontal_kern_indexed(p, idx, st.size).unwrap_or(0.0);
            }
            w += font.metrics_indexed(idx, st.size).advance_width;
            if i + 1 < n {
                w += st.tracking * st.size;
            }
            prev = Some(idx);
        }
        w
    }

    /// Baseline offset from the top of a CSS line box of this style.
    fn baseline_in_line(&self, st: &TextStyle) -> f32 {
        let font = &self.fonts.faces[&st.face];
        let lm = font.horizontal_line_metrics(st.size).expect("horizontal font");
        let content = lm.ascent - lm.descent;
        (st.line_px() - content) / 2.0 + lm.ascent
    }

    /// Draw one line of text whose CSS line box starts at `top`. Returns the
    /// advance width.
    pub fn text(&mut self, x: f32, top: f32, s: &str, st: &TextStyle) -> f32 {
        let base = top + self.baseline_in_line(st);
        self.text_baseline(x, base, s, st)
    }

    /// Draw text centred vertically on `cy` (for buttons and badges).
    pub fn text_vcenter(&mut self, x: f32, cy: f32, s: &str, st: &TextStyle) -> f32 {
        self.text(x, cy - st.line_px() / 2.0, s, st)
    }

    /// Draw text, truncating with an ellipsis to fit `max_w`.
    pub fn text_fit(&mut self, x: f32, top: f32, s: &str, st: &TextStyle, max_w: f32) -> f32 {
        if self.measure(s, st) <= max_w {
            return self.text(x, top, s, st);
        }
        let chars: Vec<char> = s.chars().collect();
        for n in (0..chars.len()).rev() {
            let t: String = chars[..n].iter().collect::<String>().trim_end().to_string() + "…";
            if self.measure(&t, st) <= max_w {
                return self.text(x, top, &t, st);
            }
        }
        0.0
    }

    pub fn text_baseline(&mut self, x: f32, base: f32, s: &str, st: &TextStyle) -> f32 {
        let font = &self.fonts.faces[&st.face];
        let mut pen = x;
        let mut prev: Option<u16> = None;
        let n = s.chars().count();
        for (i, ch) in s.chars().enumerate() {
            let idx = font.lookup_glyph_index(ch);
            if let Some(p) = prev {
                pen += font.horizontal_kern_indexed(p, idx, st.size).unwrap_or(0.0);
            }
            let key = (st.face, idx, (st.size * 64.0) as u32);
            let (m, cov) = self.glyphs.entry(key).or_insert_with(|| font.rasterize_indexed(idx, st.size));
            let gx = (pen + m.xmin as f32).round() as i32;
            let gy = (base - m.height as f32 - m.ymin as f32).round() as i32;
            blit_coverage(&mut self.pm, gx, gy, m.width, m.height, cov, st.color);
            pen += m.advance_width;
            if i + 1 < n {
                pen += st.tracking * st.size;
            }
            prev = Some(idx);
        }
        pen - x
    }

    // -------------------------------------------------------- icons ----

    /// Draw a 24x24-viewBox line icon scaled to `size` px at (x, y). `round`
    /// gives round caps and joins (menu glyphs and tiles use them, the status
    /// bar doesn't).
    pub fn icon(&mut self, icon: Icon, x: f32, y: f32, size: f32, color: Color, stroke: f32, round: bool) {
        let key = (icon, (size * 4.0) as u32, color, (stroke * 100.0) as u32 * 2 + round as u32);
        self.icons.entry(key).or_insert_with(|| {
            let px = size.ceil().max(1.0) as u32;
            let svg = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="{px}" height="{px}"><g fill="none" stroke="{c}" stroke-width="{stroke}" color="{c}"{caps}>{body}</g></svg>"#,
                c = color.css(),
                caps = if round { r#" stroke-linecap="round" stroke-linejoin="round""# } else { "" },
                body = icon.svg()
            );
            let mut pm = Pixmap::new(px, px).expect("icon size");
            if let Ok(tree) = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()) {
                let s = px as f32 / tree.size().width();
                resvg::render(&tree, Transform::from_scale(s, s), &mut pm.as_mut());
            }
            pm
        });
        let pm = &self.icons[&key];
        self.pm.draw_pixmap(
            x.round() as i32,
            y.round() as i32,
            pm.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }

    /// The canvas as tightly packed 0x00RRGGBB words (what KMS XRGB8888 and
    /// minifb want). The canvas is opaque, so premultiplication is a no-op.
    pub fn xrgb_row(&self, y: u32, out: &mut [u32]) {
        let w = self.pm.width() as usize;
        let row = &self.pm.data()[(y as usize) * w * 4..(y as usize + 1) * w * 4];
        for (o, p) in out.iter_mut().zip(row.chunks_exact(4)) {
            *o = (p[0] as u32) << 16 | (p[1] as u32) << 8 | p[2] as u32;
        }
    }

    pub fn save_png(&self, path: &Path) -> Result<(), String> {
        self.pm.save_png(path).map_err(|e| e.to_string())
    }
}

fn blit_coverage(pm: &mut Pixmap, x: i32, y: i32, w: usize, h: usize, cov: &[u8], c: Color) {
    let (pw, ph) = (pm.width() as i32, pm.height() as i32);
    let data = pm.data_mut();
    let ca = c.3 as u32;
    for row in 0..h as i32 {
        let py = y + row;
        if py < 0 || py >= ph {
            continue;
        }
        for col in 0..w as i32 {
            let px = x + col;
            if px < 0 || px >= pw {
                continue;
            }
            let a = cov[(row as usize) * w + col as usize] as u32 * ca / 255;
            if a == 0 {
                continue;
            }
            let o = ((py * pw + px) * 4) as usize;
            let inv = 255 - a;
            // premultiplied source-over
            data[o] = ((c.0 as u32 * a + data[o] as u32 * inv) / 255) as u8;
            data[o + 1] = ((c.1 as u32 * a + data[o + 1] as u32 * inv) / 255) as u8;
            data[o + 2] = ((c.2 as u32 * a + data[o + 2] as u32 * inv) / 255) as u8;
            data[o + 3] = (a + data[o + 3] as u32 * inv / 255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn icon_inner_paths() {
        let svg = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="96" height="96"><g fill="none" stroke="#ff0000" stroke-width="2" color="#ff0000">{}</g></svg>"##,
            crate::icons::Icon::Pad.svg()
        );
        let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).unwrap();
        let mut pm = resvg::tiny_skia::Pixmap::new(96, 96).unwrap();
        resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pm.as_mut());
        pm.save_png(std::env::var("ICON_OUT").unwrap_or("/dev/null".into())).unwrap();
        // centre of the D-pad cross (7, 12.5) * 4
        let px = pm.pixel(28, 50).unwrap();
        assert!(px.alpha() > 128, "cross not drawn: {px:?}");
    }
}
