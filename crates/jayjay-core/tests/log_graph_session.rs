//! Proves `Repo::start_log_graph()` publishes ordered prefixes progressively and honors cancellation, per `dag-loading-performance-plan.md`.

use std::time::Duration;

use jayjay_core::{GraphLoadToken, LogGraphEvent, LogGraphRequest, LogGraphSnapshot, Repo};
use jj_test::{init_jj_repo, run_jj_in};

fn build_linear_repo(commit_count: u32) -> (tempfile::TempDir, std::path::PathBuf) {
    let temp_dir = init_jj_repo();
    let repo_path = temp_dir.path().join("repo");
    for i in 0..commit_count {
        run_jj_in(&repo_path, &["new", "-m", &format!("c{i}")]);
    }
    (temp_dir, repo_path)
}

fn request(revset: &str, initial_rows: u32, background_batch_rows: u32) -> LogGraphRequest {
    LogGraphRequest {
        revset: revset.to_owned(),
        initial_rows,
        background_batch_rows,
        first_result_budget: Duration::from_secs(10),
        row_ceiling: u32::MAX,
    }
}

fn snapshots(events: &[LogGraphEvent]) -> Vec<&LogGraphSnapshot> {
    events
        .iter()
        .filter_map(|event| match event {
            LogGraphEvent::Snapshot(snapshot) => Some(snapshot),
            _ => None,
        })
        .collect()
}

#[test]
fn session_publishes_growing_prefixes_before_finishing() {
    let (_temp_dir, repo_path) = build_linear_repo(20);
    let repo = Repo::open(&repo_path).expect("open repo");
    let full = repo.log_graph("all()").expect("full load");

    let mut events = Vec::new();
    repo.start_log_graph(request("all()", 4, 3), GraphLoadToken::new(), |event| {
        events.push(event);
    });

    let published = snapshots(&events);
    assert!(
        published.len() >= 2,
        "expected more than one published prefix, got {}",
        published.len()
    );

    // Loaded-row counts strictly increase and the last one covers everything.
    let loaded_rows: Vec<u32> = published
        .iter()
        .map(|snapshot| snapshot.loaded_rows)
        .collect();
    assert!(loaded_rows.windows(2).all(|window| window[0] < window[1]));
    assert_eq!(*loaded_rows.last().unwrap(), full.len() as u32);

    // Every published prefix is a stable, ordered prefix of the fully materialized result.
    for snapshot in &published {
        let ids: Vec<&str> = snapshot
            .entries
            .iter()
            .map(|entry| entry.change.commit_id.id.as_str())
            .collect();
        let full_prefix: Vec<&str> = full[..ids.len()]
            .iter()
            .map(|entry| entry.change.commit_id.id.as_str())
            .collect();
        assert_eq!(ids, full_prefix);
        assert_eq!(snapshot.layout.rows.len(), snapshot.entries.len());
    }

    assert!(published.last().unwrap().is_complete);
    assert!(
        !published[..published.len() - 1]
            .iter()
            .any(|snapshot| snapshot.is_complete)
    );

    assert!(matches!(events.last(), Some(LogGraphEvent::Finished)));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, LogGraphEvent::Finished))
            .count(),
        1
    );
}

#[test]
fn reaching_the_row_ceiling_pauses_and_continue_loads_the_rest() {
    let (_temp_dir, repo_path) = build_linear_repo(40);
    let repo = Repo::open(&repo_path).expect("open repo");
    let full = repo.log_graph("all()").expect("full load");
    assert!(
        full.len() > 20,
        "fixture must exceed the ceiling plus look-ahead"
    );

    // A ceiling well below the revset size pauses instead of finishing.
    let mut paused = request("all()", 4, 3);
    paused.row_ceiling = 8;
    let mut events = Vec::new();
    repo.start_log_graph(paused, GraphLoadToken::new(), |event| events.push(event));

    let published = snapshots(&events);
    let last = published.last().expect("at least one published prefix");
    assert_eq!(
        last.loaded_rows, 8,
        "the pause publishes exactly the ceiling"
    );
    assert!(!last.is_complete, "a paused prefix is not complete");
    assert!(matches!(events.last(), Some(LogGraphEvent::Paused)));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, LogGraphEvent::Finished)),
        "a ceiling pause must not report completion"
    );

    // Continuing with a higher ceiling loads the remaining rows and finishes.
    let mut resumed = request("all()", 4, 3);
    resumed.row_ceiling = 10_000;
    let mut resumed_events = Vec::new();
    repo.start_log_graph(resumed, GraphLoadToken::new(), |event| {
        resumed_events.push(event)
    });
    let resumed_published = snapshots(&resumed_events);
    assert_eq!(
        resumed_published.last().unwrap().loaded_rows as usize,
        full.len()
    );
    assert!(resumed_published.last().unwrap().is_complete);
    assert!(matches!(
        resumed_events.last(),
        Some(LogGraphEvent::Finished)
    ));
}

#[test]
fn cancellation_stops_the_stream_and_sends_no_later_snapshots() {
    let (_temp_dir, repo_path) = build_linear_repo(40);
    let repo = Repo::open(&repo_path).expect("open repo");
    let token = GraphLoadToken::new();

    let mut events: Vec<LogGraphEvent> = Vec::new();
    let mut canceled_after_first = false;
    let cancel_token = token.clone();
    repo.start_log_graph(request("all()", 4, 3), token, |event| {
        if matches!(event, LogGraphEvent::Snapshot(_)) && !canceled_after_first {
            cancel_token.cancel();
            canceled_after_first = true;
        }
        events.push(event);
    });

    let snapshot_count = snapshots(&events).len();
    assert_eq!(snapshot_count, 1, "no snapshot should follow cancellation");
    assert!(matches!(events.last(), Some(LogGraphEvent::Canceled)));
    assert!(!snapshots(&events)[0].is_complete);
}

#[test]
fn a_revset_smaller_than_the_initial_batch_publishes_once_and_finishes() {
    let (_temp_dir, repo_path) = build_linear_repo(3);
    let repo = Repo::open(&repo_path).expect("open repo");
    let full = repo.log_graph("all()").expect("full load");

    let mut events = Vec::new();
    repo.start_log_graph(request("all()", 50, 500), GraphLoadToken::new(), |event| {
        events.push(event);
    });

    let published = snapshots(&events);
    assert_eq!(published.len(), 1);
    assert_eq!(published[0].loaded_rows as usize, full.len());
    assert!(published[0].is_complete);
    assert!(matches!(events.last(), Some(LogGraphEvent::Finished)));
}

#[test]
fn cancellation_from_the_final_snapshot_reports_canceled_not_finished() {
    let (_temp_dir, repo_path) = build_linear_repo(3);
    let repo = Repo::open(&repo_path).expect("open repo");
    let token = GraphLoadToken::new();
    let cancel_token = token.clone();
    let mut events = Vec::new();

    repo.start_log_graph(request("all()", 50, 500), token, |event| {
        if matches!(&event, LogGraphEvent::Snapshot(snapshot) if snapshot.is_complete) {
            cancel_token.cancel();
        }
        events.push(event);
    });

    assert!(matches!(events.last(), Some(LogGraphEvent::Canceled)));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, LogGraphEvent::Finished))
    );
}

#[test]
fn an_invalid_revset_reports_failure_not_partial_success() {
    let (_temp_dir, repo_path) = build_linear_repo(3);
    let repo = Repo::open(&repo_path).expect("open repo");

    let mut events = Vec::new();
    repo.start_log_graph(
        request("not a valid revset (((", 50, 500),
        GraphLoadToken::new(),
        |event| events.push(event),
    );

    assert!(snapshots(&events).is_empty());
    assert!(matches!(events.last(), Some(LogGraphEvent::Failed(_))));
}
