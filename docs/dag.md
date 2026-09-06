# DAG Architecture

## Overview

The `dag` work gives JayJay the same graph ordering and lane topology as the pinned `jj log`, while
loading large histories progressively. Rust computes one authoritative layout that SwiftUI and GPUI
render without reconstructing repository topology.

This provides:

- CLI-compatible ordering, connectivity, synthetic elisions, and wide-graph topology;
- a useful initial result without materialising an arbitrarily large history;
- bounded publication and metadata batches, cancellation intervals, and automatic row retention;
- identical topology in the SwiftUI and GPUI shells;
- explicit clipping and focus behavior when the graph is wider than the sidebar.

## Core pipeline

`Repo::start_log_graph` evaluates the selected revset once against a pinned repository snapshot. It
streams `jj_lib`'s `TopoGroupedGraph`, applies `revsets.log-graph-prioritize`, and sends the ordered
node/parent sequence to `sapling-renderdag::GraphRowRenderer`, the renderer used by the pinned `jj`
release.

The adapter in `crates/jayjay-core/src/dag/renderdag.rs` converts renderer output into JayJay-owned
`DagRowShape` values. No upstream renderer type crosses the core boundary. SwiftUI receives the
result through UniFFI; GPUI consumes the same core model directly.

The layout preserves direct, indirect, and missing edges, including edges whose targets are outside
the published prefix. Wide graphs retain every lane and connector. A row may terminate one edge and
reuse that column for another continuing link. Layout never changes `GraphEntry.edges` or
`ChangeInfo.parents`.

With `ui.log-synthetic-elided-nodes` enabled, each indirect edge expands through a unique synthetic
node before layout. JayJay folds the resulting synthetic row into the preceding real row's
`elisions_after` bands, preserving `jj log` emission order without creating a selectable change.
With the setting disabled, indirect edges remain dashed ancestor connections.

Shells draw the path from the owning change through the band glyph to the target lane as a dotted
line. A band is four corner radii tall, so its elbows use the same radius as the row's own fork and
merge curves.

`DagLayout` contains ordered row shapes and aggregate width information. Rows record the commit and
node column, vertical cells, an optional fork/merge link line, missing-edge terminations, and
synthetic bands. Debug validation checks row order, identities, column bounds, node placement, and
elision ownership.

## Progressive loading

The default loading policy is:

- `INITIAL_LOG_BATCH_ROWS = 50` for the normal first snapshot;
- `BACKGROUND_LOG_BATCH_ROWS = 500` for work and progress checkpoints;
- `FIRST_RESULT_BUDGET = 10 seconds`, after which any complete prefix is published rather than
  withholding the first result;
- `MAX_AUTO_LOADED_ROWS = 10,000`, derived from a 400 MB retained-memory budget and a conservative
  40 KB per row.

These limits do not change default-history depth or explicit revset semantics.

The session emits cumulative `Snapshot` values containing entries and the layout for exactly that
prefix. It also emits progress, deferred empty-state corrections, pause notifications, and one
terminal finished, canceled, or failed event. Entry order is append-only, but layout may change as a
larger prefix extends existing connectors. Shells therefore replace entries and layout atomically.

Publication thresholds grow geometrically, keeping cumulative layout and transfer work linear in
the retained row count. Only retained rows become `GraphEntry` values, and repository metadata is
indexed per published set. Expensive empty-state checks for merge and off-page commits run after the
prefix is available and return corrections separately.

Cancellation is cooperative between bounded units of streaming and metadata work and preserves the
latest complete snapshot. At the retained-row ceiling, **Continue Loading** raises the limit and
resumes the same stream without re-evaluating the revset. Shell generation guards reject late events
from obsolete repository snapshots.

## Presentation and focus

Shell geometry uses a preferred lane pitch, compresses only to a legibility floor, and caps the
graph gutter by sidebar and absolute-width limits. Lanes beyond that width are clipped behind an
overflow badge; clipping never changes the core layout. Each change's text begins after that row's
graph prefix rather than after the widest row in the result. Drawing and hit testing share the same
geometry.

**Hide Unrelated Changes** focuses the graph on a change's connected lineage: its ancestors and
descendants within the current revset. The overflow badge, the row context menu, and the Repository
menu (`mod+shift+L`) provide it. The core composes the focused revset as:

```text
(<base>) & (::<target> | <target>::)
```

The base revset remains unchanged while focused. Normal rows use their selection revision;
divergent rows use the exact commit ID. A pending target is selected and revealed when it appears in
a progressive snapshot. A **Related to** pill identifies the target and clears focus, as do
**Clear Focus** in the context menu and Escape after active drag and multi-selection handling.

Depth-based **Load More** is disabled while focused because the effective revset is explicit. The
retained-row ceiling and **Continue Loading** still apply. Focus state is transient across launches.

## Elision expansion

**Show Elided Revisions**, in the context menu of a row that owns elision bands or the Repository
menu for the selected row, adds each band's hidden path to the queried revset:

```text
(<base>) | (present(<target>)::present(<owner>))
```

`present` keeps an expansion from failing the load after either end is abandoned. Expansions stack,
sit beneath focus (focus scopes the expanded base), and survive depth-based **Load More**. A new base
revset clears them. **Hide Expanded Revisions** applies only to an expanded row and removes that
row's expansions. Expansion state is transient across launches.

## `jj log` parity

`crates/jayjay-core/tests/log_graph_parity.rs` creates a hermetic repository and compares JayJay's
production progressive graph path with the pinned `jj log`. Both sides use the same revset, limit,
synthetic-elision setting, prioritisation, and isolated configuration. The fixture covers elisions
enabled and disabled, a displayed descendant of the working copy, parents beyond the prefix, a
focused revset, and an expanded elision.

Run the gate with:

```bash
just test-rust jayjay-core --test log_graph_parity
```

For diagnosis against another repository, `dump_log_graph_parity` prints JayJay's structural ASCII
graph and the equivalent bounded `jj log` invocation:

```bash
cargo run -p jayjay-core --example dump_log_graph_parity -- <repo> '<revset>' <limit>
```

Application shells consume `DagLayout`, not this diagnostic projection. Shell-only drawing changes
normally use geometry tests because CLI parity ends at the app-owned layout boundary.

## Performance evidence

Warm release measurements on a repository with approximately 339,000 revisions produced:

| Query | First snapshot | Retained result | Total |
| --- | ---: | ---: | ---: |
| `jayjay()` | 83 ms | 639 rows, finished | 342 ms |
| `::trunk()` with a 500-row ceiling | 49 ms | 500 rows, paused | 61 ms |
| `::trunk()` with the production ceiling | 72 ms | 10,000 rows, paused | 238 ms |

The 10,000-row run peaked at 224 MB RSS on the measurement machine. These are diagnostic results,
not CI thresholds. The profiler reports repository-open time, total load time, retained rows, and
per-stage busy time:

```bash
cargo run --release -p jayjay-core --example profile_log_graph -- <repo> '<revset>'
```

### Comparison with full materialisation

Before this work, the shells loaded the graph through `Repo::log_graph`, which materialises the
whole revset and then computes the layout. Nothing is shown until both finish. The table compares
that path with the progressive session on a repository with approximately 346,000 revisions.
Each run is a separate release process. The full-load column times `log_graph` plus
`DagLayout::compute` only; shell rendering of every row comes on top.

| Revset | Full load: first paint, peak RSS | Progressive: first snapshot, total, peak RSS |
| --- | ---: | ---: |
| default at depth 20 (640 rows) | 636 ms, 313 MB | 50 ms, 415 ms (finished), 245 MB |
| `::trunk()` (340,826 rows) | 129.5 s, 2.47 GB | 38 ms, 192 ms (10,000 rows, paused), 238 MB |
| `all()` (346,108 rows) | 129.0 s, 2.52 GB | 62 ms, 223 ms (10,000 rows, paused), 237 MB |

The progressive session shows the depth-20 default view 13 times sooner. On whole-history revsets it bounds
both time and memory: the full load blocks for over two minutes and holds about 2.5 GB.

The advantage shrinks with repository size. On repositories with 650 to 1,250 revisions, `all()`
loads in 74–77 ms with full materialisation and finishes in 72–74 ms progressively, with the first
snapshot at 59–64 ms. Revset evaluation dominates both paths at that size, and the default revset
loads in about 2 ms on either path once warm.

## Validation map

- Core layout and invariants: `crates/jayjay-core/src/dag/`
- Progressive sessions: `crates/jayjay-core/tests/log_graph_session.rs`
- Ordering and metadata: `crates/jayjay-core/tests/log_graph_ordering.rs`
- Real-CLI topology: `crates/jayjay-core/tests/log_graph_parity.rs`
- Focus revsets: `crates/jayjay-core/src/revset/expressions.rs` and `crates/jayjay-core/tests/log_graph_parity.rs`
- SwiftUI geometry and state: `shell/mac/Tests/JayJayTests/DAGGeometryTests.swift`,
  `DAGRowViewModelTests.swift`, and `RepoViewModelFocusTests.swift`
- GPUI geometry and state: `shell/gpui/src/repo/window/dag/`,
  `shell/gpui/src/repo/view_model/loaders/mod.rs`, and `shell/gpui/tests/gpui/`
