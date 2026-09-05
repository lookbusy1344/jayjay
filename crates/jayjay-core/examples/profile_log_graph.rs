//! Profiling entry point for progressive log-graph loading. Harvests the core's per-stage tracing
//! spans and reports repository open, total load time, rows retained, and each stage's summed busy
//! time — the breakdown `dag-loading-performance-plan.md` calls for.
//!
//! Run against a real repository:
//!   cargo run --release --example profile_log_graph -- <repo-path> [revset]
//! Or against a generated linear repository of N changes:
//!   cargo run --release --example profile_log_graph -- --synthetic <N> [revset]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jayjay_core::{GraphLoadToken, LogGraphEvent, LogGraphRequest, Repo};
use tracing::span;
use tracing::subscriber::set_global_default;
use tracing_subscriber::Registry;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;

#[derive(Clone, Default)]
struct Stage {
    count: u64,
    total: Duration,
}

/// Sums span busy time by span name across every thread that enters the span. A sequential stage
/// reads as wall time; a parallelized stage (e.g. empty-state checks) sums across worker threads, so
/// its total can exceed wall time. Stages that publish repeatedly also accumulate across prefixes.
#[derive(Clone, Default)]
struct StageTimings(Arc<Mutex<HashMap<&'static str, Stage>>>);

impl StageTimings {
    fn record(&self, name: &'static str, elapsed: Duration) {
        let mut stages = self.0.lock().unwrap();
        let stage = stages.entry(name).or_default();
        stage.count += 1;
        stage.total += elapsed;
    }

    /// Stages, slowest first.
    fn sorted(&self) -> Vec<(&'static str, Stage)> {
        let mut rows: Vec<_> = self
            .0
            .lock()
            .unwrap()
            .iter()
            .map(|(name, stage)| (*name, stage.clone()))
            .collect();
        rows.sort_by_key(|(_, stage)| std::cmp::Reverse(stage.total));
        rows
    }
}

struct EnterAt(Instant);

struct TimingLayer(StageTimings);

impl<S> Layer<S> for TimingLayer
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_enter(&self, id: &span::Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(EnterAt(Instant::now()));
        }
    }

    fn on_exit(&self, id: &span::Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id)
            && let Some(EnterAt(started)) = span.extensions_mut().remove::<EnterAt>()
        {
            self.0.record(span.name(), started.elapsed());
        }
    }
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// The revset from an optional positional arg, ignoring flags (e.g. `--full`) and defaulting to `all()`.
fn positional_revset(arg: Option<&String>) -> String {
    arg.filter(|value| !value.starts_with("--"))
        .cloned()
        .unwrap_or_else(|| "all()".to_owned())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let timings = StageTimings::default();
    set_global_default(Registry::default().with(TimingLayer(timings.clone())))
        .expect("install profiling subscriber");

    // Keep the synthetic repo's temp dir alive for the duration of the run.
    let mut _synthetic_guard = None;
    let (repo_path, revset) = match args.first().map(String::as_str) {
        None | Some("-h" | "--help") => {
            eprintln!(
                "usage: profile_log_graph <repo-path> [revset]\n       profile_log_graph --synthetic <N> [revset]"
            );
            std::process::exit(2);
        }
        Some("--synthetic") => {
            let count: u32 = args
                .get(1)
                .and_then(|n| n.parse().ok())
                .expect("--synthetic requires a change count");
            let revset = positional_revset(args.get(2));
            let (guard, path) = build_synthetic_repo(count);
            _synthetic_guard = Some(guard);
            (path, revset)
        }
        Some(path) => (
            std::path::PathBuf::from(path),
            positional_revset(args.get(1)),
        ),
    };

    let open_started = Instant::now();
    let repo = Repo::open(&repo_path).expect("open repository");
    let open_elapsed = open_started.elapsed();

    // The default ceiling keeps a huge revset bounded (it pauses rather than materializing every
    // row); pass --ceiling N to compare retained-memory plateaus, or --full on a small repository.
    let mut request = LogGraphRequest::new(revset.clone());
    if args.iter().any(|arg| arg == "--full") {
        request.row_ceiling = u32::MAX;
    } else if let Some(index) = args.iter().position(|arg| arg == "--ceiling") {
        request.row_ceiling = args
            .get(index + 1)
            .and_then(|value| value.parse().ok())
            .expect("--ceiling requires a positive row count");
    }

    let mut rows_retained = 0u32;
    let mut rows_consumed = 0u64;
    let mut snapshots = 0u32;
    let mut first_snapshot: Option<Duration> = None;
    let mut outcome = "no terminal event";
    let load_started = Instant::now();
    let token = GraphLoadToken::new();
    let pause_token = token.clone();
    repo.start_log_graph(request, token, |event| match event {
        LogGraphEvent::Snapshot(snapshot) => {
            first_snapshot.get_or_insert_with(|| load_started.elapsed());
            rows_retained = snapshot.loaded_rows;
            snapshots += 1;
        }
        LogGraphEvent::Finished => outcome = "finished",
        LogGraphEvent::Paused => {
            outcome = "paused at ceiling";
            pause_token.cancel();
        }
        LogGraphEvent::Canceled => {}
        LogGraphEvent::Failed(_) => outcome = "failed",
        LogGraphEvent::Progress(progress) => rows_consumed = progress.consumed_rows,
    });
    let load_elapsed = load_started.elapsed();

    println!("revset: {revset}");
    println!("repository open: {:.1} ms", millis(open_elapsed));
    match first_snapshot {
        Some(elapsed) => println!("first snapshot: {:.1} ms", millis(elapsed)),
        None => println!("first snapshot: none published"),
    }
    println!(
        "total load: {:.1} ms  ({outcome}, {snapshots} snapshots, {rows_consumed} rows consumed, {rows_retained} rows retained)",
        millis(load_elapsed)
    );
    if rows_retained > 0 {
        println!(
            "throughput: {:.1} rows/ms",
            f64::from(rows_retained) / millis(load_elapsed)
        );
    }
    println!("\nper-stage busy time (summed across publishes):");
    println!("  {:<34}{:>8}{:>12}", "stage", "count", "ms");
    for (name, stage) in timings.sorted() {
        println!(
            "  {:<34}{:>8}{:>12.1}",
            name,
            stage.count,
            millis(stage.total)
        );
    }
}

fn build_synthetic_repo(count: u32) -> (tempfile::TempDir, std::path::PathBuf) {
    let temp_dir = jj_test::init_jj_repo();
    let repo_path = temp_dir.path().join("repo");
    for i in 0..count {
        jj_test::run_jj_in(&repo_path, &["new", "-m", &format!("c{i}")]);
    }
    (temp_dir, repo_path)
}
