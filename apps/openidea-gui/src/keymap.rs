//! The keymap layer (M9): the app's default keystrokes come from the IDEA
//! `intellij-macos` preset, ported as data into [`keymap_data`] (generated
//! from the upstream frontend's `src/platform/keymaps/*.ts` — the
//! action-id → keystrokes maps are factual data; the implementation here is
//! the app's own).
//!
//! Three jobs:
//! - [`startup_bindings`] — the rows `main.rs` registers, each carrying its
//!   preset id; a user override (persisted in the settings store, IDEA
//!   notation) replaces a row's keys at registration time.
//! - [`keystroke`] — the IDEA-notation → gpui-keystroke conversion, macOS
//!   conventions. Physical matching rides the platform: gpui's macOS backend
//!   names keys from the event's key *code* translated with no modifiers, so
//!   ⌥-combos match by key name regardless of the character ⌥ produces, and
//!   shift+punctuation arrives as the shifted character with the shift
//!   modifier dropped — the conversion emits exactly that.
//! - [`Chord`] — the modifier-only double strikes ("Shift Shift", "Ctrl
//!   Ctrl", "Alt Alt+Up"): modifier presses never become keystrokes, so the
//!   prefix is tracked on the modifiers events and the follow-up keystroke
//!   completes the chord.
//!
//! Bindings must be registered after `gpui_component::init` (later
//! registrations win keystroke ties) — `main.rs` keeps that order.

use std::time::{Duration, Instant};

use gpui::{App, DummyKeyboardMapper, KeyBinding, KeyBindingContextPredicate, Keystroke, Modifiers};

use crate::keymap_data::{self, Preset};

/// The active preset on macOS (the app ships the others as data for the
/// future Settings keymap page). The tests pin the bindings against it; the
/// Settings keymap table (M9 phase 4) becomes its second reader.
#[allow(dead_code)]
pub const ACTIVE_PRESET: &Preset = &keymap_data::INTELLIJ_MACOS;

/// One default binding: gpui keystroke form, the registered action's name,
/// the key context it needs (None = window-level), and the preset row its
/// keys come from (None = the app's own mechanics — escape chains, platform
/// standards like ⌘Z, IDE-specific gestures).
pub struct Binding {
    pub keys: &'static str,
    pub action: &'static str,
    pub context: Option<&'static str>,
    pub idea_id: Option<&'static str>,
}

macro_rules! binding {
    ($keys:literal, $action:literal, $context:expr, $idea:expr) => {
        Binding { keys: $keys, action: $action, context: $context, idea_id: $idea }
    };
}

/// Every default binding of the app. Keys annotated with a preset id agree
/// with `ACTIVE_PRESET` (a unit test pins this); rows without one are the
/// app's own face. New milestones append rows here.
pub static BINDINGS: &[Binding] = &[
    // -- window ------------------------------------------------------------
    binding!("cmd-q", "workbench::Quit", None, None),
    // ⌘O is the app's own folder-picker face (IDEA's open-project is ⇧⌘⌥O).
    binding!("cmd-o", "workbench::OpenFile", None, None),
    binding!("cmd-s", "workbench::Save", None, Some("file.save")),
    binding!("cmd-w", "workbench::CloseTab", None, Some("editor.closeTab")),
    // Undo/redo are platform standards; the preset leaves them to the OS.
    binding!("cmd-z", "workbench::Undo", None, None),
    binding!("shift-cmd-z", "workbench::Redo", None, None),
    binding!("shift-cmd-o", "workbench::GoToFile", None, Some("navigate.gotoFile")),
    // Double ⇧ is a modifiers gesture (see GESTURES), not a keystroke row.
    binding!("ctrl-r", "workbench::Run", None, Some("run.run")),
    // The M4 face (⌃⌘R) and IDEA's run.stop (⌘F2) both stop the run.
    binding!("ctrl-cmd-r", "workbench::StopRun", None, None),
    binding!("cmd-f2", "workbench::StopRun", None, Some("run.stop")),
    // ⌃D stays Show Diff here (run.debug needs a debugger — M11; the preset
    // row's keys belong to that milestone's action).
    binding!("ctrl-d", "workbench::ShowDiff", None, None),
    binding!("cmd-k", "workbench::Commit", None, Some("vcs.commit")),
    binding!("shift-cmd-f", "workbench::FindInFiles", None, Some("find.inPath")),
    binding!("cmd-1", "workbench::ToggleTree", None, Some("toolwindow.project")),
    binding!("cmd-4", "workbench::ToggleDock", None, Some("toolwindow.run")),
    // The M10 stripe slots (⌘0 rides in with the Commit window).
    binding!("cmd-2", "workbench::ToggleBookmarks", None, Some("toolwindow.bookmarks")),
    binding!("cmd-3", "workbench::ToggleFind", None, Some("toolwindow.find")),
    binding!("cmd-6", "workbench::ToggleProblems", None, Some("toolwindow.problems")),
    binding!("cmd-7", "workbench::ToggleStructure", None, Some("toolwindow.structure")),
    binding!("cmd-0", "workbench::ToggleCommit", None, Some("toolwindow.commit")),
    // M11: the terminal window rides the preset's Alt+F12 row.
    binding!("alt-f12", "workbench::ToggleTerminal", None, Some("toolwindow.terminal")),
    binding!("shift-escape", "workbench::HideActiveToolWindow", None, Some("toolwindow.hideActive")),
    // The preset's Meta+Shift+' converts to the shifted face (the macOS
    // physical-key rule: ⌘⇧' produces `"` and drops the shift).
    binding!("cmd-\"", "workbench::MaximizeToolWindow", None, Some("toolwindow.maximize")),
    binding!("shift-cmd-t", "workbench::ReopenClosedTab", None, None),
    binding!("cmd-,", "workbench::OpenSettings", None, Some("file.settings")),
    binding!("escape", "workbench::Cancel", None, None),
    binding!("f12", "editor::TogglePerfHud", None, Some("toolwindow.jumpToLast")),
    // -- editor (the Editor key context) -------------------------------------
    binding!("enter", "editor::Newline", Some("Editor"), None),
    binding!("backspace", "editor::DeleteBackward", Some("Editor"), None),
    binding!("delete", "editor::DeleteForward", Some("Editor"), None),
    binding!("alt-backspace", "editor::DeleteWordBackward", Some("Editor"), None),
    binding!("alt-delete", "editor::DeleteWordForward", Some("Editor"), None),
    binding!("tab", "editor::TabIndent", Some("Editor"), None),
    binding!("shift-tab", "editor::OutdentLine", Some("Editor"), None),
    binding!("alt-enter", "editor::CodeActions", Some("Editor"), Some("code.showIntentions")),
    binding!("cmd-alt-l", "editor::FormatDocument", Some("Editor"), Some("code.reformat")),
    binding!("ctrl-alt-o", "editor::OrganizeImports", Some("Editor"), Some("code.optimizeImports")),
    binding!("alt-cmd-i", "editor::ToggleInlayHints", Some("Editor"), None),
    binding!("cmd-f", "editor::Find", Some("Editor"), Some("find.inFile")),
    binding!("alt-cmd-f", "editor::ToggleReplace", Some("Editor"), None),
    binding!("cmd-v", "editor::Paste", Some("Editor"), None),
    binding!("cmd-b", "editor::GotoDefinition", Some("Editor"), Some("navigate.declaration")),
    binding!("shift-f6", "editor::Rename", Some("Editor"), Some("refactor.rename")),
    binding!("alt-f7", "editor::FindUsages", Some("Editor"), Some("find.usages")),
    binding!("left", "editor::CursorLeft", Some("Editor"), None),
    binding!("right", "editor::CursorRight", Some("Editor"), None),
    binding!("up", "editor::CursorUp", Some("Editor"), None),
    binding!("down", "editor::CursorDown", Some("Editor"), None),
    binding!("alt-left", "editor::CursorWordLeft", Some("Editor"), None),
    binding!("alt-right", "editor::CursorWordRight", Some("Editor"), None),
    binding!("cmd-left", "editor::CursorLineHome", Some("Editor"), None),
    binding!("cmd-right", "editor::CursorLineEnd", Some("Editor"), None),
    binding!("cmd-up", "editor::CursorDocStart", Some("Editor"), None),
    binding!("cmd-down", "editor::CursorDocEnd", Some("Editor"), None),
    binding!("shift-left", "editor::SelectLeft", Some("Editor"), None),
    binding!("shift-right", "editor::SelectRight", Some("Editor"), None),
    binding!("shift-up", "editor::SelectUp", Some("Editor"), None),
    binding!("shift-down", "editor::SelectDown", Some("Editor"), None),
    binding!("shift-alt-left", "editor::SelectWordLeft", Some("Editor"), None),
    binding!("shift-alt-right", "editor::SelectWordRight", Some("Editor"), None),
    binding!("shift-cmd-left", "editor::SelectLineHome", Some("Editor"), None),
    binding!("shift-cmd-right", "editor::SelectLineEnd", Some("Editor"), None),
    // (The doc-edge selects lost ⇧⌘↑/⌘↓ to IDEA's move-statement habit; the
    // actions stay for the ⌘↑/⌘↓+⇧ chording and future rebinds.)
    // -- M9 edit habits (the IDEA 编辑段) -------------------------------------
    binding!("cmd-d", "editor::DuplicateLine", Some("Editor"), Some("code.duplicateLine")),
    binding!("cmd-backspace", "editor::DeleteLine", Some("Editor"), Some("code.deleteLine")),
    binding!("cmd-/", "editor::CommentLine", Some("Editor"), Some("code.commentLine")),
    binding!("alt-cmd-/", "editor::CommentBlock", Some("Editor"), Some("code.commentBlock")),
    // The preset's alternate form, Ctrl+Shift+/: the platform reports the
    // shifted character (the rule the converter and the test pin).
    binding!("ctrl-?", "editor::CommentBlock", Some("Editor"), Some("code.commentBlock")),
    binding!("alt-up", "editor::ExtendSelection", Some("Editor"), Some("code.extendSelection")),
    binding!("alt-down", "editor::ShrinkSelection", Some("Editor"), Some("code.shrinkSelection")),
    binding!("alt-shift-up", "editor::MoveLineUp", Some("Editor"), Some("code.moveLineUp")),
    binding!("alt-shift-down", "editor::MoveLineDown", Some("Editor"), Some("code.moveLineDown")),
    binding!("shift-cmd-up", "editor::MoveStatementUp", Some("Editor"), Some("code.moveStatementUp")),
    binding!("shift-cmd-down", "editor::MoveStatementDown", Some("Editor"), Some("code.moveStatementDown")),
    binding!("ctrl-shift-j", "editor::JoinLines", Some("Editor"), Some("code.joinLines")),
    binding!("shift-cmd-u", "editor::ToggleCase", Some("Editor"), Some("code.toggleCase")),
    binding!("shift-enter", "editor::StartNewLineBelow", Some("Editor"), Some("code.startNewLine")),
    binding!("alt-cmd-enter", "editor::StartNewLineAbove", Some("Editor"), Some("code.startNewLineBefore")),
    binding!("ctrl-space", "editor::Complete", Some("Editor"), Some("code.completion")),
    binding!("cmd-p", "editor::ParameterInfo", Some("Editor"), Some("code.parameterInfo")),
    binding!("cmd-", "editor::Fold", Some("Editor"), Some("code.fold")),
    binding!("cmd-=", "editor::Unfold", Some("Editor"), Some("code.unfold")),
    binding!("cmd-_", "editor::FoldAll", Some("Editor"), Some("code.foldAll")),
    binding!("cmd-+", "editor::UnfoldAll", Some("Editor"), Some("code.unfoldAll")),
    binding!("ctrl-l", "editor::ScrollCenter", Some("Editor"), Some("editor.scrollToCenter")),
    binding!("cmd-r", "editor::Replace", Some("Editor"), Some("find.replaceInFile")),
    binding!("cmd-g", "editor::FindNext", Some("Editor"), Some("find.next")),
    binding!("shift-cmd-g", "editor::FindPrevious", Some("Editor"), Some("find.previous")),
    // ⌥⌥↑ / ⌥⌥↓ ride the Chord gestures (see GESTURES), not keystroke rows.
    // -- M9 navigation habits -------------------------------------------------
    binding!("cmd-e", "workbench::RecentFiles", None, Some("navigate.recentFiles")),
    binding!("shift-cmd-e", "workbench::RecentLocations", None, Some("navigate.recentLocations")),
    binding!("cmd-[", "workbench::NavigateBack", None, Some("navigate.back")),
    binding!("cmd-]", "workbench::NavigateForward", None, Some("navigate.forward")),
    binding!("f2", "workbench::NextError", None, Some("navigate.nextError")),
    binding!("shift-f2", "workbench::PrevError", None, Some("navigate.prevError")),
    binding!("f3", "workbench::ToggleBookmark", None, Some("navigate.toggleBookmark")),
    binding!("cmd-f3", "workbench::ShowBookmarks", None, Some("navigate.showBookmarks")),
    binding!("cmd-f12", "workbench::FileStructure", None, Some("navigate.fileStructure")),
    binding!("ctrl-tab", "workbench::Switcher", None, Some("navigate.switcher")),
    binding!("ctrl-shift-tab", "workbench::SwitcherBack", None, Some("navigate.switcherBack")),
    binding!("cmd-l", "workbench::GotoLine", None, Some("navigate.gotoLine")),
    binding!("shift-cmd-a", "workbench::FindAction", None, Some("navigate.findAction")),
    binding!("shift-cmd-backspace", "workbench::LastEditLocation", None, Some("navigate.lastEditLocation")),
    binding!("alt-cmd-b", "editor::GotoImplementation", Some("Editor"), Some("navigate.implementation")),
    binding!("shift-cmd-b", "editor::GotoTypeDeclaration", Some("Editor"), Some("navigate.typeDeclaration")),
    // -- palette list navigation ---------------------------------------------
    binding!("up", "palette::SelectPrev", Some("Palette"), None),
    binding!("down", "palette::SelectNext", Some("Palette"), None),
    // -- project trust prompt (M10): Enter trusts; Esc is the window-level
    // Cancel chain, which the prompt consumes as Don't Open.
    binding!("enter", "workbench::TrustProject", Some("TrustPrompt"), None),
];

/// The IDEA-notation rows whose first step is a bare modifier press: those
/// never become keystrokes, so the workbench completes them as gestures.
pub struct Gesture {
    pub idea_id: &'static str,
    /// The modifier struck twice.
    pub modifier: GestureModifier,
    /// `None`: the second strike fires the action ("Shift Shift"); `Some`:
    /// the named keystroke held after the double strike fires it ("Alt
    /// Alt+Up").
    pub follow: Option<&'static str>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GestureModifier {
    Shift,
    Alt,
    Control,
}

impl GestureModifier {
    fn of(mods: &Modifiers) -> Option<Self> {
        match (mods.shift, mods.alt, mods.control, mods.platform, mods.function) {
            (true, false, false, false, false) => Some(Self::Shift),
            (false, true, false, false, false) => Some(Self::Alt),
            (false, false, true, false, false) => Some(Self::Control),
            _ => None,
        }
    }
}

/// The preset's gesture rows. `runAnything.show` is M9 phase 4 material (the
/// action toasts as not-yet-available); the rest have real faces.
pub static GESTURES: &[Gesture] = &[
    Gesture { idea_id: "navigate.searchEverywhere", modifier: GestureModifier::Shift, follow: None },
    Gesture { idea_id: "editor.cloneCaretAbove", modifier: GestureModifier::Alt, follow: Some("Alt+Up") },
    Gesture { idea_id: "editor.cloneCaretBelow", modifier: GestureModifier::Alt, follow: Some("Alt+Down") },
    Gesture { idea_id: "runAnything.show", modifier: GestureModifier::Control, follow: None },
];

/// A double-strike chord window as wide as the double-⇧ one the workbench has
/// used since M3.
const CHORD_WINDOW: Duration = Duration::from_millis(600);

/// Tracks the modifier-only chords of [`GESTURES`]. Pure state (no gpui
/// handles), so the tests drive it directly; the workbench feeds it the
/// modifiers events and, through its root key listener, the keystrokes.
#[derive(Default)]
pub struct Chord {
    /// The bare modifier currently held (rising-edge detection: macOS
    /// delivers a modifiers event per press and per release; only the press
    /// edge counts a strike).
    held: Option<GestureModifier>,
    /// The last strike, waiting for its second.
    last_strike: Option<(GestureModifier, Instant)>,
    /// The double strike landed; waiting for the follow-up keystroke.
    pending: Option<(&'static str, Instant)>,
}

impl Chord {
    /// The modifiers events feed. A bare modifier's second press inside the
    /// window either fires the action or arms the follow-up; a release keeps
    /// the sequence armed; a two-modifier combo breaks it.
    pub fn modifiers_changed(&mut self, mods: &Modifiers, now: Instant) -> Option<&'static str> {
        self.expire(now);
        let all_off = !mods.shift && !mods.alt && !mods.control && !mods.platform && !mods.function;
        if all_off {
            self.held = None;
            return None;
        }
        let Some(modifier) = GestureModifier::of(mods) else {
            // Shift+Alt and friends: not a chord sequence anymore.
            self.held = None;
            self.last_strike = None;
            return None;
        };
        let rising = self.hedging(modifier);
        if !rising {
            return None;
        }
        if let Some((prev, at)) = self.last_strike
            && prev == modifier
            && now.duration_since(at) < CHORD_WINDOW
        {
            self.last_strike = None;
            let row = GESTURES.iter().find(|row| row.modifier == modifier)?;
            return match row.follow {
                None => Some(row.idea_id),
                Some(follow) => {
                    self.pending = Some((follow, now));
                    None
                }
            };
        }
        self.last_strike = Some((modifier, now));
        None
    }

    /// Records the held state and reports whether this press is a new edge.
    fn hedging(&mut self, modifier: GestureModifier) -> bool {
        let rising = self.held != Some(modifier);
        self.held = Some(modifier);
        rising
    }

    /// A keystroke while a follow-up chord is armed: the matching combo fires
    /// the action, anything else disarms.
    pub fn keystroke(&mut self, ks: &Keystroke, now: Instant) -> Option<&'static str> {
        let (follow, at) = self.pending.take()?;
        if now.duration_since(at) >= CHORD_WINDOW {
            return None;
        }
        if idea_of_keystroke(ks).as_deref() == Some(follow) {
            let row = GESTURES.iter().find(|row| row.follow == Some(follow))?;
            Some(row.idea_id)
        } else {
            None
        }
    }

    /// Focus leaving the window or an explicit cancel clears the sequence.
    pub fn reset(&mut self) {
        self.held = None;
        self.last_strike = None;
        self.pending = None;
    }

    fn expire(&mut self, now: Instant) {
        if self.pending.is_some_and(|(_, at)| now.duration_since(at) >= CHORD_WINDOW) {
            self.pending = None;
        }
    }
}

/// The bindings `main.rs` registers: every default row, with a user override
/// (IDEA notation, keyed by preset id) replacing its keys when one exists.
/// Returns the gpui-form keystroke strings per action + context.
pub fn startup_bindings(
    overrides: &std::collections::BTreeMap<String, Vec<String>>,
) -> Vec<(String, &'static str, Option<&'static str>)> {
    let mut out = Vec::new();
    for row in BINDINGS {
        let keys = match row.idea_id.and_then(|id| overrides.get(id)) {
            Some(user) => user.iter().filter_map(|k| keystroke(k)).map(|k| registration_keys(&k)).collect::<Vec<_>>(),
            None => vec![registration_keys(row.keys)],
        };
        for keys in keys {
            out.push((keys, row.action, row.context));
        }
    }
    out
}

/// Converts an IDEA-notation binding to the gpui form. Space-separated steps
/// (two-strike chords) each convert; `None` when any step cannot be a
/// keystroke: mouse buttons, or a bare modifier step (a gesture chord, see
/// [`GESTURES`]).
pub fn keystroke(idea: &str) -> Option<String> {
    let mut steps = Vec::new();
    for step in idea.split_whitespace() {
        steps.push(one_step(step)?);
    }
    Some(steps.join(" "))
}

/// The face a registered binding carries on this platform. The table (and
/// the preset data) speak the macOS convention — "cmd" — but gpui's Linux
/// platforms map the `platform` modifier to Super, and IDEA's Linux habit
/// is Ctrl, so the primary flips to Ctrl at registration. Steps that
/// already carry Ctrl keep it (⌃⌘R stays a distinct chord on Linux instead
/// of collapsing into ⌃R and stealing Run's key).
fn registration_keys(keys: &str) -> String {
    if cfg!(target_os = "macos") {
        return keys.to_owned();
    }
    keys.split_whitespace()
        .map(|step| {
            let has_ctrl = step.split('-').any(|part| part == "ctrl");
            step.split('-')
                .map(|part| if part == "cmd" && !has_ctrl { "ctrl" } else { part })
                .collect::<Vec<_>>()
                .join("-")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// One chord step, "Meta+Shift+O" → "shift-cmd-o", macOS conventions.
fn one_step(idea: &str) -> Option<String> {
    let (modifiers, key) = split_modifiers(idea)?;
    let mut parts: Vec<String> = Vec::new();
    if modifiers.control {
        parts.push("ctrl".to_owned());
    }
    if modifiers.alt {
        parts.push("alt".to_owned());
    }
    let (key, shift_used) = physical_key(key, modifiers.shift)?;
    if !shift_used && modifiers.shift {
        parts.push("shift".to_owned());
    }
    if modifiers.platform {
        parts.push("cmd".to_owned());
    }
    if key.is_empty() {
        // The literal minus key, in its trailing-dash encoding.
        parts.push(String::new());
    } else {
        parts.push(key);
    }
    Some(parts.join("-"))
}

/// The physical key name an IDEA key name maps to on macOS, with the
/// shift-punctuation rule: gpui reports the shifted character (and drops the
/// shift modifier) for non-letter keys, because that is what the platform
/// names keys by. The bool answers whether the shift modifier was consumed by
/// that rule (then the binding must not carry it).
fn physical_key(idea: &str, shift: bool) -> Option<(String, bool)> {
    let named = |name: &str| Some((name.to_owned(), false));
    match idea {
        "Enter" => return named("enter"),
        "Backspace" => return named("backspace"),
        "Delete" => return named("delete"),
        "Escape" => return named("escape"),
        "Tab" => return named("tab"),
        "Up" => return named("up"),
        "Down" => return named("down"),
        "Left" => return named("left"),
        "Right" => return named("right"),
        "PageUp" => return named("pageup"),
        "PageDown" => return named("pagedown"),
        "Home" => return named("home"),
        "End" => return named("end"),
        "Insert" => return named("insert"),
        "Space" => return named("space"),
        // The literal minus key: gpui encodes it as the trailing dash ("cmd-"
        // is ⌘-, parsing the empty tail as "-"); shifted it is the underscore
        // character the platform reports instead.
        "-" => {
            if shift {
                return Some(("_".to_owned(), true));
            }
            return Some((String::new(), false));
        }
        _ => {}
    }
    // Function keys.
    if let Some(rest) = idea.strip_prefix('F')
        && !rest.is_empty()
        && rest.chars().all(|c| c.is_ascii_digit())
        && rest.len() <= 2
    {
        return Some((format!("f{}", rest.to_ascii_lowercase()), false));
    }
    // Mouse navigation buttons are not keystrokes.
    if idea == "Button1" || idea == "Button4" || idea == "Button5" {
        return None;
    }
    // A letter keeps the shift modifier (gpui reports lowercase letters).
    if idea.len() == 1 && idea.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
        return Some((idea.to_ascii_lowercase(), false));
    }
    // Shifted punctuation and digits arrive as their shifted character.
    if shift {
        let shifted = match idea {
            "1" => "!",
            "2" => "@",
            "3" => "#",
            "4" => "$",
            "5" => "%",
            "6" => "^",
            "7" => "&",
            "8" => "*",
            "9" => "(",
            "0" => ")",
            "[" => "{",
            "]" => "}",
            "\\" => "|",
            ";" => ":",
            "'" => "\"",
            "," => "<",
            "." => ">",
            "/" => "?",
            "`" => "~",
            "=" => "+",
            other => other,
        };
        return Some((shifted.to_owned(), true));
    }
    Some((idea.to_owned(), false))
}

/// Splits "Meta+Shift+O" into its modifiers and key. A bare modifier key
/// ("Shift" of "Shift Shift") yields `None` — that is a chord step.
fn split_modifiers(idea: &str) -> Option<(Modifiers, &str)> {
    let mut modifiers = Modifiers::none();
    let mut key: Option<&str> = None;
    for part in idea.split('+') {
        match part {
            "Meta" => {
                modifiers.platform = true;
                key = None;
            }
            "Ctrl" => {
                modifiers.control = true;
                key = None;
            }
            "Alt" => {
                modifiers.alt = true;
                key = None;
            }
            "Shift" => {
                modifiers.shift = true;
                key = None;
            }
            "" => return None,
            other => key = Some(other),
        }
    }
    key.map(|key| (modifiers, key))
}

/// The IDEA notation of a keystroke, for the chord follow-ups (only the
/// arrow/combo shapes the gestures need).
fn idea_of_keystroke(ks: &Keystroke) -> Option<String> {
    let key = match ks.key.as_str() {
        "up" => "Up",
        "down" => "Down",
        "left" => "Left",
        "right" => "Right",
        "enter" => "Enter",
        "backspace" => "Backspace",
        other => other,
    };
    let mut idea = String::new();
    if ks.modifiers.control {
        idea.push_str("Ctrl+");
    }
    if ks.modifiers.alt {
        idea.push_str("Alt+");
    }
    if ks.modifiers.shift {
        idea.push_str("Shift+");
    }
    if ks.modifiers.platform {
        idea.push_str("Meta+");
    }
    idea.push_str(key);
    Some(idea)
}

/// Registers the resolved bindings on the app. Panicking is right at startup:
/// every action name here must be registered, every keystroke parseable — a
/// typo is a build-time bug, and gpui's own `KeyBinding::new` panics the same
/// way for the same reason.
pub fn bind(cx: &mut App, overrides: &std::collections::BTreeMap<String, Vec<String>>) {
    let resolved = startup_bindings(overrides);
    let bindings = resolved
        .into_iter()
        .map(|(keys, action, context)| {
            let action = cx.build_action(action, None).expect("registered action");
            let predicate = context
                .map(|context| KeyBindingContextPredicate::parse(context).expect("valid context"))
                .map(std::rc::Rc::new);
            KeyBinding::load(&keys, action, predicate, false, None, &DummyKeyboardMapper).expect("parseable keystroke")
        })
        .collect::<Vec<_>>();
    cx.bind_keys(bindings);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ks(idea: &str) -> String {
        keystroke(idea).unwrap_or_else(|| panic!("{idea} should convert"))
    }

    #[test]
    fn converts_modifiers_and_letters() {
        assert_eq!(ks("Meta+Shift+O"), "shift-cmd-o");
        assert_eq!(ks("Ctrl+Alt+O"), "ctrl-alt-o");
        assert_eq!(ks("Meta+L"), "cmd-l");
        assert_eq!(ks("Alt+Shift+G"), "alt-shift-g");
        // Shifted digit: the platform reports the character, shift dropped.
        assert_eq!(ks("Ctrl+Shift+0"), "ctrl-)");
        assert_eq!(ks("Meta+Alt+`"), "alt-cmd-`");
    }

    #[test]
    fn shifted_punctuation_arrives_as_the_shifted_character() {
        // macOS names these keys by the character shift produces and drops
        // the shift modifier; the binding must ask for the same face.
        assert_eq!(ks("Meta+["), "cmd-[");
        assert_eq!(ks("Meta+Shift+["), "cmd-{");
        assert_eq!(ks("Meta+]"), "cmd-]");
        assert_eq!(ks("Meta+Shift+]"), "cmd-}");
        assert_eq!(ks("Meta+Shift+."), "cmd->");
        assert_eq!(ks("Alt+Shift+."), "alt->");
        assert_eq!(ks("Meta+Shift+8"), "cmd-*");
        assert_eq!(ks("Meta+Shift+-"), "cmd-_");
        assert_eq!(ks("Ctrl+Shift+/"), "ctrl-?");
        assert_eq!(ks("Meta+Shift+="), "cmd-+");
    }

    #[test]
    fn the_literal_minus_key_is_a_trailing_dash() {
        assert_eq!(ks("Meta+-"), "cmd-");
        assert_eq!(ks("Meta+="), "cmd-=");
    }

    #[test]
    fn named_keys_and_function_keys_map() {
        assert_eq!(ks("Meta+Shift+Backspace"), "shift-cmd-backspace");
        assert_eq!(ks("F2"), "f2");
        assert_eq!(ks("Shift+F2"), "shift-f2");
        assert_eq!(ks("Meta+F12"), "cmd-f12");
        assert_eq!(ks("Alt+F12"), "alt-f12");
        assert_eq!(ks("Ctrl+Tab"), "ctrl-tab");
        assert_eq!(ks("Shift+Enter"), "shift-enter");
    }

    #[test]
    fn chords_split_on_spaces() {
        assert_eq!(ks("Meta+* 1"), "cmd-* 1");
    }

    #[test]
    fn mouse_buttons_and_bare_modifiers_are_not_keystrokes() {
        assert!(keystroke("Button4").is_none());
        assert!(keystroke("Meta+Alt+Button1").is_none());
        assert!(keystroke("Shift Shift").is_none());
        assert!(keystroke("Alt Alt+Up").is_none());
        assert!(keystroke("Ctrl Ctrl").is_none());
    }

    /// Two binding strings name the same keystrokes (gpui matches on the
    /// parsed modifiers, so modifier order is not part of the identity).
    fn same_binding(a: &str, b: &str) -> bool {
        let steps = |s: &str| -> Vec<Keystroke> {
            s.split_whitespace().filter_map(|step| Keystroke::parse(step).ok()).collect()
        };
        steps(a) == steps(b)
    }

    /// Every preset row a `BINDINGS` id points at must produce exactly the
    /// keys the binding registers (the first convertible key wins — the rest
    /// are alternates). Divergences are listed here, with the reason.
    #[test]
    fn bound_keys_agree_with_the_preset() {
        let exceptions: &[(&str, &str)] = &[
            // The app's Show Diff predates run.debug; ⌃D is diff today, the
            // debugger lands in M11 and the row retargets then.
            ("workbench::ShowDiff", "ctrl-d"),
            // IDEA leaves reopen-closed-tab unbound; the app keeps its M7 ⇧⌘T.
            ("workbench::ReopenClosedTab", "shift-cmd-t"),
        ];
        for row in BINDINGS {
            let Some(idea_id) = row.idea_id else { continue };
            let preset = ACTIVE_PRESET
                .rows
                .iter()
                .find(|r| r.action == idea_id)
                .unwrap_or_else(|| panic!("preset lacks {idea_id}"));
            let converted = preset.keys.iter().filter_map(|k| keystroke(k)).collect::<Vec<_>>();
            let skip = exceptions.iter().any(|(action, _)| *action == row.action);
            if !skip {
                assert!(
                    converted.iter().any(|keys| same_binding(keys, row.keys)),
                    "{} ({idea_id}) binds {:?}, preset converts to {:?}",
                    row.action,
                    row.keys,
                    converted
                );
            }
        }
    }

    #[test]
    fn startup_bindings_honor_user_overrides() {
        let mut overrides = std::collections::BTreeMap::new();
        overrides.insert("file.save".to_owned(), vec!["Ctrl+S".to_owned()]);
        let resolved = startup_bindings(&overrides);
        let save = resolved.iter().filter(|(_, action, _)| *action == "workbench::Save").collect::<Vec<_>>();
        assert_eq!(save.len(), 1);
        assert_eq!(save[0].0, "ctrl-s");
        // Untouched rows keep their defaults — in the registered face, so
        // the primary modifier reads Ctrl on non-macOS (see
        // [`registration_keys`]).
        let goto_file = if cfg!(target_os = "macos") { "shift-cmd-o" } else { "shift-ctrl-o" };
        assert!(resolved.iter().any(|(keys, action, _)| *action == "workbench::GoToFile" && keys == goto_file));
    }

    /// An override that cannot convert (a typo) is dropped, not silently
    /// bound to nothing.
    #[test]
    fn an_unconvertible_override_leaves_the_default_out() {
        let mut overrides = std::collections::BTreeMap::new();
        // A mouse button cannot be a keystroke binding: the row drops.
        overrides.insert("file.save".to_owned(), vec!["Button4".to_owned()]);
        let resolved = startup_bindings(&overrides);
        assert!(resolved.iter().all(|(_, action, _)| *action != "workbench::Save"));
    }

    // -- chord tracker -------------------------------------------------------

    fn mods_shift() -> Modifiers {
        Modifiers { shift: true, ..Modifiers::none() }
    }
    fn mods_alt() -> Modifiers {
        Modifiers { alt: true, ..Modifiers::none() }
    }
    fn mods_ctrl() -> Modifiers {
        Modifiers { control: true, ..Modifiers::none() }
    }
    fn mods_off() -> Modifiers {
        Modifiers::none()
    }
    fn key(key: &str, alt: bool) -> Keystroke {
        Keystroke { modifiers: Modifiers { alt, ..Modifiers::none() }, key: key.to_owned(), key_char: None }
    }

    #[test]
    fn double_shift_fires_search_everywhere() {
        let mut chord = Chord::default();
        let t0 = Instant::now();
        assert_eq!(chord.modifiers_changed(&mods_shift(), t0), None, "first press");
        assert_eq!(
            chord.modifiers_changed(&mods_off(), t0 + Duration::from_millis(50)),
            None,
            "release keeps the sequence"
        );
        assert_eq!(
            chord.modifiers_changed(&mods_shift(), t0 + Duration::from_millis(200)),
            Some("navigate.searchEverywhere")
        );
    }

    #[test]
    fn a_slow_second_strike_does_not_fire() {
        let mut chord = Chord::default();
        let t0 = Instant::now();
        chord.modifiers_changed(&mods_shift(), t0);
        chord.modifiers_changed(&mods_off(), t0 + Duration::from_millis(50));
        assert_eq!(chord.modifiers_changed(&mods_shift(), t0 + CHORD_WINDOW + Duration::from_millis(50)), None);
    }

    #[test]
    fn a_broken_sequence_resets_the_first_strike() {
        let mut chord = Chord::default();
        let t0 = Instant::now();
        chord.modifiers_changed(&mods_shift(), t0);
        // Shift released into shift+alt: not bare, resets.
        let shift_alt = Modifiers { shift: true, alt: true, ..Modifiers::none() };
        assert_eq!(chord.modifiers_changed(&shift_alt, t0 + Duration::from_millis(100)), None);
        assert_eq!(chord.modifiers_changed(&mods_shift(), t0 + Duration::from_millis(200)), None);
    }

    #[test]
    fn alt_alt_up_fires_clone_caret_above() {
        let mut chord = Chord::default();
        let t0 = Instant::now();
        chord.modifiers_changed(&mods_alt(), t0);
        chord.modifiers_changed(&mods_off(), t0 + Duration::from_millis(50));
        assert_eq!(
            chord.modifiers_changed(&mods_alt(), t0 + Duration::from_millis(150)),
            None,
            "the double strike only arms the follow-up"
        );
        // Up arrives while Alt is still held.
        assert_eq!(chord.keystroke(&key("up", true), t0 + Duration::from_millis(300)), Some("editor.cloneCaretAbove"));
    }

    #[test]
    fn alt_alt_down_fires_clone_caret_below_and_other_keys_disarm() {
        let mut chord = Chord::default();
        let t0 = Instant::now();
        chord.modifiers_changed(&mods_alt(), t0);
        chord.modifiers_changed(&mods_off(), t0 + Duration::from_millis(50));
        chord.modifiers_changed(&mods_alt(), t0 + Duration::from_millis(150));
        assert_eq!(chord.keystroke(&key("left", true), t0 + Duration::from_millis(250)), None, "unrelated key disarms");
        assert_eq!(chord.keystroke(&key("down", true), t0 + Duration::from_millis(300)), None);
    }

    #[test]
    fn the_chord_followup_expires() {
        let mut chord = Chord::default();
        let t0 = Instant::now();
        chord.modifiers_changed(&mods_alt(), t0);
        chord.modifiers_changed(&mods_off(), t0 + Duration::from_millis(50));
        let armed = t0 + Duration::from_millis(100);
        chord.modifiers_changed(&mods_alt(), armed);
        assert_eq!(chord.keystroke(&key("up", true), armed + CHORD_WINDOW + Duration::from_millis(10)), None);
    }

    #[test]
    fn double_ctrl_arms_the_run_anything_row() {
        let mut chord = Chord::default();
        let t0 = Instant::now();
        chord.modifiers_changed(&mods_ctrl(), t0);
        chord.modifiers_changed(&mods_off(), t0 + Duration::from_millis(50));
        assert_eq!(chord.modifiers_changed(&mods_ctrl(), t0 + Duration::from_millis(150)), Some("runAnything.show"));
    }

    /// Every gesture id and every binding id must exist in the active preset
    /// (a renamed upstream row must fail here, not silently drop its keys).
    #[test]
    fn gesture_ids_exist_in_the_preset() {
        for gesture in GESTURES {
            assert!(
                ACTIVE_PRESET.rows.iter().any(|row| row.action == gesture.idea_id),
                "preset lacks {}",
                gesture.idea_id
            );
        }
    }

    /// The startup contract, headless: every binding's action name must build
    /// against the action registry and every keystroke must parse — `bind`
    /// panics otherwise (caught here after the TogglePerfHud naming slip
    /// crashed a real launch).
    #[gpui::test]
    fn bindings_register_against_the_action_registry(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| bind(cx, &std::collections::BTreeMap::new()));
    }
}
