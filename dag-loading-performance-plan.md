# Progressive DAG Loading

## Status

Implemented. This document records the runtime contract for repository graph loading. The policy
constants and session types live in `crates/jayjay-core/src/repo/graph_load.rs`; the repository
pipeline lives in `crates/jayjay-core/src/repo/log.rs`.

## Purpose

Graph loading must make a useful prefix available without first materialising an arbitrarily large
revset. The core therefore owns one progressive, cancellable load session whose snapshots contain
both entries and their matching DAG layout. SwiftUI receives the session through UniFFI; GPUI calls
the same Rust API directly.

The shells never reconstruct topology or compute a second layout from transferred entries.

## Policy

- `INITIAL_LOG_BATCH_ROWS = 50`: first normal publication threshold.
- `BACKGROUND_LOG_BATCH_ROWS = 500`: bounded work and progress checkpoint size.
- `FIRST_RESULT_BUDGET = 10 seconds`: after this time, publish any complete prefix instead of
  withholding all useful data. It is not a worker timeout.
- `MAX_AUTO_LOADED_ROWS = 10,000`: automatic retained-row ceiling derived from the 400 MB memory
  budget and the conservative 40 KB-per-row allowance.

These are presentation and resource policies, independent of the selected revset. Default-history
depth and explicit revset semantics remain unchanged.

## Session contract

`Repo::start_log_graph` evaluates and prioritises the revset once against a pinned repository
snapshot. It streams `TopoGroupedGraph` in the same order as `jj log`, incrementally materialises
complete rows, and emits `LogGraphEvent` values:

- `Snapshot`: an ordered cumulative entry prefix and the `DagLayout` computed for exactly that
  prefix;
- `Progress`: consumed/materialised counts and elapsed time;
- `EmptyStates`: late corrections for merge and off-page-boundary commits whose emptiness requires
  more expensive parent-tree work;
- `Paused`: the retained-row ceiling was reached while more history remains;
- one terminal `Finished`, `Canceled`, or `Failed` event.

Snapshots are cumulative and append-only in entry order. Layout may be replaced as a prefix grows,
but entries and layout in one snapshot are always mutually consistent. Geometrically increasing
publication thresholds keep cumulative layout and transfer work linear in the final retained row
count.

## Cancellation and continuation

`GraphLoadToken` is request-scoped. Cancellation is cooperative and is checked between bounded
units of graph streaming and metadata work. Cancellation keeps the latest fully published snapshot;
partially materialised rows never cross the API boundary.

At the automatic ceiling the worker emits `Paused` and waits on the same token. **Continue Loading**
raises the ceiling and resumes the existing stream. It does not re-evaluate the revset or start a
second worker.

Shell generation guards remain mandatory: cancellation stops work, while generation checks prevent
late events from an obsolete repository snapshot from replacing newer state.

## Bounded metadata work

Only retained rows are converted to `GraphEntry`. Repository metadata is indexed per published set,
including refs, workspaces, immutable membership, divergence, empty state, and shortest IDs. The
synthetic root is removed before commit materialisation. A complete-materialisation `log_graph()`
API remains for tests and non-UI callers, but desktop loading uses the progressive session.

## Performance evidence

The large-repository validation target contained approximately 339,000 revisions in `::trunk()`.
Representative warm release measurements from the implementation pass were:

| Query | First snapshot | Retained result | Total |
| --- | ---: | ---: | ---: |
| `jayjay()` | 83 ms | 639 rows, finished | 342 ms |
| `::trunk()` with a 500-row ceiling | 49 ms | 500 rows, paused | 61 ms |
| `::trunk()` with the production ceiling | 72 ms | 10,000 rows, paused | 238 ms |

The 10,000-row run peaked at 224 MB RSS on the measurement machine. These numbers are diagnostic
evidence, not CI thresholds; repository shape, hardware, and cache state affect wall time.

Use the maintained profiler for new measurements:

```bash
cargo run --release -p jayjay-core --example profile_log_graph -- <repo> '<revset>'
```

## Regression coverage

- `crates/jayjay-core/tests/log_graph_session.rs`: publication, cancellation, pause/continue,
  invalid revsets, and deferred empty-state correction.
- `crates/jayjay-core/tests/log_graph_ordering.rs`: CLI-compatible ordering and metadata.
- `crates/jayjay-core/tests/log_graph_parity.rs`: real-CLI topology parity at bounded prefixes.
- SwiftUI and GPUI view-model tests: stale-generation rejection, selection preservation, and
  refresh/cancel state.

The deterministic tests prove behavior. Large-repository profiling separately checks latency and
memory characteristics.
