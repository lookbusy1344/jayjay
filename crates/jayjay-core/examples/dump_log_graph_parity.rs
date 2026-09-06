//! Manual real-CLI parity dump for `docs/jj-log-dag-parity-plan.md` item 10 (the read-only
//! `~/Documents/dev/junk/rust` benchmark). Prints JayJay's unprojected structural ASCII graph
//! (the same renderer path the deterministic `log_graph_parity` tests assert against) at named
//! progressive checkpoints, alongside the real `jj log -n N` oracle command for the same prefix,
//! so the two can be diffed by hand. This is a diagnostic tool only — production code never
//! renders ASCII text; shells consume `DagLayout`.
//!
//! Bounded like the production progressive session: the row ceiling is the largest requested
//! checkpoint, so an unbounded revset (e.g. `::trunk()` on a ~344k-revision history) is never
//! eagerly materialized in full.
//!
//! Usage:
//!   cargo run -p jayjay-core --example dump_log_graph_parity -- <repo-path> <revset> [limits...]
//!
//! Limits are real-revision checkpoints (matching `jj log -n N`), defaulting to
//! INITIAL_LOG_BATCH_ROWS and MAX_AUTO_LOADED_ROWS.

use jayjay_core::dag::debug_render_log_graph_ascii;
use jayjay_core::{
    GraphLoadToken, INITIAL_LOG_BATCH_ROWS, LogGraphEvent, LogGraphRequest, MAX_AUTO_LOADED_ROWS,
    Repo,
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (repo_path, revset, mut limits) = match args.as_slice() {
        [] | [_] => {
            eprintln!(
                "usage: dump_log_graph_parity <repo-path> <revset> [limits...]\n\
                 limits default to INITIAL_LOG_BATCH_ROWS ({INITIAL_LOG_BATCH_ROWS}) and \
                 MAX_AUTO_LOADED_ROWS ({MAX_AUTO_LOADED_ROWS})"
            );
            std::process::exit(2);
        }
        [repo_path, revset] => (
            repo_path.clone(),
            revset.clone(),
            vec![INITIAL_LOG_BATCH_ROWS, MAX_AUTO_LOADED_ROWS],
        ),
        [repo_path, revset, rest @ ..] => (
            repo_path.clone(),
            revset.clone(),
            rest.iter()
                .map(|value| value.parse().expect("limit must be a positive integer"))
                .collect(),
        ),
    };
    limits.sort_unstable();
    limits.dedup();
    let ceiling = *limits.last().expect("at least one checkpoint");

    let repo = Repo::open(std::path::Path::new(&repo_path)).expect("open repository");
    let synthetic = repo
        .log_graph_synthetic_elided_nodes()
        .expect("read ui.log-synthetic-elided-nodes");

    println!("repo: {repo_path}");
    println!("revset: {revset}");
    println!("ui.log-synthetic-elided-nodes: {synthetic}\n");

    let mut request = LogGraphRequest::new(revset.clone());
    request.initial_rows = limits[0];
    request.row_ceiling = ceiling;
    let token = GraphLoadToken::new();
    let cancel = token.clone();
    let mut next_checkpoint = 0usize;
    repo.start_log_graph(request, token, |event| {
        let LogGraphEvent::Snapshot(snapshot) = event else {
            return;
        };
        while next_checkpoint < limits.len() && snapshot.loaded_rows >= limits[next_checkpoint] {
            let limit = limits[next_checkpoint] as usize;
            let prefix = &snapshot.entries[..limit.min(snapshot.entries.len())];
            let ours = debug_render_log_graph_ascii(prefix, synthetic);
            println!(
                "==== checkpoint: {limit} real rows (prefix available: {}) — jayjay ====\n{ours}\n",
                snapshot.entries.len()
            );
            println!(
                "==== checkpoint: {limit} real rows — run for comparison ====\n\
                 jj --ignore-working-copy -R {repo_path} log -r '{revset}' -n {limit} \
                 --color never --config ui.paginate=never --config ui.graph.style=ascii \
                 --config ui.log-synthetic-elided-nodes={synthetic} -T 'commit_id ++ \"\\n\"'\n"
            );
            next_checkpoint += 1;
        }
        if next_checkpoint >= limits.len() {
            cancel.cancel();
        }
    });
}
