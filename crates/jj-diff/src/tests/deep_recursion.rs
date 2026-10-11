use super::*;
use crate::histogram_stack::with_histogram_stack;
use crate::line_diff::{LineOp, line_diff};

/// Smaller than the 512 KiB stacks of Swift's cooperative pool, where diffs like these crashed the app.
const SMALL_WORKER_STACK: usize = 256 << 10;
/// Changing every other token gives the deepest recursion: one matched block per unchanged token.
const CHANGE_STRIDE: usize = 2;
const LINES: usize = 1200;
const WORDS: usize = 1200;
/// The grown-stack ceiling, matching `jayjay-core`'s `on_worker_stack`.
const CAPPED_STACK: usize = 32 << 20;

fn on_small_stack<R: Send + 'static>(work: impl FnOnce() -> R + Send + 'static) -> R {
    std::thread::Builder::new()
        .stack_size(SMALL_WORKER_STACK)
        .spawn(work)
        .expect("spawn worker")
        .join()
        .expect("diff worker panicked")
}

#[test]
fn many_scattered_line_changes_diff_on_a_small_worker_stack() {
    let line = |i: usize| format!("line {i}");
    let old = (0..LINES).map(line).collect::<Vec<_>>().join("\n") + "\n";
    let new = (0..LINES)
        .map(|i| {
            if i % CHANGE_STRIDE == 0 {
                format!("changed {i}")
            } else {
                line(i)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";

    let ops = on_small_stack(move || line_diff(&old, &new, false));

    let expected: Vec<LineOp> = (0..LINES)
        .flat_map(|i| {
            if i % CHANGE_STRIDE == 0 {
                vec![LineOp::Remove, LineOp::Add]
            } else {
                vec![LineOp::Equal]
            }
        })
        .collect();
    assert_eq!(ops, expected);
}

#[test]
fn many_scattered_word_changes_diff_on_a_small_worker_stack() {
    let line = |changed: &str| {
        (0..WORDS)
            .map(|i| {
                if i % CHANGE_STRIDE == 0 {
                    format!("{changed}{i}")
                } else {
                    format!("keep{i}")
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
            + "\n"
    };
    let old = line("old");
    let new = line("new");

    let diff = on_small_stack(move || compute_file_diff("long.txt", &old, &new, false));

    let removed = diff
        .lines
        .iter()
        .find(|l| l.style == DiffSpanStyle::Removed)
        .expect("removed line");
    let added = diff
        .lines
        .iter()
        .find(|l| l.style == DiffSpanStyle::Added)
        .expect("added line");
    // Each changed word keeps its shared index suffix, so only the prefix is marked.
    let changed_words = WORDS / CHANGE_STRIDE;
    assert_eq!(
        styled_text(&span_info(removed), DiffSpanStyle::Removed),
        "old".repeat(changed_words)
    );
    assert_eq!(
        styled_text(&span_info(added), DiffSpanStyle::Added),
        "new".repeat(changed_words)
    );
}

#[test]
fn an_unbounded_depth_runs_on_a_capped_grown_stack() {
    let remaining = on_small_stack(|| with_histogram_stack(usize::MAX, stacker::remaining_stack))
        .expect("remaining stack is known on this platform");
    assert!(
        remaining > SMALL_WORKER_STACK,
        "stayed on the 256 KiB worker stack: {remaining} bytes left"
    );
    assert!(
        remaining <= CAPPED_STACK,
        "grown past the cap: {remaining} bytes left"
    );
}
