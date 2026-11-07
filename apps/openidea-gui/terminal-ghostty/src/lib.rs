//! Ghostty-backed terminal surface: the libghostty-vt emulation core (Zig,
//! statically linked) behind a self-contained GPUI element — the M11
//! integrated terminal's engine, mounted by the workbench's Terminal tool
//! window.
//!
//! Public surface:
//! - [`spawn_ghostty_view`] — spawn a `$SHELL` PTY rooted at a directory and
//!   return a ready-to-embed [`GhosttyView`]; [`Theme`] carries the embedder's
//!   foreground/background (this crate owns no palette)
//! - [`GhosttyView`] — the component: rendering, keyboard/IME input,
//!   render-driven resize, paste; [`GhosttyView::live_title`] mirrors the
//!   terminal's OSC title, [`GhosttyView::live_cwd`] its OSC 7 working
//!   directory, and [`GhosttyView::kill`] ends the process (dropping the view
//!   kills it too)
//! - [`TerminalProcess`] / [`ProcessOutput`] — the process seam for hosts
//!   whose process is owned elsewhere; the built-in [`pty::Pty`] implements it
//!   for shell tabs
//! - [`terminal`] — the safe wrapper over the C API (terminal state, render
//!   snapshots, key encoding), for callers that need to go below the view
//! - [`sys`] — raw FFI bindings (doc-hidden; the transcription reference is
//!   the Ghostty rev pinned in `build.rs`)
//!
//! Build prerequisites and the artifact cache are documented in `build.rs`:
//! `GHOSTTY_SRC` / `GHOSTTY_ZIG` override the sources and toolchain, and
//! `TERMINAL_GHOSTTY_CACHE` moves the build cache out of `OUT_DIR` so the zig
//! build is paid once per (Ghostty rev, zig identity) across worktrees.
//!
//! Known gaps (deliberate, the M11 minimal face):
//! - no text selection / copy, no scrollback indicator, no search overlay;
//! - no mouse reporting: TUIs that ask for it (`htop`) see the wheel as
//!   scrollback only and never receive clicks (vim/less work keyboard-first);
//! - a paste libghostty refuses (`GHOSTTY_REJECTED`: the text could inject
//!   commands outside bracketed-paste mode) is dropped;
//! - in-flight IME composition is not painted (committed text is);
//! - shell integration (OSC 7 pwd reporting) covers zsh and bash — fish/nu
//!   fall back to the project root for the new-tab cwd.

mod process;
pub mod pty;
pub mod shell_integration;
pub mod terminal;
mod view;

mod input;

#[doc(hidden)]
pub mod sys;

#[cfg(feature = "test-support")]
pub mod test_support;

pub use process::{ProcessOutput, TerminalProcess};
pub use terminal::{Cell, Frame, KeyEncoder, RenderState, Rgb, Terminal};
pub use view::{GhosttyEvent, GhosttyView, Theme, spawn_ghostty_view};
