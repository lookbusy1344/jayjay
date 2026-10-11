use std::fs;
use std::path::Path;

use jayjay_core::diff::{
    ChangeGroup, ContextExpansionFinish, ContextExpansionRequest, ContextExpansionSession,
    DiffLine, DiffSide, DiffSpanStyle, build_diff_display_lines, canonical_review_snapshot,
    change_groups, compute_file_diff, display_group_canonical_indices,
};
use jayjay_core::{
    DiffContent, DiffHunk, DiffLoadRequest, DiffProjectionMode, HunkType, LoadedDiff, Repo,
};
use jj_test::ffi_bench::{BenchFile, BenchScenario};
use jj_test::{FormatFixture, init_jj_repo, run_jj_in};

fn open(path: &Path) -> Repo {
    let repo = Repo::open(path).expect("open repo");
    repo.refresh_working_copy().expect("snapshot");
    repo
}

/// `old` committed in a parent change, `new` in the working copy.
fn edited_repo(path: &str, old: &[u8], new: &[u8]) -> (tempfile::TempDir, Repo) {
    let temp_dir = init_jj_repo();
    let repo_path = temp_dir.path().join("repo");
    fs::write(repo_path.join(path), old).expect("write old");
    run_jj_in(&repo_path, &["new", "-m", "edit"]);
    fs::write(repo_path.join(path), new).expect("write new");
    let repo = open(&repo_path);
    (temp_dir, repo)
}

fn request(path: &str, hunk_type: HunkType) -> DiffLoadRequest {
    DiffLoadRequest {
        rev: "@".into(),
        from_rev: None,
        path: path.into(),
        old_path: None,
        hunk_type,
        projection_mode: DiffProjectionMode::Processed,
        ignore_whitespace: false,
    }
}

fn text_hunk(path: &str, hunk_type: HunkType, old: Option<&str>, new: Option<&str>) -> DiffHunk {
    DiffHunk {
        path: path.into(),
        old_path: None,
        old: DiffContent::new(old.map(Into::into), None),
        new: DiffContent::new(new.map(Into::into), None),
        hunk_type,
        supports_conflict_editor: false,
        supports_file_editor: true,
        review_identity: "identity".into(),
        projection: None,
        matches_source: false,
    }
}

/// (old line, new line, style, text) per row.
fn rows(lines: &[DiffLine]) -> Vec<(Option<u32>, Option<u32>, DiffSpanStyle, String)> {
    lines
        .iter()
        .map(|line| (line.old_line_no, line.new_line_no, line.style, line.text()))
        .collect()
}

fn group_spans(groups: &[ChangeGroup]) -> Vec<(u32, u32)> {
    groups
        .iter()
        .map(|group| (group.start_line, group.end_line))
        .collect()
}

#[test]
fn loads_display_rows_and_groups_for_one_changed_line() {
    let (_dir, repo) = edited_repo("a.txt", b"one\ntwo\nthree\n", b"one\nTWO\nthree\n");

    let loaded = repo
        .load_diff(&request("a.txt", HunkType::Modified))
        .expect("load diff");
    let display = loaded.display_rows();

    assert_eq!(
        rows(&display.lines),
        vec![
            (Some(1), Some(1), DiffSpanStyle::Context, "one".into()),
            (Some(2), None, DiffSpanStyle::Removed, "two".into()),
            (None, Some(2), DiffSpanStyle::Added, "TWO".into()),
            (Some(3), Some(3), DiffSpanStyle::Context, "three".into()),
        ]
    );
    assert_eq!(group_spans(&display.groups), vec![(2, 3)]);
    assert_eq!(display.groups[0].anchor_side, DiffSide::Old);
    assert_eq!(display.groups[0].anchor_line, 2);
    assert_eq!(
        loaded.hunk().old.content.as_deref(),
        Some("one\ntwo\nthree\n")
    );
    assert_eq!(
        loaded.hunk().new.content.as_deref(),
        Some("one\nTWO\nthree\n")
    );
    assert!(loaded.supports_side_by_side());
    assert!(!loaded.has_context_regions());
    assert_eq!(
        loaded
            .review()
            .map(|review| (review.snapshot.fingerprints.len(), review.group_map.clone())),
        Some((1, vec![vec![0]]))
    );
}

#[test]
fn rename_diffs_against_the_source_path() {
    let temp_dir = init_jj_repo();
    let repo_path = temp_dir.path().join("repo");
    fs::write(repo_path.join("old.txt"), "alpha\nbeta\ngamma\n").expect("write source");
    run_jj_in(&repo_path, &["new", "-m", "rename"]);
    fs::remove_file(repo_path.join("old.txt")).expect("remove source");
    fs::write(repo_path.join("new.txt"), "alpha\nBETA\ngamma\n").expect("write target");
    let repo = open(&repo_path);

    let loaded = repo
        .load_diff(&DiffLoadRequest {
            old_path: Some("old.txt".into()),
            ..request("new.txt", HunkType::Renamed)
        })
        .expect("load rename");

    assert_eq!(
        rows(&loaded.display_rows().lines),
        vec![
            (Some(1), Some(1), DiffSpanStyle::Context, "alpha".into()),
            (Some(2), None, DiffSpanStyle::Removed, "beta".into()),
            (None, Some(2), DiffSpanStyle::Added, "BETA".into()),
            (Some(3), Some(3), DiffSpanStyle::Context, "gamma".into()),
        ]
    );
    assert_eq!(loaded.hunk().old_path.as_deref(), Some("old.txt"));
}

#[test]
fn ignore_whitespace_hides_a_whitespace_only_edit() {
    let (_dir, repo) = edited_repo("a.txt", b"one\ntwo\n", b"one\ntwo  \n");

    let loaded = repo
        .load_diff(&DiffLoadRequest {
            ignore_whitespace: true,
            ..request("a.txt", HunkType::Modified)
        })
        .expect("load diff");

    assert!(loaded.display_rows().groups.is_empty());
    assert!(loaded.whitespace_only_hidden());
    // The exact-whitespace snapshot still holds the edit; no display group maps onto it.
    let review = loaded.review().expect("review state");
    assert_eq!(review.snapshot.fingerprints.len(), 1);
    assert!(review.group_map.is_empty());
}

#[test]
fn projections_and_binary_files_carry_no_review_state() {
    let fixture = FormatFixture::build();
    let repo = open(&fixture.path);
    let notebook = repo
        .load_diff(&request(FormatFixture::NOTEBOOK, HunkType::Added))
        .expect("load notebook");
    assert_eq!(
        notebook.hunk().projection.map(|projection| projection.mode),
        Some(DiffProjectionMode::Processed)
    );
    assert!(!notebook.display_rows().lines.is_empty());
    assert!(notebook.review().is_none());

    let (_dir, repo) = edited_repo("blob.bin", b"\0\x01\x02old", b"\0\x01\x02new");
    let binary = repo
        .load_diff(&request("blob.bin", HunkType::Modified))
        .expect("load binary");
    assert!(binary.review().is_none());
}

#[test]
fn fingerprints_match_the_persisted_literals() {
    let digests = |old: &str, new: &str| -> Vec<String> {
        LoadedDiff::from_hunk(
            text_hunk("a.txt", HunkType::Modified, Some(old), Some(new)),
            false,
        )
        .review()
        .expect("review state")
        .snapshot
        .fingerprints
        .iter()
        .map(|fingerprint| fingerprint.digest.clone())
        .collect()
    };

    assert_eq!(
        digests("keep\nA\rB\nend\n", "keep\na\rb\nend\n"),
        vec!["a79c03a09045d43b19fd82c6a3409fc2872a2e0b69272830e021b9fe4dd34bb5"]
    );
    assert_eq!(
        digests("keep\r\n\r\nA\r\nend", "keep\r\n\r\na\r\nend"),
        vec!["498cda66f9d6a31c4e49769bf3ab9c5626f3140d41ffc78a973217bc02c17f62"]
    );
}

/// Debug builds diff the full `dense` file in over a minute; a prefix keeps its every-third-line pattern.
const DENSE_SAMPLE_LINES: usize = 600;
const MANY_SAMPLE_FILES: usize = 4;

fn differential_sample(scenario: BenchScenario) -> Vec<BenchFile> {
    let prefix = |text: &str| -> String {
        text.split_inclusive('\n')
            .take(DENSE_SAMPLE_LINES)
            .collect()
    };
    match scenario {
        BenchScenario::Dense => scenario
            .files()
            .into_iter()
            .map(|file| BenchFile {
                old: prefix(&file.old),
                new: prefix(&file.new),
                path: file.path,
            })
            .collect(),
        BenchScenario::Many => scenario
            .files()
            .into_iter()
            .take(MANY_SAMPLE_FILES)
            .collect(),
        BenchScenario::Sparse | BenchScenario::Wide => scenario.files(),
    }
}

#[test]
fn matches_the_separate_display_and_review_functions_on_bench_fixtures() {
    for scenario in BenchScenario::ALL {
        for file in differential_sample(scenario) {
            for ignore_whitespace in [false, true] {
                let loaded = LoadedDiff::from_hunk(
                    text_hunk(
                        &file.path,
                        HunkType::Modified,
                        Some(&file.old),
                        Some(&file.new),
                    ),
                    ignore_whitespace,
                );
                let diff = compute_file_diff(&file.path, &file.old, &file.new, ignore_whitespace);
                let display = build_diff_display_lines(&diff.lines);
                let context = format!("{} {} iw={ignore_whitespace}", scenario.name(), file.path);

                let rows = loaded.display_rows();
                assert_eq!(rows.lines, display, "{context}");
                assert_eq!(rows.groups, change_groups(&display), "{context}");
                let review = loaded.review().expect("text files have review state");
                assert_eq!(
                    review.snapshot,
                    canonical_review_snapshot(&file.old, &file.new),
                    "{context}"
                );
                assert_eq!(
                    review.group_map,
                    display_group_canonical_indices(&file.old, &file.new, ignore_whitespace),
                    "{context}"
                );
            }
        }
    }
}

#[test]
fn context_expansion_runs_from_the_loaded_diff() {
    let old: String = (1..=30).map(|line| format!("line {line}\n")).collect();
    let new = old.replace("line 15\n", "line fifteen\n");
    let loaded = LoadedDiff::from_hunk(
        text_hunk("a.txt", HunkType::Modified, Some(&old), Some(&new)),
        false,
    );
    assert!(loaded.has_context_regions());

    let mut session = ContextExpansionSession::default();
    let attempt = session
        .begin("basis", ContextExpansionRequest::AllRegions)
        .expect("attempt");
    let outcome = attempt.run(Some(loaded.expansion_source()));
    let ContextExpansionFinish::Applied { diff, .. } = session.finish("basis", outcome) else {
        panic!("expansion applies");
    };
    let expanded = loaded.with_expanded(diff);

    let display = expanded.display_rows();
    assert_eq!(display.lines.len(), 31);
    assert!(!expanded.has_context_regions());
    assert_eq!(group_spans(&display.groups), vec![(15, 16)]);
    assert_eq!(expanded.review(), loaded.review());

    let mut session = ContextExpansionSession::default();
    let stale = session
        .begin("basis", ContextExpansionRequest::AllRegions)
        .expect("attempt");
    session.reset();
    let outcome = stale.run(Some(loaded.expansion_source()));
    assert!(matches!(
        session.finish("basis", outcome),
        ContextExpansionFinish::Discarded
    ));
}
