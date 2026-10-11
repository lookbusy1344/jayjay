//! Replays the SwiftUI shell's diff call sequences against the FFI types and reports bytes crossed, call count, Rust-side allocations and Rust-side time.
//! Usage: `ffi_wire <fixture-repo> [open|sbs|expand|preload] [scenario] [--calls]`; `--calls` adds a per-call breakdown. See `plans/ffi-diff-transport.md`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::ops::AddAssign;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use jayjay_core::diff::{
    self, ContextExpansionFinish, ContextExpansionRequest, ContextExpansionSession, DiffLine,
    FileDiff,
};
use jayjay_core::{DiffHunk, Repo, review_display_group_map_from_hunk, review_snapshot_from_hunk};
use jayjay_uniffi::{ContextExpansionSource, UniFfiTag};
use jj_test::ffi_bench::BenchScenario;
use uniffi::{Lift, Lower};

struct CountingAllocator;

static COUNTING: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);

fn record(size: usize) {
    if COUNTING.load(Ordering::Relaxed) {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(size as u64, Ordering::Relaxed);
    }
}

// SAFETY: forwards every call to `System` unchanged; counting has no effect on the returned memory.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record(new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// Timed runs per scenario stop after this budget, within `MIN_RUNS..=MAX_RUNS`, so a full run finishes in seconds.
const TIME_BUDGET: Duration = Duration::from_secs(1);
const MIN_RUNS: usize = 2;
const MAX_RUNS: usize = 10;
const SBS_MIN_COLS: u32 = 40;
const SBS_MAX_COLS: u32 = 89;
const SBS_COL_STEP: usize = 5;
const IGNORE_WHITESPACE: bool = false;
const EXPANSION_BASIS: &str = "bench";

/// Totals for one call name or one replay. Bytes are the serialized `RustBuffer` sizes of every argument and return value, as both sides of the real boundary copy them.
#[derive(Default, Clone, Copy)]
struct Totals {
    calls: u64,
    bytes_to_rust: u64,
    bytes_to_swift: u64,
    rust_allocations: u64,
    rust_allocated_bytes: u64,
    rust_time: Duration,
    /// The share of `rust_time` spent lifting arguments and lowering returns.
    marshal_time: Duration,
}

impl AddAssign for Totals {
    fn add_assign(&mut self, other: Self) {
        self.calls += other.calls;
        self.bytes_to_rust += other.bytes_to_rust;
        self.bytes_to_swift += other.bytes_to_swift;
        self.rust_allocations += other.rust_allocations;
        self.rust_allocated_bytes += other.rust_allocated_bytes;
        self.rust_time += other.rust_time;
        self.marshal_time += other.marshal_time;
    }
}

#[derive(Default)]
struct Ledger {
    current: &'static str,
    by_call: BTreeMap<&'static str, Totals>,
}

impl Ledger {
    fn current(&mut self) -> &mut Totals {
        self.by_call.entry(self.current).or_default()
    }

    fn total(&self) -> Totals {
        let mut total = Totals::default();
        for &totals in self.by_call.values() {
            total += totals;
        }
        total
    }

    /// Starts one FFI call, named after its Swift-side function.
    fn call(&mut self, name: &'static str) {
        self.current = name;
        self.current().calls += 1;
    }

    /// Runs `work` as Rust-side time: allocations and elapsed time count.
    fn rust<T>(&mut self, work: impl FnOnce() -> T) -> T {
        self.measure(false, work)
    }

    fn marshal<T>(&mut self, work: impl FnOnce() -> T) -> T {
        self.measure(true, work)
    }

    fn measure<T>(&mut self, marshal: bool, work: impl FnOnce() -> T) -> T {
        let allocations = ALLOCATIONS.load(Ordering::Relaxed);
        let bytes = ALLOCATED_BYTES.load(Ordering::Relaxed);
        COUNTING.store(true, Ordering::Relaxed);
        let started = Instant::now();
        let value = work();
        let elapsed = started.elapsed();
        COUNTING.store(false, Ordering::Relaxed);
        let totals = self.current();
        totals.rust_time += elapsed;
        if marshal {
            totals.marshal_time += elapsed;
        }
        totals.rust_allocations += ALLOCATIONS.load(Ordering::Relaxed) - allocations;
        totals.rust_allocated_bytes += ALLOCATED_BYTES.load(Ordering::Relaxed) - bytes;
        value
    }

    /// Swift → Rust argument: Swift lowers it (uncounted), Rust lifts it (counted).
    fn arg<T: Lower<UniFfiTag> + Lift<UniFfiTag>>(&mut self, value: T) -> T {
        let buffer = T::lower_into_rust_buffer(value);
        self.current().bytes_to_rust += buffer.len() as u64;
        self.marshal(|| T::try_lift_from_rust_buffer(buffer).expect("lift argument"))
    }

    /// Rust → Swift return: Rust lowers it (counted), Swift lifts it (uncounted); the lifted value is what Swift sends on later calls.
    fn ret<T: Lower<UniFfiTag> + Lift<UniFfiTag>>(&mut self, value: T) -> T {
        let buffer = self.marshal(|| T::lower_into_rust_buffer(value));
        self.current().bytes_to_swift += buffer.len() as u64;
        T::try_lift_from_rust_buffer(buffer).expect("lift return")
    }
}

/// `DiffStore.loadDiff`: `showFile` then `computeNativeDiff` (`DiffStore.swift`).
fn load_diff(ledger: &mut Ledger, repo: &Repo, rev: &str, path: &str) -> (DiffHunk, FileDiff) {
    ledger.call("showFile");
    let rev_arg = ledger.arg(rev.to_owned());
    let path_arg = ledger.arg(path.to_owned());
    let hunk = ledger.rust(|| repo.show_file(&rev_arg, &path_arg).expect("show file"));
    let hunk = ledger.ret(hunk);

    ledger.call("computeNativeDiff");
    let path_arg = ledger.arg(path.to_owned());
    let old = ledger.arg(hunk.old.content.clone().unwrap_or_default());
    let new = ledger.arg(hunk.new.content.clone().unwrap_or_default());
    let file_diff =
        ledger.rust(|| diff::compute_file_diff(&path_arg, &old, &new, IGNORE_WHITESPACE));
    let file_diff = ledger.ret(file_diff);
    (hunk, file_diff)
}

/// `diffDisplayLines` then `changeGroups` (`DiffSection+Loading.swift`, `PreparedContextExpansion`).
fn display_projection(ledger: &mut Ledger, lines: Vec<DiffLine>) -> Vec<DiffLine> {
    ledger.call("diffDisplayLines");
    let lines = ledger.arg(lines);
    let display = ledger.rust(|| diff::build_diff_display_lines(&lines));
    let display = ledger.ret(display);

    ledger.call("changeGroups");
    let display_arg = ledger.arg(display.clone());
    let groups = ledger.rust(|| diff::change_groups(&display_arg));
    ledger.ret(groups);
    display
}

/// `DiffSectionLoadedDiff.withReviewFingerprints` (`DiffLoadedContent.swift`).
fn review_fingerprints(ledger: &mut Ledger, hunk: &DiffHunk) {
    ledger.call("reviewSnapshotFromDiffHunk");
    let hunk_arg = ledger.arg(hunk.clone());
    let snapshot = ledger.rust(|| review_snapshot_from_hunk(&hunk_arg));
    ledger.ret(snapshot);

    ledger.call("reviewDisplayGroupMapFromDiffHunk");
    let hunk_arg = ledger.arg(hunk.clone());
    let mapping = ledger.rust(|| review_display_group_map_from_hunk(&hunk_arg, IGNORE_WHITESPACE));
    ledger.ret(mapping);
}

/// Opening one file in the unified view: load, display projection, review fingerprints.
fn open_file(ledger: &mut Ledger, repo: &Repo, rev: &str, path: &str) -> FileDiff {
    let (hunk, file_diff) = load_diff(ledger, repo, rev, path);
    display_projection(ledger, file_diff.lines.clone());
    review_fingerprints(ledger, &hunk);
    file_diff
}

/// One side-by-side width change: `buildSideBySideRows` then `wrapSbsRows` (`SideBySideDiffCoordinator.swift`).
fn sbs_resize(ledger: &mut Ledger, lines: &[DiffLine], cols: u32) {
    ledger.call("buildSideBySideRows");
    let lines = ledger.arg(lines.to_vec());
    let rows = ledger.rust(|| diff::build_side_by_side_rows(&lines));
    let rows = ledger.ret(rows);

    ledger.call("wrapSbsRows");
    let rows = ledger.arg(rows);
    let wrapped = ledger.rust(|| diff::wrap_sbs_rows(&rows, cols, cols));
    ledger.ret(wrapped);
}

/// `expandContext(.allRegions)` (`DiffSection+ContextExpansion.swift`): begin, run with the full source, finish, then the display projection.
fn expand_all(ledger: &mut Ledger, hunk: &DiffHunk, file_diff: FileDiff) {
    let mut session = ContextExpansionSession::default();
    ledger.call("ContextExpansionSession.begin");
    let attempt = ledger.rust(|| {
        session
            .begin(EXPANSION_BASIS, ContextExpansionRequest::AllRegions)
            .expect("begin expansion")
    });

    ledger.call("ContextExpansionAttempt.run");
    let source = ledger.arg(ContextExpansionSource {
        diff: file_diff,
        old_content: hunk.old.content.clone().unwrap_or_default(),
        new_content: hunk.new.content.clone().unwrap_or_default(),
    });
    let outcome = ledger.rust(|| {
        attempt.run(Some(diff::ContextExpansionSource {
            diff: source.diff,
            old_content: source.old_content.into(),
            new_content: source.new_content.into(),
        }))
    });

    ledger.call("ContextExpansionSession.finish");
    let finish = ledger.rust(|| session.finish(EXPANSION_BASIS, outcome));
    let ContextExpansionFinish::Applied { diff, .. } = ledger.ret(finish) else {
        panic!("expansion was not applied");
    };
    display_projection(ledger, diff.lines);
}

fn changed_paths(repo: &Repo, rev: &str) -> Vec<String> {
    repo.show_summary(rev)
        .expect("show summary")
        .diff
        .into_iter()
        .map(|hunk| hunk.path)
        .collect()
}

fn replay(repo: &Repo, workload: &str, scenario: BenchScenario) -> Ledger {
    let rev = scenario.bookmark();
    let paths = changed_paths(repo, &rev);
    let mut ledger = Ledger::default();
    match workload {
        "open" => {
            for path in &paths {
                open_file(&mut ledger, repo, &rev, path);
            }
        }
        "preload" => {
            for path in &paths {
                load_diff(&mut ledger, repo, &rev, path);
            }
        }
        "sbs" => {
            let (_, file_diff) = load_diff(&mut Ledger::default(), repo, &rev, &paths[0]);
            let widths: Vec<u32> = (SBS_MIN_COLS..=SBS_MAX_COLS)
                .step_by(SBS_COL_STEP)
                .collect();
            for &cols in widths.iter().chain(widths.iter().rev()) {
                sbs_resize(&mut ledger, &file_diff.lines, cols);
            }
        }
        "expand" => {
            let (hunk, file_diff) = load_diff(&mut Ledger::default(), repo, &rev, &paths[0]);
            expand_all(&mut ledger, &hunk, file_diff);
        }
        _ => unreachable!(),
    }
    ledger
}

const WORKLOADS: [(&str, &[BenchScenario]); 4] = [
    (
        "open",
        &[
            BenchScenario::Sparse,
            BenchScenario::Dense,
            BenchScenario::Wide,
            BenchScenario::Many,
        ],
    ),
    ("sbs", &[BenchScenario::Dense, BenchScenario::Wide]),
    ("expand", &[BenchScenario::Sparse]),
    ("preload", &[BenchScenario::Many]),
];

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn print_row(
    label: &str,
    scenario: &str,
    totals: Totals,
    times: (Duration, Duration),
    runs: usize,
) {
    let (rust, marshal) = times;
    println!(
        "{label}\t{scenario}\t{}\t{:.2}\t{:.2}\t{}\t{:.2}\t{:.1}\t{:.1}\t{runs}",
        totals.calls,
        mib(totals.bytes_to_rust),
        mib(totals.bytes_to_swift),
        totals.rust_allocations,
        mib(totals.rust_allocated_bytes),
        rust.as_secs_f64() * 1000.0,
        marshal.as_secs_f64() * 1000.0,
    );
}

fn medians(runs: &[Ledger], totals: impl Fn(&Ledger) -> Totals) -> (Duration, Duration) {
    (
        median(runs.iter().map(|run| totals(run).rust_time).collect()),
        median(runs.iter().map(|run| totals(run).marshal_time).collect()),
    )
}

fn median(mut times: Vec<Duration>) -> Duration {
    times.sort();
    times[times.len() / 2]
}

fn main() {
    let mut args: Vec<_> = std::env::args()
        .skip(1)
        .filter(|arg| arg != "--bench")
        .collect();
    let breakdown = args.iter().any(|arg| arg == "--calls");
    args.retain(|arg| arg != "--calls");
    let (path, workload_filter, scenario_filter) = match args.as_slice() {
        [path] => (Path::new(path), None, None),
        [path, workload] => (Path::new(path), Some(workload.as_str()), None),
        [path, workload, scenario] => (
            Path::new(path),
            Some(workload.as_str()),
            Some(BenchScenario::parse(scenario).expect("unknown scenario")),
        ),
        _ => {
            panic!("usage: ffi_wire <fixture-repo> [open|sbs|expand|preload] [scenario] [--calls]")
        }
    };
    let repo = Repo::open(path).expect("open fixture repo");

    println!(
        "workload\tscenario\tcalls\tMiB→rust\tMiB→swift\trust allocs\trust alloc MiB\trust ms\tmarshal ms\truns"
    );
    for (workload, scenarios) in WORKLOADS {
        if workload_filter.is_some_and(|filter| filter != workload) {
            continue;
        }
        for &scenario in scenarios {
            if scenario_filter.is_some_and(|filter| filter != scenario) {
                continue;
            }
            // Warm-up fills grammar and highlight caches so the counted runs measure steady state.
            replay(&repo, workload, scenario);
            let started = Instant::now();
            let mut runs: Vec<Ledger> = Vec::new();
            while runs.len() < MIN_RUNS
                || (runs.len() < MAX_RUNS && started.elapsed() < TIME_BUDGET)
            {
                runs.push(replay(&repo, workload, scenario));
            }
            let times = medians(&runs, Ledger::total);
            print_row(
                workload,
                scenario.name(),
                runs[0].total(),
                times,
                runs.len(),
            );
            if breakdown {
                for (&name, &totals) in &runs[0].by_call {
                    let times = medians(&runs, |run| run.by_call[name]);
                    print_row(
                        &format!("  {name}"),
                        scenario.name(),
                        totals,
                        times,
                        runs.len(),
                    );
                }
            }
        }
    }
}
