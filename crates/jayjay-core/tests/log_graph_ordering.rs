//! Proves `Repo::log_graph()` orders commits the same way the pinned `jj` CLI does, since both must feed `jj_lib::graph::TopoGroupedGraph` with the same input.

use jayjay_core::{EdgeType, Repo};
use jj_test::{
    build_fork_merge_repo, commit_ids_from_cli_log, current_op_id, init_jj_repo, run_git, run_jj,
    run_jj_in,
};

fn commit_ids_from_log_graph(repo: &Repo, revset: &str) -> Vec<String> {
    repo.log_graph(revset)
        .expect("load graph")
        .into_iter()
        .map(|entry| entry.change.commit_id.id)
        .collect()
}

#[test]
fn log_graph_matches_cli_order_for_fork_and_merge() {
    let (_temp_dir, repo_path) = build_fork_merge_repo();
    let repo_str = repo_path.to_str().expect("repo path utf-8");
    let repo = Repo::open(&repo_path).expect("open repo");

    let ours = commit_ids_from_log_graph(&repo, "all()");
    let cli = commit_ids_from_cli_log(repo_str, "all()");

    assert_eq!(ours, cli);
}

#[test]
fn log_graph_prioritize_config_emits_configured_branch_first() {
    let (_temp_dir, repo_path) = build_fork_merge_repo();
    let repo_str = repo_path.to_str().expect("repo path utf-8");

    // Without prioritization, B's branch is queued before C's simply because it was authored first; force C's branch to go first via the CLI's own config knob.
    run_jj(&[
        "-R",
        repo_str,
        "config",
        "set",
        "--repo",
        "revsets.log-graph-prioritize",
        "subject(exact:\"C\")",
    ]);
    let repo = Repo::open(&repo_path).expect("open repo");

    let ours = commit_ids_from_log_graph(&repo, "all()");
    let cli = commit_ids_from_cli_log(repo_str, "all()");

    assert_eq!(ours, cli);

    let c_index = ours
        .iter()
        .position(|id| Some(id.as_str()) == cli_commit_for(repo_str, "C").as_deref())
        .expect("C present");
    let b_index = ours
        .iter()
        .position(|id| Some(id.as_str()) == cli_commit_for(repo_str, "B").as_deref())
        .expect("B present");
    assert!(
        c_index < b_index,
        "prioritized branch C should be emitted before B"
    );
}

#[test]
fn log_graph_turns_the_hidden_root_edge_into_a_missing_termination() {
    let (_temp_dir, repo_path) = build_fork_merge_repo();
    let repo = Repo::open(&repo_path).expect("open repo");

    let entries = repo.log_graph("all()").expect("load graph");
    let initial = entries
        .iter()
        .find(|entry| entry.change.description.trim() == "A")
        .expect("initial commit");

    assert_eq!(initial.edges.len(), 1);
    assert_eq!(initial.edges[0].edge_type, EdgeType::Missing);
}

#[test]
fn log_graph_indexes_refs_and_workspaces_by_commit() {
    let temp_dir = init_jj_repo();
    let repo_path = temp_dir.path().join("repo");
    run_jj_in(&repo_path, &["new", "-m", "indexed parent"]);
    run_jj_in(&repo_path, &["new", "-m", "child"]);
    run_jj_in(&repo_path, &["bookmark", "create", "feature", "-r", "@-"]);
    run_git(&repo_path, &["tag", "indexed-tag", "HEAD"]);
    run_jj_in(&repo_path, &["status"]);
    let other_workspace = temp_dir.path().join("other-workspace");
    run_jj_in(
        &repo_path,
        &[
            "workspace",
            "add",
            "--name",
            "other",
            "-r",
            "@-",
            other_workspace.to_str().expect("workspace path utf-8"),
        ],
    );

    let entries = Repo::open(&repo_path)
        .expect("open repo")
        .log_graph("all()")
        .expect("load graph");
    let parent = entries
        .iter()
        .find(|entry| entry.change.bookmarks.iter().any(|name| name == "feature"))
        .expect("bookmarked row");

    assert_eq!(parent.change.bookmarks, ["feature"]);
    assert_eq!(parent.change.tags, ["indexed-tag"]);
    assert!(
        entries
            .iter()
            .any(|entry| entry.change.workspaces == ["other"])
    );
}

#[test]
fn log_graph_preserves_empty_state_with_displayed_parents() {
    let temp_dir = init_jj_repo();
    let repo_path = temp_dir.path().join("repo");
    run_jj_in(&repo_path, &["describe", "-m", "changed"]);
    std::fs::write(repo_path.join("file.txt"), "content").expect("write fixture file");
    run_jj_in(&repo_path, &["status"]);
    run_jj_in(&repo_path, &["new", "-m", "empty"]);

    let entries = Repo::open(&repo_path)
        .expect("open repo")
        .log_graph("all()")
        .expect("load graph");
    let by_description = |description: &str| {
        entries
            .iter()
            .find(|entry| entry.change.description.trim() == description)
            .unwrap_or_else(|| panic!("missing {description}"))
    };

    assert!(by_description("empty").change.is_empty);
    assert!(!by_description("changed").change.is_empty);
}

#[test]
fn log_graph_marks_a_displayed_version_divergent_when_its_sibling_is_filtered_out() {
    let temp_dir = init_jj_repo();
    let repo_path = temp_dir.path().join("repo");
    let base_op = current_op_id(&repo_path);

    std::fs::write(repo_path.join("file.txt"), "left\n").expect("write left version");
    run_jj_in(&repo_path, &["describe", "-m", "left version"]);
    std::fs::write(repo_path.join("file.txt"), "right\n").expect("write right version");
    run_jj_in(
        &repo_path,
        &["--at-op", &base_op, "describe", "-m", "right version"],
    );

    let repo = Repo::open(&repo_path).expect("open repo");
    let one_version = repo
        .log("all()")
        .expect("load divergent versions")
        .into_iter()
        .find(|change| change.description.trim() == "left version")
        .expect("left version");
    let entries = repo
        .log_graph(&one_version.commit_id.id)
        .expect("load one divergent version");

    assert_eq!(entries.len(), 1);
    assert!(entries[0].change.is_divergent);
}

fn cli_commit_for(repo_str: &str, description: &str) -> Option<String> {
    let output = run_jj(&[
        "-R",
        repo_str,
        "log",
        "--no-graph",
        "-r",
        &format!("subject(exact:\"{description}\")"),
        "-T",
        "commit_id",
        "--color",
        "never",
    ]);
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!text.is_empty()).then_some(text)
}
