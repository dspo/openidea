#!/usr/bin/env python3
"""Generates src/reconcile_data.rs: the per-preset-id reconciliation status
(done / placeholder / na) for the app's implementation level at generation
time. Re-run after a sync or after new milestones land the next batch:

    python3 script/gen-reconcile.py

Curation rules (kept here so M14 can re-run the same audit):
- Done: ids the keymap binds (`keymap.rs` idea_id rows, read live) plus the
  EXTRA_DONE list — ids with a real face whose row carries no idea_id or is
  bound from a gesture.
- Placeholder: the curated PLACEHOLDERS list — actions Find Action can show
  and toast "not yet available" for.
- Na: everything else — the out-of-scope families (database, GitHub, agents,
  debugger, HTTP/SQL consoles, image/markdown viewers, diff-viewer keyboard
  model, terminal) and the deferred editor machinery.
"""

import re
import sys
from pathlib import Path

APP = Path(__file__).resolve().parent.parent
PRESET = APP.parent.parent / "src" / "platform" / "keymaps" / "intellij-macos.ts"
KEYMAP = APP / "src" / "keymap.rs"

# Ids with a real face in the app, beyond the keymap's bound rows.
EXTRA_DONE = [
    "editor.cloneCaretAbove",
    "editor.cloneCaretBelow",
    "runAnything.show",  # completes the ⌃⌃ chord; the action itself toasts as not-yet-available
]

# Real faces that landed late, described per row in the report; the roadmap's
# M9 编辑段 items the app implements at the line level (moveStatement is the
# line-level approximation — kept Done with the note living in M9-REPORT).
PLACEHOLDERS = [
    # navigation the app shows but cannot do yet
    "navigate.gotoClass",
    "navigate.gotoSymbol",
    "navigate.searchText",
    "navigate.navigationBar",
    "navigate.callHierarchy",
    "navigate.typeHierarchy",
    "navigate.gotoTest",
    "navigate.superMethod",
    "navigate.selectIn",
    "navigate.showFilePath",
    "navigate.customRegion",
    "navigate.nextOccurrence",
    "navigate.prevOccurrence",
    "navigate.nextMethod",
    "navigate.previousMethod",
    "navigate.toggleBookmarkWithMnemonic",
    "navigate.showBookmarks",  # list exists; mnemonics do not
    "navigate.nextHighlightedUsage",
    "navigate.prevHighlightedUsage",
    # find
    "find.showUsages",
    "find.highlightUsages",
    "find.replaceInPath",
    "agents.findInConversation",
    # code machinery
    "code.quickDocumentation",
    "code.quickDefinition",
    "code.applyQuickFix",
    "code.reformatDialog",
    "code.moveElementLeft",
    "code.moveElementRight",
    "code.completeStatement",
    "code.surroundWith",
    "code.insertLiveTemplate",
    "code.surroundWithLiveTemplate",
    "code.collapseRecursively",
    "code.expandRecursively",
    "code.addCaretNextOccurrence",
    "code.selectAllOccurrences",
    "code.unselectOccurrence",
    "code.addCaretsToLineEnds",
    "code.splitLine",
    "code.expandToLevel1",
    "code.expandToLevel2",
    "code.expandToLevel3",
    "code.expandToLevel4",
    "code.expandToLevel5",
    "code.expandAllToLevel1",
    "code.expandAllToLevel2",
    "code.expandAllToLevel3",
    "code.expandAllToLevel4",
    "code.expandAllToLevel5",
    "code.collapseBlock",
    "code.autoIndentLines",
    "code.cyclicExpandWord",
    "code.cyclicExpandWordBackward",
    "code.generate",
    "code.implementMethods",
    "code.overrideMethods",
    "code.unwrap",
    "code.foldSelection",
    "code.expressionTypeInfo",
    "code.pasteFromHistory",
    "code.pasteAsPlainText",
    "code.sortLines",
    "code.reverseLines",
    "analyze.inspectFile",
    "editor.codeBlockStart",
    "editor.codeBlockEnd",
    "editor.codeBlockStartWithSelection",
    "editor.codeBlockEndWithSelection",
    "editor.contextInfo",
    "editor.matchBrace",
    "editor.toggleInsertState",
    "editor.toggleColumnSelection",
    "editor.inlineCompletion.trigger",
    "editor.inlineCompletion.accept",
    "editor.inlineCompletion.acceptWord",
    "editor.inlineCompletion.dismiss",
    "editor.inlineCompletion.next",
    "editor.inlineCompletion.previous",
    "editor.toggleZen",
    "editor.increaseFontSize",
    "editor.decreaseFontSize",
    "editor.copyPath",
    "editor.copyReference",
    "editor.nextTab",
    "editor.prevTab",
    "editor.splitRight",
    "editor.nextSplitter",
    "editor.prevSplitter",
    "editor.reopenClosedTab",
    "editor.cloneCaretAbove",
    "editor.cloneCaretBelow",
    "file.newScratchFile",
    "file.openProject",
    "file.reloadFromDisk",
    "toolwindow.hideAll",
    "toolwindow.maximize",
    "toolwindow.resizeLeft",
    "toolwindow.resizeRight",
    "toolwindow.resizeUp",
    "toolwindow.resizeDown",
    "toolwindow.terminal",
    "toolwindow.hideActive",
    "toolwindow.focusEditor",
    "toolwindow.closeContent",
    "layout.restoreDefault",
    "window.toggleFullScreen",
    "window.nextTab",
    "window.previousTab",
    "window.next",
    "window.previous",
    # vcs beyond commit/diff
    "vcs.push",
    "vcs.commitAndPush",
    "vcs.toggleAmend",
    "vcs.showMessageHistory",
    "vcs.update",
    "vcs.rollback",
    "vcs.addToVcs",
    "vcs.nextChange",
    "vcs.prevChange",
    "vcs.branches",
    "vcs.annotate",
    "vcs.localHistory",
    "vcs.newBranch",
    "vcs.operations",
    "run.debug",
    "run.runContext",
    "run.debugContext",
    "run.rerun",
    "run.updateApplication",
    "run.chooseConfig",
    "run.chooseDebugConfig",
    "run.editConfigs",
    "workbench.stopBackgroundProcesses",
    "build.buildProject",
    "build.buildModule",
    "refactor.changeSignature",
    "refactor.extractVariable",
    "refactor.extractMethod",
    "refactor.extractConstant",
    "refactor.extractField",
    "refactor.extractParameter",
    "refactor.inline",
    "refactor.move",
    "refactor.safeDelete",
    "refactor.thisMenu",
    "diff.nextDifference",
    "diff.prevDifference",
    "diff.jumpToSource",
    "diff.applyLeftSide",
    "diff.applyRightSide",
    "diff.nextFile",
    "diff.prevFile",
    "diff.showSettings",
    "localHistory.recentChanges",
    "view.quickSwitchScheme",
    "view.zoomIn",
    "view.zoomOut",
    "view.resetZoom",
    "web.openInBrowser",
    "project.newFile",
    "project.compareWith",
    "find.openInWindow",
]

NA_FAMILIES = (
    "db.",
    "debug.",
    "coverage.",
    "github.",
    "agents.",
    "http.",
    "graphql.",
    "terminal.",
    "markdown.",
    "image.",
    "usages.",
    "find.popup.",
    "find.field.",
)

ENTRY = re.compile(r'"([A-Za-z0-9_.]+)"\s*:\s*\[')


def preset_ids() -> list[str]:
    text = PRESET.read_text(encoding="utf-8")
    return ENTRY.findall(text)


def bound_ids() -> set[str]:
    text = KEYMAP.read_text(encoding="utf-8")
    return set(re.findall(r'Some\("([A-Za-z0-9_.]+)"\)', text))


def main() -> int:
    ids = preset_ids()
    done = bound_ids() | set(EXTRA_DONE)
    placeholders = [p for p in PLACEHOLDERS if p not in done]
    unknown = [p for p in placeholders if p not in ids]
    if unknown:
        print("placeholder ids missing from the preset:", unknown, file=sys.stderr)
        return 1

    out = [line.rstrip() for line in """//! GENERATED by script/gen-reconcile-data.py — the per-preset-id
//! reconciliation status at generation time. Regenerate after a sync or a
//! milestone; M14's final audit re-runs the same script. The curation rules
//! live in the generator.
//!
//! Row semantics: `Done` = the action has a real face (the keymap binds it,
//! or it is in the generator's EXTRA_DONE); `Placeholder` = Find Action shows
//! it and triggering toasts "not yet available"; `Na` = out of M9's scope
//! (the tool families, the debugger, the deferred editor machinery).

/// One preset row's reconciliation state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Done,
    Placeholder,
    Na,
}

/// The per-id status, in the preset's own order.
#[allow(dead_code)]
pub static STATUS: &[(&str, Status)] = &[""".splitlines()]
    for idea in ids:
        if idea in done:
            status = "Status::Done"
        elif idea in placeholders:
            status = "Status::Placeholder"
        else:
            status = "Status::Na"
        out.append(f'    ("{idea}", {status}),')
    out.append("];")
    out.append("")

    target = APP / "src" / "reconcile_data.rs"
    target.write_text("\n".join(out) + "\n", encoding="utf-8")
    counts = {"done": 0, "placeholder": 0, "na": 0}
    for idea in ids:
        if idea in done:
            counts["done"] += 1
        elif idea in placeholders:
            counts["placeholder"] += 1
        else:
            counts["na"] += 1
    print(f"rows: {len(ids)} — done {counts['done']}, placeholder {counts['placeholder']}, na {counts['na']}")
    print(f"wrote {target}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
