//! The performance budget contract (M8): `perf/budgets.json` names each
//! measured metric with its debug/release budgets and the milestone
//! baselines it is judged against. The HUD shows the live key→px sample
//! against its budget; `script/verify.sh` is the authoritative gate — it
//! parses the transcript's `[perf]`/`[find]` lines into a measured JSON and
//! exits non-zero on a breach. The file is compiled in: a broken or missing
//! table is a build error, never a runtime surprise.

use serde::Deserialize;
use std::sync::OnceLock;

/// The compiled-in budget table.
pub fn budgets() -> &'static Budgets {
    static BUDGETS: OnceLock<Budgets> = OnceLock::new();
    BUDGETS
        .get_or_init(|| serde_json::from_str(include_str!("../perf/budgets.json")).expect("perf/budgets.json parses"))
}

#[derive(Debug, Deserialize)]
pub struct Budgets {
    pub metrics: Vec<Metric>,
}

#[derive(Debug, Deserialize)]
pub struct Metric {
    pub id: String,
    pub label: String,
    /// The debug-build budget (this is what `cargo run` sessions and the
    /// harness's debug leg judge).
    pub budget_ms: f32,
    pub budget_release_ms: f32,
    /// The prior milestone's number, when one exists (None: first
    /// instrumented this milestone).
    pub baseline_debug_ms: Option<f32>,
    pub baseline_release_ms: Option<f32>,
    pub source: String,
}

impl Budgets {
    /// The budget for `id` in the running build (`cfg!(debug_assertions)`).
    pub fn budget_for(&self, id: &str) -> Option<f32> {
        let release = !cfg!(debug_assertions);
        self.metrics
            .iter()
            .find(|metric| metric.id == id)
            .map(|metric| if release { metric.budget_release_ms } else { metric.budget_ms })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_parses_and_covers_the_contract_metrics() {
        let budgets = budgets();
        for id in [
            "key_to_px",
            "completion_first_frame",
            "find_in_files_first_response",
            "scroll_frame_during_index",
            "open_first_paint",
        ] {
            let metric = budgets
                .metrics
                .iter()
                .find(|metric| metric.id == id)
                .unwrap_or_else(|| panic!("budgets.json lacks {id}"));
            assert!(metric.budget_ms > 0.0 && metric.budget_release_ms > 0.0, "{id} budgets must be positive");
            assert!(!metric.source.is_empty(), "{id} must cite its source");
        }
        // The one budget the roadmap fixes (M2's 16 ms) stays honest.
        assert_eq!(budgets.budget_for("key_to_px"), Some(16.0));
    }
}
