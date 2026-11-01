// SPDX-License-Identifier: GPL-2.0-or-later
//! The Mortar UI theme: one set of palette, type and shape tokens shared by
//! the greeter, the quick menu (this crate) and mortar-shell's overlay.
//!
//! Style v2 follows the "Pond" visual language (bold flat colour blocks on a
//! pale blue-grey ground, 1 px ink outlines, square corners, lime selection
//! blocks, lavender/yellow/orange tags; Instrument Serif italic for big
//! moments, Onest for UI text, Geist Mono for labels and hints). Spec:
//! notes/ui/style.md in the planning notes.
//!
//! Light is the default; `MORTAR_UI_THEME=dark` (or `theme::set`) selects
//! the night variant, which keeps the same accent blocks and their inks.

use crate::gfx::Color;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub name: &'static str,
    /// Screen background.
    pub ground: Color,
    /// Recessed areas, inactive tiles, "off" pills.
    pub ground_2: Color,
    /// Ink: 1 px outlines and primary text.
    pub line: Color,
    /// Secondary text, dividers.
    pub line_soft: Color,
    /// Disabled, hint text.
    pub line_faint: Color,
    /// Selection / primary action block, and its text.
    pub lime: Color,
    pub lime_ink: Color,
    /// Tags, secondary accent.
    pub lavender: Color,
    pub lavender_ink: Color,
    /// Warnings, attention.
    pub yellow: Color,
    pub yellow_ink: Color,
    /// Destructive / power actions.
    pub orange: Color,
    pub orange_ink: Color,
    /// Dark surfaces (scrims, terminal).
    pub night: Color,
}

pub const LIGHT: Theme = Theme {
    name: "light",
    ground: Color::hex(0xe1ebf7),
    ground_2: Color::hex(0xbfcbd8),
    line: Color::hex(0x1d1d1d),
    line_soft: Color::hex(0x4c5164),
    line_faint: Color::hex(0x9ba7b5),
    lime: Color::hex(0xcef058),
    lime_ink: Color::hex(0x1b322d),
    lavender: Color::hex(0xcba6f7),
    lavender_ink: Color::hex(0x3f1d68),
    yellow: Color::hex(0xffd330),
    yellow_ink: Color::hex(0x1d1d1d),
    orange: Color::hex(0xff9f67),
    orange_ink: Color::hex(0x1d1d1d),
    night: Color::hex(0x1d1d1d),
};

pub const DARK: Theme = Theme {
    name: "dark",
    ground: Color::hex(0x1d1d1d),
    ground_2: Color::hex(0x2c2c2c),
    line: Color::hex(0xe1ebf7),
    line_soft: Color::hex(0x9ba7b5),
    line_faint: Color::hex(0x4c5164),
    night: Color::hex(0x111111),
    ..LIGHT
};

impl Theme {
    pub fn by_name(s: &str) -> Option<Theme> {
        match s {
            "light" | "" => Some(LIGHT),
            "dark" | "night" => Some(DARK),
            _ => None,
        }
    }
}

static CURRENT: OnceLock<Theme> = OnceLock::new();

/// Choose the theme before anything draws. Returns false if one was already
/// fixed (by an earlier `set` or `get`).
pub fn set(t: Theme) -> bool {
    CURRENT.set(t).is_ok()
}

/// The active theme: `set`, else `$MORTAR_UI_THEME`, else light.
pub fn get() -> &'static Theme {
    CURRENT.get_or_init(|| {
        std::env::var("MORTAR_UI_THEME").ok().and_then(|n| Theme::by_name(&n)).unwrap_or(LIGHT)
    })
}

// --------------------------------------------------------------- type ----
// Faces (see gfx::Face): Serif / SerifItalic = Instrument Serif,
// Body400/500/600 = Onest, Mono400/600 = Geist Mono.

// -------------------------------------------------------------- shape ----

/// Corner radius of boxes, tiles, rows and tags: square.
pub const RADIUS: f32 = 0.0;
/// Outline width in px at 1024 px wide (scaled, never below 1 px).
pub const HAIRLINE: f32 = 1.0;
/// Motion (for animated surfaces): default and drawer durations in ms.
pub const MOTION_MS: u32 = 150;
pub const DRAWER_MS: u32 = 560;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dark_keeps_accents() {
        assert_eq!(DARK.lime, LIGHT.lime);
        assert_ne!(DARK.ground, LIGHT.ground);
        assert_eq!(Theme::by_name("night"), Some(DARK));
    }
}
