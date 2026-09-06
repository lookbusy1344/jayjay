# Bounded DAG Loading and Rendering Plan

## Status

Proposed replacement for the original DAG parity plan. The `TopoGroupedGraph` and
`sapling-renderdag` work already implemented on this change remain the topology
foundation; this plan corrects the unbounded selection, metadata loading, and
presentation policies exposed by testing against the Rust repository.

This document does not change runtime behaviour.

## Problem Statement

The new renderer is structurally more accurate than the old lane allocator, but the
application gives it an unsuitable input and then tries to display every resulting
lane. On a large, branch-heavy repository this has two user-visible consequences:

1. Initial repository loading takes seconds before the first useful graph appears.
2. A wide lane count found anywhere in the loaded history determines the pitch for
   every row, compressing the useful lanes at the top into an illegible strip.

The screenshots demonstrate the second failure. `main_branch.png` keeps a small
number of readable lanes. `new_dag.png` mathematically preserves the full graph but
compresses its globally widest point into the sidebar, so the top rows lose useful
separation. `smartgit_example.png` and `smartgit2.png` show the preferred visual
principle: retain a legible lane pitch and terminate low-value connectors with
explicit arrows instead of squeezing or overlaying unrelated lanes.

## Findings

### The default revset confuses context depth with page size

JayJay currently uses:

```text
present(@) | ancestors(immutable_heads().., 20) | trunk()
```

The `20` was intended to load about twenty rows, but `ancestors(revisions, 20)` is a
depth per selected history, not a result limit. On a repository with many visible
heads it can select hundreds of revisions and create dozens of simultaneous lanes.

The pinned `jj` release instead defaults `revsets.log` to `builtin_log()`, whose
built-in expression is:

```text
present(@) | ancestors(immutable_heads().., 2) | trunk()
```

`jj log --limit N` is a separate operation. It feeds the complete revset graph to
`TopoGroupedGraph`, applies branch prioritisation, and only then takes the first `N`
ordered revisions. The input to `TopoGroupedGraph` is deliberately not truncated.

References:

- [`jj log` v0.44.0 ordering and limit placement](https://github.com/jj-vcs/jj/blob/v0.44.0/cli/src/commands/log.rs#L204-L223)
- [`jj` default log revset](https://github.com/jj-vcs/jj/blob/v0.44.0/cli/src/config/revsets.toml)
- [SmartGit graph anchors and filtering](https://docs.syntevo.com/SmartGit/Latest/Manual/GUI/Graph-View)

### Measured scale on the Rust checkout

On `~/Documents/dev/junk/rust` on 2026-09-03:

| Query/work | Result | Warm elapsed time |
| --- | ---: | ---: |
| `jj log` default (`builtin_log()`) | 2 revisions | 0.02 s |
| JayJay default expression at depth 20 | 639 revisions | 0.03 s in the CLI |
| Same expression with `--limit 20` | 20 revisions | 0.02 s in the CLI |
| Stream all `immutable()` IDs | 344,187 IDs | 2.06 s |
| Stream all `parents(immutable())` IDs | 344,080 IDs | 2.04 s |

The 639-revision terminal graph reached a commit-text offset of 130 characters,
whereas the first twenty revisions reached 22. This is consistent with the global
lane count making the current SwiftUI rendering collapse horizontally.

These timings show that revset evaluation and topological ordering are not the main
startup cost. `Repo::log_graph()` eagerly streams the entire immutable history twice
before materialising its displayed commits. It also calculates empty-tree status per
displayed commit and scans local tags per displayed commit. SwiftUI then sends every
`GraphEntry` back across UniFFI to compute the DAG layout in a second Rust call.

### Required large-repository validation target

`~/Documents/dev/junk/rust` is the canonical manual performance and rendering target
for this work. The implementation is not complete when only synthetic fixtures and
unit tests pass. Every performance-related slice must be exercised against that
checkout, and final acceptance requires opening it in JayJay and comparing the
result with `jj log`, `main_branch.png`, `smartgit_example.png`, and `smartgit2.png`.

Treat the checkout as read-only test data: do not fetch, rewrite, create changes, or
alter its configuration. Use `jj --ignore-working-copy` for CLI measurements. Record
the repository operation ID or visible-head count with benchmark results so later
runs can identify repository drift without attempting to normalize it.

Hermetic `jj-test` fixtures remain mandatory for CI and deterministic correctness,
but they supplement rather than replace validation on the Rust checkout.

## Decisions

### 1. Make selection, pagination, and layout separate concepts

Use three named policies with independent responsibilities:

- `DEFAULT_LOG_CONTEXT_DEPTH = 2`: controls which nearby revisions the default
  revset selects, matching `jj`.
- `LOG_PAGE_SIZE = 20`: controls how many real change rows are initially returned and
  how many each **Load More** action adds.
- `MAX_LOADED_LOG_ROWS = 200`: is the hard presentation ceiling for one graph view.
  At the ceiling the UI asks the user to narrow the revset instead of constructing an
  effectively unbounded graph.

The exact page and ceiling values are implementation defaults, not persisted user
preferences. Validate them on the Rust checkout and the existing small fixtures
before changing them. Do not encode the page size inside `ancestors()` again.

Represent the query as `LogQuery::Default` or `LogQuery::Explicit(String)` rather
than inferring default mode by comparing formatted strings. In default mode, resolve
the repository's `revsets.log` setting and fall back to the pinned `builtin_log()`
semantics. An explicitly entered revset remains authoritative, but it is still
subject to the row limit. Resetting the filter switches back to `Default`; it does
not copy a built-in expression into shell state.

### 2. Apply the row limit exactly where `jj log` does

Change the repository graph API to accept a requested row limit. Its pipeline is:

```text
parse/evaluate selected revset
        |
        v
TopoGroupedGraph over the complete stream_graph()
        |
        +-- apply revsets.log-graph-prioritize
        |
        v
take(requested_limit + 1)
        |
        +-- extra row determines has_more and is discarded
        v
materialise metadata for at most requested_limit commits
```

Do not apply `.take()` to `revset.stream_graph()` before `TopoGroupedGraph`; that can
remove the configured priority commit and produces ordering different from the CLI.

Return a page-shaped value rather than making shells infer pagination from row count:

```rust
pub struct LogGraphPage {
    pub entries: Vec<GraphEntry>,
    pub layout: DagLayout,
    pub has_more: bool,
    pub applied_limit: u32,
}
```

`has_more` comes from the `limit + 1` ordered row, not from `entries.len() >= limit`.
Both shells keep the selected revset unchanged when loading more and increase only
the requested limit. Custom revsets therefore gain bounded, predictable pagination
too.

### 3. Bound metadata work to the returned page

Remove the two repository-wide `ImmutableIds` scans from log loading. Once the
limited ordered commit IDs are known, determine immutability and immutable-child
membership only for those IDs. Prefer intersections of the displayed-ID set with the
typed `immutable()` and `parents(immutable())` expressions; compare this with reused
`containing_fn()` predicates in a focused benchmark and keep the bounded form with
the lower cost.

Build commit-indexed ref metadata once per page:

- local bookmarks by commit ID;
- local tags by commit ID;
- workspace commit IDs;
- remote-ref presence needed for discardable-working-copy policy.

`commit_to_change_info()` then performs lookups rather than scanning repository views
for every row. Calculate `commit.is_empty()` only for returned rows; it remains
required UI data but must never run for discarded rows or the look-ahead row.

Add tracing spans/counters around revset evaluation, grouping, limited commit
materialisation, immutability membership, empty checks, layout projection, and FFI
conversion. Wall-clock assertions do not belong in unit tests; counters will prove
that expensive work is bounded.

### 4. Compute the page and its layout in one Rust operation

`DagLayout` remains app-owned Rust business logic. Compute it from the limited page
inside `Repo::log_graph_page()` and return it with the entries. SwiftUI must not call
`logGraph()` and then marshal the same entries back through `computeDagLayout()`.
GPUI consumes the same `LogGraphPage` directly.

Keep the standalone pure layout function only if focused unit tests or non-repository
portable consumers require it. It should not be part of the desktop loading path.

### 5. Preserve topology, but project long connectors explicitly

Uniform compression is acceptable only while lanes remain visually distinct. Replace
the current "preserve every lane at any pitch" policy with a deterministic display
projection modeled on the SmartGit examples.

The projection operates on the limited ordered page and never modifies
`GraphEntry.edges` or `ChangeInfo.parents`, which remain semantic data for graph
inspection and actions. It produces layout-only edges for `GraphRowRenderer`:

1. Keep adjacent and short connectors intact.
2. Cut connectors whose target is outside the loaded page.
3. Cut connectors spanning more than `MAX_CONTINUOUS_CONNECTOR_ROWS` (initially 12).
4. Render the projected graph and inspect its maximum simultaneous lane count.
5. If it exceeds `MAX_VISIBLE_DAG_LANES` (initially 8), cut additional longest,
   lowest-priority connectors deterministically until it fits.

Connector priority, highest first:

1. the first-parent spine from `@` or the selected change;
2. edges whose source or target has a local bookmark or workspace;
3. direct edges before indirect edges;
4. shorter row spans before longer spans;
5. commit ID as the final stable tie-breaker.

Never cut an adjacent first-parent edge. If even the protected adjacent topology
requires more than the lane budget, allow that local row band to exceed the preferred
width rather than overlaying lanes or changing graph meaning.

A cut edge becomes explicit continuation metadata:

```rust
pub struct DagContinuation {
    pub key: String,
    pub edge_kind: DagEdgeKind,
    pub direction: DagContinuationDirection,
    pub related_commit_id: String,
}

pub enum DagContinuationDirection {
    Outgoing,
    Incoming,
}
```

The source row receives a downward outgoing arrow. If the target is inside the page,
the target row receives the matching incoming marker. Stable `key` and edge colour
visually pair the two ends. If the target is outside the page, only the outgoing
marker appears and its accessibility label says that the parent is outside the
loaded range. This is distinct from an edge omitted by the revset, which remains an
indirect or missing edge according to jj's graph semantics.

Feed cut edges to `GraphRowRenderer` as anonymous layout boundaries so the lane is
released and can be reused. Do not map multiple logical lanes onto one display lane.

### 6. Keep a legible fixed lane pitch

Once the projection has bounded normal graphs, use the existing native lane pitches
and node sizes. Remove global uniform compression below a named minimum legible pitch.
The graph column width is derived from the projected lane count and capped by the
sidebar allocation. The ordinary eight-lane budget fits within the existing 192-point
absolute cap at current pitch values.

Use one geometry value for the page in each shell so node drawing and drag hit testing
agree. Continuation markers are layout decoration only: they are not selectable,
draggable, keyboard-focusable, or rebase targets. Their tooltip/accessibility text
may reveal the related change if it is loaded.

### 7. Preserve interaction and refresh semantics

Selection, multi-selection, bookmark dragging, and rebase actions continue to use
real commit IDs and `ChangeInfo.parents`; none may derive semantics from a projected
lane or continuation marker.

Build entries, projection, and row shapes off the UI thread and apply one immutable
`LogGraphPage` under the existing supersession guards. Loading more may extend a
continuation into a real connector, but the entire page must swap atomically so there
is no transient disagreement between rows and geometry.

## Implementation Discipline

Keep the implementation tight and consistent with the project guidelines:

- make the smallest cohesive changes needed for the measured loading and rendering
  failures;
- reuse the existing graph, repository, pagination, and shell abstractions where
  they still fit rather than introducing parallel compatibility paths;
- add a type or helper only when it owns a distinct responsibility or removes real
  duplication;
- keep comments minimal and single-line; comment only non-obvious reasons, invariants,
  or upstream constraints, never restate control flow or type names;
- avoid speculative configuration, generalized graph frameworks, and transitional
  wrappers with no remaining caller;
- keep tests focused on one observable contract at the lowest useful layer and avoid
  assertions that merely mirror constants or field wiring;
- do not update release documentation, the website, Help Book, README feature lists,
  or shell-parity documentation as part of this implementation.

Each implementation slice must leave no dead fields, old depth-based pagination,
duplicate layout calculation, or obsolete renderer branch behind. The two required
cleanup rounds apply to the whole change, not only the files touched most recently.

## Test-Driven Implementation Sequence

### 1. Lock the selection and limit contract

Add failing core/repository tests before changing production code:

- the fallback default expression has context depth two;
- a repository `revsets.log` override is honoured only for the default view;
- an explicit revset bypasses `revsets.log`;
- `log_graph_page(revset, N)` returns at most `N` entries;
- `has_more` is true only when the ordered stream contains row `N + 1`;
- prioritisation happens before limiting, matching pinned `jj log --limit` output;
- metadata conversion is called at most `N` times, not for the look-ahead row;
- increasing the limit preserves the earlier commit-order prefix.

Use `jj-test` fork/merge fixtures for CLI parity. Use a fake/counting graph source for
bounded-work assertions where repository I/O is not relevant.

### 2. Separate context depth from pagination

Replace `DEFAULT_REVSET_DEPTH` and `build_default_revset(depth)` with `LogQuery` and
the independent default-expression and row-limit policies. Introduce `LogGraphPage`,
implement the post-`TopoGroupedGraph` look-ahead limit, and migrate SwiftUI and GPUI
load-more state to `has_more` plus `applied_limit`.

Run the narrow core and shell view-model tests after this slice. At this point the
Rust checkout must return twenty initial rows even if its selected revset contains
hundreds.

### 3. Bound commit metadata construction

Add instrumentation-backed regression tests proving that immutable membership, tag
index construction, empty checks, and `ChangeInfo` creation scale with the returned
page. Implement the displayed-ID membership query and per-page ref indexes, then
remove `ImmutableIds` and repeated per-commit repository-view scans.

Benchmark the core page load against the Rust checkout after each change. Record
median warm results for at least ten runs; report open, graph, metadata, layout, and
total durations separately.

### 4. Collapse the desktop FFI round trip

Add one UniFFI conversion test for `LogGraphPage`, including `has_more`, entries,
layout rows, and a continuation marker. Return the page from the repository call and
remove the SwiftUI desktop call that sends entries back to `computeDagLayout()`.
Regenerate bindings with `just ffi`.

### 5. Specify connector projection with pure Rust tests

Before implementing projection, add table-driven structural tests for:

- a linear history, which is unchanged;
- a nearby fork and merge, which remains fully connected;
- an edge to a parent outside the page, which gets one outgoing marker;
- a long in-page edge, which gets paired outgoing/incoming markers;
- a wide graph that is reduced to the lane budget without hiding change rows;
- a protected first-parent spine, which is never cut;
- deterministic output independent of hash-map iteration;
- direct, indirect, and missing-edge semantics remaining distinguishable;
- real `GraphEntry.edges` remaining unchanged after projection.

Assertions target app-owned row shapes and continuation records, not terminal glyphs
or screenshots.

### 6. Render continuation markers in SwiftUI

Add Swift-only geometry tests for arrow endpoints, clipping, and the minimum legible
lane pitch. Extend `DAGRow+GraphColumn.swift` to draw paired continuation markers on
top of renderer row shapes. Keep all interaction hit testing sourced from real node
centres.

Use a purpose-built visual fixture with one short merge, one long connector, one
out-of-page parent, and more candidate lanes than the budget. Add an XCUITest only if
an interaction cannot be proved below the UI layer; do not add pixel assertions.

### 7. Render the same projection in GPUI

Consume the shared continuation records and projected row shapes. Add a focused
component test only for changed interaction state; Rust tests remain the topology and
projection oracle.

### 8. Validate and profile on the Rust checkout

Open `~/Documents/dev/junk/rust` in both shells and inspect the rendered graph at the
initial page and after **Load More**. Compare ordering and elision with `jj log` and
compare lane density and continuation arrows with all four supplied screenshots.

With warm caches, record:

- time until the first twenty rows are visible;
- rows materialised;
- maximum projected lanes;
- immutable IDs enumerated;
- empty checks performed;
- bytes/records crossing UniFFI;
- load-more time from twenty to forty rows.

Investigate any remaining phase above 100 ms rather than masking it with a spinner.
Do not make wall-clock timing a CI assertion. Commit deterministic work bounds and
the synthetic wide-history fixture as the regression protection. Do not declare the
work ready if the synthetic fixture passes but the Rust checkout remains slow or
illegible.

### 9. Cleanup and verification

Perform two full cleanup rounds over the complete diff. In particular remove:

- depth-based pagination parsing in both shells;
- shell-side `entries.len() >= depth` inference;
- repository-wide immutable ID sets;
- repeated tag/ref scans;
- the SwiftUI desktop layout round trip;
- uniform sub-legible compression and its obsolete tests;
- any layout compatibility helpers retained only for the old pipeline.

Re-run the focused core, UniFFI, Swift, and GPUI tests after cleanup. Before declaring
the implementation ready, run:

```bash
jj --ignore-working-copy log -r 'divergent()'
```

Run `jj fix` and `just lint` once only when preparing the implementation change for
commit or publication, following the repository workflow.

## Expected Areas of Change

Core and bindings:

- `crates/jayjay-core/src/repo/revsets.rs`
- `crates/jayjay-core/src/repo/log.rs`
- `crates/jayjay-core/src/repo/resolve/changes.rs`
- `crates/jayjay-core/src/dag/`
- `crates/jayjay-uniffi/src/repo.rs`
- `crates/jayjay-uniffi/src/dag.rs`

SwiftUI:

- `shell/mac/Sources/JayJay/Repo/ViewModel/Core/RepoViewModel.swift`
- `shell/mac/Sources/JayJay/Repo/ViewModel/Core/RepoViewModel+Refresh.swift`
- `shell/mac/Sources/JayJay/Repo/DAG/DAGLayout.swift`
- `shell/mac/Sources/JayJay/Repo/DAG/DAGGeometry.swift`
- `shell/mac/Sources/JayJay/Repo/DAG/DAGRow+GraphColumn.swift`
- focused tests under `shell/mac/Tests/JayJayTests/`

GPUI:

- `shell/gpui/src/repo/view_model/`
- `shell/gpui/src/repo/toggles.rs`
- `shell/gpui/src/repo/window/dag/`
- focused component tests only where interaction behaviour changes

Exact file splits should follow the nearby responsibility folders. Keep `mod.rs` and
`lib.rs` limited to declarations and re-exports.

## Acceptance Criteria

- The default view uses the repository's `revsets.log` setting, falling back to
  pinned `jj` `builtin_log()` semantics with context depth two.
- The initial graph contains at most twenty real changes and no graph view can exceed
  the named 200-row ceiling without narrowing its revset.
- Ordering and row limiting match pinned `jj log --limit` for the same revset and
  `revsets.log-graph-prioritize` configuration.
- Repository-wide immutable histories are not enumerated during graph loading; all
  per-commit metadata work is bounded by the returned row count.
- SwiftUI performs one bounded repository-to-UI page conversion, not a second
  entries-to-Rust layout round trip.
- Ordinary graphs use the preferred lane pitch. Long or excess connectors become
  explicit continuation arrows; unrelated lanes are never overlaid and nodes are
  never silently removed by visual pruning.
- The projected graph normally contains at most eight simultaneous lanes. Protected
  adjacent topology may exceed that limit rather than become incorrect.
- Opening `~/Documents/dev/junk/rust` is a mandatory acceptance check, not an
  optional benchmark or substitute fixture.
- The Rust checkout shows a readable top graph comparable in density to
  `main_branch.png`, `smartgit_example.png`, and `smartgit2.png`, with the first
  twenty rows visible in under one second on the measurement machine with warm
  caches.
- Load More increases the row limit without rewriting the selected revset and swaps
  entries, layout, and pagination state atomically.
- Selection, keyboard navigation, accessibility, bookmark dragging, and rebase hit
  testing continue to use real change nodes and commit IDs.
- Focused Rust, UniFFI, Swift, and GPUI tests pass after two cleanup rounds, with no
  task-created divergent jj changes.
- The final diff is narrowly scoped, contains no obsolete compatibility path or
  speculative abstraction, and uses comments only where the reason is not apparent
  from the code.

## Risks and Mitigations

### Cutting an edge can obscure ancestry

Continuation markers are explicit, paired when both endpoints are loaded, and retain
the related commit ID for tooltips/accessibility. Real parent edges remain untouched
for commands. Never replace a cut connector with an apparently direct edge to a
different node.

### A lane budget can become another incorrect collapse rule

The budget cuts complete connectors at explicit boundaries; it never maps two lanes
to the same coordinates. Protected adjacent topology may exceed the budget. Pure
structural tests cover this distinction.

### User revset configuration can still be expensive

The complete revset must reach `TopoGroupedGraph` for CLI-compatible ordering, so an
adversarial revset can still have evaluation cost. The post-order limit guarantees
bounded commit materialisation and rendering. Trace evaluation separately; optimise
or allow cancellation later only if measurement shows it is material.

### Increasing the page can alter continuation placement

Projection is deterministic for a given ordered page and the page swaps atomically.
Prefer cuts based on stable row distance and protected spines so extending the tail
does not churn unrelated top rows.

### Upstream renderer instability

Keep `sapling-renderdag` pinned and isolated behind JayJay-owned row shapes. Connector
projection is an app-owned pre-render step, so upstream changes cannot silently alter
the pruning policy.
