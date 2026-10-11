# Plan: Diff Transport Across the UniFFI Boundary

Baseline revision: `b3f0be45` (`feat: use icons for file status badges`).

## Problem

UniFFI serializes every record into a `RustBuffer`, copies it, and decodes it field by field on the other side. Each `String` costs a UTF-8 check and a heap allocation. `DiffLine` holds `Vec<DiffSpan>` and each span owns a `String`, so a 6,000-line diff is tens of thousands of allocations per crossing.

The SwiftUI diff path crosses the boundary far more often than it needs to. Opening one file in the unified view:

| # | Call | Direction | Payload |
| --- | --- | --- | --- |
| 1 | `show_file` / `fetchContent` | Rust → Swift | full old + new text in `DiffHunk` |
| 2 | `computeNativeDiff` (`DiffStore.swift:122`) | Swift → Rust | full old + new text |
| 3 | | Rust → Swift | `FileDiff` (all lines, all spans) |
| 4 | `diffDisplayLines` (`DiffSection+Loading.swift:155`) | Swift → Rust → Swift | all lines, twice |
| 5 | `changeGroups` (`DiffSection+Loading.swift:160`) | Swift → Rust | all display lines |
| 6 | `reviewSnapshotFromDiffHunk` (`DiffLoadedContent.swift:90`) | Swift → Rust | full old + new text |
| 7 | `reviewDisplayGroupMapFromDiffHunk` (`DiffLoadedContent.swift:99`) | Swift → Rust | full old + new text; Rust diffs again |

Side-by-side adds, on the main thread, for every width or font change (`SideBySideDiffCoordinator.swift:126-127`):

| # | Call | Payload |
| --- | --- | --- |
| 8 | `buildSideBySideRows` | all lines in, all rows out |
| 9 | `wrapSbsRows` | all rows in, one full `SideBySideRow` per visual row out; Swift keeps `.row` and drops the geometry |

Context expansion (`DiffSection+ContextExpansion.swift:21`) sends the whole `FileDiff` plus both texts back to Rust, receives the expanded `FileDiff`, then repeats steps 4–5.

Memory: `DiffCache` keeps a 64 MiB budget (`DiffStore.swift:198`), but `bytes()` counts only the two texts (`DiffStore.swift:242`). The `FileDiff`, display lines and groups that sit beside them are uncounted, so the real footprint is a multiple of the budget.

## Goal

Each diff payload crosses the boundary once, in the direction Rust → Swift, in the shape Swift renders. Rust owns the texts, the diff and every derived view. Swift holds a handle and the data it draws.

Out of scope:

- GPUI. It links the crates directly and pays no marshalling cost.
- `log`, `log_graph`, bookmarks, op log, mutations. Their payloads are small or rare.
- Moving blocking FFI calls off Swift's cooperative pool. `DiffStore.preload` is sequential, so the pool is not starved today. Revisit if a later change adds parallel loads.

## Work Rules

- Work in the main checkout; no sibling workspace.
- One phase per jj change, each with `jj describe`. Phase 0 lands alone so the baseline is reproducible from its own revision.
- TDD: each phase starts with failing tests. Expected values come from hand-built inputs, not from code output.
- Fingerprints and review identity are a persisted contract. No phase may change a fingerprint for the same input. The existing `jayjay-review` fingerprint tests must pass unchanged.

## Phase 0: Benchmarks and Baseline

Status: implemented on top of `b3f0be45`. No behavior change beyond two seams with production callers: `SideBySideRowSource.visualRows` (the side-by-side coordinator's data step) and `DiffSection.prepareLoadedDiff` (now internal).

### Fixtures

`crates/jj-test/src/ffi_bench.rs` generates the content; the `ffi-bench-fixture` binary builds a jj repo from it: `cargo run -p jj-test --bin ffi-bench-fixture -- /tmp/jayjay-ffi-bench`. One base change holds every old file; each scenario is a child of it, reachable as bookmark `bench-<name>`.

| Id | Content | Purpose |
| --- | --- | --- |
| `sparse` | 6,000-line Rust file (line rule from `crates/jj-diff/benches/profile_diff.rs`), one line changed | collapsed diff, large context |
| `dense` | same file, every third line rewritten | many spans, many groups |
| `wide` | 300 lines of 400–2,000 chars mixing ASCII, CJK, accents and an emoji ZWJ family; every tenth line changed | wrapping, UTF-16 vs grapheme offsets |
| `many` | 40 files × 500 lines, every tenth line changed | cache and retained memory |

`wide` stays under jj's 1 MiB `snapshot.max-new-file-size` default; a larger file is left untracked.

### Rust replay: bytes, allocations, Rust time

`crates/jayjay-uniffi/benches/ffi_wire.rs` replays the exact Swift call sequence for each workload against the real `Repo` and core functions. Every argument and return value goes through UniFFI's `Lower`/`Lift`, so the byte counts are the serialized `RustBuffer` sizes both sides copy. A counting global allocator records Rust-side allocations during lifting, compute and lowering; Swift-side lowering and lifting are outside the count.

```sh
just bench-wire                 # all workloads, ~14 s after the first build
just bench-wire open dense --calls
```

Timed runs per scenario stop after 1 s, between 2 and 10 runs, after one warm-up. The side-by-side sweep is 20 width changes (40→85→40 cols in steps of 5). `rust ms` is all Rust-side time; `marshal ms` is the part of it spent lifting arguments and lowering returns. Swift's share of marshalling is not in this bench; the Swift benches below include it.

### Swift end-to-end: time and retained memory

`shell/mac/Tests/JayJayBenchmarks/DiffFFIBenchmarks.swift`, target `JayJayBenchmarks`, scheme `JayJayBenchmarks`, build configuration `Bench` (Release optimization, `ENABLE_TESTABILITY`, arm64 only). Swift packages build with their Release settings under `Bench`. It links the release staticlib from `just shell::ffi-release`.

```sh
just bench-ffi                                                  # fixture, ffi-release, build, run, print table
just bench-ffi JayJayBenchmarks/DiffFFIBenchmarks/testPreload   # one benchmark
```

The first run builds the release staticlib and the app (several minutes); later runs rebuild only what changed. The test run itself takes about 10 s.

| Test | Production path | Scenario |
| --- | --- | --- |
| `testOpenUnified{Sparse,Dense}` | `DiffStore.loadDiff` + `DiffSection.prepareLoadedDiff` (incl. review fingerprints) | `sparse`, `dense` |
| `testResizeSideBySide{Dense,Wide}` | `SideBySideRowSource.visualRows` over 8 width changes (40→85→40 cols in steps of 15), reported per width change | `dense`, `wide` |
| `testExpandContext` | the FFI sequence of `expandContext(.allRegions)`: begin, run with full source, finish, `diffDisplayLines`, `changeGroups` | `sparse` |
| `testPreload` | `DiffStore.loadDiff` over every file, sequential, as `DiffStore.preload` | `many` |

Same timing rule as the Rust replay. Retained memory is `malloc_zone_statistics` `size_in_use` / `blocks_in_use` and `phys_footprint`, read on the last run while its result is alive, minus a sample taken just before that run. Results go to `/tmp/jayjay-ffi-bench-results.tsv`. The host app is running, so small deltas are noise; `phys_footprint` deltas are too coarse to use.

### Baseline (`b3f0be45`, Apple Silicon, 2026-10-11)

Rust replay, `just bench-wire --calls`:

| Workload | Scenario | Calls | MiB → Rust | MiB → Swift | Rust allocs | Rust ms | of which marshal ms |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| open | sparse | 6 | 3.32 | 1.11 | 73,441 | 4.1 | 0.1 |
| open | dense | 6 | 6.90 | 5.55 | 1,127,211 | 1,932.7 | 6.4 |
| open | wide | 6 | 5.29 | 2.70 | 12,863 | 12.9 | 2.8 |
| open | many | 240 | 10.36 | 8.35 | 1,696,910 | 322.6 | 10.6 |
| sbs (20 widths) | dense | 40 | 97.47 | 131.67 | 16,056,088 | 290.5 | 145.2 |
| sbs (20 widths) | wide | 40 | 30.38 | 46.09 | 1,250,622 | 203.6 | 21.5 |
| expand | sparse | 5 | 4.13 | 3.02 | 420,150 | 6.4 | 3.3 |
| preload | many | 80 | 1.20 | 4.58 | 515,760 | 177.2 | 2.7 |

Breakdown of `open dense` (Rust ms): `computeNativeDiff` 467, `reviewDisplayGroupMapFromDiffHunk` 1,012, `reviewSnapshotFromDiffHunk` 449, the rest under 4.

Swift end-to-end, `just bench-ffi`. On `b3f0be45` both `dense` tests crashed (finding 1). This table is measured on the benchmark change rebased onto the stack fix (`a372b31f`, bookmark `fix-diff-stack-overflow`), which changes no diff output or marshalling; the other rows match the `b3f0be45` run within 2 %. A full run takes about 21 s including app launch.

| Benchmark | Scenario | Median ms | Runs | Retained MiB | Retained blocks |
| --- | --- | ---: | ---: | ---: | ---: |
| open_unified | sparse | 4.8 | 10 | 1.13 | 37 |
| open_unified | dense | 1,796.0 | 2 | 10.15 | 42,853 |
| resize_sbs, per width change | dense | 267.2 | 2 | 0.00 | 29 |
| resize_sbs, per width change | wide | 32.6 | 4 | 0.02 | 492 |
| expand_context | sparse | 122.4 | 9 | 2.78 | 11,799 |
| preload | many | 1,193.0 | 2 | 8.43 | 18,662 |

### Findings

1. **Opening `dense` crashed the app on `b3f0be45`.** `computeNativeDiff` runs in `Task.detached` on a Swift cooperative-pool thread with a 544 KiB stack. `similar`'s histogram diff recurses once per matched block and overflowed it 755 levels deep (`SIGBUS`, stack pointer on the guard page; reports `JayJay-2026-10-11-104146.ips`, `-104152.ips`). Fixed separately in `fix(diff): grow the stack for histogram diffs with many scattered changes` (bookmark `fix-diff-stack-overflow`), which covers the line and word diffs. `similar` 3.2.0 and its current `main` still recurse this way.
2. **For `dense`, Rust compute dominates, not marshalling.** Review fingerprints recompute the diff twice (1.46 s of 1.93 s); marshalling is 6 ms on the Rust side. Phase 1's "compute once" is worth more here than removing copies.
3. **For collapsed and multi-file paths, Swift-side marshalling dominates.** `expand_context sparse`: 123 ms in Swift vs 6.4 ms in Rust. `preload many`: 1,214 ms vs 177 ms. `resize_sbs`, per width change on the main thread: `wide` 33 ms vs 10 ms in Rust, `dense` 267 ms vs 15 ms. A quarter-second stall per resize step on a large diff is the strongest case for Phase 2.
4. **Retained memory.** 40 cached files of 1.2 MiB text retain 8.1 MiB: about 6.7× the text. `DiffCache` budgets only the text.

## Status and Open Items (2026-10-11)

History: `main` (`b3f0be45`) → `ouvvunot` stack fix, pushed as bookmark `fix-diff-stack-overflow`, no PR → `rptqmryu` this benchmark change, local only.

Conclusion from the baseline: the FFI work is still worth doing.
- Phase 2 has the strongest case: 267 ms main-thread stall per width change on `dense`, about 15 ms of it Rust.
- Phase 1 removes most Swift-side marshalling for `preload` (1,193 ms Swift vs 177 ms Rust) and `expand_context` (122 ms vs 6 ms). For `open dense` the cost is Rust compute, mainly review fingerprints re-diffing twice (about 1.5 s of 1.8 s); compute once.
- Phase 3 stays conditional.

Changes on top of `rptqmryu`: `wuzrtlqo` Phase 1, `yoqrxrpy` Phase 2, `pnxnkrpt` Phase 2 follow-up, `tryromnz` wasm warning fix for the stack helper, `otntqvvo` eviction check, `tvuwoklx` this write-up, `mrplptsn` review-state budget fix, `umpplnzw` Abandon Selected Lines regression test. Phase 3 is closed (see Results). Pushed as bookmark `ffi-diff-transport`, no PR.

Checks (2026-10-11), run once at the tip of the stack; no commit was checked on its own:
- `jj fix -s rptqmryu`: reformatted `wuzrtlqo`, `yoqrxrpy`, `pnxnkrpt`.
- `just lint`: clippy clean. SwiftLint rejected Phase 1's six-parameter `DiffStore.loadFromRepo`; it now takes a `DiffLoadRequest` from `DiffStore.loadRequest`, folded into `wuzrtlqo`.
- `just test-wasm`: builds without warnings. `histogram_stack.rs` from the pushed stack fix warned four times on wasm; `tryromnz` gates the native-only constants.
- `cargo test --workspace --no-fail-fast`: 1,446 passed, 2 failed. Both failures drive the external `git` binary and fail without these changes (listed below).
- `just test-app`: JayJayTests 300 of 300, JayJayDiffUI 108 of 108.
- UI scenes, 17 tests in BookmarkDiff, ContextExpansion, DiffEdit, DiffStatsReload, ExternalDiffTool, FileDiff, FileEditor, FileRename, HideReviewedFiles, ImageDiffResize, Interdiff, MergeEditorNavigation, ReviewHunkBaseline, ReviewNotes, ReviewSplit: 15 passed. `ImageDiffResizeScene/testResizeImagesAndRestoreEqualWidths` (pane 1027 px, expected 1067 ± 4) and `ReviewNotesScene/testAddAndResolveReviewNoteFromGutterMarker` (note context has one extra line) fail the same way on `rptqmryu`, so they are not regressions.
- Handle lifetime: `DiffFFIBenchmarks/testEvictionReleasesLoadedDiffs` loads the 40 `many` files into a `DiffCache`. After a warm-up pass, loading retains 9.52 MiB; clearing leaves 0.18 MiB, eviction down to one entry 0.45 MiB. The test asserts both stay under 10 % of the loaded bytes, and fails when `clear()` keeps its entries. With a one-file warm-up, 9 MiB of jj object caches stayed resident and hid the result.
- Independent review of `f1a32df6..7ddd893e` (fresh agent, code reading only): no Critical, High or Medium findings.
- Review triage: see the `jayjay review status` result reported with this write-up.

Review finding, fixed in `mrplptsn`:
- Low: `LoadedDiff::retained_bytes` and the `DiffCache` budget left out the review state, one 64-char fingerprint and one group-map vector per change group, held in Rust and again in Swift's `CachedDiff.review`. On a group-dense file the cache could run 30–50 % over budget (traced, not measured). `ReviewDisplayParts::retained_bytes` now counts the Rust copy and `CachedDiff.retainedBytes` the Swift copy; tests in `loaded_diff/tests.rs` and `DiffCacheTests` failed before the fix.

Verification gaps from the review:
- Abandon Selected Lines maps the selection through display rows. The base indexed raw rows with display-row numbers and could abandon unselected lines when a raw run interleaves removed and added lines. Phase 1 fixes this; `AbandonSelectedLinesTests` (child of `mrplptsn`) covers it and fails when the keys come from the raw diff's order. No UI scene drives the action.
- `Repo::load_diff` has no test for the interdiff branch, the file-text fallback, or the nil-revision and error fallbacks in `DiffStore.loadFromRepo`.
- No test covers `DiffSection` installing the expanded `LoadedDiff` and recomputing its flags.
- `slicing_rows_by_the_layout_reproduces_wrapped_rows` uses single-span rows only; highlighted multi-span rows and CRLF are not covered.
- The eviction check lives in the benchmark target, outside `just test-app`.

Known gaps:
- Phase 1 retained memory rose 8 % against a 30 % reduction target; accepted 2026-10-11.
- Optional: packed display rows (84 ms of Swift lifting on `dense`, see Phase 3), rows cached on the handle for side-by-side (about 6 ms per width on `dense`), async side-by-side layout.

Open items:
- Stack fix: files with more than about 45,000 separate change blocks can still overflow the 32 MiB cap. The word diff may allocate a grown stack per long changed line; growing once per file avoids that. Not measured.
- Real fix belongs upstream: `similar`'s `diff_impl` (`src/algorithms/histogram.rs`) recurses on the right region; turning that into a loop or explicit work list keeps output identical. A patch exists on the fork branch https://github.com/lookbusy1344/similar/tree/fix-histogram-stack (`22bccba2de7f`, `fix(histogram): replace recursion with an explicit work list`); no upstream PR yet. Once released, `with_histogram_stack` and the `stacker` dependency can go. Upstream `ce3a403` (after 3.2.0, unreleased) reuses anchor-search buffers and should speed up `dense`.
- `jj-diff/src/lib.rs` says histogram "matches `jj diff`"; jj uses its own algorithm in `jj-core` (`src/diff.rs`, LCS over uncommon words, loops over gaps), so output can differ.
- Swift run takes about 21 s; dropping the warm-up for `open dense` and `preload` would bring it near 15 s.
- `just lint` does not lint Swift test targets.
- Two tests fail locally where they drive the external `git` binary, unrelated to these changes and not checked on `main`: `jayjay-core` `repo::init::tests::preserves_git_worktree_init_error`, GPUI `repo_workspace_switch::removing_a_workspace_drops_drafts_parked_in_other_windows`.

## Phase 1: Rust-Owned Loaded Diff

### Core type

`jayjay-core/src/repo/diff/loaded.rs`, `pub struct LoadedDiff`:

- Fields: `path`, `old: Arc<str>`, `new: Arc<str>`, `file_diff: FileDiff`, `display_lines: Vec<DiffLine>`, `groups: Vec<ChangeGroup>`, `review: Option<ReviewDisplayParts>` (snapshot + group map), `projection`, `previews`.
- `Repo::load_diff(&self, request: &DiffLoadRequest) -> Result<LoadedDiff, JayError>`. `DiffLoadRequest` carries `rev`, `from_rev`, `path`, `old_path`, `hunk_type`, `projection_mode`, `ignore_whitespace`. It reuses the blob reads behind `show_file*` / `interdiff_file*` and runs the existing `compute_native_diff`, `build_diff_display_lines`, `change_groups`, `review_snapshot_from_hunk` and `review_display_group_map_from_hunk` once each.
- `LoadedDiff::from_texts(path, old, new, ignore_whitespace)` for repo-free callers. Production callers: the external diff tool (`load_external_diff`) and the WASM consumer.
- Review snapshot and group map each re-diff the texts today (finding 2). Check whether `display_group_canonical_indices` can reuse the display diff; if it needs the canonical diff, compute that once and share it between snapshot and map. Fingerprints must not change (test 5).
- `retained_bytes(&self) -> u64`: text bytes plus line/span string bytes plus fixed per-line overhead.

The core type holds no FFI concerns, so GPUI can adopt it later without changes.

### FFI object

`jayjay-uniffi/src/loaded_diff.rs`: `#[derive(uniffi::Object)] pub struct LoadedDiff(core::LoadedDiff)` with thin methods:

```rust
#[uniffi::export]
impl JayJayRepo {
    fn load_diff(&self, request: DiffLoadRequest) -> Result<Arc<LoadedDiff>, JayError>;
}

#[uniffi::export]
impl LoadedDiff {
    fn file_diff_meta(&self) -> FileDiffMeta;          // path, language, whitespace_only_hidden, has_context_regions
    fn display_lines(&self) -> Vec<DiffLine>;
    fn groups(&self) -> Vec<ChangeGroup>;
    fn review_display(&self) -> Option<ReviewDisplayParts>;
    fn retained_bytes(&self) -> u64;
    fn old_text(&self) -> String;                      // file editor, merge editor only
    fn new_text(&self) -> String;
}
```

The raw `FileDiff.lines` do not cross. Swift reads `fileDiff.lines` today only for `contextRegion` presence and SBS input. `has_context_regions` and Phase 2 cover both. Before deleting a Swift use, `rg` every `fileDiff` access and list it in the change description.

### Context expansion

`ContextExpansionAttempt::run` takes `Option<Arc<LoadedDiff>>` in place of `ContextExpansionSource`. Rust reads the texts and diff from the handle. `ContextExpansionFinish::Applied` returns a new `Arc<LoadedDiff>` that carries its own display lines and groups. Steps 4–5 disappear from `PreparedContextExpansion`. The generation and basis supersession rules stay as they are.

### Swift changes

- `DiffStore.CachedDiff` holds `loaded: LoadedDiff`, `displayLines`, `groups`, `reviewDisplay`. Texts are not copied into Swift unless an editor asks.
- `DiffCache.bytes` returns `loaded.retainedBytes()` plus `displayLines` string bytes. The budget then reflects both copies.
- `DiffSection+Loading`, `DiffLoadedContent.withReviewFingerprints`, `DiffSection+ContextExpansion` read from the cached value.

### Removal

After Swift migrates, delete exports with no remaining Swift or WASM caller: candidates `compute_native_diff*` on `JayJayRepo`, `diff_display_lines`, `change_groups`, `review_snapshot_from_diff_hunk`, `review_display_group_map_from_diff_hunk`, `ContextExpansionSource`. Confirm each with `rg` across `shell/mac` and the WASM consumer first. Keep the standalone `compute_file_diff*` exports; WASM uses them.

### Tests (written first)

Rust, `jayjay-core/tests/integration/loaded_diff.rs`:

1. Three-line file, middle line changed. Hand-written expected display lines (line numbers, styles, texts) and one group spanning display lines 2–3.
2. Rename with edits: `old_path` resolves; display shows the rename source text.
3. `ignore_whitespace`: whitespace-only change gives an empty group list and `whitespace_only_hidden = true`.
4. Projection (notebook) and binary preview: `review_display` is `None`, matching `review_text_pair`'s exclusions.
5. Fingerprint stability: for the inputs in the existing `jayjay-review` fingerprint tests, `review_display` fingerprints equal those tests' literals.
6. Differential: across the four bench fixtures, `display_lines` and `groups` equal the current free functions applied to the same texts. Two production paths compared; the free functions stay in core for GPUI, so the check stays valid after the FFI exports go.
7. `retained_bytes` for a hand-built two-line diff equals the hand-computed sum.
8. Context expansion via handle: expanding all regions of a 30-line single-change file reveals every line; a stale generation returns `Discarded`.

Swift, `JayJayTests`:

9. `DiffCache` evicts by `retainedBytes`: two entries whose texts fit the budget but whose retained bytes do not; the older is evicted.

### Exit criteria

Phase 0 benchmarks rerun on the Phase 1 revision. Expected: wire bytes for `open` drop to about one `FileDiff`'s worth (rows 1–7 collapse to one return); Rust time for `open dense` drops by the two review recomputations (about 1.4 s) once the review map reuses the computed diff. Record the tables. Proceed to Phase 2 if `preload many` and `expand_context sparse` Swift medians improve by at least 30 % and `preload many` retained bytes drop by at least 30 %. These thresholds follow from finding 3; revise them if finding 1's fix changes the threading model.

## Phase 2: Side-by-Side Layout in Rust

### Design

`LoadedDiff` caches side-by-side rows, built once:

```rust
fn sbs_rows(&self) -> Vec<SideBySideRow>;                       // fetched once per loaded diff
fn sbs_layout(&self, old_cols: u32, new_cols: u32) -> SbsLayout; // fetched per width change
```

`SbsLayout { breaks: Vec<SbsRowBreaks> }`. `SbsRowBreaks { row_ix: u32, old: Vec<u32>, new: Vec<u32> }` gives UTF-16 offsets into each side's line text where a visual line ends. Rust computes them with the existing grapheme-cell logic in `jj-diff/src/wrap/chunks.rs`, then converts to UTF-16. Swift slices `NSString` ranges and does no grapheme or cell work.

Core: add `wrap_sbs_breaks(rows, old_cols, new_cols) -> Vec<SbsRowBreaks>` in `jj-diff/src/wrap/side_by_side.rs` beside `wrap_sbs_rows`. `wrap_sbs_rows` stays for GPUI. Both read from one chunking function so their geometry cannot diverge.

Swift: `SideBySideRowSource` holds the rows once. On width change it calls `sbs_layout` in a detached task and applies the result guarded by a generation token. It keeps the current layout on screen until the new one arrives. `RenderInputs` stops holding `[DiffLine]` and compares a diff identity plus cols, which also removes a deep array comparison from every `updateNSView`.

### Tests (written first)

Rust, `jj-diff/src/wrap/tests/`:

1. ASCII, 10 chars, cols 4 → breaks at 4, 8.
2. CJK, 5 wide chars (2 cells each), cols 4 → breaks at UTF-16 offsets 2, 4.
3. Emoji ZWJ family (one grapheme, 11 UTF-16 units, 2 cells) at a boundary: never split inside the cluster.
4. Combining mark after a base char at the boundary: kept with its base.
5. Surrogate pair (`𝒳`, 2 UTF-16 units, 1 cell): offsets land on pair boundaries.
6. `cols = 0` behaves as `cols = 1`, matching `wrap_sbs_rows`.
7. Empty line, separator row, full-width conflict row: one visual line, no breaks.
8. Uneven sides: old wraps to 3, new to 1; both report the row with their own breaks.
9. Consistency: for the `wide` fixture, the visual row count from `wrap_sbs_breaks` equals `wrap_sbs_rows(...).len()` at cols 40, 63, 89.

Swift, `JayJayDiffUI` package tests:

10. Applying breaks to a line containing emoji and CJK yields the substrings from test 2/3, compared to literal strings.

### Exit criteria

`resize_sbs` rerun. A width change sends only `SbsLayout` across, mostly integers. Record the table.

## Phase 3: Flat Line Encoding (Conditional)

Run only if the Phase 1 Instruments trace or the post-Phase-2 `blocks_in_use` shows span `String` decoding among the top allocation sites. Otherwise record the evidence and close the plan.

### Design

FFI-only record, built in `jayjay-uniffi` from `DiffLine` (conversion, no logic):

```rust
#[derive(uniffi::Record)]
pub struct DisplayLine {
    pub old_line_no: Option<u32>,
    pub new_line_no: Option<u32>,
    pub style: DiffSpanStyle,
    pub text: String,              // one allocation per line
    pub runs: Vec<SpanRun>,        // run-length styling
    pub conflict_kind: ConflictLineKind,
    pub no_eof_newline: bool,
    pub context_region: Option<ContextRegion>,
}

#[derive(uniffi::Record)]
pub struct SpanRun { pub utf16_len: u32, pub style: DiffSpanStyle, pub token: SyntaxToken }
```

`LoadedDiff::display_lines`, `sbs_rows` and `highlight_file_lines` return this shape. Swift builds one `NSMutableAttributedString` per line from `text` and applies attributes by `NSRange` from the runs. The separator label, note text (`DiffSection+NoteActions.swift:105`) and merge-editor highlights read `text`. Other FFI APIs that carry `DiffLine` (`DiffEditFileDiff`, conflict blocks) stay unchanged until measured.

### Tests (written first)

1. Rust: a line with spans `["fn ", "名前", "👨‍👩‍👧"]` encodes to `text = "fn 名前👨‍👩‍👧"` and runs with `utf16_len` 3, 2, 8.
2. Rust: empty-span line encodes to empty text and no runs.
3. Swift: rendering test 1's line produces attribute ranges `{0,3} {3,2} {5,8}`.
4. Swift: separator and no-EOF-newline lines render the same strings as before. Expected values are literals taken from the current rendering code's constants, not from running it.

### Exit criteria

Rerun all benchmarks. Keep the phase only if `blocks_in_use` for `open_unified(dense)` drops by at least 30 %; otherwise revert it.

## Validation Per Phase

| Claim | Command |
| --- | --- |
| Generation | `just ffi` |
| Rust behavior | `just test-rust jayjay-core loaded_diff`, `just test-rust jj-diff wrap` |
| Swift behavior | `just test-app` filtered to the new tests; `cd shell/mac/Packages/JayJayDiffUI && swift test` |
| UI workflows | `just test-ui` for the diff scenes: unified, side-by-side, context expansion, review marks, merge editor |
| WASM build | `just test-wasm` (exports changed) |
| Measurements | `just bench-wire --calls`, `just bench-ffi` |

Before declaring a phase done: two cleanup rounds on the whole diff, `jj --ignore-working-copy log -r 'divergent()'`, `jj describe`. Before pushing: `jj fix`, `just lint`, and an independent review per `agents/code-review.md`.

## Results

### Phase 1 (2026-10-11, on top of `rptqmryu`)

Design changes from the plan, driven by measurement:
- `LoadedDiff` retains texts, the review parts and the diff packed into three buffers (`loaded_diff/packed.rs`). Display rows are rebuilt per request. Storing `FileDiff` plus display rows in Rust retained 36 MiB for `preload many` (one heap block per span).
- The Swift cache holds the handle, texts, review parts and flags. Display rows cross when a section shows the diff (`DisplayRows`, one call).
- Review snapshot and group map share one canonical diff and reuse the display rows; the canonical diff is still computed once more (exact whitespace, uncollapsed).
- Side-by-side reads `sideBySideRows()` once per handle; per width only `wrapSbsRows` crosses. Phase 2 removes that.
- Abandon Selected Lines maps the selection through display rows; it indexed raw rows, which order removed/added lines differently.

Rust replay, `just bench-wire`:

| Workload | Scenario | Calls | MiB → Rust | MiB → Swift | Rust allocs | Rust ms |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| open | sparse | 5 | 0.00 | 1.11 | 37,128 | 2.7 |
| open | dense | 5 | 0.00 | 3.61 | 807,710 | 1,033.9 |
| open | wide | 5 | 0.00 | 2.15 | 7,491 | 6.5 |
| open | many | 200 | 0.00 | 4.97 | 1,354,590 | 238.2 |
| sbs (20 widths) | dense | 21 | 58.58 | 76.02 | 8,504,692 | 178.5 |
| sbs (20 widths) | wide | 21 | 19.35 | 27.71 | 1,197,766 | 199.5 |
| expand | sparse | 6 | 0.00 | 1.51 | 342,123 | 5.6 |
| preload | many | 160 | 0.00 | 1.35 | 867,040 | 228.9 |

Swift end-to-end, `just bench-ffi`:

| Benchmark | Scenario | Median ms | Runs | Retained MiB | Retained blocks |
| --- | --- | ---: | ---: | ---: | ---: |
| open_unified | sparse | 3.1 | 10 | 2.26 | 38 |
| open_unified | dense | 956.6 | 2 | 10.35 | 30,602 |
| resize_sbs, per width change | dense | 170.4 | 2 | -0.17 | -3,930 |
| resize_sbs, per width change | wide | 33.5 | 4 | 0.02 | 68 |
| expand_context | sparse | 62.1 | 10 | 4.89 | 11,937 |
| preload | many | 256.7 | 4 | 9.13 | 5,917 |

Exit criteria: `preload many` −78 % and `expand_context sparse` −49 % in time pass. `preload many` retained bytes rise 8 % (8.43 → 9.13 MiB; blocks 18,662 → 5,917), so the 30 % memory criterion fails: the cache holds texts in both Rust and Swift. `open dense` Rust time is 1.03 s, not the expected ~0.5 s; the remaining cost is the highlighted display diff plus the canonical review diff.

Decision (2026-10-11): accept Phase 1 with the 8 % retained-memory rise and proceed to Phase 2. Fetching texts from the handle on demand (SVG, Markdown and HTML previews, diff edit, Abandon Selected Lines) would remove the Swift copy; it stays open as a follow-up.

Not yet run on Phase 1: UI tests, `just test-wasm`, full Rust and app suites, `jj fix`, `just lint`, review triage. Abandon Selected Lines has no regression test for the display-row mapping.

### Phase 2 (2026-10-11, on top of Phase 1)

Design as built:
- `wrap_sbs_layout` (`jj-diff/src/wrap/layout.rs`) returns one `SbsVisualLine { row_ix, old, new, full_width_source }` per visual line, with UTF-16 ranges into the row text. It maps the cell ranges `wrap_sbs_rows` computes, so the two cannot diverge. Full-width rows name the side both ranges index.
- `LoadedDiff.sideBySideLayout(oldCols:newCols:)` rebuilds rows from the packed diff and wraps them per call. Swift fetches rows once per handle and slices them by the ranges (`SideBySideRowSource.visualRows(rows:layout:)`).
- The layout call stays synchronous on the main thread; async with a generation token is not done.

Rust replay, `just bench-wire sbs` (20 widths): dense 0.00 MiB → Rust, 8.22 MiB → Swift (2.93 of it the rows, once), 505 ms Rust; wide 0.00 / 2.84 MiB, 341 ms.

Swift, `just bench-ffi`, per width change: dense 45.8 ms (3 runs), wide 24.8 ms (6 runs), no retained memory.

Follow-up (child of `yoqrxrpy`), driven by a per-call probe: the 45.8 ms on dense was 25 ms Rust wrapping, 13 ms lifting 12,000 `SbsVisualLine` records into Swift (about 1.1 µs each) and 5 ms slicing.
- `wrap_sbs_layout` walks the same chunker as `wrap_sbs_rows` (`side_extents`, `chunks.rs`), which counts UTF-16 units and keeps no text. It no longer calls `wrap_sbs_rows`.
- The layout lists only rows that wrap or span full width. Any other row is one visual line, and Swift uses the row it holds unchanged.
- `LoadedDiff.packedSideBySideLayout` returns the layout as one byte buffer, six little-endian `u32` per line (`jayjay-uniffi/src/packed_layout.rs`). `SideBySideLayoutLine.decode` (JayJayDiffUI) reads it. Swift copies one buffer and lifts no records.
- The call stays synchronous on the main thread.

Validated: `cargo test -p jj-diff -p jayjay-uniffi`, `cargo test -p jayjay-core --test integration loaded_diff`, `swift test` in JayJayDiffUI (108 tests), UI scene `ReviewNotesScene/testSideBySideBannerCountsOnlyOpenNotes`, `cargo clippy -p jj-diff -p jayjay-uniffi --all-targets`. Not run: `jj fix`, `just lint`, `just test-wasm`, `just test-app`, the other diff UI scenes.

Rust replay, `just bench-wire sbs` (20 widths): dense 6.04 MiB → Swift over 20 calls, 141.5 ms Rust (7.1 ms per width, 6.3 of it unpacking and rebuilding rows); wide 2.12 MiB, 155.2 ms (7.8 ms per width, grapheme work on 400–2,000-char lines).

Swift, `just bench-ffi`, full run:

| Benchmark | Scenario | Median ms | Runs | Retained MiB | Retained blocks |
| --- | --- | ---: | ---: | ---: | ---: |
| open_unified | sparse | 3.0 | 10 | 2.26 | 43 |
| open_unified | dense | 959.5 | 2 | 9.09 | 26,813 |
| resize_sbs, per width change | dense | 14.2 | 9 | 0.00 | 0 |
| resize_sbs, per width change | wide | 10.6 | 10 | 0.00 | 0 |
| expand_context | sparse | 62.1 | 10 | 4.89 | 11,944 |
| preload | many | 255.3 | 4 | 9.38 | 9,822 |

Against the baseline, a width change costs 14.2 ms on dense (267.2 ms) and 10.6 ms on wide (32.6 ms). Every dense row is 85–120 chars and wraps at every bench width, so this is the worst case for leaving out rows that fit.

Remaining options, not done: rows cached on the handle while side-by-side is shown would save about 6 ms per width on dense, at the cost of holding the rows in Rust; moving the call off the main thread would hide the rest.

### Phase 3: closed (2026-10-11)

Its condition is not met. On `open_unified dense`, 6,000 of 100,469 span strings exceed Swift's 15-byte inline limit, so span `String` decoding is not a top allocation site. One text per line plus a run array would hold about as many blocks as the current spans array plus long span strings, so the 30 % `blocks_in_use` criterion cannot be met.

The probe found a time cost instead: `displayRows()` on dense takes about 84 ms in Swift against 4.7 ms in Rust, from lifting about 100,000 span records. Phase 3's `SpanRun` records would keep that cost. A packed buffer, as for the side-by-side layout, would remove it. That is about 9 % of `open_unified dense`, whose 960 ms is Rust compute (highlighted display diff plus the canonical review diff). A new plan item if wanted.

## Risks

- **Context-expansion supersession.** The expanded result becomes a new handle; the basis/generation guard must still reject stale outcomes. Covered by Phase 1 test 8 and the existing UI scene.
- **Review fingerprints.** Any change to the text fed into fingerprinting makes existing marks look stale. Covered by Phase 1 test 5.
- **Async SBS layout.** During a live resize, the old layout stays up for one frame or more. Acceptable per the shell convention to keep valid state until its replacement loads. Watch for visible lag on `wide`.
- **Handle lifetime.** Swift `DiffCache` eviction must release the Rust `LoadedDiff`. Check with the `many` retained-memory metric after eviction: footprint must return within 10 % of the empty baseline.
