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
/// In-file find matches (M5): the warm warning tone at ~19% alpha, the
/// current match at ~33%.
const FIND_BG: u32 = (WARNING << 8) | 0x30;
const FIND_CURRENT_BG: u32 = (WARNING << 8) | 0x55;
/// Inlay hints (M6): the muted label tone, quieter than code so a hint reads
/// as meta.
const HINT_TEXT: u32 = 0x9dabbf;

pub const FONT_FAMILY: &str = "Menlo";
pub const FONT_SIZE: f32 = 14.0;
pub const LINE_HEIGHT: f32 = 1.5;

/// The live editor font size (the M7 setting): stored in 0.1 px units in an
/// atomic, so every per-frame reader (`px_font_size` call sites in the
/// editor/chrome) picks a change up on the next render without threading a
/// settings value through them.
static FONT_SIZE_CELL: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new((FONT_SIZE * 10.0) as usize);

pub fn set_font_size(size: f32) {
    FONT_SIZE_CELL.store((size * 10.0).round() as usize, std::sync::atomic::Ordering::Relaxed);
}

pub fn font_size() -> f32 {
    FONT_SIZE_CELL.load(std::sync::atomic::Ordering::Relaxed) as f32 / 10.0
}
/// Inset between the editor element bounds and the text.
pub const TEXT_INSET: f32 = 16.0;
/// Gutter paddings: element edge to line numbers, numbers to the text area.
pub const GUTTER_PAD_L: f32 = 12.0;
pub const GUTTER_PAD_R: f32 = 14.0;
/// Editor scrollbars (M5): track + thumb thickness and colors, always visible.
pub const SCROLLBAR_SIZE: f32 = 8.0;
const SCROLLBAR_TRACK: u32 = 0x26282b;
const SCROLLBAR_THUMB: u32 = 0x3f4349;

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

// Git status palette (M4): the IDE convention (modified blue, added green,
// untracked olive, deleted red), tuned to the low-contrast dark. Hardcoded
// like the syntax palette.
const GIT_MODIFIED: u32 = 0x74a3f0;
const GIT_ADDED: u32 = 0x6fbf7f;
const GIT_UNTRACKED: u32 = 0xc2a25f;
const GIT_DELETED: u32 = 0xd16b55;

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
/// An in-file find match (the current one paints [`find_current_bg`]).
pub fn find_bg() -> gpui::Rgba {
    rgba(FIND_BG)
}
pub fn find_current_bg() -> gpui::Rgba {
    rgba(FIND_CURRENT_BG)
}
/// Inlay hint labels (M6): a quieter tone than code so hints read as meta.
pub fn hint_text() -> gpui::Hsla {
    rgba((HINT_TEXT << 8) | 0xff).into()
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

pub fn scrollbar_track() -> gpui::Hsla {
    rgba((SCROLLBAR_TRACK << 8) | 0xff).into()
}

pub fn scrollbar_thumb() -> gpui::Hsla {
    rgba((SCROLLBAR_THUMB << 8) | 0xff).into()
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
    px(font_size())
}

pub fn px_line_height() -> Pixels {
    px(font_size() * LINE_HEIGHT)
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

/// The color a changed file's name paints with in the tree; `None` leaves the
/// default (unchanged files).
pub fn git_status_color(kind: ide_vcs::ChangeKind) -> Option<gpui::Hsla> {
    let c = match kind {
        ide_vcs::ChangeKind::Modified => GIT_MODIFIED,
        ide_vcs::ChangeKind::Added | ide_vcs::ChangeKind::Renamed => GIT_ADDED,
        ide_vcs::ChangeKind::Untracked => GIT_UNTRACKED,
        ide_vcs::ChangeKind::Deleted | ide_vcs::ChangeKind::Conflicted => GIT_DELETED,
        ide_vcs::ChangeKind::Ignored => return None,
    };
    Some(rgba((c << 8) | 0xff).into())
}

/// The diff view's `+` line color (the git added green).
pub fn diff_added() -> gpui::Hsla {
    rgba((GIT_ADDED << 8) | 0xff).into()
}

// The log graph's lane palette (M13): eight hues in the app's dark family,
// assigned by lane index (a branch's line keeps its color down its lane).
const GRAPH_LANES: [u32; 8] = [
    0x74a3f0, // blue (the modified tone)
    0x6fbf7f, // green (the added tone)
    0xd6a55f, // amber
    0xd19a66, // orange
    0xb48ce0, // purple
    0x5fbdb0, // teal
    0xd16b55, // red (the deleted tone)
    0x8fb0d9, // steel
];

/// The color of graph lane `index` (wrapping past the palette).
pub fn graph_lane_color(index: u16) -> gpui::Hsla {
    rgba((GRAPH_LANES[(index as usize) % GRAPH_LANES.len()] << 8) | 0xff).into()
}

// The terminal surface's palette (M11): the app's dark greys bridge into the
// ghostty core's RGB (programs' own palette/OSC colors still win per-index;
// these are the defaults the render state resolves against).
pub fn terminal_fg() -> terminal_ghostty::Rgb {
    terminal_ghostty::Rgb::new(0xd1, 0xd3, 0xd9)
}

pub fn terminal_bg() -> terminal_ghostty::Rgb {
    terminal_ghostty::Rgb::new(0x19, 0x1a, 0x1c)
}
