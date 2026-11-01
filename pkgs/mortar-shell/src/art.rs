// SPDX-License-Identifier: GPL-2.0-or-later
//! The shell's palette and the isometric app illustrations ("Pond" style:
//! flat fills, 1.4 px ink outlines), ported from the prototype's SVG box
//! primitives and drawn as vector polygons (no bitmaps).
//!
//! Coordinates follow the prototype: an isometric projection
//! ip(x, y, z) = ((x - y) cos 30deg, (x + y) sin 30deg - z) inside a
//! 200 x 170 box centred on (0, 0).

use crate::scene::{Shape, Xf, B, P};
use mortar_ui::gfx::Color;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock};

/// Colours of the shell UI, day or night (style v2 tokens plus the
/// prototype's pink and tile colours).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pal {
    pub night: bool,
    pub ground: Color,
    pub ground_2: Color,
    pub ink: Color,
    pub soft: Color,
    pub faint: Color,
    pub tile: Color,
    pub lime: Color,
    pub lime_ink: Color,
    pub lavender: Color,
    pub lavender_ink: Color,
    pub yellow: Color,
    pub orange: Color,
    pub pink: Color,
    /// Fixed dark ink (text on yellow, screens in the art).
    pub dark: Color,
}

pub const DAY: Pal = Pal {
    night: false,
    ground: Color::hex(0xe1ebf7),
    ground_2: Color::hex(0xbfcbd8),
    ink: Color::hex(0x1d1d1d),
    soft: Color::hex(0x4c5164),
    faint: Color::hex(0x9ba7b5),
    tile: Color::hex(0xe1ebf7),
    lime: Color::hex(0xcef058),
    lime_ink: Color::hex(0x1b322d),
    lavender: Color::hex(0xcba6f7),
    lavender_ink: Color::hex(0x3f1d68),
    yellow: Color::hex(0xffd330),
    orange: Color::hex(0xff9f67),
    pink: Color::hex(0xff77a8),
    dark: Color::hex(0x1d1d1d),
};

pub const NIGHT: Pal = Pal {
    night: true,
    ground: Color::hex(0x1d1d1d),
    ground_2: Color::hex(0x2c2c2c),
    ink: Color::hex(0xe1ebf7),
    soft: Color::hex(0x9ba7b5),
    faint: Color::hex(0x5d6672),
    tile: Color::hex(0x24272b),
    ..DAY
};

/// Which illustration an app gets (config `art`, else guessed from its id,
/// name and icon).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Art {
    M8,
    Pico8,
    Games,
    Claude,
    Puzzle,
    Term,
    #[default]
    Generic,
}

impl Art {
    pub fn guess(id: &str, name: &str, icon: mortar_ui::icons::Icon) -> Art {
        let s = format!("{id} {name}").to_ascii_lowercase();
        use mortar_ui::icons::Icon;
        if s.contains("claude") || icon == Icon::Claude {
            Art::Claude
        } else if s.contains("m8") {
            Art::M8
        } else if s.contains("pico") {
            Art::Pico8
        } else if s.contains("puzzle") {
            Art::Puzzle
        } else if s.contains("game") || s.contains("play") || s.contains("retro") {
            Art::Games
        } else if icon == Icon::Term || s.contains("term") || s.contains("shell") {
            Art::Term
        } else {
            Art::Generic
        }
    }
    /// The colour the app grows out of its tile in (launch transition).
    pub fn accent(self, p: &Pal) -> Color {
        match self {
            Art::M8 => p.yellow,
            Art::Pico8 => p.pink,
            Art::Games => p.lime,
            Art::Claude => p.orange,
            Art::Puzzle => p.lavender,
            Art::Term => DAY.ground_2,
            Art::Generic => p.lavender,
        }
    }
}

const C30: f32 = 0.866_025_4;
const S30: f32 = 0.5;

fn ip(x: f32, y: f32, z: f32) -> P {
    ((x - y) * C30, (x + y) * S30 - z)
}

/// Draws illustrations into a builder; `ink` is the outline colour
/// (currentColor: ink, or lime-ink on a selected tile).
pub struct Iso<'b, 'a> {
    pub b: &'b mut B<'a>,
    pub pal: Pal,
    pub ink: Color,
}

impl Iso<'_, '_> {
    fn poly(&mut self, pts: &[P], fill: Color, stroke: bool) {
        self.b.poly(pts, fill);
        if stroke {
            self.b.stroke(pts, true, 1.4, self.ink);
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn boxed(&mut self, x: f32, y: f32, z: f32, w: f32, d: f32, h: f32, top: Color, side: Color) {
        let t = [ip(x, y, z + h), ip(x + w, y, z + h), ip(x + w, y + d, z + h), ip(x, y + d, z + h)];
        let l = [ip(x, y + d, z), ip(x + w, y + d, z), ip(x + w, y + d, z + h), ip(x, y + d, z + h)];
        let r = [ip(x + w, y, z), ip(x + w, y + d, z), ip(x + w, y + d, z + h), ip(x + w, y, z + h)];
        self.poly(&l, side, true);
        self.poly(&r, side, true);
        self.poly(&t, top, true);
    }
    fn bx(&mut self, x: f32, y: f32, z: f32, w: f32, d: f32, h: f32, top: Color) {
        let side = self.pal.tile;
        self.boxed(x, y, z, w, d, h, top, side);
    }
}

pub fn spark() -> &'static Arc<Shape> {
    static S: OnceLock<Arc<Shape>> = OnceLock::new();
    S.get_or_init(|| Arc::new(Shape::from_path(mortar_ui::icons::CLAUDE_SPARK_PATH)))
}

/// The Claude Spark, flat (never skewed), `size` px wide, centred on (cx, cy),
/// in its Clay colour.
pub fn draw_spark(b: &mut B, cx: f32, cy: f32, size: f32) {
    let s = size / 94.0;
    let m = Xf::translate(cx - 47.0 * s, cy - 47.0 * s).mul(&Xf::scale(s, s));
    b.shape(spark(), m, Color::hex(mortar_ui::icons::CLAUDE_CLAY));
}

/// Draw `art` with its 200x170 box centred on (cx, cy), scaled by `scale`.
pub fn draw(b: &mut B, art: Art, cx: f32, cy: f32, scale: f32, pal: Pal, ink: Color) {
    let (dx, dy) = match art {
        Art::M8 => (0.0, 20.0),
        Art::Pico8 => (0.0, 40.0),
        Art::Games => (0.0, 36.0),
        Art::Claude => (0.0, 36.0),
        Art::Puzzle => (0.0, 26.0),
        Art::Term => (0.0, 30.0),
        Art::Generic => (0.0, 30.0),
    };
    b.push(Xf::translate(cx, cy).mul(&Xf::scale(scale, scale)).mul(&Xf::translate(dx, dy)));
    let p = pal;
    let mut iso = Iso { b, pal, ink };
    let dark = p.dark;
    match art {
        Art::M8 => {
            iso.bx(-46.0, -30.0, 0.0, 92.0, 60.0, 12.0, p.yellow);
            iso.poly(&[ip(-40.0, -24.0, 12.0), ip(10.0, -24.0, 12.0), ip(10.0, 10.0, 12.0), ip(-40.0, 10.0, 12.0)], dark, true);
            for i in 0..2 {
                for j in 0..4 {
                    iso.bx(18.0 + i as f32 * 11.0, -24.0 + j as f32 * 12.0, 12.0, 8.0, 8.0, 4.0, p.lavender);
                }
            }
        }
        Art::Pico8 => {
            iso.bx(-34.0, -10.0, 0.0, 68.0, 18.0, 92.0, p.tile);
            iso.poly(&[ip(-26.0, 8.0, 18.0), ip(26.0, 8.0, 18.0), ip(26.0, 8.0, 76.0), ip(-26.0, 8.0, 76.0)], p.pink, true);
            iso.poly(&[ip(-18.0, 8.0, 50.0), ip(18.0, 8.0, 50.0), ip(18.0, 8.0, 68.0), ip(-18.0, 8.0, 68.0)], dark, true);
        }
        Art::Games => {
            let cols = [p.tile, p.lavender, p.lime];
            for (i, c) in cols.iter().enumerate() {
                let i = i as f32;
                iso.bx(-40.0 + i * 4.0, -26.0 - i * 3.0, i * 16.0, 70.0, 46.0, 14.0, *c);
            }
        }
        Art::Claude => {
            iso.bx(-34.0, -34.0, 0.0, 68.0, 68.0, 22.0, p.tile);
            // Anthropic's Spark floats above the plinth, flat (unskewed)
            let c = ip(0.0, 0.0, 70.0);
            draw_spark(iso.b, c.0, c.1, 78.0);
        }
        Art::Puzzle => {
            for i in 0..3 {
                for j in 0..3 {
                    let raised = (i == 0 && j == 1) || (i == 2 && j == 2);
                    let me = i == 1 && j == 1;
                    let h = if raised { 26.0 } else if me { 14.0 } else { 4.0 };
                    let top = if me { p.orange } else if raised { p.lavender } else { p.tile };
                    iso.bx(-45.0 + i as f32 * 30.0, -45.0 + j as f32 * 30.0, 0.0, 30.0, 30.0, h, top);
                }
            }
        }
        Art::Term => {
            iso.bx(-44.0, -10.0, 0.0, 88.0, 16.0, 66.0, p.tile);
            iso.poly(&[ip(-36.0, 6.0, 10.0), ip(36.0, 6.0, 10.0), ip(36.0, 6.0, 58.0), ip(-36.0, 6.0, 58.0)], dark, true);
            iso.poly(&[ip(-28.0, 6.0, 22.0), ip(-12.0, 6.0, 22.0), ip(-12.0, 6.0, 28.0), ip(-28.0, 6.0, 28.0)], p.lime, false);
            iso.bx(-20.0, -30.0, -8.0, 40.0, 30.0, 6.0, p.tile);
        }
        Art::Generic => {
            iso.bx(-40.0, -40.0, 0.0, 80.0, 80.0, 16.0, p.tile);
            iso.bx(-22.0, -22.0, 16.0, 44.0, 44.0, 22.0, p.lavender);
            iso.bx(-10.0, -10.0, 38.0, 20.0, 20.0, 18.0, p.yellow);
        }
    }
    b.pop();
}

#[cfg(test)]
mod tests {
    use super::*;
    use mortar_ui::icons::Icon;

    #[test]
    fn guesses() {
        assert_eq!(Art::guess("m8", "M8", Icon::Usb), Art::M8);
        assert_eq!(Art::guess("games", "PuzzleScript", Icon::Pad), Art::Puzzle);
        assert_eq!(Art::guess("play", "Games", Icon::Pad), Art::Games);
        assert_eq!(Art::guess("claude-pad", "Claude", Icon::Pad), Art::Claude);
        assert_eq!(Art::guess("terminal", "Terminal", Icon::Term), Art::Term);
        assert_eq!(Art::guess("pico8", "PICO-8", Icon::Pad), Art::Pico8);
        assert_eq!(Art::guess("x", "Thing", Icon::Gear), Art::Generic);
    }
}
