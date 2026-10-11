//! Deterministic repository for the FFI diff-transport benchmarks; Rust benches and the Swift `JayJayBenchmarks` target read the same changes.

use std::fs;
use std::path::Path;

use crate::cmd::{configure_test_user, init_colocated, run_jj_in};

/// One benchmark change, reachable in the fixture repo as the bookmark `bench-<name>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BenchScenario {
    /// 6,000-line Rust file, one line changed: a collapsed diff with large context.
    Sparse,
    /// The same file with every third line rewritten: many spans and groups.
    Dense,
    /// Long lines mixing ASCII, CJK and emoji ZWJ sequences: wrapping and UTF-16 offsets. Stays under jj's 1 MiB `snapshot.max-new-file-size` default.
    Wide,
    /// Many medium files with sparse changes: cache and retained memory.
    Many,
}

pub struct BenchFile {
    pub path: String,
    pub old: String,
    pub new: String,
}

const LARGE_FILE_LINES: usize = 6000;
const SPARSE_CHANGED_LINE: usize = 3000;
const DENSE_CHANGE_STRIDE: usize = 3;
const WIDE_LINES: usize = 300;
const WIDE_MIN_CHARS: usize = 400;
const WIDE_LENGTH_SPREAD: usize = 1601;
const WIDE_LENGTH_STEP: usize = 37;
const WIDE_CHANGE_STRIDE: usize = 10;
const MANY_FILES: usize = 40;
const MANY_LINES: usize = 500;
const MANY_CHANGE_STRIDE: usize = 10;
const WIDE_SEGMENTS: [&str; 4] = ["token ", "名前の変更 ", "👨‍👩‍👧 ", "café "];

impl BenchScenario {
    pub const ALL: [Self; 4] = [Self::Sparse, Self::Dense, Self::Wide, Self::Many];

    pub fn name(self) -> &'static str {
        match self {
            Self::Sparse => "sparse",
            Self::Dense => "dense",
            Self::Wide => "wide",
            Self::Many => "many",
        }
    }

    pub fn bookmark(self) -> String {
        format!("bench-{}", self.name())
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|scenario| scenario.name() == name)
    }

    pub fn files(self) -> Vec<BenchFile> {
        match self {
            Self::Sparse => vec![large_file("sparse.rs", |i| i == SPARSE_CHANGED_LINE)],
            Self::Dense => vec![large_file("dense.rs", |i| i % DENSE_CHANGE_STRIDE == 0)],
            Self::Wide => vec![wide_file()],
            Self::Many => (0..MANY_FILES).map(many_file).collect(),
        }
    }
}

/// Same line rule as `crates/jj-diff/benches/profile_diff.rs`.
fn large_file(path: &str, changed: impl Fn(usize) -> bool) -> BenchFile {
    let line = |i: usize| {
        format!(
            "let value_{i}: u32 = {i} + 1; // a long comment to exercise wrapping at eighty display columns"
        )
    };
    let old = (0..LARGE_FILE_LINES).map(line).collect::<Vec<_>>();
    let new = (0..LARGE_FILE_LINES)
        .map(|i| {
            if changed(i) {
                format!("let value_{i}: u32 = 9999 + 2; // changed")
            } else {
                line(i)
            }
        })
        .collect::<Vec<_>>();
    BenchFile {
        path: path.to_owned(),
        old: old.join("\n") + "\n",
        new: new.join("\n") + "\n",
    }
}

fn wide_file() -> BenchFile {
    let line = |i: usize| {
        let target = WIDE_MIN_CHARS + (i * WIDE_LENGTH_STEP) % WIDE_LENGTH_SPREAD;
        let mut text = format!("{i}: ");
        let mut chars = text.chars().count();
        for segment in WIDE_SEGMENTS.iter().cycle() {
            if chars >= target {
                break;
            }
            text.push_str(segment);
            chars += segment.chars().count();
        }
        text
    };
    let old = (0..WIDE_LINES).map(line).collect::<Vec<_>>();
    let new = (0..WIDE_LINES)
        .map(|i| {
            if i % WIDE_CHANGE_STRIDE == 0 {
                format!("{} changed", line(i))
            } else {
                line(i)
            }
        })
        .collect::<Vec<_>>();
    BenchFile {
        path: "wide.txt".to_owned(),
        old: old.join("\n") + "\n",
        new: new.join("\n") + "\n",
    }
}

fn many_file(index: usize) -> BenchFile {
    let line = |i: usize| format!("fn item_{index}_{i}() -> u32 {{ {i} }}");
    let old = (0..MANY_LINES).map(line).collect::<Vec<_>>();
    let new = (0..MANY_LINES)
        .map(|i| {
            if i % MANY_CHANGE_STRIDE == 0 {
                format!("fn item_{index}_{i}() -> u32 {{ {i} + 1 }}")
            } else {
                line(i)
            }
        })
        .collect::<Vec<_>>();
    BenchFile {
        path: format!("many/file_{index:02}.rs"),
        old: old.join("\n") + "\n",
        new: new.join("\n") + "\n",
    }
}

/// Build the fixture at `path` (must not exist): one base change with every old file, and one child of it per scenario holding that scenario's new files.
pub fn build_ffi_bench_repo(path: &Path) {
    init_colocated(path);
    configure_test_user(path);
    let write = |file: &str, content: &str| {
        let target = path.join(file);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).expect("create fixture directory");
        }
        fs::write(target, content).expect("write fixture file");
    };
    for scenario in BenchScenario::ALL {
        for file in scenario.files() {
            write(&file.path, &file.old);
        }
    }
    run_jj_in(path, &["describe", "-m", "ffi-bench base"]);
    for scenario in BenchScenario::ALL {
        run_jj_in(path, &["new", "root()+", "-m", scenario.name()]);
        for file in scenario.files() {
            write(&file.path, &file.new);
        }
        run_jj_in(
            path,
            &["bookmark", "create", &scenario.bookmark(), "-r", "@"],
        );
    }
    run_jj_in(path, &["new", "root()+"]);
}
