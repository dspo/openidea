#!/bin/bash
# The M8 regression harness: one command, three stages, non-zero on any
# failure. Local-runnable only (no CI).
#
#   1. Engineering gate: cargo fmt --check, clippy -D warnings, unit tests.
#   2. Screenshot smoke: a scripted session drives a fixed state set
#      (open → HUD → find bar → settings overlay), window-scoped captures
#      under /tmp, transcript markers asserted.
#   3. Budget gate: the perf scenario (debug + release, Go module) measured
#      fresh, medians compared against perf/budgets.json.
#
# Needs an unlocked session for stages 2–3 (synthetic input + window
# captures). Scratch lives under /tmp; only PIDs this script started are
# killed.
set -euo pipefail
cd "$(dirname "$0")/.."

OUT=${M8_VERIFY_OUT:-/tmp/m8-verify}
REPO_PROJ=${M8_VERIFY_REPO:-}
mkdir -p "$OUT"

echo "== stage 1: engineering gate (fmt + clippy + tests)"
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --quiet

# A locked screen cannot drive stages 2–3: stop with a clear message rather
# than a pile of capture failures.
if [ -n "${CGSSessionScreenIsLocked:-}" ]; then
  echo "== stage 2/3 skipped: the screen is locked (CGSSessionScreenIsLocked set)"
  echo "   unlock and re-run for the smoke + budget legs."
  exit 0
fi

echo "== stage 2: screenshot smoke"
# The smoke drives this crate's own source (a real, self-contained project).
python3 script/verify.py smoke --binary target/debug/openidea-gui \
  --project . --open src/main.rs \
  --outdir "$OUT" --name smoke >/dev/null

echo "== stage 3: budget gate (Go module scenario, debug + release)"
SCRATCH=$(mktemp -d /tmp/m8-budget-XXXXXX)
bash script/gen-go-module.sh "$SCRATCH" 100 >/dev/null
cargo build --quiet
cargo build --release --quiet
python3 script/verify.py perf --binary target/debug/openidea-gui \
  --project "$SCRATCH" --open pkg001/api.go --outdir "$OUT" --name gate-debug >/dev/null
python3 script/verify.py perf --release --binary target/release/openidea-gui \
  --project "$SCRATCH" --open pkg001/api.go --outdir "$OUT" --name gate-release >/dev/null
python3 script/budget_gate.py "$OUT/gate-debug-measured.json" "$OUT/gate-release-measured.json"

echo "== verify: all stages green (evidence in $OUT)"

# Optional repository-scale leg (rust-analyzer indexing, the heavier find):
#   M8_VERIFY_REPO=<openidea checkout> script/verify.sh --repo
if [ "${1:-}" = "--repo" ] && [ -n "$REPO_PROJ" ]; then
  echo "== optional: repository-scale leg ($REPO_PROJ)"
  python3 script/verify.py perf --binary target/debug/openidea-gui \
    --project "$REPO_PROJ" --open crates/ide-search/src/lib.rs --outdir "$OUT" --name gate-repo-debug --scroll-seconds 45
  python3 script/budget_gate.py "$OUT/gate-repo-debug-measured.json"
fi
