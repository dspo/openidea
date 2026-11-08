//! The Git window's log-graph data plane (M13): lane assignment over the
//! commits ide-vcs's log returns. The connection edges come from upstream's
//! `Commit::graph_parents` (already rewritten for the visible filters — the
//! same data the Tauri host's log view draws), so this module only decides
//! which lane each commit and edge occupies: the classic `git log --graph`
//! walk, pure and unit-tested.

use ide_vcs::Commit;

/// One connection drawn from a commit's dot down to a parent's lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GraphEdge {
    /// The lane the edge leaves (the commit's own lane).
    pub from: u16,
    /// The lane it lands in (where the parent sits or will sit).
    pub to: u16,
    /// Upstream's `graph_dashed` flag: the parent is outside the filtered
    /// range (e.g. beyond a `retain_until` boundary).
    pub dashed: bool,
}

/// The graph face of one log row: where the dot sits, which lanes pass
/// straight through, and which edges leave the dot downward.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphRow {
    /// The commit's own lane.
    pub lane: u16,
    /// An edge from an earlier row reaches this dot (false for a tip the
    /// walk started fresh).
    pub incoming: bool,
    /// The lane the first parent continues in: equal to `lane` (a straight
    /// line below the dot) or another lane (the parent is shared with an
    /// earlier branch, so the line converges).
    pub first_out: Option<u16>,
    /// Merge edges leaving the dot (second and later parents).
    pub edges: Vec<GraphEdge>,
    /// Lanes occupied before this row that keep their line through it (full
    /// verticals; lanes a merge edge opens start at the row's bottom, not
    /// here).
    pub through: Vec<u16>,
    /// Number of lanes this row's drawing spans.
    pub width: u16,
}

/// Assigns lanes to `commits` (log order: children before parents).
pub fn assign_lanes(commits: &[Commit]) -> Vec<GraphRow> {
    let mut rows = Vec::with_capacity(commits.len());
    // Lane -> the commit id the lane's line currently leads to (`None` free).
    let mut active: Vec<Option<&str>> = Vec::new();
    for commit in commits {
        let parents: Vec<&str> = match &commit.graph_parents {
            Some(parents) => parents.iter().map(String::as_str).collect(),
            None => commit.parents.iter().map(String::as_str).collect(),
        };
        let dashed = |index: usize| commit.graph_dashed.get(index).copied().unwrap_or(false);
        // The lanes whose lines reach this row from above.
        let before: Vec<u16> = (0..active.len() as u16).filter(|lane| active[*lane as usize].is_some()).collect();
        let (lane, incoming) = match active.iter().position(|slot| *slot == Some(commit.id.as_str())) {
            Some(lane) => {
                active[lane] = None;
                (lane as u16, true)
            }
            None => (free_lane(&mut active), false),
        };
        let mut row = GraphRow { lane, incoming, first_out: None, edges: Vec::new(), through: Vec::new(), width: 0 };
        if let Some((first, _rest)) = parents.split_first() {
            match active.iter().position(|slot| *slot == Some(*first)) {
                // The first parent is already drawn elsewhere (a shared
                // parent): the line converges instead of continuing.
                Some(other) => row.first_out = Some(other as u16),
                None => {
                    active[lane as usize] = Some(first);
                    row.first_out = Some(lane);
                }
            }
        }
        for (index, parent) in parents.iter().enumerate().skip(1) {
            let to = match active.iter().position(|slot| *slot == Some(*parent)) {
                Some(to) => to as u16,
                None => {
                    let to = free_lane(&mut active);
                    active[to as usize] = Some(parent);
                    to
                }
            };
            row.edges.push(GraphEdge { from: lane, to, dashed: dashed(index) });
        }
        row.through = before.into_iter().filter(|lane| *lane != row.lane).collect();
        row.width = active.len() as u16;
        rows.push(row);
    }
    rows
}

/// The first free lane, or a new one after the occupied suffix (the reuse
/// keeps lines from drifting right as branches close).
fn free_lane(active: &mut Vec<Option<&str>>) -> u16 {
    if let Some(lane) = active.iter().position(|slot| slot.is_none()) {
        return lane as u16;
    }
    active.push(None);
    (active.len() - 1) as u16
}

/// The UTC face of an epoch-ms timestamp (the log rows and blame carry git's
/// committer/author times): `yyyy-MM-dd HH:mm`. Civil-from-days, no
/// time-zone dependency (dependency-free and stable; local time is a noted
/// limitation).
pub fn format_date_ms(ms: f64) -> String {
    let secs = (ms as i64).div_euclid(1000);
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (hour, min) = (rem / 3600, (rem % 3600) / 60);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    if month <= 2 {
        y += 1;
    }
    format!("{y:04}-{month:02}-{d:02} {hour:02}:{min:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(id: &str, parents: &[&str]) -> Commit {
        Commit {
            id: id.into(),
            short_id: id.into(),
            parents: parents.iter().map(|p| (*p).into()).collect(),
            graph_parents: Some(parents.iter().map(|p| (*p).into()).collect()),
            graph_dashed: Vec::new(),
            history_path: None,
            log_position: None,
            log_total: None,
            graph_boundary_parents: Vec::new(),
            on_current_branch: None,
            author: "a".into(),
            email: String::new(),
            committer: String::new(),
            committer_email: String::new(),
            time_ms: 0.0,
            summary: id.into(),
            message: id.into(),
            message_truncated: false,
            refs: Vec::new(),
            commit_time_ms: 0.0,
            dashed: Vec::new(),
        }
    }

    #[test]
    fn linear_history_stays_in_lane_zero() {
        let commits = [commit("c", &["p"]), commit("p", &["g"]), commit("g", &[])];
        let rows = assign_lanes(&commits);
        assert_eq!(rows.len(), 3);
        for (index, row) in rows.iter().enumerate() {
            assert_eq!(row.lane, 0, "row {index}");
            assert_eq!(row.width, 1);
            assert!(row.edges.is_empty());
            assert!(row.through.is_empty());
        }
        // The tips start fresh; the parents continue their child's line.
        assert!(!rows[0].incoming);
        assert!(rows[1].incoming && rows[2].incoming);
        assert_eq!(rows[0].first_out, Some(0));
        assert_eq!(rows[2].first_out, None, "a root has no continuing line");
    }

    #[test]
    fn a_branch_opens_a_second_lane_and_converges() {
        // c1 -> base; c2 -> base (two children of one commit).
        let commits = [commit("c1", &["base"]), commit("c2", &["base"]), commit("base", &[])];
        let rows = assign_lanes(&commits);
        // c1 claims lane 0, base continues there.
        assert_eq!(rows[0].lane, 0);
        assert_eq!(rows[0].first_out, Some(0));
        // c2 claims a fresh lane 1 and its first parent is already drawn in
        // lane 0, so the line converges.
        assert_eq!(rows[1].lane, 1);
        assert!(!rows[1].incoming);
        assert_eq!(rows[1].first_out, Some(0));
        assert_eq!(rows[1].width, 2);
        assert_eq!(rows[1].through, vec![0], "base's lane passes through c2's row");
        // base sits in lane 0.
        assert_eq!(rows[2].lane, 0);
    }

    #[test]
    fn a_merge_draws_an_edge_into_a_lane_that_starts_at_its_bottom() {
        // merge <- main, feature; main <- base; feature <- base.
        let commits = [
            commit("merge", &["main", "feature"]),
            commit("main", &["base"]),
            commit("feature", &["base"]),
            commit("base", &[]),
        ];
        let rows = assign_lanes(&commits);
        assert_eq!(rows[0].lane, 0);
        // The second parent opens lane 1 with a merge edge 0 -> 1; lane 1 is
        // NOT a pass-through (feature's line starts at this row's bottom).
        assert_eq!(rows[0].edges, vec![GraphEdge { from: 0, to: 1, dashed: false }]);
        assert_eq!(rows[0].first_out, Some(0));
        assert!(rows[0].through.is_empty());
        // main continues in lane 0; feature arrives through lane 1.
        assert_eq!(rows[1].lane, 0);
        assert_eq!(rows[2].lane, 1);
        assert!(rows[2].incoming);
        assert_eq!(rows[2].through, vec![0], "main's lane passes feature's row");
    }

    #[test]
    fn dashed_flags_reach_their_merge_edges() {
        let mut merge = commit("m", &["a", "b"]);
        merge.graph_dashed = vec![false, true];
        let commits = [merge, commit("a", &[]), commit("b", &[])];
        let rows = assign_lanes(&commits);
        assert_eq!(rows[0].edges, vec![GraphEdge { from: 0, to: 1, dashed: true }]);
        assert_eq!(rows[0].first_out, Some(0), "the undashed first parent continues straight");
    }

    #[test]
    fn lanes_are_reused_as_branches_close() {
        // Two pairs of branch tips over one base: after base's row frees its
        // lane, the later tips reuse lanes instead of drifting right.
        let commits = [
            commit("a1", &["base"]),
            commit("b1", &["base"]),
            commit("base", &[]),
            commit("a2", &["base"]),
            commit("b2", &["base"]),
        ];
        let rows = assign_lanes(&commits);
        let lanes: Vec<u16> = rows.iter().map(|row| row.lane).collect();
        assert_eq!(lanes, vec![0, 1, 0, 0, 1], "lane 0 is freed by base and reused by a2");
    }

    #[test]
    fn parents_fall_back_to_revision_parents_without_graph_parents() {
        let mut c = commit("c", &["p"]);
        c.graph_parents = None;
        let rows = assign_lanes(&[c, commit("p", &[])]);
        assert_eq!(rows[0].first_out, Some(0));
        assert_eq!(rows[1].lane, 0);
    }

    #[test]
    fn dates_format_as_utc() {
        assert_eq!(format_date_ms(0.0), "1970-01-01 00:00");
        // 2024-11-07 17:20:00 UTC.
        assert_eq!(format_date_ms(1_731_000_000_000.0), "2024-11-07 17:20");
    }
}
