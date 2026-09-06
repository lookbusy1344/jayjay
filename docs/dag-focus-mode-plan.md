# DAG Focus Mode

## Status

Implemented in SwiftUI and GPUI.

## Purpose

Some valid `jj log` graphs are wider than a readable sidebar. JayJay preserves their native topology
and marks clipped rows with an overflow badge. Focus mode makes that marker actionable: it reloads
the graph with a narrower revset containing only the selected change's connected lineage.

Focus changes the requested revisions; it never projects or rewrites topology. The focused result is
still the exact `jj log` graph for its effective revset.

## Revset contract

The core helper `focus_revset(base, target)` produces:

```text
(<base>) & (::<target> | <target>::)
```

This intersects the user's base revset with the target's ancestors and descendants, inclusive.
Both shells call the shared helper at their graph-refresh boundary.

The base revset remains unchanged while focused. Focusing a second change replaces the target rather
than intersecting with the first focused result. Applying a different base filter clears focus.

Normal rows use their selection revision. A divergent row uses its exact commit ID so focus does not
silently include sibling versions of the same change.

## Interaction

Focus can be entered through:

- the overflow badge on a clipped row;
- the Repository menu or keyboard path for the selected change.

The keyboard path remains available when the selected row is not clipped. While focused, an always-
visible pill identifies the target and provides a clear action. Escape follows existing cancellation
priority and clears focus only after active drag and multi-selection states have been handled.

The previous graph remains visible but stale and non-interactive while its focused or unfocused
replacement loads. This preserves orientation without presenting old rows as current repository
state.

## Progressive selection

A focused target can appear after descendants in `TopoGroupedGraph` order and therefore may be absent
from the first snapshot. Each shell retains a pending focus target until the matching row appears,
then selects and reveals it exactly once. Later snapshots preserve selection without scrolling again.

Clearing focus pins the current selection through the expanding base graph. For non-divergent changes,
a rewritten commit with the same change ID is accepted; an exact divergent commit target is not
retargeted to a sibling.

If a focused load fails or finishes without the target, the last valid graph and the clear-focus pill
remain available. The shell does not silently fall back to an apparently current full graph.

## Paging

Depth-based **Load More** is disabled while focused because the effective revset is explicit. The
independent progressive-session memory ceiling still applies, and **Continue Loading** can resume a
paused focused session.

## Accessibility and hit testing

The overflow badge has an explicit hit target and tooltip separate from row selection and drag
gestures. SwiftUI also supplies hover treatment, accessibility labels, and identifiers. GPUI exposes
stable debug selectors for component tests. Both shells provide the menu/keyboard path and an
always-visible clear action, so focus does not depend on pointer access to a clipped badge.

## Validation

- `crates/jayjay-core/src/repo/revsets.rs`: composition and parenthesisation.
- `crates/jayjay-core/tests/integration/revsets.rs`: focused-set semantics.
- `crates/jayjay-core/tests/log_graph_parity.rs`: focused effective-revset parity with the real CLI.
- `shell/mac/Tests/JayJayTests/RepoViewModelFocusTests.swift`: refresh, selection, recovery, and
  transition state.
- `shell/gpui/src/repo/view_model/loaders/mod.rs`: the equivalent GPUI state-machine tests.

Focus state is intentionally transient and is not restored between application sessions.
