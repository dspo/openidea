//! The tool-window stripe system (M10): the contribution table, the
//! per-window user state, and the state machine the stripes own — IDEA's
//! ToolWindowManager stripped to what this workbench renders.
//!
//! The model is pure: the workbench holds the state map, renders whatever
//! [`stripe_buttons`] / the visibility queries decide, and routes slot keys
//! through [`slot_window`]. The semantics follow the reference frontend's
//! extension points (clean-room: behavior notes only):
//!
//! - three anchors (left / right / bottom), each window orders within its
//!   anchor's stripe;
//! - `Stripe::Shown` windows carry a stripe button from launch; `OnUse`
//!   windows earn theirs at first use (Find, Usages…);
//! - `toolwindow.<id>` (⌘digit): activate = raise + focus, second press on
//!   the active window hides it; `Meta+Shift+digit` is this app's plain-hide;
//! - deferred windows register their data row (title, icon, slot) but never
//!   render — the milestone that lights them flips `lit`.

use std::collections::BTreeMap;

// The full Lucide catalog (the legacy `gpui_component::IconName` subset
// misses most tool-window faces).
use gpui_kit_assets::IconName;

/// Which window edge a tool window docks to.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Anchor {
    Left,
    Right,
    Bottom,
}

/// Whether a window's stripe button exists from launch (`Shown`) or appears
/// at first use (`OnUse` — the Find / Usages habit).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stripe {
    Shown,
    OnUse,
}

/// One row of the tool-window table: the data a stripe button and a header
/// need, plus the milestone face (`lit`).
pub struct WindowDesc {
    pub id: &'static str,
    pub title: &'static str,
    pub icon: IconName,
    pub anchor: Anchor,
    /// Order within its anchor's stripe; lower first.
    pub order: u8,
    /// The ⌘digit slot (IDEA's Meta+1..Meta+0 map).
    pub slot: Option<u8>,
    pub stripe: Stripe,
    /// The keymap preset row this window's slot comes from (None = no slot
    /// bound yet). Data for the keymap/reconcile views (M10's menus read the
    /// table, not this field).
    #[allow(dead_code)]
    pub idea_id: Option<&'static str>,
    /// Deferred windows keep their row but never render (M11/M13 light them).
    pub lit: bool,
}

macro_rules! window {
    ($id:literal, $title:literal, $icon:ident, $anchor:ident, $order:literal, $slot:expr, $stripe:ident, $idea:expr, $lit:expr) => {
        WindowDesc {
            id: $id,
            title: $title,
            icon: IconName::$icon,
            anchor: Anchor::$anchor,
            order: $order,
            slot: $slot,
            stripe: Stripe::$stripe,
            idea_id: $idea,
            lit: $lit,
        }
    };
}

/// The whole table, lit and deferred together (the deferred rows are the
/// "缓建窗口留注册数据位" of the acceptance list).
pub const WINDOWS: &[WindowDesc] = &[
    // -- left stripe ---------------------------------------------------------
    window!("project", "Project", FolderOpen, Left, 0, Some(1), Shown, Some("toolwindow.project"), true),
    window!("bookmarks", "Bookmarks", BookMarked, Left, 1, Some(2), Shown, Some("toolwindow.bookmarks"), true),
    window!("structure", "Structure", ListTree, Left, 2, Some(7), Shown, Some("toolwindow.structure"), true),
    window!("commit", "Commit", GitCommitHorizontal, Left, 3, Some(0), Shown, Some("toolwindow.commit"), true),
    // -- bottom stripe -------------------------------------------------------
    window!("problems", "Problems", CircleAlert, Bottom, 0, Some(6), Shown, Some("toolwindow.problems"), true),
    window!("find", "Find", Search, Bottom, 1, Some(3), OnUse, Some("toolwindow.find"), true),
    window!("usages", "Find Usages", Search, Bottom, 2, None, OnUse, None, true),
    window!("run", "Run", Play, Bottom, 3, Some(4), Shown, Some("toolwindow.run"), true),
    // -- deferred (data rows only; the milestone that lights them) -----------
    window!("debug", "Debug", Bug, Bottom, 4, Some(5), Shown, Some("toolwindow.debug"), false),
    window!("services", "Services", Server, Bottom, 5, Some(8), Shown, Some("toolwindow.services"), false),
    window!("git", "Git", GitBranch, Bottom, 6, Some(9), Shown, Some("toolwindow.git"), false),
    window!("terminal", "Terminal", SquareTerminal, Bottom, 7, None, OnUse, Some("toolwindow.terminal"), true),
    window!("database", "Database", Database, Right, 0, None, Shown, None, false),
    window!("github", "Pull Requests", Github, Right, 1, None, Shown, None, false),
    window!("docker", "Docker", Container, Right, 2, None, Shown, None, false),
    window!("maven", "Maven", Package, Right, 3, None, Shown, None, false),
    window!("gradle", "Gradle", Hammer, Right, 4, None, Shown, None, false),
    window!("profiler", "Profiler", Gauge, Right, 5, None, OnUse, None, false),
];

/// The table row for a window id.
pub fn window(id: &str) -> Option<&'static WindowDesc> {
    WINDOWS.iter().find(|desc| desc.id == id)
}

/// A window's user state: where it docks, whether it is open, and the two
/// stripe decisions (`used` turns an OnUse button on; `stripe_shown` records
/// the user removing a button from the stripe).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WindowState {
    pub visible: bool,
    pub used: bool,
    pub stripe_shown: bool,
    /// None = follow the table's anchor (a Move to writes an override).
    pub anchor: Option<Anchor>,
}

impl WindowState {
    /// The fresh state: closed, its table stripe face.
    pub fn defaults(desc: &WindowDesc) -> Self {
        Self { visible: false, used: desc.stripe == Stripe::Shown, stripe_shown: true, anchor: None }
    }

    pub fn anchor_of<'a>(&'a self, desc: &'a WindowDesc) -> Anchor {
        self.anchor.unwrap_or(desc.anchor)
    }

    /// Whether the stripe carries this window's button now.
    pub fn stripe_button(&self, desc: &WindowDesc) -> bool {
        self.stripe_shown
            && match desc.stripe {
                Stripe::Shown => true,
                Stripe::OnUse => self.used,
            }
    }
}

/// The full state view: the user map plus the two workbench-level decisions
/// (which left window is up, which window is maximized).
pub struct Layout<'a> {
    pub windows: &'a BTreeMap<String, WindowState>,
    pub left_active: Option<&'a str>,
    pub maximized: Option<&'a str>,
}

impl<'a> Layout<'a> {
    /// The state for `id`, synthesizing defaults for windows the user never
    /// touched (the map stays sparse).
    pub fn state(&self, desc: &WindowDesc) -> WindowState {
        self.windows.get(desc.id).cloned().unwrap_or_else(|| WindowState::defaults(desc))
    }

    /// The lit windows of one anchor with a stripe button, in stripe order.
    pub fn stripe_buttons(&self, anchor: Anchor) -> Vec<(&'static WindowDesc, WindowState)> {
        let mut rows: Vec<_> = WINDOWS
            .iter()
            .filter(|desc| desc.lit && self.state(desc).anchor_of(desc) == anchor)
            .map(|desc| {
                let state = self.state(desc);
                (desc, state)
            })
            .collect();
        rows.retain(|(desc, state)| state.stripe_button(desc));
        rows.sort_by_key(|(desc, _)| desc.order);
        rows
    }

    /// The window a ⌘digit slot activates (lit windows only — a deferred
    /// slot keeps its preset row unbound).
    pub fn slot_window(&self, digit: u8) -> Option<&'static WindowDesc> {
        WINDOWS.iter().find(|desc| desc.lit && desc.slot == Some(digit))
    }

    /// Whether opening `id` would simply raise it (already the active window
    /// of its anchor) — the second ⌘digit press then hides it instead.
    pub fn is_active(&self, desc: &WindowDesc) -> bool {
        let state = self.state(desc);
        if !state.visible {
            return false;
        }
        match state.anchor_of(desc) {
            Anchor::Left => self.left_active == Some(desc.id),
            // The bottom area is one strip: the active page is the raised
            // window (the workbench's dock_page picks which).
            Anchor::Bottom | Anchor::Right => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(
        windows: &[(&str, WindowState)],
        left_active: Option<&'static str>,
        maximized: Option<&'static str>,
    ) -> Layout<'static> {
        let map = windows
            .iter()
            .map(|(id, state)| ((*id).to_owned(), state.clone()))
            .collect::<BTreeMap<String, WindowState>>();
        // Leaking the map is the test-harness price for Layout<'static>; the
        // production path borrows the settings struct.
        let leaked: &'static BTreeMap<String, WindowState> = Box::leak(Box::new(map));
        Layout { windows: leaked, left_active, maximized }
    }

    #[test]
    fn slot_map_is_the_preset_map() {
        // IDEA's Meta+1..Meta+0 map (the keymap preset rows pin the keys;
        // this pins which window answers).
        let l = layout(&[], None, None);
        let expect: [(u8, &str); 6] =
            [(1, "project"), (2, "bookmarks"), (3, "find"), (4, "run"), (6, "problems"), (7, "structure")];
        for (digit, id) in expect {
            assert_eq!(l.slot_window(digit).map(|d| d.id), Some(id), "slot {digit}");
        }
        // Deferred slots stay dark: no lit window answers 5 / 8 / 9 yet.
        assert!(l.slot_window(5).is_none());
        assert!(l.slot_window(8).is_none());
        assert!(l.slot_window(9).is_none());
    }

    #[test]
    fn stripes_order_by_anchor_and_filter_unlit_and_hidden() {
        let on_use_hidden =
            WindowState { used: false, ..WindowState::defaults(WINDOWS.iter().find(|d| d.id == "find").unwrap()) };
        let l = layout(
            &[
                ("find", on_use_hidden),
                (
                    "project",
                    WindowState {
                        stripe_shown: false,
                        ..WindowState::defaults(WINDOWS.iter().find(|d| d.id == "project").unwrap())
                    },
                ),
            ],
            None,
            None,
        );
        let left: Vec<&str> = l.stripe_buttons(Anchor::Left).iter().map(|(d, _)| d.id).collect();
        // Project's button was hidden by the user.
        assert_eq!(left, vec!["bookmarks", "structure", "commit"]);
        let bottom: Vec<&str> = l.stripe_buttons(Anchor::Bottom).iter().map(|(d, _)| d.id).collect();
        assert_eq!(bottom, vec!["problems", "run"], "find is OnUse and unused; usages has no button yet");
    }

    #[test]
    fn first_use_earns_the_on_use_button() {
        let mut used = WindowState::defaults(WINDOWS.iter().find(|d| d.id == "find").unwrap());
        used.used = true;
        let l = layout(&[("find", used)], None, None);
        let bottom: Vec<&str> = l.stripe_buttons(Anchor::Bottom).iter().map(|(d, _)| d.id).collect();
        assert_eq!(bottom, vec!["problems", "find", "run"]);
    }

    #[test]
    fn move_to_re_anchors_a_window() {
        let mut moved = WindowState::defaults(WINDOWS.iter().find(|d| d.id == "problems").unwrap());
        moved.anchor = Some(Anchor::Right);
        let l = layout(&[("problems", moved)], None, None);
        assert!(l.stripe_buttons(Anchor::Bottom).iter().all(|(d, _)| d.id != "problems"));
        let right: Vec<&str> = l.stripe_buttons(Anchor::Right).iter().map(|(d, _)| d.id).collect();
        assert_eq!(right, vec!["problems"], "the right stripe lights up around the moved window");
    }

    #[test]
    fn active_means_visible_and_raised() {
        let structure = WINDOWS.iter().find(|d| d.id == "structure").unwrap();
        let open = || WindowState { visible: true, ..WindowState::defaults(structure) };
        let l = layout(&[("structure", open())], Some("structure"), None);
        assert!(l.is_active(structure), "visible + left-active");
        let l = layout(&[("structure", open())], Some("project"), None);
        assert!(!l.is_active(structure), "visible but another window is raised");
        let closed = WindowState::defaults(structure);
        let l = layout(&[("structure", closed)], Some("structure"), None);
        assert!(!l.is_active(structure), "not visible");
    }

    #[test]
    fn every_row_has_a_face() {
        for desc in WINDOWS {
            assert!(!desc.title.is_empty());
            // Every window's state synthesizes from its row alone.
            let _ = WindowState::defaults(desc);
        }
        // Slot collisions would make ⌘digit ambiguous.
        let mut slots: Vec<u8> = WINDOWS.iter().filter(|d| d.lit).filter_map(|d| d.slot).collect();
        slots.sort_unstable();
        slots.dedup();
        assert_eq!(slots.len(), WINDOWS.iter().filter(|d| d.lit).filter_map(|d| d.slot).count());
    }
}
