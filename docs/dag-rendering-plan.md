# DAG Rendering Architecture

## Status

Implemented. This document records the current renderer contract shared by SwiftUI and GPUI.

## Source of truth

JayJay uses `sapling-renderdag::GraphRowRenderer`, the renderer used by the pinned `jj` release, for
lane allocation and connector topology. The adapter in `crates/jayjay-core/src/dag/renderdag.rs`
converts upstream rows into JayJay-owned `DagRowShape` values. No upstream renderer type crosses the
core boundary.

The layout preserves the exact topology supplied by the repository graph:

- direct, indirect, and missing edges remain distinguishable;
- parents beyond a progressive prefix retain their native outgoing lanes;
- wide graphs retain every lane and connector;
- no lane budget deletes, reroutes, or overlays topology.

This replaces the former heuristic allocator, which compacted every graph wider than four lanes
into a shared overflow lane and could no longer represent the graph faithfully.

## Synthetic elisions

When `ui.log-synthetic-elided-nodes` is enabled, each indirect edge is expanded before rendering as
a direct edge through a unique synthetic node. This matches `jj log` semantics. The GUI composes the
synthetic row into an `elisions_after` band owned by the preceding real change rather than exposing
it as a selectable change.

An elision band participates in lane allocation but has no repository identity and is not a target
for selection, dragging, rebasing, or keyboard navigation. Multiple indirect edges keep stable edge
order and receive distinct synthetic identities even when they share a target.

When the setting is disabled, indirect edges remain dashed ancestor connections without synthetic
bands.

## App-owned row shapes

`DagLayout` contains ordered `DagRowShape` values and aggregate width information. Each row records:

- its commit ID and node column;
- vertical cells before and after the node;
- an optional link line for forks, merges, and lane moves;
- termination columns for missing edges;
- zero or more synthetic elision bands.

The row-shape boundary lets both shells draw identical topology without depending on terminal glyphs
or renderer internals. Debug builds validate row order, uniqueness, column bounds, node placement,
and elision ownership at the Rust source boundary.

## Width presentation

Topology and viewport policy are separate. Each shell derives drawing and hit testing from one
immutable geometry value for the complete layout:

- preferred lane pitch is used while it fits;
- pitch compresses only to a named legibility floor;
- graph width is capped by the sidebar allocation and an absolute maximum;
- lanes beyond the drawable width are clipped behind an explicit trailing overflow badge.

The overflow badge means that native lanes continue outside the visible gutter. It never means the
core removed or merged those lanes. Activating the badge enters focus mode, which narrows the revset
instead of mutating the graph; see [DAG Focus Mode](dag-focus-mode-plan.md).

Each row places its text after that row's visible graph prefix rather than reserving the widest
prefix in the entire result. Node painting, bookmark and rebase hit testing, and drag targets use the
same geometry.

## Progressive snapshots

A layout is computed from exactly the real rows in one published snapshot. Growing the prefix may
extend an existing connector, so the shell replaces the entries and layout atomically. Selection and
scroll anchoring use repository identities, never row indices or lane positions.

SwiftUI receives the layout beside entries through UniFFI. GPUI consumes the same core value
directly and indexes the corresponding entry and row shape by the same row index.

## Invariants

- `DagLayout.rows` and displayed entries have identical order and count.
- Layout never changes `GraphEntry.edges` or `ChangeInfo.parents`.
- A missing edge terminates; it does not create a selectable row.
- Synthetic bands never appear as top-level changes.
- Shell geometry may clip pixels but never changes graph topology.
- Repository operations use commit/change identity, not rendered columns.

## Validation

Focused automated coverage lives in:

- `crates/jayjay-core/src/dag/renderdag.rs` and `validate.rs`;
- `crates/jayjay-core/tests/log_graph_parity.rs`;
- `shell/mac/Tests/JayJayTests/DAGGeometryTests.swift` and `DAGRowViewModelTests.swift`;
- GPUI DAG unit and component tests under `shell/gpui/src/repo/window/dag/` and
  `shell/gpui/tests/gpui/`.

The real-CLI oracle is described in [jj log DAG Parity](jj-log-dag-parity-plan.md).
