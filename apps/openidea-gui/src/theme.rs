//! Palette. Hardcoded constants on purpose (M0/M1 carry no token/theme
//! layer); the values lift from the main app's `src/styles/tokens.css`:
//! dark, low-contrast greys, one cool accent.

use gpui::{Pixels, px, rgba};

pub const BG_APP: u32 = 0x191a1c;
pub const BG_SURFACE: u32 = 0x212326;
pub const BG_RAISED: u32 = 0x2a2d31;
pub const FG: u32 = 0xd1d3d9;
pub const FG_MUTED: u32 = 0x8c8f95;
pub const ACCENT: u32 = 0x3871e1;
pub const BORDER: u32 = 0x33363a;
pub const WARNING: u32 = 0xc9a23a;
pub const ERROR: u32 = 0xd16b55;
/// Marked-text background: the accent at ~18% alpha over the surface.
pub const MARKED_BG: u32 = (ACCENT << 8) | 0x2e;
/// Selected-row background in the file tree: the accent at ~22% alpha.
pub const SELECTED_BG: u32 = (ACCENT << 8) | 0x38;

pub const FONT_FAMILY: &str = "Menlo";
pub const FONT_SIZE: f32 = 14.0;
pub const LINE_HEIGHT: f32 = 1.5;
/// Inset between the editor element bounds and the text.
pub const TEXT_INSET: f32 = 16.0;
/// Gutter paddings: element edge to line numbers, numbers to the text area.
pub const GUTTER_PAD_L: f32 = 12.0;
pub const GUTTER_PAD_R: f32 = 14.0;

pub fn app_bg() -> gpui::Hsla {
    rgba((BG_APP << 8) | 0xff).into()
}
pub fn surface_bg() -> gpui::Hsla {
    rgba((BG_SURFACE << 8) | 0xff).into()
}
pub fn raised_bg() -> gpui::Hsla {
    rgba((BG_RAISED << 8) | 0xff).into()
}
pub fn selected_bg() -> gpui::Hsla {
    rgba(SELECTED_BG).into()
}
pub fn fg() -> gpui::Hsla {
    rgba((FG << 8) | 0xff).into()
}
pub fn fg_muted() -> gpui::Hsla {
    rgba((FG_MUTED << 8) | 0xff).into()
}
pub fn accent() -> gpui::Hsla {
    rgba((ACCENT << 8) | 0xff).into()
}
pub fn border() -> gpui::Hsla {
    rgba((BORDER << 8) | 0xff).into()
}
pub fn warning() -> gpui::Hsla {
    rgba((WARNING << 8) | 0xff).into()
}
pub fn error() -> gpui::Hsla {
    rgba((ERROR << 8) | 0xff).into()
}

pub fn font() -> gpui::Font {
    gpui::Font { family: FONT_FAMILY.into(), ..Default::default() }
}

pub fn px_font_size() -> Pixels {
    px(FONT_SIZE)
}

pub fn px_line_height() -> Pixels {
    px(FONT_SIZE * LINE_HEIGHT)
}
