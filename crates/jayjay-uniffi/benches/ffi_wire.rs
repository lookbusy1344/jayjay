//! Replays the SwiftUI shell's diff call sequences against the FFI types and reports bytes crossed, call count, Rust-side allocations and Rust-side time.
//! Usage: `ffi_wire <fixture-repo> [open|sbs|expand|preload] [scenario] [--calls]`; `--calls` adds a per-call breakdown. See `plans/ffi-diff-transport.md`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::ops::AddAssign;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use jayjay_core::diff::{
    self, ContextExpansionFinish, ContextExpansionRequest, ContextExpansionSession,
};
use jayjay_core::{DiffHunk, DiffLoadRequest, DiffProjectionMode, LoadedDiff, Repo};
use jayjay_uniffi::{LoadedDiffFlags, UniFfiTag};
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

/// `DiffStore.loadDiff`: `loadDiff`, then `CachedDiff` reads the hunk, review state and flags (`DiffStore.swift`). The returned handle crosses as a pointer.
fn load_diff(ledger: &mut Ledger, repo: &Repo, rev: &str, hunk: &DiffHunk) -> LoadedDiff {
    ledger.call("loadDiff");
    let request = ledger.arg(DiffLoadRequest {
        rev: rev.to_owned(),
        from_rev: None,
        path: hunk.path.clone(),
        old_path: hunk.old_path.clone(),
        hunk_type: hunk.hunk_type,
        projection_mode: DiffProjectionMode::Processed,
        ignore_whitespace: IGNORE_WHITESPACE,
    });
    let loaded = ledger.rust(|| repo.load_diff(&request).expect("load diff"));

    ledger.call("LoadedDiff.hunk");
    let content = ledger.rust(|| loaded.hunk());
    ledger.ret(content);
    read_state(ledger, &loaded);
    loaded
}

/// `CachedDiff(loaded:content:)`: review state and flags.
fn read_state(ledger: &mut Ledger, loaded: &LoadedDiff) {
    ledger.call("LoadedDiff.reviewDisplay");
    let review = ledger.rust(|| loaded.review().cloned());
    ledger.ret(review);

    ledger.call("LoadedDiff.flags");
    let flags = ledger.rust(|| LoadedDiffFlags {
        whitespace_only_hidden: loaded.whitespace_only_hidden(),
        has_context_regions: loaded.has_context_regions(),
        has_conflicts: loaded.has_conflicts(),
        supports_side_by_side: loaded.supports_side_by_side(),
    });
    ledger.ret(flags);
}

/// `DiffSectionLoadedDiff`: the display rows of the diff a section shows.
fn read_display_rows(ledger: &mut Ledger, loaded: &LoadedDiff) {
    ledger.call("LoadedDiff.displayRows");
    let rows = ledger.rust(|| loaded.display_rows());
    ledger.ret(rows);
}

/// Opening one file in the unified view: load, then its display rows.
fn open_file(ledger: &mut Ledger, repo: &Repo, rev: &str, hunk: &DiffHunk) {
    let loaded = load_diff(ledger, repo, rev, hunk);
    read_display_rows(ledger, &loaded);
}

/// `SideBySideCoordinator`: rows once per diff, then `wrapSbsRows` per width change.
fn sbs_sweep(ledger: &mut Ledger, loaded: &LoadedDiff, widths: &[u32]) {
    ledger.call("LoadedDiff.sideBySideRows");
    let rows = ledger.rust(|| loaded.side_by_side_rows());
    let rows = ledger.ret(rows);
    for &cols in widths {
        ledger.call("wrapSbsRows");
        let rows = ledger.arg(rows.clone());
        let wrapped = ledger.rust(|| diff::wrap_sbs_rows(&rows, cols, cols));
        ledger.ret(wrapped);
    }
}

/// `expandContext(.allRegions)` (`DiffSection+ContextExpansion.swift`): begin, run from the handle, finish into a new handle, then read its state and display rows.
fn expand_all(ledger: &mut Ledger, loaded: &LoadedDiff) {
    let mut session = ContextExpansionSession::default();
    ledger.call("ContextExpansionSession.begin");
    let attempt = ledger.rust(|| {
        session
            .begin(EXPANSION_BASIS, ContextExpansionRequest::AllRegions)
            .expect("begin expansion")
    });

    ledger.call("ContextExpansionAttempt.run");
    let outcome = ledger.rust(|| attempt.run(Some(loaded.expansion_source())));

    ledger.call("ContextExpansionSession.finish");
    let expanded = ledger.rust(|| match session.finish(EXPANSION_BASIS, outcome) {
        ContextExpansionFinish::Applied { diff, .. } => loaded.with_expanded(diff),
        _ => panic!("expansion was not applied"),
    });
    read_state(ledger, &expanded);
    read_display_rows(ledger, &expanded);
}

fn changed_hunks(repo: &Repo, rev: &str) -> Vec<DiffHunk> {
    repo.show_summary(rev).expect("show summary").diff
}

fn replay(repo: &Repo, workload: &str, scenario: BenchScenario) -> Ledger {
    let rev = scenario.bookmark();
    let hunks = changed_hunks(repo, &rev);
    let mut ledger = Ledger::default();
    match workload {
        "open" => {
            for hunk in &hunks {
                open_file(&mut ledger, repo, &rev, hunk);
            }
        }
        "preload" => {
            for hunk in &hunks {
                load_diff(&mut ledger, repo, &rev, hunk);
            }
        }
        "sbs" => {
            let loaded = load_diff(&mut Ledger::default(), repo, &rev, &hunks[0]);
            let widths: Vec<u32> = (SBS_MIN_COLS..=SBS_MAX_COLS)
                .step_by(SBS_COL_STEP)
                .collect();
            let sweep: Vec<u32> = widths.iter().chain(widths.iter().rev()).copied().collect();
            sbs_sweep(&mut ledger, &loaded, &sweep);
        }
        "expand" => {
            let loaded = load_diff(&mut Ledger::default(), repo, &rev, &hunks[0]);
            expand_all(&mut ledger, &loaded);
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
