// SPDX-License-Identifier: MIT
//! Software framebuffer (0xAARRGGBB, one u32 per output pixel) and everything drawn into it:
//! PuzzleScript levels in their real colours, the 34x13 text terminal, and UI text in the
//! PuzzleScript font. The whole frame is uploaded to one streaming texture per redraw, so
//! the platform side only needs the simplest SDL_Renderer path (or a dumb framebuffer).
use crate::ffi::{self, Game, Session, PS_COLOR_TRANSPARENT, PS_TERMINAL_HEIGHT, PS_TERMINAL_WIDTH};

pub struct Canvas {
    pub w: i32,
    pub h: i32,
    pub px: Vec<u32>,
}

pub const fn rgb(c: u32) -> u32 {
    0xFF00_0000 | (c & 0xFF_FFFF)
}

pub fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |s: u32| {
        let x = ((a >> s) & 255) as f32;
        let y = ((b >> s) & 255) as f32;
        ((x + (y - x) * t).round() as u32) << s
    };
    0xFF00_0000 | ch(16) | ch(8) | ch(0)
}

impl Canvas {
    pub fn new(w: i32, h: i32) -> Canvas {
        Canvas { w, h, px: vec![0xFF00_0000; (w.max(1) * h.max(1)) as usize] }
    }

    pub fn clear(&mut self, c: u32) {
        self.px.fill(rgb(c));
    }

    pub fn fill(&mut self, x: i32, y: i32, w: i32, h: i32, c: u32) {
        let (x0, y0) = (x.max(0), y.max(0));
        let (x1, y1) = ((x + w).min(self.w), (y + h).min(self.h));
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let c = rgb(c);
        for yy in y0..y1 {
            let row = (yy * self.w) as usize;
            self.px[row + x0 as usize..row + x1 as usize].fill(c);
        }
    }

    /// Darkens/tints a rectangle towards `c` by `t` (for overlays).
    pub fn shade(&mut self, x: i32, y: i32, w: i32, h: i32, c: u32, t: f32) {
        let (x0, y0) = (x.max(0), y.max(0));
        let (x1, y1) = ((x + w).min(self.w), (y + h).min(self.h));
        for yy in y0..y1 {
            for xx in x0..x1 {
                let i = (yy * self.w + xx) as usize;
                self.px[i] = mix(self.px[i], c, t);
            }
        }
    }

    pub fn frame(&mut self, x: i32, y: i32, w: i32, h: i32, t: i32, c: u32) {
        self.fill(x, y, w, t, c);
        self.fill(x, y + h - t, w, t, c);
        self.fill(x, y, t, h, c);
        self.fill(x + w - t, y, t, h, c);
    }

    // ---- PuzzleScript font ------------------------------------------------------------------
    // Glyphs are 5x12 with a 6-pixel advance; rows 0..2 only hold accents, the cap height is
    // rows 3..9. `y` is the top of the 12-row cell.

    pub fn glyph(&mut self, cp: u32, x: i32, y: i32, s: i32, c: u32) {
        let rows = match ffi::glyph(cp).or_else(|| ffi::glyph('?' as u32)) {
            Some(r) => r,
            None => return,
        };
        for (r, bits) in rows.iter().enumerate() {
            for cx in 0..5 {
                if bits & (1 << (4 - cx)) != 0 {
                    self.fill(x + cx * s, y + r as i32 * s, s, s, c);
                }
            }
        }
    }

    /// Draws text at pixel scale `s`; returns the advance.
    pub fn text(&mut self, t: &str, x: i32, y: i32, s: i32, c: u32) -> i32 {
        let mut cx = x;
        for ch in t.chars() {
            if ch != ' ' {
                self.glyph(ch as u32, cx, y, s, c);
            }
            cx += 6 * s;
        }
        cx - x
    }

    pub fn text_centered(&mut self, t: &str, cx: i32, y: i32, s: i32, c: u32) {
        let w = text_width(t, s);
        self.text(t, cx - w / 2, y, s, c);
    }
}

pub fn text_width(t: &str, s: i32) -> i32 {
    t.chars().count() as i32 * 6 * s - s
}

/// Truncates `t` with an ellipsis so it fits `maxw` pixels at scale `s`.
pub fn fit(t: &str, maxw: i32, s: i32) -> String {
    if text_width(t, s) <= maxw {
        return t.to_string();
    }
    let n = ((maxw + s) / (6 * s)).max(1) as usize;
    let mut out: String = t.chars().take(n.saturating_sub(1)).collect();
    out = out.trim_end().to_string();
    out.push('\u{2026}');
    out
}

/// Word-wraps `t` to lines of at most `cols` characters.
pub fn wrap(t: &str, cols: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for para in t.split('\n') {
        let mut line = String::new();
        for word in para.split_whitespace() {
            let mut word = word.to_string();
            while word.chars().count() > cols {
                if !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                }
                let head: String = word.chars().take(cols).collect();
                word = word.chars().skip(cols).collect();
                lines.push(head);
            }
            let need = line.chars().count() + if line.is_empty() { 0 } else { 1 } + word.chars().count();
            if need > cols && !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(&word);
        }
        lines.push(line);
    }
    lines
}

// ---- game rendering -------------------------------------------------------------------------

/// Per-game render state: sprite bitmaps cached for the current cell size.
pub struct GameRender {
    base: i32,                 // largest sprite dimension (5 for nearly every game)
    cell: i32,                 // cell size the cache was built for
    cache: Vec<Vec<u32>>,      // per object: cell*cell pixels, 0 = transparent
    scanline: bool,
}

impl GameRender {
    pub fn new(g: &Game) -> GameRender {
        let mut base = 1;
        for o in g.objects() {
            base = base.max(o.spriteW).max(o.spriteH);
        }
        if g.objects().iter().all(|o| o.sprite.is_null()) {
            base = 5;
        }
        GameRender { base, cell: 0, cache: Vec::new(), scanline: g.raw().metadata.scanline }
    }

    fn build_cache(&mut self, g: &Game, cell: i32) {
        self.cell = cell;
        let c = cell as usize;
        self.cache = g
            .objects()
            .iter()
            .map(|o| {
                let mut bmp = vec![0u32; c * c];
                let colors: &[u32] = if o.colors.is_null() || o.ncolors <= 0 {
                    &[]
                } else {
                    unsafe { std::slice::from_raw_parts(o.colors, o.ncolors as usize) }
                };
                if o.sprite.is_null() || o.spriteW <= 0 || o.spriteH <= 0 {
                    // no sprite matrix: a solid square of the first colour (as the web player)
                    if let Some(&col) = colors.first() {
                        if col != PS_COLOR_TRANSPARENT {
                            bmp.fill(rgb(col));
                        }
                    }
                    return bmp;
                }
                let (sw, sh) = (o.spriteW as usize, o.spriteH as usize);
                let sprite = unsafe { std::slice::from_raw_parts(o.sprite, sw * sh) };
                // whole output pixels per sprite pixel when the cell allows it (crisp), else nearest
                let pw = (c / sw).max(1);
                let ph = (c / sh).max(1);
                let exact = c >= sw && c >= sh;
                for y in 0..c {
                    for x in 0..c {
                        let (sx, sy, within_y) = if exact {
                            (x / pw, y / ph, y % ph)
                        } else {
                            (x * sw / c, y * sh / c, 0)
                        };
                        if sx >= sw || sy >= sh {
                            continue;
                        }
                        if self.scanline && exact && within_y >= (ph + 1) / 2 {
                            continue;
                        }
                        let k = sprite[sy * sw + sx];
                        if k < 0 || k as usize >= colors.len() || colors[k as usize] == PS_COLOR_TRANSPARENT {
                            continue;
                        }
                        bmp[y * c + x] = rgb(colors[k as usize]);
                    }
                }
                bmp
            })
            .collect();
    }

    /// Draws the session (level or text terminal) filling the canvas.
    pub fn draw(&mut self, cv: &mut Canvas, g: &Game, s: &Session) {
        let md = &g.raw().metadata;
        if s.in_text_mode() {
            draw_terminal(cv, &s.text_lines(), md.background_color, md.text_color);
            return;
        }
        cv.clear(md.background_color);
        let stride = g.raw().strideObj as usize;
        let lv = match s.level(stride) {
            Some(l) => l,
            None => return,
        };
        let (mut x0, mut y0, mut x1, mut y1) = s.view_rect();
        x0 = x0.max(0);
        y0 = y0.max(0);
        x1 = x1.min(lv.width as i32);
        y1 = y1.min(lv.height as i32);
        let (vw, vh) = (x1 - x0, y1 - y0);
        if vw <= 0 || vh <= 0 {
            return;
        }
        // integer multiple of the sprite size, letterboxed (graphics.js canvasResize)
        let k = (cv.w / (vw * self.base)).min(cv.h / (vh * self.base));
        let cell = if k >= 1 { k * self.base } else { (cv.w / vw).min(cv.h / vh).max(1) };
        if cell != self.cell || self.cache.len() != g.objects().len() {
            self.build_cache(g, cell);
        }
        let ox = (cv.w - vw * cell) / 2;
        let oy = (cv.h - vh * cell) / 2;
        let nobj = g.objects().len();
        let c = cell as usize;
        for x in x0..x1 {
            for y in y0..y1 {
                let base = (x as usize * lv.height + y as usize) * stride;
                let cellbits = &lv.objects[base..base + stride];
                let px0 = ox + (x - x0) * cell;
                let py0 = oy + (y - y0) * cell;
                for (w, &word) in cellbits.iter().enumerate() {
                    let mut bits = word;
                    while bits != 0 {
                        let b = bits.trailing_zeros() as usize;
                        bits &= bits - 1;
                        let o = w * 32 + b;
                        if o >= nobj {
                            break;
                        }
                        let bmp = &self.cache[o];
                        for yy in 0..c {
                            let py = py0 + yy as i32;
                            if py < 0 || py >= cv.h {
                                continue;
                            }
                            let row = (py * cv.w) as usize;
                            for xx in 0..c {
                                let v = bmp[yy * c + xx];
                                let pxx = px0 + xx as i32;
                                if v != 0 && pxx >= 0 && pxx < cv.w {
                                    cv.px[row + pxx as usize] = v;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The 34x13 text terminal (title screen and messages) at the largest integer scale: 6x12
/// cells, as graphics.js lays it out.
pub fn draw_terminal(cv: &mut Canvas, lines: &[String], bg: u32, fg: u32) {
    cv.clear(bg);
    let tw = PS_TERMINAL_WIDTH as i32 * 6;
    let th = PS_TERMINAL_HEIGHT as i32 * 12;
    let s = (cv.w / tw).min(cv.h / th).max(1);
    let ox = (cv.w - tw * s) / 2;
    let oy = (cv.h - th * s) / 2;
    for (r, line) in lines.iter().enumerate().take(PS_TERMINAL_HEIGHT) {
        for (col, ch) in line.chars().enumerate().take(PS_TERMINAL_WIDTH) {
            if ch != ' ' {
                cv.glyph(ch as u32, ox + col as i32 * 6 * s, oy + r as i32 * 12 * s, s, fg);
            }
        }
    }
}

// ---- screenshots ----------------------------------------------------------------------------

pub fn save_png(cv: &Canvas, path: &std::path::Path) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let f = std::fs::File::create(path)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), cv.w as u32, cv.h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(std::io::Error::other)?;
    let mut data = Vec::with_capacity(cv.px.len() * 3);
    for &p in &cv.px {
        data.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
    }
    w.write_image_data(&data).map_err(std::io::Error::other)?;
    Ok(())
}
