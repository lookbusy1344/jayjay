# `jj log` DAG Parity and Elision Plan

## Status and relationship to the existing DAG plan

This is a focused follow-up to [dag-rendering-plan.md](dag-rendering-plan.md). It does
not replace that plan. The existing plan remains the source for progressive loading,
shell architecture and performance goals. This document
changes the elision strategy and tightens the parity contract:

- the real, pinned `jj log` command is the gold standard for graph ordering,
  synthetic elision nodes, connectivity, and column allocation;
- JayJay should use the same upstream graph algorithm and renderer inputs wherever
  possible;
- a synthetic elision node participates in layout exactly as it does in `jj log`, but
  the GUI may compose its visual band into the preceding change row rather than
  exposing it as a selectable row;
- JayJay must not change graph topology to satisfy a fixed lane budget. Wide graphs
  are a presentation/viewport problem and retain the CLI's lanes and edges.

Implement this on the current DAG stack in the current checkout. Do not create a Git
worktree, hidden worktree, or sibling Jujutsu workspace for this work.

## Change-stack strategy

Implement this as follow-up changes stacked on the current `dag` work. Commit each
behaviourally coherent slice once its focused tests pass, but optimize for correct
behaviour, reviewable tests, and safe iteration rather than trying to reconstruct the
existing stack while implementation is still moving.

During this phase:

- stay in the current checkout;
- base new work on the current `dag` stack rather than `main`;
- do not rebase, squash, split, reorder, or otherwise rebuild the existing DAG
  changes as part of feature implementation;
- keep behaviourally distinct follow-up changes in separate logical commits, using
  conventional commit descriptions with explanatory bodies;
- do not move bookmarks, push, or publish;
- preserve intermediate tests that establish the `jj log` parity contract even if a
  later implementation change makes them pass by a different internal route.

Do not rebuild the branch merely because implementation is complete. Once the work
has been validated and explicitly accepted, rebuild it as an optimal commit series in
a separate, explicitly authorized history-edit phase. That later phase should
organize commits by coherent dependency and behaviour rather than preserve the
chronology of experimentation. A likely final ordering is:

1. deterministic `jj log` oracle fixture and parity harness;
2. shared synthetic-elision render-node model and upstream-compatible layout;
3. progressive-prefix reconciliation and native-width presentation;
4. UniFFI representation and generated bindings;
5. SwiftUI composition;
6. GPUI composition and cross-shell cleanup.

Treat that ordering as a target to reassess from the completed diff, not as a reason
to force unfinished work into those commits now. The later rebuild must preserve the
green parity test at every meaningful commit where the production code it exercises
exists.

## Implementation progress

As of 8 September 2026, the implementation stack contains these completed slices:

- a deterministic real-CLI parity fixture and oracle for synthetic elision enabled
  and disabled, plus the working-copy-descendant ordering case;
- the upstream-compatible indirect-edge expansion and nested synthetic-band layout;
- progressive-prefix retention for direct and indirect off-prefix connectors;
- repository configuration and nested elision-band data across UniFFI;
- composed, non-interactive elision bands in SwiftUI and GPUI;
- removal of the global eight-lane projection and its progressive look-ahead, so
  every snapshot uses the exact real-row prefix supplied to `jj log --limit N`;
- row-local SwiftUI content offsets so change text follows that row's rendered graph
  prefix rather than the widest prefix in the whole layout.

These slices are implemented but the feature is not complete. The deterministic
fixture now exercises the real progressive session and its corresponding
`jj log --limit N` output; the remaining work is cross-shell width behavior,
large-repository validation, structural validation, and performance evidence.

### Large-repository diagnosis and current status

The progressive-prefix defect is fixed: direct and indirect edges now retain targets
beyond the loaded `GraphEntry` prefix, including synthetic elision bands, matching
`jj log --limit N`.

Manual validation exposed a second and more pervasive mismatch. The shared layout
projected every loaded graph to `MAX_VISIBLE_DAG_LANES = 8`. The 639-row Rust
`jayjay()` query reaches approximately 32 native CLI columns; the equivalent
`::trunk()` prefix reaches approximately 52. JayJay cuts valid connectors until the
global layout fits eight lanes, then `build_continuations()` turns the cuts into the
coloured up/down arrows visible in the app. Those arrows are app-invented projection
markers, not synthetic elisions emitted by `jj log`.

Applying one budget to the widest point in the complete loaded graph also changes
narrower earlier regions that would fit by themselves. That makes global destructive
projection incompatible with the parity goal.

For the Rust checkout, `jj log -r 'jayjay()'` is the exact CLI query corresponding
to JayJay's default graph. Both `jayjay()` and `::trunk()` are required structural
and visual parity targets; `::trunk()` additionally remains the whole-history scale
and performance benchmark.

Destructive projection and its look-ahead are now removed from the shared layout.
A graph wider than eight columns retains its complete native lanes and connectors,
and a progressive snapshot computes geometry from exactly its published real-row
prefix. Manual SwiftUI validation is dramatically closer to the CLI and no longer
shows the former projection arrows.

The next implementation slice is shell width and full-query validation:

1. Let each shell consume the native graph width without clipping or reinterpreting
   topology.
2. Compare genuine `(elided revisions)` bands—not projection markers—against both
   Rust CLI queries at equivalent real-row limits.
3. Add resize or viewport behavior only where a native prefix cannot fit, without
   deleting or rerouting connectors.
4. Complete GPUI validation and record both query results and performance evidence.

### Native-width viewport TDD (done)

`DagGeometry`/`DAGGeometry` in both shells already implement and unit-test the
required policy: lane pitch compresses toward a legible floor
(`MINIMUM_LEGIBLE_LANE_PITCH`) as native column count grows, the rendered graph width
is capped (`ABSOLUTE_GRAPH_MAX_WIDTH`, `MAX_SIDEBAR_FRACTION`), and neither shell ever
drops a column or a connector to fit — width is a pixel/pitch problem, not a topology
problem. Rust (`shell/gpui/src/repo/window/dag/mod.rs`) and Swift
(`shell/mac/Tests/JayJayTests/DAGGeometryTests.swift`) both cover: preferred pitch on
a narrow graph, compression floor on a wide graph in a narrow sidebar, the responsive
and absolute width caps, and that node/link geometry stays internally consistent
(deterministic hit-testing, first/last column inside frame) at every combination.
`just test-rust jayjay-core dag::` and `just test-gpui dag` are green; a live Swift
XCTest run was not executed this session (no `xcodebuild`/`swift test` invocation was
made) — Swift coverage above is evidence the tests exist and compile against the
current `DAGGeometry`, not that they were run.

### Real-CLI progressive-limit parity on `~/Documents/dev/junk/rust` (validated)

Added `crates/jayjay-core/examples/dump_log_graph_parity.rs`, a diagnostic-only binary
that loads a bounded progressive prefix via `start_log_graph` (never the whole
revset) and prints JayJay's unprojected structural ASCII graph through
`debug_render_log_graph_ascii` at named real-row checkpoints, alongside the exact
`jj log -n N` command to diff it against.

Evidence recorded 2026-09-08, `jj 0.45.1`, JayJay commit `9d40a65d` (working copy on
top, empty):

- `jayjay()` at 50 real rows: topology identical to the CLI oracle byte-for-byte
  except the node glyph (our debug renderer always draws `o`; the CLI draws `+` for
  an immutable commit — an immutability marker the debug ASCII helper does not
  compute and was never asked to). No elision bands at this depth.
- `jayjay()` at 500 real rows: topology identical once node glyphs are stripped
  (`tr -d '@o+'`); 38 genuine `(elided revisions)` bands present and matching in
  position on both sides.
- `::trunk()` at 500 real rows: topology identical once node glyphs are stripped; 0
  elision bands at this depth (mainline history is close to linear this early); max
  line length 129 chars, consistent with the plan's earlier finding that `::trunk()`
  is the wider of the two revsets.
- `jayjay()` at 500 rows reached a max ASCII line length of 105 characters.

This is real-repository manual validation, not a replacement for the deterministic
`log_graph_parity.rs` CI gate. The junk/rust checkout was read only (`--ignore-
working-copy` throughout); no fetch, snapshot, or config change was made to it.

### `DagLayout` structural validator (done)

Added `crates/jayjay-core/src/dag/validate.rs`: `DagLayout::validate(&self,
expected_commit_ids)` checks real-row count and order against the input entries,
unique real commit ids, that every band's `node_column` decodes to `Empty` in its own
node line (the renderer draws the node glyph separately from pass-through
connectors, so a non-empty cell there means a connector is drawn over the row's own
node), that every column reference (node, termination, link-line width) stays below
`logical_column_count`, that every elision band has a non-empty target id, and that
no column both terminates and carries a continuing vertical link in the same band.
Wired into `DagLayout::compute_inputs` as a `panic!` behind `#[cfg(debug_assertions)]`
so malformed data fails at its Rust source rather than reaching a shell. 10 focused
unit tests exercise each failure directly. `just test-rust jayjay-core dag::` and the
three `log_graph_*` integration suites are green with the validator active on every
computed layout.

The two `jayjay-uniffi` DAG round-trip tests this validator session left failing are
green again; see "Projection surface removed" below.

### Performance validation on `~/Documents/dev/junk/rust` (recorded)

Machine: this development machine (macOS/Darwin), `jj 0.45.1`, JayJay commit
`f8a5ef91` (structural-validator commit), release profile for JayJay
(`cargo build --release --example profile_log_graph`), operation id
`2e37ab8ec14f` unchanged before/after. `jayjay()` resolves to 639 real revisions;
`::trunk()` resolves to 338,860.

CLI oracle wall time (5 samples, warm cache, output to `/dev/null`, median shown):

- `jj log -r 'jayjay()' -n 500`: 0.33 s
- `jj log -r '::trunk()' -n 500`: 0.26 s
- `jj log -r 'jayjay()'` (full 639 rows): 0.49 s
- `jj log -r '::trunk()'` (full 338,860 rows, graph mode): 120.65 s — the whole-
  history scale reference; not something JayJay's progressive design is meant to
  match, per this document's completion criteria (prefix equivalence, not full
  retention).

`profile_log_graph` (JayJay core, release, single run each — see `docs/dag-loading-
performance-plan.md` for the multi-sample protocol this borrows stage names from):

- `jayjay()`, no ceiling override (defaults, 639 rows fit under
  `MAX_AUTO_LOADED_ROWS`): first snapshot 83.0 ms, total load 342.0 ms, finished,
  639/639 rows retained. `merge_commit_trees` (614 calls) dominates busy time —
  JayJay computes conflict/emptiness metadata per row that the CLI's bare
  `commit_id` template does not, so this is not directly comparable to the CLI's
  wall time.
- `::trunk()` at `--ceiling 500` (matching the CLI checkpoint above): first
  snapshot 48.6 ms, total load 61.4 ms, paused at ceiling, 500/500 rows retained.
- `::trunk()` at the production default ceiling (`MAX_AUTO_LOADED_ROWS` = 10,000):
  first snapshot 72.0 ms, total load 238.4 ms, paused at ceiling, 10,000/10,000
  rows retained; peak RSS 224 MB (`/usr/bin/time -l`), ≈22 KB/retained row —
  consistent with the ≈34 KB/row figure `LOG_GRAPH_MEMORY_BYTES_PER_ROW_BUDGET`'s
  comment already documents.

`::trunk()` was deliberately never run with `--full`: forcing full materialization
of 338,860 rows would mean disabling the production memory ceiling this document
explicitly says not to disable for a diagnostic. The bounded default (paused at
10,000 rows in 238 ms) is the intended behavior being measured, not a limitation of
the measurement.

No regression is evident: first-snapshot times stay two orders of magnitude under
the 10-second `FIRST_RESULT_BUDGET`, and the row-ceiling pause fires exactly at the
configured threshold for an effectively unbounded revset.

### GPUI smoke validation (functional only, not visual)

Built and launched the GPUI shell against `~/Documents/dev/junk/rust`
(`just shell::gpui-run ~/Documents/dev/junk/rust`) with its default `jayjay()`
revset. The process started, stayed alive past graph load (checked at +5 s and
+13 s via `pgrep`), and was stopped cleanly with no crash. This session has no
screenshot/visual-diff capability for a native macOS window, so the plan's "inspect
light and dark mode plus a selected elision-owning row" and "confirm the elision
label is inside the owning change row" validation steps are **not done** — this run
only proves the GPUI shell loads and renders the real large repository without
panicking, not that its rendering matches the CLI pixel-for-pixel or that
interaction/selection state is correct. A person with the running app should still
do the visual pass described in "TDD implementation sequence" step 8.

### Structural validator false positive: crashed the app on the real repository

After the above sessions, opening `~/Documents/dev/junk/rust` in the real SwiftUI
app crashed on startup (`EXC_BREAKPOINT`/`SIGTRAP`, `swift_unexpectedError` inside
UniFFI's generated `try! rustCall()` for `startLogGraph`, called from
`RepoViewModel.startGraphRefresh`). Reproduced directly in Rust with a debug build
of `profile_log_graph` against `jayjay()`:

```
thread 'main' panicked at crates/jayjay-core/src/dag/layout.rs:39:17:
DagLayout structural validation failed: row "e2e9ce3ecbbf1cac9e6771d7f25386b8ae59cf01":
column 31 both terminates and carries a continuing vertical link
```

Cause: the validator's "no column both terminates and carries a continuing vertical
link in the same band" rule was wrong. `sapling-renderdag` legitimately reuses a
column freed by a terminating lane within the same row for an unrelated edge — the
manual, deterministic unit-test fixtures never exercised a real graph wide enough to
reuse a column, so this false positive only surfaced against the real repository.
This was a defect in this session's own validator, not a pre-existing issue.

Fix: removed the check entirely from `validate_band` in
`crates/jayjay-core/src/dag/validate.rs`. The corresponding unit test was rewritten
from `..._is_rejected` to `..._is_accepted`, asserting the corrected (permissive)
behavior, so this false positive cannot silently regress. Reverified after the fix:
`jayjay()` and `::trunk()` (checked to a 5,000-row ceiling) load without panicking in
a debug build, and the actual SwiftUI app stays running 18+ seconds after opening
the real repository with no new crash report.

This is a reminder that the validator's invariants need to be checked against real,
wide, real-world graphs — not just synthetic unit fixtures — before being trusted.
The remaining checks (row order/uniqueness, node-column decoding, column bounds,
elision-band target) were exercised against `jayjay()` and `::trunk()` in this same
reproduction without further false positives, but that is not a substitute for
broader validation before this validator is fully trusted.

### Projection surface removed; oracle, fixture and validator tightened (done)

The dead width-projection surface is gone end to end: `DagContinuation`,
`DagContinuationDirection` and `elided_fork_column` no longer exist in
`row_shape.rs`, the UniFFI records, the generated Swift bindings, GPUI's paint path,
or `DAGRow+GraphColumn.swift`. `to_row_shape` took an always-empty continuation list,
so the fork-lane branch behind it was unreachable code. The two `jayjay-uniffi`
round-trip tests that asserted on those fields now assert the shape production
actually emits: an unloaded parent keeps a native lane, and a real row owns its
synthetic band. `cargo test -p jayjay-uniffi dag` is green.

Both shells render the band the same way now. GPUI paints an `(elided revisions)`
label per band beside the band's glyph, matching SwiftUI's label and its
`Elided revisions` accessibility value; it previously drew a bare dot. Band space is
reserved from the layout's widest band stack (`dag_row_height`) rather than borrowed
from a fixed row height, so a multi-band row can no longer invert its own geometry —
the old arithmetic went negative at five bands in a 76 px row. SwiftUI's labels moved
into an unpadded bottom strip so a label lines up with the band the graph column
paints for it.

The CLI oracle and every fixture command now run under an isolated `JJ_CONFIG`, and
`revsets.log-graph-prioritize` is pinned in repository config so JayJay's in-process
`jj-lib` reads the same value the child `jj log` does. A developer's or CI image's own
templates, revsets, or graph style can no longer steer the gold standard.

The parity fixture gained `two-elisions`, a merge whose two excluded parents resolve
to sibling targets (`fork-a`, `fork-b`) so neither edge is redundant and one real row
owns two stacked bands. The enabled-synthetic parity test asserts that row exists and
that the excluded root still produces a missing-edge termination, so the byte-for-byte
CLI comparison demonstrably covers both boundaries.

`DagLayout::validate` now takes the layout inputs and checks that each row's bands
match that row's indirect-edge targets, in edge order, and that no band exists at all
when `ui.log-synthetic-elided-nodes` is off — the invariant that catches a band
attaching to the wrong owner. Following the false-positive lesson above, the
strengthened validator was run in a debug build against `~/Documents/dev/junk/rust`
for both `jayjay()` (639 rows) and `::trunk()` (5,000-row ceiling) before being kept;
neither panicked.

Evidence: `cargo test -p jayjay-core -p jayjay-uniffi -p jj-test` (496 tests),
`just test-gpui` (541), `just test-app` (all suites, 0 failures — the first executed
Swift test run for this stack), `just ffi`, `just lint` (clippy `-D warnings` plus
swiftlint), `jj fix`. Not done: any visual/dark-mode pass in either shell, and no new
performance sample after these changes.

## Outcome

The native DAG should be a GUI rendering of the same logical graph produced by the
pinned `jj log` command:

- the same real revisions, in the same order;
- the same working-copy placement, including cases where `@` is not the first row;
- the same direct, indirect, and missing-edge semantics;
- a synthetic elision node for every indirect edge when
  `ui.log-synthetic-elided-nodes` is enabled;
- the same renderer column allocation before pixels are assigned;
- no apparent parent/ancestor relationship between unrelated adjacent changes;
- the same result after progressive snapshots widen, apart from newly available
  real rows resolving lanes that previously continued below the prefix boundary.

SwiftUI and GPUI should consume one shared Rust layout. Shells choose native drawing
primitives, fonts, colours, and row composition; they do not reinterpret graph
topology.

## Observed regression

The supplied `a.png` was generated from this repository. It places `qlmv` and `zxl`
in the same display column on adjacent rows. The line from `qlmv` ends at a small
continuation marker, and `zxl` immediately reuses the freed column. That alignment
strongly suggests that `zxl` is the next displayed ancestor of `qlmv`.

The repository disproves that implication:

- `qlmv` has direct parent `wwzm`;
- `zxl` has direct parent `plpy`;
- neither `qlmv::zxl` nor `zxl::qlmv` contains a path;
- their histories converge only further back.

The corresponding `jj log` output keeps the continuing/elided lineage alive while
placing the unrelated lineage in another column. It also labels the omission as
`(elided revisions)`. The implementation now preserves both signals for complete and
progressive graphs; remaining validation must prove the shells present the native
width faithfully.

## Upstream behaviour to preserve

JayJay pins `jj-lib 0.45.1` and `sapling-renderdag 0.1.0`. The installed CLI used by
the repository is `jj 0.45.1`, and upstream `jj 0.45.1` uses the same
`sapling-renderdag 0.1.0` dependency.

`jj log` performs the following operations in graph mode:

1. Evaluate `revset.stream_graph()`.
2. Feed the complete stream into `TopoGroupedGraph`.
3. Apply `revsets.log-graph-prioritize` to that grouped stream.
4. Apply `--limit` after grouping and prioritisation.
5. Convert each graph edge for the terminal renderer.
6. If `ui.log-synthetic-elided-nodes` is enabled, replace each indirect edge from
   real source `S` to real target `T` with:

   ```text
   S --direct--> synthetic(T) --direct--> T
   ```

7. Call `GraphRowRenderer::next_row()` for the real change.
8. Immediately call `GraphRowRenderer::next_row()` once for every synthetic target,
   using `(elided revisions)` as its content.

The synthetic node is not cosmetic. Because it is passed through the same stateful
renderer, it affects lane lifetime, link geometry, and the column chosen for the next
real node. Reproducing only the label or drawing a dashed segment after layout would
not reproduce `jj log`.

The upstream CLI implementation in `cli/src/commands/log.rs` is the reference for
the transformation. The shared `sapling-renderdag::GraphRowRenderer` remains the
reference for column allocation and row geometry. Do not create a separate GUI lane
allocator.

### Working-copy placement

`jj log` does not impose an absolute "put `@` first" rule. The default
`revsets.log-graph-prioritize` value is `present(@)`, so `TopoGroupedGraph`
prioritizes the branch containing the working copy while preserving topological
ordering. A displayed descendant of `@` must therefore remain above `@`. An explicit
revset may also omit `@`, and reversed log output has different ordering again.

JayJay must apply the same prioritisation expression through `TopoGroupedGraph`; it
must not pin, extract, or prepend the working-copy entry in either shell. Add a
real-CLI oracle case with a linear `A -> B -> C` history whose working copy is edited
back to `B`. For `B::`, the pinned `jj log` command renders real descendant `C` above
`@ B`; JayJay's ordered entries and final DAG structure must match that output.

## Gold-standard test contract

The parity gate must execute the real pinned `jj log` binary. A test that compares
JayJay only with a duplicated Rust implementation, `jj-lib` output, or
`GraphRowRenderer` in isolation is useful but is not the acceptance test.

### Deterministic CLI oracle

Extend `jj-test` with a helper that runs `jj log` against a supplied fixture with all
presentation-affecting inputs pinned:

- graph mode enabled;
- colour disabled;
- pager disabled;
- a fixed ASCII graph style;
- `ui.log-synthetic-elided-nodes=true` unless the test explicitly covers the false
  case;
- a one-line commit template containing a unique full commit-ID sentinel;
- a fixed real-node symbol and synthetic-node symbol;
- an explicit revset and limit;
- repository/user configuration isolated from the developer's global templates and
  graph style.

The helper should fail early with a precise version diagnostic if the executable's
major/minor version does not match the workspace's pinned `jj-lib` contract. It must
not silently update goldens for a different CLI version.

Treat this version check as an upgrade gate. A future `jj-lib` or pinned CLI upgrade
must rerun the real-command parity suite and review the upstream `log.rs` synthetic
node transformation before changing the accepted version. Do not mechanically bump
the version assertion or regenerate expected traces when the command's graph changes.

Do not use `jj log --no-graph`: it bypasses `TopoGroupedGraph` and cannot verify the
behaviour under test. The existing `commit_ids_from_cli_log()` helper already follows
this rule for ordering; the new helper should generalise it to capture graph
structure.

### Comparing command output to JayJay structure

Build the comparison in two linked legs so the real command remains the oracle while
the production GUI structure is checked field by field:

1. Capture normalized output from the real `jj log` command. Preserve every graph
   prefix and synthetic elision line; normalize only unstable payload such as
   temporary paths or timestamps, which the pinned template should normally remove.
2. Ask JayJay's production graph pipeline for its ordered real/synthetic render-node
   sequence for the same repository, revset, prioritisation, and limit.
3. In the test only, feed that production render-node sequence into the same
   `sapling-renderdag` ASCII output renderer configured like the CLI.
4. Compare the complete normalized graph output line by line with the real command.
   A failure should show the first differing graph line and both complete traces.
5. Capture the `GraphRow` values produced while rendering the JayJay sequence and
   compare them field by field with the production `DagLayout` conversion. This
   second assertion proves that the structure sent to SwiftUI/GPUI is the structure
   whose textual rendering matched `jj log`.

This avoids a fragile, hand-written parser for box-drawing glyphs while still making
the executable `jj log` output—not a local reimplementation—the pass/fail oracle.
The shared output renderer is merely the deterministic projection that allows
JayJay's structural node sequence to be compared with the command.

### Required parity fixture

Create one deterministic composite repository in `jj-test` that covers the
regression and the essential graph semantics without multiplying fixtures:

- a common immutable base;
- a main lineage with at least one revision excluded by the tested revset;
- a visible source whose indirect target therefore produces a synthetic elision;
- an unrelated visible feature head emitted immediately after that source;
- a short fork and merge;
- one missing/root boundary;
- local references on both sides so prioritisation is deterministic.

Name fixture changes by role, not by the incidental `qlmv`/`zxl` IDs. Include an
explicit assertion that the source and adjacent feature head are unrelated. The
parity result must then demonstrate that the continuing/elided lane remains distinct
from the feature head's column, matching `jj log`.

Use a small second case only where the composite fixture cannot prove a boundary
condition:

- a `--limit`/progressive-prefix case proving that the prefix matches `jj log
  --limit N` and that limits count real revisions, not synthetic nodes;
- a working-copy-with-descendant case proving that `present(@)` prioritisation does
  not violate child-before-parent ordering or force `@` into the first row;
- a disabled `ui.log-synthetic-elided-nodes=false` case proving that JayJay follows
  the same repository configuration as `jj log` rather than forcing synthetic nodes.

Do not snapshot a rendering from this live repository as the required test. The
current history is valuable for manual validation, but it changes and is unsuitable
as a deterministic CI fixture.

## Shared Rust representation

### Render-node input

Introduce an app-owned input type representing exactly what is sent to the upstream
renderer:

```rust
enum DagRenderNodeId {
    Change(String),
    Elision { target_commit_id: String },
}

struct DagRenderNode {
    id: DagRenderNodeId,
    parents: Vec<DagRenderEdge>,
    owner_commit_id: String,
}
```

The precise type names may follow nearby conventions, but the representation must:

- distinguish a real change from a synthetic elision without inventing a fake
  `ChangeInfo`;
- use the same synthetic identity scheme and insertion order as `jj log`;
- retain the owning real source so a shell can compose the synthetic band into that
  source's row;
- support more than one indirect target from a merge;
- preserve direct and missing edges unchanged;
- remain internal to DAG layout rather than leaking upstream renderer types through
  UniFFI.

Extract the indirect-edge expansion into a pure function. Its expected sequence is
tested before integrating it with `GraphRowRenderer`.

### Rendered output

Separate the common geometry of one upstream renderer row from the selectable change
that owns it:

```rust
struct DagRenderedBand {
    node_column: u32,
    incoming: Option<DagEdgeKind>,
    node_line: Vec<DagVerticalCell>,
    link_line: Option<Vec<DagLinkCell>>,
    termination_columns: Vec<u32>,
    pad_line: Vec<DagVerticalCell>,
}

struct DagElisionBand {
    target_commit_id: String,
    graph: DagRenderedBand,
}

struct DagRowShape {
    commit_id: String,
    graph: DagRenderedBand,
    elisions_after: Vec<DagElisionBand>,
}
```

This is an illustrative decomposition, not a mandate to preserve these exact names.
The required properties are:

- `DagLayout.rows` remains one-to-one with real `GraphEntry` values;
- every synthetic renderer row is retained structurally under its owning real row;
- `DagLayout::row(commit_id)` continues to return only real rows;
- synthetic bands have no selection revision, context menu, drag identity, bookmark
  target, or detail view;
- `logical_column_count` includes columns used by both real and synthetic bands;
- accessibility can announce `Elided revisions` without presenting a fake commit.

Keeping one real `DagRowShape` per entry prevents synthetic nodes from disturbing
selection indexes, keyboard navigation, row-frame caches, progressive entry counts,
or mutation actions.

### Semantic provenance

Rust must identify why a graph segment ends or becomes synthetic. SwiftUI and GPUI
must not infer the reason from dash patterns, target lookup, row position, or whether
a related commit happens to be loaded. Preserve distinct structured cases for:

- a revset elision represented by a synthetic `DagElisionBand`;
- a genuinely missing/root boundary.

Width pressure is not semantic provenance and must not create a continuation. An
edge whose target is beyond the current progressive prefix stays in the renderer
state just as it does for `jj log --limit N`. `DagEdgeKind::Indirect` describes
ancestry and is not a substitute for the explicit synthetic elision band.

### Structural validation

Add a cheap Rust validator for complete `DagLayout` values and invoke it with
`debug_assert!` at the layout boundary. Unit tests should exercise failures directly.
At minimum, validate:

- exactly one real row for every input `GraphEntry`, in the same order;
- unique real commit IDs;
- exactly one node cell in each real or synthetic rendered band;
- every column reference is below `logical_column_count`;
- every elision band has a real owner and a non-empty target ID;
- synthetic IDs cannot resolve through `DagLayout::row()`;
- no missing-edge termination is also represented as an ordinary connector in the
  same band;
- nested elision order matches the render-node sequence consumed by
  `GraphRowRenderer`.

This validator protects both shells and makes malformed FFI data fail near its Rust
source rather than appear as misleading graph artwork.

### Configuration parity

Read `ui.log-synthetic-elided-nodes` from the same repository settings snapshot used
for the log request. Match `jj log` in both states:

- `true`: expand indirect edges into synthetic nodes and expose elision bands;
- `false`: retain the upstream indirect-edge representation and do not add a label.

Resolve the setting in Rust before layout and carry the resulting structure across
FFI. Do not make Swift or GPUI read Jujutsu configuration independently. A settings
change takes effect on the next graph refresh under the repository's existing
settings-lifecycle contract.

## Layout and width handling

The order of operations is part of the parity contract:

1. Obtain ordered real graph entries through the existing `TopoGroupedGraph` path.
2. Expand indirect edges into synthetic render nodes using the `jj log` algorithm.
3. Render the unprojected sequence with `GraphRowRenderer`.
4. Convert every real and synthetic `GraphRow` value into app-owned row shapes
   without deleting or rerouting connectors.
5. Map the resulting logical columns to pixels in each shell. Resize, clip with an
   explicit overflow affordance, or scroll when necessary; do not mutate topology.

This keeps the `qlmv` lane occupied while the unrelated `zxl` lineage receives the
column chosen by `GraphRowRenderer`. It also makes column allocation deterministic
and inherited from the CLI renderer rather than a shell or projection heuristic.

For a progressive prefix, compare the unprojected result with `jj log --limit N`
using the same number of real revisions. Synthetic bands do not consume the row
ceiling, `loaded_rows`, progress totals, or Continue Loading increments.

The shared layout therefore has no width-projection provenance. Genuine missing
edges and synthetic elisions retain their existing explicit representations.

## SwiftUI presentation

The synthetic node should not become a separate `ForEach` item. Compose it into the
preceding change's `DAGRow`:

- retain the normal change content and hit target;
- allocate a named compact height for each trailing elision band;
- render the real `DagRenderedBand` followed by its synthetic band or bands in one
  Canvas sequence;
- place `(elided revisions)` or the shorter native equivalent alongside the
  synthetic node in the lower portion of the owning row;
- use a subdued secondary/tertiary style, not a warning colour;
- retain sufficient contrast inside a selected row background;
- expose `Elided revisions` through the owning row's accessibility value;
- keep the entire composed row selectable as the real change while the elision
  region itself has no separate action.

Do not infer elision from dashes in Swift. The view renders explicit
`DagElisionBand` data supplied by Rust.

Refactor `DAGRow+GraphColumn.swift` so graph geometry can render a sequence of bands
without duplicating the existing node/link/pad drawing logic. Keep constants such as
the synthetic band height and label spacing named. Geometry used by rebase and
bookmark hit testing remains anchored to the real change node, never the synthetic
node.

Multiple synthetic targets should follow the same order as `jj log`. Do not collapse
them into one marker if doing so changes renderer state or hides distinct lanes.

## GPUI presentation

GPUI consumes the same nested real/synthetic row shape directly from
`jayjay-core`. Mirror the SwiftUI composition:

- one interactive GPUI item per real change;
- one or more non-interactive elision bands painted inside that item;
- the same band order, lane positions, and semantic label;
- real node bounds remain the source of selection and drag behaviour.

Keep colour derivation in the existing theme system. Do not add platform-specific
topology or a second elision-collapse policy.

## UniFFI boundary

Expose the app-owned rendered-band and elision-band records through
`jayjay-uniffi`. The boundary should preserve nested structure without requiring
Swift to match synthetic IDs back to source commits.

Update the existing DAG round-trip test so it contains:

- one real row with a synthetic elision band;
- the synthetic band's target identity and renderer columns;
- a missing-edge termination;
- more than one real row, proving row lookup remains based on real commit IDs.

Regenerate bindings with `just ffi` only after the Rust types and their focused tests
are stable.

## TDD implementation sequence

### 1. Add the real-CLI parity harness

Before production changes:

- extend `jj-test` with the deterministic composite fixture;
- add the pinned `jj log` graph-output helper;
- add a failing integration test in `log_graph_ordering.rs` or a focused sibling
  test module;
- prove the failure is the current elision/column mismatch, not timestamps, colours,
  user configuration, or root filtering;
- retain the existing ordering test as a smaller diagnostic rather than replacing
  it.

The primary regression assertion must compare the actual CLI graph with JayJay's
structure-derived graph and explicitly identify the unrelated adjacent changes whose
columns differ today.

### 2. Expand indirect edges into render nodes

Add pure core tests first for:

- no synthetic node for a direct edge;
- one synthetic node inserted immediately after one indirect edge;
- stable insertion order for multiple indirect targets;
- direct source-to-synthetic and synthetic-to-target connectivity;
- missing edges remaining missing;
- synthetic identities never becoming real commit identities.

Then implement the same transformation used by `jj log 0.45.1`.

### 3. Preserve synthetic `GraphRow` output

Add failing structural tests proving:

- the synthetic row affects the next real node's column;
- the regression fixture places the unrelated adjacent head in a different column;
- a linear direct history remains unchanged;
- `DagLayout.rows.len()` still equals the real entry count;
- `logical_column_count` includes synthetic bands;
- multiple elisions attach to their source in CLI order.

Then refactor the common band geometry and retain synthetic rows under their owner.

### 4. Preserve progressive prefixes and remove destructive projection

Add tests before changing layout policy:

- an off-prefix or long connector matches the actual `jj log --limit N` column
  allocation without a cut;
- working-copy prioritisation matches the actual CLI without pinning `@` above a
  displayed descendant;
- a graph wider than eight columns retains every CLI connector and native column;
- width never removes an elision band;
- an unrelated next row cannot reuse an elision lane when another lane is free;
- increasing the progressive limit resolves below-prefix lanes into real graph
  structure atomically.

Then remove width projection from shared topology. Handle width in shell layout or
viewport behavior without inventing graph semantics.

### 5. Cross the UniFFI boundary

Add the failing UniFFI round-trip assertion, update remote records, run `just ffi`,
and compile the generated Swift API before changing views.

### 6. Render the composed SwiftUI row

Add Swift-only geometry tests for:

- band vertical bounds inside one owning row;
- real-node hit-test coordinates remaining unchanged;
- one and multiple elision bands fitting without clipping;
- selected-row contrast inputs, if represented as view-model state rather than
  pixels.

Then render explicit elision bands in `DAGRow`. Do not add a pixel-golden test. Add
an XCUITest only if an interaction regression cannot be proved at the view-model or
geometry layer.

### 7. Render the same structure in GPUI

Keep topology assertions in Rust core tests. Add only a focused GPUI component test
if composing a synthetic band changes row selection, pointer bounds, or drag state.

### 8. Validate against this repository

Use this repository as a manual, non-hermetic validation target after deterministic
tests pass:

- compare JayJay with `jj log` around the histories represented by `qlmv` and `zxl`;
- confirm the two unrelated changes do not share an apparently continuous lane;
- confirm the elision label is inside the owning change row;
- compare default revset, an explicit narrow revset, and a progressive prefix;
- inspect light and dark mode plus a selected elision-owning row;
- verify both SwiftUI and GPUI where their platform support permits.

Manual validation supplements but never replaces the real-CLI parity test.

### 9. Guard performance and allocation behaviour

Synthetic nodes increase renderer calls but must not reintroduce whole-history work
into the progressive path. Add focused measurements or counters proving:

- a snapshot renders exactly one band per real entry plus one band per synthesized
  indirect target;
- row ceilings and metadata materialisation remain functions of real entry count;
- layout validation and real-row lookup are linear during construction and indexed
  or constant-time in row rendering;
- SwiftUI and GPUI do not scan the whole layout to find a row or its elisions;
- the first 50-row snapshot on the existing performance fixture remains within the
  established first-result budget, or any measurable regression is reported before
  acceptance;
- repeated progressive snapshots do not retain obsolete synthetic-band structures.

Use the existing profiling entry point and DAG performance fixtures where applicable.
Report timings separately from correctness; a green parity test does not establish
acceptable first-paint performance.

### 10. Use the Rust checkout as the demanding parity benchmark

`~/Documents/dev/junk/rust` is the canonical non-hermetic large-history target for
this follow-up. Use `jayjay()` for exact parity with the app's default query and
`::trunk()` for the broader ancestry and scale case. Together they supplement the
deterministic `jj-test` fixture with a real, complex tree large enough to expose
ordering, lane-allocation, streaming, memory, and accidental quadratic-work
regressions.

Treat that checkout as read-only test data:

- do not fetch, snapshot, edit, reconfigure, or rewrite it;
- run CLI reads with `jj --ignore-working-copy -R ~/Documents/dev/junk/rust`;
- record the operation ID, resolved `trunk()` commit ID, `jj --version`, JayJay commit
  ID, machine description, and build profile beside every result;
- verify that the operation ID and `trunk()` target are unchanged after a comparison;
- discard and rerun a sample if the repository changed during it.

#### Output parity

Use both real commands as oracles:

```bash
jj --ignore-working-copy -R ~/Documents/dev/junk/rust log -r 'jayjay()'
jj --ignore-working-copy -R ~/Documents/dev/junk/rust log -r '::trunk()'
```

For machine comparison, apply the same deterministic colour, pager, graph-style,
node-template, and commit-template controls used by the hermetic oracle helper. Do
not replace graph mode with `--no-graph`.

Compare JayJay's unprojected production structure against the corresponding real
`jj log -r <revset> --limit N` output for both revsets at these named product
boundaries:

- `INITIAL_LOG_BATCH_ROWS`;
- each cumulative publish threshold reached during background loading;
- `MAX_AUTO_LOADED_ROWS` immediately before the session pauses;
- the next explicitly continued ceiling exercised during validation.

At every checkpoint, `N` counts real revisions only. Require the same real revision
order, synthetic elision sequence, connectivity, working-copy placement, and
renderer columns as the CLI. Record shell viewport behavior separately; it must not
change the shared graph structure.

Do not require the GUI to retain every revision in the approximately 344k-revision
history at once. Prefix equivalence to `jj log --limit N` is the product contract;
the retained-memory ceiling remains intentional. If full-query structural parity is
measured, implement the diagnostic as a streaming comparison or digest with bounded
memory rather than disabling the production memory policy.

#### Performance comparison

Measure two baselines because they answer different questions:

1. The real `jj log -r 'jayjay()'` and `jj log -r '::trunk()'` commands measure the
   gold-standard traversal, ordering, synthetic-elision, terminal-layout, and output
   cost for the product and whole-history queries respectively.
2. JayJay's release-mode `profile_log_graph` entry point measures repository open,
   first snapshot, progressive checkpoints, metadata construction, DAG layout,
   retained rows, rendered bands, and total core load cost.

Also measure end-to-end time until the first `INITIAL_LOG_BATCH_ROWS` are visible in
SwiftUI and GPUI. Keep that shell measurement separate from core/CLI timing because
native view construction, UniFFI transfer, avatars, and metadata have no terminal
equivalent.

Use warm caches and at least ten samples after one discarded warm-up run. Record the
median and range for:

- complete CLI wall time for both `jayjay()` and `::trunk()` with output redirected
  away from the interactive terminal;
- CLI time to the equivalent real-row limits used by progressive checkpoints;
- JayJay repository-open time;
- time to the first snapshot and first visible rows;
- time to each background publish threshold and the automatic ceiling;
- DAG expansion, validation, and conversion time separately;
- real-row and synthetic-band counts;
- maximum native lane count and shell viewport width;
- bytes/records crossing UniFFI;
- peak resident memory and retained bytes per real row;
- cancellation latency while the same query is loading.

Performance parity means matching `jj log`'s scaling characteristics and avoiding
material overhead in the shared traversal/layout path, not claiming that a native
GUI paints in the same wall time as terminal text. Compare results with both the CLI
and the pre-change DAG implementation. Investigate any material regression before
acceptance; do not conceal it behind the ten-second foreground budget or a loading
indicator. Keep deterministic work-count assertions in CI, while wall-clock and RSS
results remain recorded manual evidence from the named machine and repository.

## Focused commands and evidence

Use the smallest test command for each slice:

```bash
just test-rust jayjay-core <parity-test-filter>
just test-rust jayjay-core <dag-layout-filter>
just test-rust jayjay-uniffi <dag-round-trip-filter>
just test-gpui <component-filter>
just ffi
```

Run the narrow Rust parity and layout tests after each core cleanup. Run the relevant
Swift test target after the Swift renderer changes. Do not claim CLI parity from a
unit test that did not execute `jj log`; report that gate separately by command and
test name.

At completion, report independent evidence for:

- real `jj log` parity on the deterministic fixture;
- pure Rust render-node and layout tests;
- progressive-prefix tests;
- UniFFI generation and round-trip tests;
- Swift compilation/tests and manual rendering;
- GPUI compilation/tests and manual rendering;
- first-snapshot timing and rendered-band counts;
- `~/Documents/dev/junk/rust` `::trunk()` output-prefix parity, CLI/core timings,
  lane counts, and peak memory;
- any platform or visual validation skipped.

Follow the repository feature loop: two full-diff cleanup rounds, rerun focused
tests, check `jj --ignore-working-copy log -r 'divergent()'`, then run `jj fix` and
`just lint` once when the change is actually ready for review. Logical implementation
commits are authorized during this phase. Do not rebase, squash, split, reorder, move
bookmarks, push, publish, or otherwise rebuild the stack until the completed work has
been explicitly accepted and that history-edit phase has been separately authorized.

## Expected file areas

The implementation is expected to remain concentrated in:

- `crates/jj-test/src/repo.rs` or a focused graph fixture module;
- `crates/jayjay-core/tests/log_graph_ordering.rs`;
- `crates/jayjay-core/src/dag/` layout modules;
- `crates/jayjay-core/src/dag/renderdag.rs`;
- `crates/jayjay-core/src/dag/row_shape.rs`;
- `crates/jayjay-uniffi/src/dag.rs`;
- `shell/mac/Sources/JayJay/Repo/DAG/DAGRow.swift`;
- `shell/mac/Sources/JayJay/Repo/DAG/DAGRow+GraphColumn.swift`;
- `shell/mac/Sources/JayJay/Repo/DAG/DAGRowViewModel.swift`;
- focused DAG tests under `shell/mac/Tests/JayJayTests/`;
- `shell/gpui/src/repo/window/dag/` and focused GPUI tests if interaction geometry
  changes.

Do not update the user guide, Help Book, website, README feature list, or release
parity matrix as part of this implementation.

## Non-goals

- Reusing the terminal renderer's final Unicode strings as GUI artwork.
- Parsing arbitrary user templates to construct native row content.
- Making synthetic elisions selectable, draggable, or mutation targets.
- Showing a count of omitted revisions; the graph edge does not provide that count
  cheaply or reliably.
- Pixel-identical terminal typography or spacing.
- Removing progressive loading.

## Completion criteria

The work is complete when all of the following hold:

1. A deterministic test executes the real pinned `jj log` command and proves that
   JayJay's graph has the same real/synthetic node sequence, connectivity, and
   renderer column structure regardless of native width.
2. A real-CLI case proves that `@` follows CLI prioritisation and remains below any
   displayed descendants.
3. The test fixture includes an elided lineage followed by an unrelated visible
   change and proves they do not appear in one continuous column.
4. SwiftUI displays `(elided revisions)` within the owning real change row while the
   synthetic band still participates in upstream layout.
5. Synthetic nodes do not affect selection, navigation, mutation targets, loaded-row
   counts, or progressive limits.
6. SwiftUI and GPUI consume the same Rust structure.
7. Width pressure never changes graph topology or introduces continuation markers
   absent from `jj log`.
8. Focused tests, binding generation, shell checks, cleanup rounds, and divergence
   checks pass with evidence reported separately.
9. Layout validation covers the Rust-to-shell invariants without shell inference.
10. First-snapshot timing and allocation evidence show that synthetic bands did not
    compromise progressive loading.
11. The read-only Rust checkout passes both `jayjay()` and `::trunk()` parity at every
    exercised product checkpoint, with CLI, core, SwiftUI, and GPUI performance
    evidence recorded at equivalent real-row limits.
