#!/usr/bin/env python3
"""The budget gate (verify.sh stage 3): compares measured medians against
perf/budgets.json and exits non-zero on a breach.

Reads one or more measured JSONs (script/verify.py perf output). A metric
passes when its MEDIAN is within the profile's budget — individual samples
are reported (a worst-case spike is data, not a gate; the report prints
them). `open_first_paint` may carry a `warm` variant note: the first launch
on a machine compiles the Metal shaders (the runtime_shaders pin), so the
gate judges the warm number when the measured file provides one.
"""

import argparse
import json
import statistics
import sys
from pathlib import Path


def metric_id_to_json_key(metric_id):
    """budgets.json ids map onto the measured JSON's *_ms keys."""
    return {
        "key_to_px": "key_to_px_ms",
        "completion_first_frame": "completion_first_frame_ms",
        "find_in_files_first_response": "find_in_files_first_response_ms",
        "scroll_frame_during_index": "scroll_frames_during_index_ms",
        "open_first_paint": "open_first_paint_ms",
    }[metric_id]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("measured", nargs="+", help="measured JSON files (verify.py perf output)")
    ap.add_argument("--budgets", default=str(Path(__file__).parent.parent / "perf" / "budgets.json"))
    args = ap.parse_args()

    budgets = json.loads(Path(args.budgets).read_text())
    by_id = {m["id"]: m for m in budgets["metrics"]}
    rows = []
    failed = False
    for path in args.measured:
        data = json.loads(Path(path).read_text())
        release = data.get("release", False)
        name = data.get("name", path)
        for metric in budgets["metrics"]:
            key = metric_id_to_json_key(metric["id"])
            value = data.get(key)
            if value is None:
                rows.append((name, metric["id"], "-", "-", "-", "n/a"))
                continue
            samples = value if isinstance(value, list) else [value]
            median = statistics.median(samples)
            budget = metric["budget_release_ms"] if release else metric["budget_ms"]
            ok = median <= budget
            failed |= not ok
            rows.append((name, metric["id"], f"{median:.1f}", f"{budget:.0f}", f"max {max(samples):.1f} (n={len(samples)})", "PASS" if ok else "FAIL"))

    width_name = max(len(r[0]) for r in rows)
    print(f"{'scenario':<{width_name}}  {'metric':<32} {'median':>8} {'budget':>8}  {'samples':<22} verdict")
    for name, mid, median, budget, extra, verdict in rows:
        print(f"{name:<{width_name}}  {mid:<32} {median:>8} {budget:>8}  {extra:<22} {verdict}")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
