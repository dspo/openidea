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
/// Editor selection background: the accent at ~24% alpha.
pub const SELECTION_BG: u32 = (ACCENT << 8) | 0x3c;
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

// Syntax palette: dark, low-contrast greys, one cool accent (same family as
// the chrome colors above); hardcoded like everything else in M2.
const SYNTAX_KEYWORD: u32 = 0x7aa7f5;
const SYNTAX_STRING: u32 = 0x8fbf7f;
const SYNTAX_ESCAPE: u32 = 0xd6a55f;
const SYNTAX_NUMBER: u32 = 0xd6a55f;
const SYNTAX_CONSTANT: u32 = 0xd19a66;
const SYNTAX_COMMENT: u32 = 0x6f7580;
const SYNTAX_FUNCTION: u32 = 0x74b6df;
const SYNTAX_TYPE: u32 = 0x5fbdb0;
const SYNTAX_PROPERTY: u32 = 0xbfc7d5;
const SYNTAX_MACRO: u32 = 0x8fb0d9;
const SYNTAX_OPERATOR: u32 = 0xa6abb5;

/// The color a highlight kind paints with.
pub fn syntax_color(kind: crate::highlight::Kind) -> gpui::Hsla {
    let c = match kind {
        crate::highlight::Kind::Keyword => SYNTAX_KEYWORD,
        crate::highlight::Kind::String => SYNTAX_STRING,
        crate::highlight::Kind::Escape => SYNTAX_ESCAPE,
        crate::highlight::Kind::Number => SYNTAX_NUMBER,
        crate::highlight::Kind::Constant => SYNTAX_CONSTANT,
        crate::highlight::Kind::Comment => SYNTAX_COMMENT,
        crate::highlight::Kind::Function => SYNTAX_FUNCTION,
        crate::highlight::Kind::Type => SYNTAX_TYPE,
        crate::highlight::Kind::Property => SYNTAX_PROPERTY,
        crate::highlight::Kind::Macro => SYNTAX_MACRO,
        crate::highlight::Kind::Operator => SYNTAX_OPERATOR,
    };
    rgba((c << 8) | 0xff).into()
}

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

/// Squiggle / gutter / tooltip color for a diagnostic severity.
pub fn severity_color_rgba(severity: ide_core::Severity) -> gpui::Rgba {
    let c = match severity {
        ide_core::Severity::Error => ERROR,
        ide_core::Severity::Warning => WARNING,
        ide_core::Severity::Info | ide_core::Severity::Hint => ACCENT,
    };
    rgba((c << 8) | 0xff)
}

pub fn severity_color(severity: ide_core::Severity) -> gpui::Hsla {
    severity_color_rgba(severity).into()
}
