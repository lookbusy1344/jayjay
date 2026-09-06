# `jj log` DAG Parity

## Status

Implemented and covered by a deterministic real-CLI oracle.

## Contract

For an identical revset and repository configuration, JayJay follows the pinned `jj log` command for:

- topological row order and branch prioritisation;
- direct, indirect, and missing-edge semantics;
- synthetic elision-node placement;
- native lane allocation and connector topology;
- behavior at a bounded progressive prefix.

The GUI may compose a synthetic elision row into the preceding change row and draw native shapes
instead of terminal glyphs. It may also clip pixels at the sidebar boundary. Those presentation
choices do not change the ordered real/synthetic node sequence or its connectivity.

## Shared pipeline

The repository layer evaluates the selected revset and feeds `stream_graph()` through
`TopoGroupedGraph`, applying `revsets.log-graph-prioritize` before consuming rows. The DAG adapter
passes the resulting node/parent sequence to `sapling-renderdag::GraphRowRenderer`.

When synthetic elisions are enabled, indirect edges are expanded into the same intermediate nodes
used by the CLI. JayJay folds their rendered bands into app-owned row shapes only after lane
allocation.

Progressive snapshots keep edges whose targets are outside the retained prefix. This matches
`jj log --limit N`: the limit bounds displayed real changes, not the topology visible from them.

## Deterministic oracle

`crates/jayjay-core/tests/log_graph_parity.rs` constructs a hermetic repository and invokes both:

1. JayJay's production progressive graph path and debug ASCII projection;
2. the pinned real `jj log` command with the identical revset, limit, synthetic-elision setting,
   prioritisation, and isolated configuration.

The comparison covers:

- synthetic elisions enabled and disabled;
- a displayed descendant of the working copy;
- parents beyond the progressive prefix;
- a focused effective revset.

The fixture pins repository and command configuration so user-global settings cannot change one side
of the comparison.

Run the gate with:

```bash
just test-rust jayjay-core --test log_graph_parity
```

## Diagnostic for real repositories

`dump_log_graph_parity` prints JayJay's structural ASCII graph at a bounded real-row checkpoint and
the equivalent `jj log -n N` invocation. It is diagnostic-only; shells consume `DagLayout`, never the
ASCII output.

```bash
cargo run -p jayjay-core --example dump_log_graph_parity -- <repo> '<revset>' <limit>
```

Compare topology independently of immutable/working-copy glyph choice when the diagnostic does not
load all metadata required to reproduce the CLI glyph.

## Recorded large-repository evidence

The implementation was validated read-only against a Rust checkout with a 639-row `jayjay()` result
and approximately 339,000 revisions in `::trunk()`:

- `jayjay()` at 50 real rows matched CLI topology;
- `jayjay()` at 500 rows matched after normalising node glyphs, including 38 synthetic elision bands;
- `::trunk()` at 500 rows matched after normalising node glyphs;
- debug validation accepted both `jayjay()` and a 5,000-row `::trunk()` prefix after its invariants
  were checked against renderer column reuse.

These observations supplement the deterministic gate. They are not a permanent claim about that
checkout's size or performance.

## Width policy

Parity deliberately rejects destructive lane budgets. Wide histories retain native lanes even when
the shell cannot display all of them at a legible pitch. The shell shows an overflow marker and can
narrow the requested revisions through focus mode. It does not cut edges and invent continuation
topology.

## Regression boundaries

A change to any of the following must extend or rerun the parity gate:

- revset evaluation, prioritisation, or graph ordering;
- progressive prefix handling;
- direct/indirect/missing edge conversion;
- synthetic-elision expansion;
- the renderer dependency or adapter;
- focus-revset composition.

Shell-only drawing changes normally use row-shape and geometry tests because the parity contract ends
at the app-owned `DagLayout` boundary.
