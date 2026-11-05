# M8 clean-room log

No Zed (GPL-3.0) sources were read in this milestone, same as M0–M7. The
implementation inputs are:

- this repository: `apps/openidea-gui` (M0–M7 state), `crates/*` public
  surfaces, `perf/budgets.json` in the main workspace (the budget-table
  convention the main repo already uses),
- the Apache-2.0 snapshots `gpui-pre 0.3.9` / `gpui-base 0.8.0` /
  `gpui-component 0.8.0` (registry sources, read for API facts: keymap
  precedence rules, `ContextMenu`/`PopupMenu` focus-on-open flow,
  `Notification` ids, `InputState::set_selected_range`),
- the M0–M7 design notes and reports in this directory,
- the reference host `~/projects/dspo/manox-app` (pinned-version partner,
  consulted for crash-log/last-run patterns only).

Prior-milestone drivers (the `/tmp/m5drive.py` lineage, cua-driver sessions)
informed the soak driver's input discipline (clipboard paste for text);
nothing was read from Zed.
