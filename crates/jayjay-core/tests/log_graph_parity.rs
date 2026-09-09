//! Real-CLI parity gate for `jj log` DAG rendering (see `docs/jj-log-dag-parity-plan.md`).
//!
//! The pinned `jj log` binary is the oracle. This module renders JayJay's own `log_graph()`
//! sequence through `jayjay_core::dag::debug_render_log_graph_ascii()`, which drives the same
//! `sapling-renderdag` renderer (and, when enabled, the same indirect-edge-to-synthetic-node
//! expansion) production layout uses, so the two sides are compared as text produced by identical
//! renderer code — not by a hand-rolled column comparison or a duplicated Rust reimplementation.

use jayjay_core::dag::{DagLayout, debug_render_log_graph_ascii};
use jayjay_core::{GraphLoadToken, LogGraphEvent, LogGraphRequest, Repo, focus_revset};
use jj_test::{
    DAG_PARITY_REVSET, build_dag_parity_repo, cli_log_graph_ascii, init_jj_repo, run_jj_in,
};

#[test]
fn log_graph_matches_the_cli_when_synthetic_elided_nodes_are_disabled() {
    let (_temp_dir, repo_path) = build_dag_parity_repo();
    let repo = Repo::open(&repo_path).expect("open repo");

    let entries = repo.log_graph(DAG_PARITY_REVSET).expect("load graph");
    let ours = debug_render_log_graph_ascii(&entries, false);
    let cli = cli_log_graph_ascii(&repo_path, DAG_PARITY_REVSET, None, false);

    // With synthetic elision off, `jj log` keeps the single dashed ancestor jump straight from
    // `source` to `base`; this is the pre-existing (still legitimate) raw-indirect-edge shape.
    assert_eq!(ours, cli, "\njayjay:\n{ours}\n\njj log:\n{cli}\n");
}

#[test]
fn log_graph_matches_the_cli_default_of_synthetic_elided_nodes_enabled() {
    let (_temp_dir, repo_path) = build_dag_parity_repo();
    let repo = Repo::open(&repo_path).expect("open repo");

    let entries = repo.log_graph(DAG_PARITY_REVSET).expect("load graph");
    let ours = debug_render_log_graph_ascii(&entries, true);
    let cli = cli_log_graph_ascii(&repo_path, DAG_PARITY_REVSET, None, true);

    // The CLI keeps `source`'s column alive across the elided edge to `base` via a synthetic
    // `(elided revisions)` row, so the unrelated `feature-head`/fork/merge lineage opens a new
    // column rather than reusing it. This is the regression `log_graph_parity.rs` was added to
    // close.
    assert_eq!(ours, cli, "\njayjay:\n{ours}\n\njj log:\n{cli}\n");

    // The structure whose text just matched must carry the fixture's two boundary cases, or the
    // comparison above proved nothing about them.
    let layout = DagLayout::compute(&entries, true);
    assert!(
        layout.rows.iter().any(|row| row.elisions_after.len() == 2),
        "`two-elisions` merges two excluded parents, so one row owns two stacked bands"
    );
    assert!(
        layout
            .rows
            .iter()
            .any(|row| !row.termination_columns.is_empty()),
        "excluding the root revision gives the fixture a missing-edge boundary"
    );
}

#[test]
fn limited_prefix_preserves_edges_whose_targets_have_not_loaded() {
    const LIMIT: usize = 2;

    let (_temp_dir, repo_path) = build_dag_parity_repo();
    let repo = Repo::open(&repo_path).expect("open repo");
    let mut request = LogGraphRequest::new(DAG_PARITY_REVSET);
    request.initial_rows = LIMIT as u32;
    request.background_batch_rows = 1;
    request.row_ceiling = LIMIT as u32;
    let token = GraphLoadToken::new();
    let cancel = token.clone();
    let mut snapshot = None;
    repo.start_log_graph(request, token, |event| {
        if let LogGraphEvent::Snapshot(published) = event {
            snapshot = Some(published);
            cancel.cancel();
        }
    });
    let snapshot = snapshot.expect("first progressive snapshot");
    let entries = &snapshot.entries;

    let ours = debug_render_log_graph_ascii(entries, true);
    let cli = cli_log_graph_ascii(&repo_path, DAG_PARITY_REVSET, Some(LIMIT), true);
    assert_eq!(ours, cli, "\njayjay:\n{ours}\n\njj log:\n{cli}\n");

    let layout = &snapshot.layout;
    assert_eq!(layout, &DagLayout::compute(entries, true));
    let source = layout.rows.first().expect("limited source row");
    assert_eq!(
        source.elisions_after.len(),
        1,
        "an indirect target beyond the real-row limit still produces jj log's synthetic elision"
    );
}

/// Focus mode is a revset change, not a topology transform: the focused graph is the exact `jj log`
/// of `focus_revset(base, target)`. Rendering that composed revset must match the real CLI's graph
/// for the identical effective revset — same synthetic nodes, connectivity, ordering, and columns.
#[test]
fn focus_revset_graph_matches_the_cli_for_the_effective_revset() {
    let (_temp_dir, repo_path) = build_dag_parity_repo();
    let repo = Repo::open(&repo_path).expect("open repo");

    let effective = focus_revset(DAG_PARITY_REVSET, "subject(exact:\"feature-head\")");
    let entries = repo.log_graph(&effective).expect("load focused graph");
    let ours = debug_render_log_graph_ascii(&entries, true);
    let cli = cli_log_graph_ascii(&repo_path, &effective, None, true);

    assert_eq!(ours, cli, "\njayjay:\n{ours}\n\njj log:\n{cli}\n");

    assert!(
        entries
            .iter()
            .all(|entry| entry.change.description.trim() != "excl-1"),
        "focusing feature-head's lineage must drop the unrelated excl-1 branch"
    );
}

/// `revsets.log-graph-prioritize` defaults to `present(@)`, which prioritizes the branch holding
/// the working copy while preserving topological order — it does not pin `@` to the first row.
/// For a linear `A -> B -> C` history edited back to `B`, `B::` must show the real descendant `C`
/// above `@ B`, exactly as the real CLI does.
#[test]
fn a_displayed_descendant_of_the_working_copy_stays_above_it() {
    let temp_dir = init_jj_repo();
    let repo_path = temp_dir.path().join("repo");
    run_jj_in(&repo_path, &["describe", "-m", "A"]);
    run_jj_in(&repo_path, &["new", "-m", "B"]);
    run_jj_in(&repo_path, &["new", "-m", "C"]);
    run_jj_in(&repo_path, &["edit", "subject(exact:\"B\")"]);

    let repo = Repo::open(&repo_path).expect("open repo");
    let revset = "subject(exact:\"B\")::";
    let entries = repo.log_graph(revset).expect("load graph");
    let ours = debug_render_log_graph_ascii(&entries, true);
    let cli = cli_log_graph_ascii(&repo_path, revset, None, true);

    assert_eq!(ours, cli, "\njayjay:\n{ours}\n\njj log:\n{cli}\n");
    let b_index = entries
        .iter()
        .position(|entry| entry.change.description.trim() == "B")
        .expect("B present");
    let c_index = entries
        .iter()
        .position(|entry| entry.change.description.trim() == "C")
        .expect("C present");
    assert!(c_index < b_index, "descendant C must be ordered above @ B");
    assert!(entries[b_index].change.is_working_copy);
}
