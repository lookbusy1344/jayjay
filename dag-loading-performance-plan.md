# DAG Loading Performance Plan

## Scope and working constraint

- Implement and validate this work in the current checkout. Do not create a sibling workspace or a Git worktree.
- Target the `kuuontso` DAG implementation at `539d9da0`.
- Keep DAG semantics shared in Rust. GPUI and SwiftUI should consume the same progressive snapshots and only own presentation and cancellation wiring.
- Preserve the current depth-based default revset behavior. This plan bounds the work before each published snapshot, but it does not reintroduce the earlier `builtin_log()`/default-revset redesign.

## Diagnosis

The slow path is eager graph materialization, not primarily revset parsing.

Observed against `/Users/johnsparrow/Documents/dev/junk/rust`:

- `all()` contains 344,188 revisions.
- `jj log --no-graph -r 'all()'` enumerates the repository in approximately 1.96 seconds.
- `jj log -r 'all()'` with graph ordering completes in approximately 3.81 seconds.

JayJay currently turns the same query into an all-or-nothing operation:

1. `Repo::collect_graph_rows()` exhausts the complete `TopoGroupedGraph` stream and loads a `Commit` for every selected revision. Its per-row `should_include_in_log()` call also performs working-copy and bookmark lookups even though the graph path principally needs to exclude jj's known synthetic root.
2. `Repo::materialize_graph_entries()` enriches every row before returning anything:
   - immutable membership is computed for the entire displayed set;
   - `repository_divergent_change_ids()` calls `resolve_change_id()` once per unique displayed change ID;
   - empty-state checks and shortest-prefix calculations run for every commit;
   - bookmark, tag, workspace, and remote-ref data are indexed for the result.
3. `DagLayout::compute()` lays out the entire result. Its lane-budget loop can rerun the renderer once per projected edge.
4. SwiftUI receives every `GraphEntry` over UniFFI and then passes the complete array back into Rust through `computeDagLayout()`, duplicating serialization and allocation.
5. GPUI calls `DagLayout::row()` from each visible row; that method linearly scans `DagLayout.rows`, making scrolling an O(visible rows × loaded rows) operation.

The current cancellation mechanisms do not stop this work:

- GPUI's spinning refresh button always starts another refresh.
- The detached `background_update()` task has no retained cancellation handle.
- Generation counters prevent stale results from being applied, but do not stop the old worker consuming CPU and memory.
- `Repo::cancel_running_jj_processes()` only controls child processes and permanently closes that process registry; `log_graph()` is in-process jj-lib work and must not use it.

## Intended behavior

### Progressive first result

- Define named policy constants such as `INITIAL_LOG_BATCH_ROWS` (proposed value: 50), `BACKGROUND_LOG_BATCH_ROWS` (proposed value: 500), and `FIRST_RESULT_BUDGET` (proposed value: ten seconds); do not scatter those values through core and shell code.
- Materialize and publish the first 50 rows as soon as they form a self-consistent graph prefix.
- Keep the same core load session running on a background worker and append further batches without blocking selection, scrolling, diff viewing, or other read-only interaction.
- Use small initial publishes and geometrically increasing cumulative publish thresholds (`50 -> 100 -> 200 -> 400 -> ...`) so the UI becomes useful quickly without recomputing and transferring the entire cumulative layout every 50 rows.
- Treat `BACKGROUND_LOG_BATCH_ROWS` as the bounded internal work/checkpoint size, not the publish cadence. Yield between batches so event handling and cancellation are not starved; publish only when the next cumulative threshold is reached or the stream finishes.
- Continue until the revset is exhausted, the repository snapshot becomes stale, an error occurs, or the user cancels.
- Treat ten seconds as the foreground first-result budget, not as the lifetime of the background load. At that point publish any fully materialized prefix and keep loading in the background.
- If no row is available after ten seconds, preserve the previous graph and show “Still loading history…” with the cancel affordance. Initial open remains in a cancellable loading state.
- Invalid revsets and repository failures remain errors; they must not be reported as slow progress or partial success.

The budget and cancellation are cooperative, checked between bounded units of jj-lib work. They may overshoot by one `stream.next()`, commit load, or metadata batch. A strict wall-clock kill would require process isolation; detaching or timing out the UI future alone would leave the expensive worker running and is not acceptable.

Progressive batching is the primary performance and responsiveness control. The ten-second budget only guarantees that the presentation acknowledges a slow first batch instead of looking hung.

Publishing every fixed 50 rows for the whole query is deliberately not the design. The Rust repository would produce about 6,884 UI updates, and recomputing a cumulative DAG at each update would make total layout and transfer work quadratic. The first 50 rows provide the fast usable result; widening thresholds preserve progressive disclosure while keeping cumulative work approximately linear.

### Cancellation

- While a graph session is active, the spinning refresh button becomes **Cancel Update** and its click cancels that session instead of starting another refresh.
- The refresh keyboard action uses the same state-dependent command, so it cannot accidentally create overlapping refreshes.
- Cancellation is latched in a request-scoped core token and checked throughout streaming, metadata materialization, and layout preparation.
- The button changes to a non-animated “Cancelling…” state immediately; the worker stops at the next cooperative check.
- Cancellation always keeps the latest fully published snapshot; unpublished rows and partial metadata are discarded.
- Cancellation never cancels fetch, push, mutation, or another window's graph request.
- Read-only interaction remains enabled while background loading continues. A repository mutation first cancels the active graph session, validates and performs the mutation, then starts a fresh session against the resulting repository state.

## Design

### 1. Add a progressive core load session

Introduce app-owned types in `jayjay-core`:

```rust
pub struct LogGraphRequest {
    pub revset: String,
    pub initial_rows: u32,
    pub background_batch_rows: u32,
    pub first_result_budget: Duration,
}

pub enum LogGraphEvent {
    Snapshot(LogGraphSnapshot),
    Progress(LogGraphProgress),
    Finished,
    Canceled,
    Failed(CoreError),
}

pub struct LogGraphSnapshot {
    pub entries: Vec<GraphEntry>,
    pub layout: DagLayout,
    pub loaded_rows: u32,
}

pub struct LogGraphProgress {
    pub consumed_rows: u64,
    pub materialized_rows: u64,
    pub elapsed: Duration,
}
```

Use a request-scoped `GraphLoadToken` backed by `Arc<AtomicBool>`. Expose `cancel()` and a cheap `check()` operation. Make the token available over UniFFI so SwiftUI can cancel the same in-process operation; GPUI uses it directly.

Keep `Duration` and `CoreError` as core concepts; expose primitive millisecond fields and the existing binding-safe error vocabulary at the UniFFI boundary.

`Repo::start_log_graph(request, token)` should start one worker that owns the `ReadonlyRepo` snapshot, revset evaluation, `TopoGroupedGraph` stream, and event sender for its entire lifetime. A channel-backed session avoids a self-referential shell object containing both the owned repository and a revset stream borrowing it. Expose a shell-friendly way to receive or drain events off the UI thread; never invoke foreign callbacks while holding repository locks.

Each published snapshot contains entries and layout computed from the same cumulative prefix. This establishes one core-owned snapshot and removes SwiftUI's `GraphEntry[] -> UniFFI -> Swift -> UniFFI -> DagLayout` round trip. Prefer append events plus a replacement layout when the binding can express them without complexity; otherwise cumulative snapshots are acceptable because geometric thresholds keep total transferred rows below roughly twice the final result.

Keep `log_graph()` only for tests or non-UI callers that explicitly require complete materialization; UI code must not call it.

### 2. Stream and publish complete prefixes

Refactor `collect_graph_rows()` into a session loop that:

1. evaluates and prioritizes the revset once against a pinned `ReadonlyRepo`;
2. incrementally consumes `TopoGroupedGraph`;
3. checks cancellation between rows and metadata batches;
4. materializes the first publishable prefix of 50 rows;
5. continues with bounded background batches until the stream is exhausted.

Do not collect the full revset before publishing. Preserve stream order exactly and append only, so selection and scroll anchors remain stable by commit ID.

Remove synthetic-root filtering from the expensive generic predicate on this path: compare the streamed commit ID with `repo.store().root_commit_id()` before `get_commit()`. Preserve `should_include_in_log()` for callers whose contract genuinely needs its broader filtering, but do not perform bookmark and working-copy lookups for every graph row merely to identify the root.

Keep `MAX_CONTINUOUS_CONNECTOR_ROWS` raw rows as look-ahead when forming a published prefix. Compute layout over the published rows plus that look-ahead, then expose only the published prefix. This lets short connectors crossing a batch boundary render continuously while longer connectors remain stable continuation markers. Without look-ahead, the last few visible rows would repeatedly change between a termination marker and a full connector as each batch arrived.

Edges targeting rows beyond the look-ahead remain in the raw graph input and project into outgoing continuations. When the target is eventually loaded it receives the paired incoming continuation. Preserve the viewport's top commit-ID anchor when replacing a cumulative layout so any bounded tail reflow does not move the user's place.

Do not fully materialize look-ahead rows merely to run layout. Introduce a lightweight internal layout input containing commit ID, ordered parents/edges, working-copy state, and the presence of a local ref. Build `GraphEntry` only for rows being published.

### 3. Make metadata work set-based and bounded

Run metadata only for retained rows. In particular:

- Replace `repository_divergent_change_ids()`'s per-change `resolve_change_id()` loop with one set-based query equivalent to `divergent() & commits(displayed_commit_ids)`. This still marks a displayed version divergent when its sibling is outside the current prefix.
- Retain the current displayed-set intersections for `immutable()` and `parents(immutable())`, but measure them independently and check cancellation/the first-result budget before and between the two evaluations.
- Retain the one-pass `CommitRefIndex`; it is preferable to per-commit scans.
- Process empty-state and `ChangeInfo` materialization in small fixed-size batches, checking the request guard between batches.
- Bound `empty_commit_cache` with an LRU or clear it when repository state advances so repeated rewrites cannot grow it indefinitely.
- Measure shortest-prefix lookup separately. Cache or batch it only if the bounded-batch profile shows it remains material.

Do not return a row whose `ChangeInfo` was only partly computed. Every published snapshot is a prefix of complete `GraphEntry` values.

If the first-result budget or cancellation arrives during metadata work, finish no additional batch. Publish only the last fully materialized prefix; no incomplete row crosses the API boundary. Entry data is append-only for a snapshot-bound session, but the cumulative `DagLayout` may be replaced as newly known topology changes projection decisions.

### 4. Compute layout at bounded cumulative thresholds

Compute `DagLayout` in core and return it beside the entries. Publish at 50 and 100 rows, then double the cumulative threshold. Geometric publishing keeps total layout input across a complete load O(total rows), whereas recomputing after every fixed 50-row batch would become quadratic.

Benchmark `DagLayout::compute()` with wide synthetic graphs and at progressive publish thresholds. If the lane-budget rerender loop is still material:

- define the deterministically sorted cut-candidate prefix as the optimization unit;
- add generated DAG tests proving that adding candidates from that prefix cannot increase the rendered lane count;
- if that invariant holds, find the smallest sufficient cut prefix with exponential search followed by binary search, reducing full render passes from O(candidates) to O(log candidates);
- if it does not hold for `sapling-renderdag`, move lane-budget selection into a single-pass renderer adapter rather than relying on an invalid monotonicity assumption.

Do not optimize this loop by silently cutting every optional edge; retain the current priority rules for working-copy ancestry, adjacent first parents, referenced changes, and direct versus indirect edges.

### 5. Remove GPUI's linear row lookup

`DagLayout.rows` and `GraphData.entries` are generated in the same order. In the `uniform_list` processor:

- retrieve `entry` and `DagRowShape` with the same `ix`;
- pass `&DagRowShape` directly to `dag_column()`;
- add a `debug_assert_eq!(row.commit_id, entry.change.commit_id.id)` at the boundary;
- remove `DagLayout::row()` from the GPUI render path.

Keep a commit-ID index only for genuinely random-access operations. SwiftUI may retain its single precomputed dictionary because its row lookup is not linear.

### 6. Track session identity and progress

Store these independently in each view model:

- the submitted revset text;
- the active `GraphLoadToken`, session handle, and request generation;
- loaded-row count and whether the stream is complete;
- first-result, background-loading, canceling, and terminal states;
- the commit-ID viewport anchor used across cumulative layout replacements.

For explicit revsets, including `all()`, background loading proceeds without changing the revset or restarting its graph stream. The status surface reports progress without claiming a total that would require exhausting the revset in advance.

Do not retain a borrowed jj-lib stream in either shell or expose it through UniFFI. The core worker owns it and emits app-owned values. If the repository operation changes, cancel the session, reject its remaining events by generation, and start a new session from a new snapshot.

Keep the default revset's current depth expansion and 20-row behavior. Progressive background loading primarily changes explicit large revsets; it must not silently widen the default revset beyond the depth the user selected.

Completing `all()` can retain hundreds of thousands of rows. Store append-only entry data without cloning the cumulative prefix inside GPUI, and measure resident memory per row before enabling automatic completion in release builds. Define a named memory/row safety ceiling; if it is reached, pause with an explicit **Continue Loading** action rather than exhausting memory or pretending the query is complete.

### 7. Wire GPUI refresh/cancel state

- Replace direct toolbar calls to `vm.refresh(false, cx)` with `vm.refresh_or_cancel(cx)`.
- On refresh start, create and store a new token before dispatching the worker.
- Split “no usable graph yet” from “background graph session active.” Applying the first snapshot ends the blocking/initializing state but leaves the toolbar spinner and cancel action active.
- On cancel, latch the token and leave the generation current until the worker reports its outcome.
- Continue using `refresh_gen` to reject stale completions after a later request.
- Clear the token only when the matching generation finishes.
- Coalesce filesystem events while a session is active, cancel the stale session once, and start one replacement session after cancellation is observed; do not create overlapping graph workers.
- Render the toolbar action and tooltip from explicit `Idle`, `Refreshing`, and `Cancelling` states rather than deriving cancellation behavior from the animation flag.
- Coalesce progress notifications to a bounded cadence so a fast stream cannot make the UI rerender once per row or metadata batch.
- Allow graph-independent reads and diff loading to use the published snapshot while the graph worker continues against its pinned `ReadonlyRepo`. Permit only one graph session per window.

Mirror the token and result handling in SwiftUI's `RepoViewModel` so both shells share the bounded core contract, even if the first user-facing cancel affordance is implemented in GPUI.

## TDD sequence

### Core tests first

1. Extract a deterministic request guard with an injectable clock. Test first-result budget, cancellation, and completion without wall-clock sleeps.
2. Add a graph-session test proving an `all()`-like history publishes 50 rows before exhausting the source, then publishes ordered cumulative prefixes until `Finished`.
3. Add a test proving cumulative publish thresholds grow geometrically and total layout input remains linear in the final row count.
4. Add a look-ahead test proving short connectors across a batch boundary remain continuous while long connectors become paired continuations.
5. Add a graph-stream test proving the synthetic root is skipped before commit materialization and ordinary rows do not invoke per-row bookmark filtering.
6. Add a first-result-budget test whose fake clock expires mid-batch and assert only the last complete prefix is published while background loading remains active.
7. Add a cancellation test proving the stream stops being polled after the token is latched and sends no later snapshots.
8. Replace the divergence implementation only after a regression test proves a displayed commit remains marked divergent when its sibling lies outside the current prefix.
9. Add a counter-based test proving metadata work before the first snapshot is proportional to the initial batch and look-ahead, not total matching revisions.
10. Add a stale-snapshot test proving an operation change cancels the session and prevents later events from being applied.

### GPUI tests next

1. Add a component test proving clicking the spinning refresh button calls `cancel()` and does not enqueue a second refresh.
2. Test the `Refreshing -> Cancelling -> Idle` transition and stale-generation rejection.
3. Test that the first snapshot enables selection, scrolling, and diff loading while the graph session continues.
4. Test that later snapshots append in order, preserve selection by commit ID, and restore the viewport anchor.
5. Test that cancellation retains the latest published snapshot and never applies unpublished rows.
6. Test indexed row pairing and the commit-ID assertion with a representative graph.

### SwiftUI adapter tests

1. Verify each snapshot returned over UniFFI contains both entries and layout and requires no layout recomputation call.
2. Verify successive session events apply on the main actor without replacing selection by array index.
3. Verify canceling the Swift task also latches the core graph token.
4. Reuse core tests for streaming semantics; do not duplicate them in Swift.

## Performance validation

Add a maintained profiling entry point or benchmark that reports these stages separately:

- repository open;
- revset evaluation;
- ordered rows consumed and rows retained;
- commit loads/filtering;
- immutable membership;
- divergence detection;
- ref indexing;
- empty-state evaluation;
- `ChangeInfo` materialization;
- DAG projection/layout;
- UniFFI conversion where applicable.

Validate release builds, cold and warm, against the Rust checkout and a synthetic wide graph. Record evidence separately for core loading, GPUI behavior, and SwiftUI behavior.

Acceptance targets:

- An initial `all()` request on the 344k-revision Rust checkout displays 50 useful rows without first enumerating or materializing all 344k rows.
- Warm first-snapshot core load is targeted below 2 seconds; after ten seconds an adverse load must visibly report continued background progress or expose its latest complete prefix.
- The user can select rows, scroll, and view diffs while later graph snapshots arrive.
- Work counters show metadata before first paint bounded by the initial batch plus look-ahead.
- Cancel produces no second refresh, stops CPU growth promptly, and never alters repository state.
- GPUI visible-row lookup is O(1) per row.
- SwiftUI consumes graph-session events over UniFFI and performs no second full-graph layout round trip.
- Cumulative layout work across progressive publishes is O(total loaded rows), excluding the separately measured lane-projection candidate search.
- Memory remains proportional to loaded rows and pauses at the explicit safety ceiling rather than growing without bound.

## Implementation order

1. Add deterministic session, first-result-budget, and cancellation tests.
2. Add `LogGraphRequest`, `LogGraphEvent`, `LogGraphSnapshot`, `GraphLoadSession`, and `GraphLoadToken` in core.
3. Stream `TopoGroupedGraph` into complete prefixes with look-ahead instead of exhausting it before returning.
4. Replace per-change divergence resolution with a set-based query and bound the empty-state cache.
5. Publish entries plus layout through the session's UniFFI events and update SwiftUI.
6. Update GPUI to consume progressive snapshots and remove linear `DagLayout::row()` rendering lookups.
7. Make the GPUI refresh control statefully cancel the active graph request; mirror core cancellation in SwiftUI.
8. Cancel and restart sessions safely across repository operations and filesystem refreshes.
9. Measure memory per row and add the automatic-loading safety ceiling with explicit continuation.
10. Profile layout projection; optimize repeated renderer passes only if measurements justify it and the required invariant is proven.
11. Run two cleanup rounds, the focused core/GPUI/Swift tests, `just ffi`, then the repository's final fix/lint gates when the implementation is ready to commit.

## Non-goals

- Do not kill or abandon background threads after a UI timeout.
- Do not use `cancel_running_jj_processes()` for in-process graph loading.
- Do not snapshot, mutate, or otherwise refresh the repository merely to paginate a read.
- Do not change `all()` to a different revset or silently claim a progressive snapshot is the complete result.
- Do not hold a jj-lib graph stream across repository reloads; cancel it and start a new snapshot-bound session.
- Do not change user-facing guide, Help Book, website, or parity documentation as part of this implementation.
