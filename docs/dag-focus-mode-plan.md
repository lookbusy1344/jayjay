# DAG Focus Mode

## Context

Wide merge-heavy histories (rust-lang/rust's bors rollups: ~32 native columns for
`jayjay()`, ~52 for `::trunk()`) overflow the graph gutter. The current treatment is
a trailing chevron badge marking a row whose native lanes exceed the gutter budget
(`DAGRowViewModel.isGraphClipped`). The badge is honest but inert: it signals that
lanes are hidden without offering any way to read them, and on a rollup-heavy repo
every clipped row shows the same mark.

Focus mode makes the badge an affordance. Clicking it scopes the graph to the
clicked change's connected lineage, so unrelated lanes drop out and the row becomes
legible. Focus is opt-in and reversible; the default graph is unchanged.

## Working conventions

- Implement in the current checkout. Do not create a sibling jj workspace, git
  worktree, or hidden agent worktree for this work.
- `~/Documents/dev/junk/rust` is the canonical large, complex-tree target for manual
  validation (wide bors-rollup history where focus matters most). Treat it as
  read-only: `jj --ignore-working-copy -R ~/Documents/dev/junk/rust ...`; no fetch,
  snapshot, edit, or reconfigure.

## Constraint: parity is preserved

`docs/jj-log-dag-parity-plan.md` fixes a hard contract: the default DAG renders the
exact `jj log` graph — same real/synthetic node sequence, connectivity, and renderer
column allocation — gated by a test that runs the real `jj log` binary. A prior
lane-projection scheme (cutting to eight lanes) was removed as a defect. "Width
pressure never changes graph topology."

Focus mode does not violate this. Focus is a **revset change**, not a topology
transform. The focused graph is the exact `jj log` of a narrower revset:

```
(<base revset>) & (::X | X::)
```

`::X` is X's ancestors (inclusive), `X::` its descendants (inclusive); their union is
X's connected lineage, intersected with the user's current base revset so focus is
always a subset of what was shown. Every lane, column, and connector in the focused
view is still exactly what `jj log` produces for that revset. The parity gate runs on
explicit-revset fixtures; extend it with a composed focus-revset case using the
existing deterministic fixture and real pinned `jj log` oracle. This is an acceptance
gate, not optional confidence: focus may change which revisions are requested, but it
must never change the real/synthetic node sequence, connectivity, or renderer column
allocation produced for that effective revset. No new lane-collapsing algorithm, no
projection markers, no change to the shared Rust layout or parity implementation.

The collapse of "uninteresting lanes" is a consequence of excluding unrelated commits
from the revset, not of hiding connectors within a fixed graph.

## Approach

State and composition live in the shell view model; the only core addition is a pure,
shared revset-composition helper.

### Rust core — shared revset helper

Add to `crates/jayjay-core/src/repo/revsets.rs`, beside `build_default_revset` and
`combined_diff_revsets`:

```rust
/// Scope `base` to the connected lineage of `target` (ancestors and descendants,
/// inclusive), so a focused graph drops lanes unrelated to `target` while staying a
/// subset of `base`. The result is a plain revset string parsed by the normal path.
pub fn focus_revset(base: &str, target: &str) -> String
```

Producing `(<base>) & (::<target> | <target>::)`. `target` is the row's selection
revision: its stable change id normally, or its exact commit id when the change is
divergent. This matches the existing `ChangeInfo.selectionRevision` identity rule and
ensures clicking one divergent row does not pull every sibling version into focus.
Keep the composition in Rust so both shells share one definition, it is unit-tested,
and revset string assembly is not duplicated (and mis-quoted) per shell. Export it
through `crates/jayjay-uniffi/src/repo.rs` next to `default_revset_with_depth` /
`revset_presets`. No change to `LogGraphRequest`, `start_log_graph`, tokens, or the
event stream — the boundary already accepts an arbitrary revset string.

### SwiftUI — state and composition

- **State:** add `focusedRevision: String?` to `RepoViewModel`
  (`shell/mac/Sources/JayJay/Repo/ViewModel/Core/RepoViewModel.swift`), beside
  `var revset`. `revset` remains the user's base filter, untouched by focus. Keep the
  short display identity separately if the focus pill cannot derive it from the
  focused row; do not assume every focused revision is a change id.
- **Composition chokepoint:** in `RepoViewModel+Refresh.swift` `refresh(...)`, where
  `RepoGraphRefreshContext` captures the revset (~line 113–119), compute the effective
  revset:
  `let effective = focusedRevision.map { focusRevset(base: revset, target: $0) } ?? revset`
  and build the `LogGraphRequest` from `effective`. Everything downstream already
  takes whatever string it is handed.
- **Enter/exit trigger:** add `focus(on:)` and `clearFocus()` that set/clear
  `focusedRevision` and call `refresh(selecting:)` exactly as `applyRevset` does
  (reset `graphRowCeiling = 0`, `graphPaused = false`), so a focus change re-queries
  through the same path. `focus(on:)` receives `entry.change.selectionRevision` and
  also selects that revision so the detail view follows.
- **Replacement transition:** focusing from the chevron, clearing focus from the pill
  or Escape, and applying or removing a base filter all leave the prior DAG visible at
  45% opacity and non-interactive until the first snapshot from the replacement query
  arrives. Keep the focus pill and filter controls interactive so the user can recover
  or supersede the request. If the focused query fails or completes without its exact
  target, retain the prior DAG in this dimmed, disabled state rather than presenting it
  as current data.
- **Pin the focused target through progressive loading:** the effective revset includes
  both ancestors and descendants, and descendants are ordered above X. X may therefore
  be absent from the first progressive snapshot. Carry X's preferred revision and
  commit id until it actually appears; do not fall back to `@` or the first row and
  discard the pending selection after the first snapshot. When X first appears, select
  it and issue one DAG reveal so it is scrolled into view. Later snapshots must not
  move selection or scroll away from X.
- **Paging under focus:** `defaultRevsetDepth(for:)` / `canLoadMore` / `loadMore`
  pattern-match the default-revset string; a composed focus revset will not match, so
  infinite-scroll paging disables while focused. This is an explicit focus-mode policy;
  note it rather than working around the string match. The graph session's independent
  row-ceiling pause/continue behavior remains available for large focused results.

### SwiftUI — chevron becomes a tap target

No `Canvas` hit-testing exists in this codebase; spatial hits are SwiftUI-measured
frames compared in a named coordinate space, and small per-region gestures are
attached to real views (the bookmark/@ chips in `DAGRow+Refs.swift` are the
precedent). Follow that pattern rather than reading Canvas coordinates.

- In `graphColumn`'s `GeometryReader` (`DAGRow+GraphColumn.swift`), which already has
  `geo.size.width`, extend the existing `.overlay { nodeLayer(...) }` with a plain-style
  `Button` positioned at `(haloCenterX, dagNodeCenterY)`. The button's transparent hit
  region may be larger than the 16-point rendered disc for reliable pointer and
  accessibility interaction, but must not overlap the row's visible node. Keep the
  badge drawing in the Canvas. `haloCenterX = width - dagOverflowMarkerInset -
  dagOverflowMarkerHaloRadius` (already computed for the badge).
- Gate the target on the same condition that draws the badge:
  `viewModel.isGraphClipped` and the `dagNodeLayerContent(...).includesOverflow`
  check, so the tap region exists exactly where the badge is drawn.
- The tap bubbles through a new closure on `DAGRow` → `DAGView` → a new method on the
  `DAGActions` protocol (`shell/mac/Sources/JayJay/Shared/DAGActions.swift`,
  implemented by `RepoViewModel`), paralleling how `selectEntry` and the bookmark-drag
  callbacks dispatch. The action carries `entry.change.selectionRevision` and calls
  `focus(on:)`.
- Keep it distinct from row selection: activating the button must not also begin the
  row's rebase gesture or dispatch the normal row selection action.

**Discoverability — make the badge read as a control.** The badge is currently a
static marker; it must look pressable:

- Pointer cursor on hover (`.onHover` → balanced `NSCursor.pointingHand.push()` /
  `NSCursor.pop()`, following existing controls).
- Keep badge hover state local to `DAGRow`; unlike a rebase target it does not coordinate
  across rows and does not belong in `DAGViewModel`. Pass the local state into
  `nodeLayer`: fill the disc with `markerColor.opacity(~0.15)`, brighten the ring from
  `0.35` to full, and grow the disc a point or two. The resting state stays quiet; the
  hover state advertises the action.
- A tooltip via `.help("Focus on this change's history")` so the meaning is
  discoverable without a click.
- Give the button an accessibility label ("Focus on <revision>") and identifier, so the
  affordance is reachable and announced under VoiceOver, not just visible.
- Provide a keyboard path to focus the selected change (a Repository menu command and
  shortcut, or an equivalent DAG command). Focus must remain reachable when no row is
  clipped. It reuses `focus(on:)` with the selected row's `selectionRevision` and is
  disabled without a selection or while already focused on that revision.

### SwiftUI — exit affordance

Once focused, the graph usually fits and the chevrons disappear, so exit cannot rely
on the badge. Provide:

- A dismissable focus pill near the revset/filter area (a chip reading e.g.
  `Focused: <short revision> ✕`), whose clear button calls `clearFocus()`. Reuse the
  revset-chip styling in `RepoContentView+Sidebar.swift`, but render the focus indicator
  outside the `showRevsetFilter` conditional. The filter is closed by default, so the
  mouse-accessible exit must remain visible whenever focus is active. Give the clear
  control an accessibility label and identifier.
- `Escape` while the DAG pane is active calls `clearFocus()` when focused. Preserve the
  existing cancellation priority: cancel an active bookmark drag, then an active rebase
  drag, then collapse multi-selection, then clear focus. One Escape performs one action.

### Focus composition rules

- **Always compose from `base`, never from the current effective revset.**
  `focus_revset` takes the user's `revset`, not the focused one, so focusing a second
  change replaces the first rather than intersecting lineages into an ever-narrower
  (and confusing) result. `focusedRevision` is a single value, not a stack.
- **A base-filter change clears focus.** `applyRevset` (preset chips, the filter
  field, `conflicts()`/`divergent()` shortcuts) sets a new base; clear `focusedRevision`
  there so the new filter shows in full rather than silently scoped to a stale change.
- **Selection.** `focus(on:)` pins that exact revision as the pending selection across
  progressive snapshots. Descendants included by `X::` must not steal selection merely
  because they are emitted before X. Once X loads, keep it selected and reveal it once;
  `clearFocus()` keeps X selected so the detail view is stable across the transition.
- **Initially non-empty.** At activation, X is a displayed row, so X ∈ base and X ∈
  `::X | X::`; the intersection contains X. A later repository rewrite can invalidate
  an exact commit-id target. If a focused refresh then fails or no longer contains the
  target, preserve the last successfully rendered graph and the always-visible focus
  pill so the user can clear focus; do not silently replace focus with the full base
  graph. Cover this recovery path explicitly.

### GPUI

Cross-shell parity is a release concern, not part of this change. Focus is entirely
shell view-model state over a shared core helper, so GPUI mirrors it later:
`focus_revset` is already shared; GPUI adds the same `focusedRevision` state, the same
composition at its refresh point, a tap target on its overflow badge, and an exit
affordance. No further core work.

## Testing

- **Rust:** unit-test `focus_revset` beside `revsets.rs` for composition shape and
  parenthesisation. Put repository-backed parsing and evaluation in
  `crates/jayjay-core/tests/integration/revsets.rs`, alongside the existing revset
  fixtures: the focused set is a subset of the base and excludes an unrelated head.
- **Real-CLI parity:** extend `crates/jayjay-core/tests/log_graph_parity.rs` with one
  focused expression built by `focus_revset` against the existing deterministic DAG
  fixture. Compare JayJay's production graph output with the real pinned `jj log`
  output for the identical effective revset, including synthetic nodes, connectivity,
  ordering, and renderer columns. Do not add a parallel layout implementation or a
  second fixture merely for focus.
- **Swift view model:** test that `focus(on:)` records the selection revision, that a
  focused refresh sends the composed effective revset, and that ceiling/paused reset.
  Test that `clearFocus()` sends the unchanged base revset, and that `applyRevset`
  clears focus before sending the new base.
- **Progressive selection:** use a focused target with enough descendants that X is
  absent from the first snapshot. Prove selection remains pending rather than falling
  back, then selects and reveals X exactly once when a later snapshot contains it.
  Appending further snapshots must retain X without another reveal.
- **Identity regression:** construct divergent rows and prove the clicked row supplies
  its commit id rather than the shared change id, so its sibling lineage is excluded.
- **Recovery:** cover a focused refresh whose exact commit target was rewritten or
  removed; the prior graph and visible clear affordance remain available.
- **Replacement transition:** cover focus, clear-focus, and base-filter changes with an
  existing graph. Assert that it becomes stale and non-interactive immediately, then
  returns to normal opacity and interaction when the first replacement snapshot is
  accepted. A failed or invalid focused replacement keeps the prior graph stale.
- **Swift interaction:** add one XCUITest scene only if the button→focus→narrower-graph
  workflow cannot be proven at the view-model/geometry layer. Prefer focused component
  assertions: the button exists only when the badge is drawn, its hit region matches
  the badge without overlapping the node, it does not dispatch the row gesture, the
  focus pill remains present while the revset editor is hidden, and Escape follows the
  stated priority. Do not use a pixel assertion.
- Do not add a pixel-golden test. The existing parity fixture and gold-standard path
  stay unchanged; only add the focused-revset oracle case.

## File areas

- `crates/jayjay-core/src/repo/revsets.rs` — `focus_revset` + unit tests.
- `crates/jayjay-core/tests/log_graph_parity.rs` — focused-revset real-CLI parity case.
- `crates/jayjay-uniffi/src/repo.rs` — export the helper; `just ffi` after Rust is
  stable.
- `shell/mac/Sources/JayJay/Repo/ViewModel/Core/RepoViewModel.swift` — `focusedRevision`.
- `shell/mac/Sources/JayJay/Repo/ViewModel/Core/RepoViewModel+Refresh.swift` —
  effective-revset composition, `focus(on:)`, `clearFocus()`.
- `shell/mac/Sources/JayJay/Shared/DAGActions.swift` — new focus action.
- `shell/mac/Sources/JayJay/Repo/DAG/DAGRow+GraphColumn.swift`,
  `DAGRow.swift`, `DAGView.swift` — badge tap target and closure wiring.
- `shell/mac/Sources/JayJay/Repo/ContentView/RepoContentView+Sidebar.swift` — focus pill.
- `shell/mac/Sources/JayJay/App/Window/RepositoryCommands.swift`,
  `shell/mac/Sources/JayJay/Repo/RepoMenuHandler.swift` — keyboard focus command if the
  Repository menu owns the selected-change action.
- `shell/mac/Sources/JayJay/Shared/AccessibilityIdentifiers.swift` — focus and clear
  identifiers when interaction coverage needs them.
- Focused tests under `shell/mac/Tests/JayJayTests/` and
  `crates/jayjay-core/src/repo/` plus `crates/jayjay-core/tests/integration/revsets.rs`.

## Non-goals

- No lane-collapsing algorithm or width projection; focus is revset scoping only.
- No change to the shared Rust `DagLayout` topology or the parity oracle/algorithm;
  focus only adds another revset case to the existing gate.
- No count of omitted revisions.
- GPUI implementation (release-time parity follow-up).
- Persisting focus across sessions.

## Verification

- `just test-rust jayjay-core revsets` (or the focus filter) — helper unit tests.
- `just test-rust jayjay-core log_graph_parity` — focused and existing real-CLI
  topology parity gates.
- `just ffi` after the Rust type is stable; compile the generated Swift.
- `just test-app JayJayTests/<focus tests>` — view-model transitions.
- `just build` then manual pass against `~/Documents/dev/junk/rust` (read-only,
  `--ignore-working-copy` for any jj): open a wide `jayjay()` graph, click a clipped
  row's chevron, confirm the graph narrows to that revision's lineage and unrelated
  lanes drop; while focusing, clearing focus, and removing a base filter, confirm the
  prior DAG remains visible at 45% opacity and cannot be selected, dragged, or otherwise
  operated until replacement data arrives; choose a row with enough descendants to push
  X below the initial result batch and confirm X becomes selected and is revealed when
  it loads; confirm the focus pill stays visible with the filter editor closed and that
  `Escape`/✕ restore the full graph; check the focused graph against
  `jj log -r '(<base>) & (::X | X::)'` for that revision. Repeat with one divergent
  row and confirm only the clicked commit version is targeted.

## Known limitations

- Focusing a mainline change (`::trunk`-adjacent) still yields a large lineage; focus
  helps side branches most.
- Focus disables infinite-scroll paging (intended).
