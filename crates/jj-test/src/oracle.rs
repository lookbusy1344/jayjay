//! The pinned `jj log` CLI as the gold-standard oracle for graph ordering, synthetic elision, and
//! renderer column allocation (see `docs/jj-log-dag-parity-plan.md`).

use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

use crate::cmd::{isolate_jj_config, run_command};

/// `jj-lib`/CLI major.minor this workspace pins. A future upgrade must rerun the parity suite and
/// review upstream `cli/src/commands/log.rs` before this is changed.
const PINNED_JJ_VERSION: &str = "0.45";

fn checked_jj_version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| {
        let output = run_command(
            "jj",
            &["--version".to_owned()],
            isolate_jj_config(&mut Command::new("jj")).arg("--version"),
        );
        let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let version = text
            .split_whitespace()
            .last()
            .unwrap_or_default()
            .trim_start_matches('v');
        let major_minor = version.rsplit_once('.').map_or(version, |(prefix, _)| prefix);
        assert_eq!(
            major_minor, PINNED_JJ_VERSION,
            "the `jj` binary on PATH reports version {text:?} (major.minor {major_minor}), but this \
             workspace pins jj-lib {PINNED_JJ_VERSION}.x. Do not mechanically bump this assertion: \
             rerun the real-CLI parity suite and review the upstream `log.rs` synthetic-node \
             transformation before accepting a new pinned version."
        );
        text
    })
}

/// The real, working-copy and synthetic-elision node symbols, fixed so the oracle compares graph
/// structure rather than commit decoration. Matches `debug_render_log_graph_ascii`'s vocabulary.
const CLI_NODE_TEMPLATE: &str = r#"coalesce(if(!self, "~"), if(current_working_copy, "@"), "o")"#;

/// Runs the pinned `jj log` binary in graph mode against `repo_path` with every presentation-
/// affecting input pinned, and returns its normalized ASCII graph output.
///
/// Do not pass `--no-graph`: it bypasses `TopoGroupedGraph` and is not a valid ordering or
/// elision oracle. Every real node's content line is exactly its full commit ID, so tests can
/// locate a specific node by searching for its ID rather than parsing box-drawing glyphs.
pub fn cli_log_graph_ascii(
    repo_path: &Path,
    revset: &str,
    limit: Option<usize>,
    synthetic_elided_nodes: bool,
) -> String {
    let _ = checked_jj_version();

    let mut args = vec![
        "-R".to_owned(),
        repo_path.display().to_string(),
        "log".to_owned(),
        "--color".to_owned(),
        "never".to_owned(),
        "--config".to_owned(),
        "ui.paginate=never".to_owned(),
        "--config".to_owned(),
        "ui.graph.style=ascii".to_owned(),
        "--config".to_owned(),
        format!("ui.log-synthetic-elided-nodes={synthetic_elided_nodes}"),
        // Pin the node symbols too. Without this the CLI decorates immutable, conflicted and
        // divergent commits with their own glyphs, so an unrelated fixture change would fail this
        // gate on decoration rather than on the graph structure it exists to police.
        "--config".to_owned(),
        format!("templates.log_node={CLI_NODE_TEMPLATE:?}"),
        "-T".to_owned(),
        "commit_id ++ \"\\n\"".to_owned(),
        "-r".to_owned(),
        revset.to_owned(),
    ];
    if let Some(limit) = limit {
        args.push("-n".to_owned());
        args.push(limit.to_string());
    }

    let mut command = Command::new("jj");
    isolate_jj_config(&mut command).args(&args);
    let output = run_command("jj", &args, &mut command);

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}
