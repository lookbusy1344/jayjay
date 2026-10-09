use std::fs;
use std::path::Path;

use jayjay_core::{DiffHunk, HunkType, Repo};
use jj_test::{init_jj_repo, run_jj_in};
use tempfile::TempDir;

const JJ_STATUS_TEMPLATE: &str = r#"self.diff().files().map(|f| f.status() ++ "\t" ++ f.source().path() ++ "\t" ++ f.path() ++ "\n").join("")"#;

const TEN_LINES: &str = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n";
const TEN_LINES_ONE_EDIT: &str = "one\ntwo\nthree\nfour\nFIVE\nsix\nseven\neight\nnine\nten\n";

/// `(status, source, target)` in jj's template vocabulary; source equals target unless renamed.
type Entry = (String, String, String);

fn entry(status: &str, source: &str, target: &str) -> Entry {
    (status.to_owned(), source.to_owned(), target.to_owned())
}

/// Commits `base` files under `@-`, then applies `change` to a fresh `@`.
fn repo_with_change(base: &[(&str, &str)], change: impl FnOnce(&Path)) -> (TempDir, Repo) {
    let temp_dir = init_jj_repo();
    let repo_path = temp_dir.path().join("repo");
    for (path, content) in base {
        let file = repo_path.join(path);
        fs::create_dir_all(file.parent().expect("parent dir")).expect("create dir");
        fs::write(file, content).expect("write base file");
    }
    run_jj_in(&repo_path, &["new", "-m", "change"]);
    change(&repo_path);
    let repo = Repo::open(&repo_path).expect("open repo");
    repo.refresh_working_copy().expect("snapshot working copy");
    (temp_dir, repo)
}

fn jj_entries(repo_path: &Path) -> Vec<Entry> {
    let output = run_jj_in(
        repo_path,
        &["log", "-r", "@", "--no-graph", "-T", JJ_STATUS_TEMPLATE],
    );
    let mut entries: Vec<Entry> = String::from_utf8(output.stdout)
        .expect("utf-8 output")
        .lines()
        .map(|line| {
            let mut fields = line.split('\t');
            let mut next = || fields.next().expect("three fields").to_owned();
            (next(), next(), next())
        })
        .collect();
    entries.sort();
    entries
}

fn jayjay_entries(hunks: &[DiffHunk]) -> Vec<Entry> {
    let mut entries: Vec<Entry> = hunks
        .iter()
        .map(|hunk| {
            let status = match hunk.hunk_type {
                HunkType::Added => "added",
                HunkType::Removed => "removed",
                HunkType::Modified => "modified",
                HunkType::Renamed => "renamed",
            };
            let source = hunk.old_path.as_deref().unwrap_or(&hunk.path);
            entry(status, source, &hunk.path)
        })
        .collect();
    entries.sort();
    entries
}

/// Both jayjay walks (content-free summary and full show) must classify files exactly as `jj diff` does.
fn assert_matches_jj(temp_dir: &TempDir, repo: &Repo, expected: &[Entry]) {
    let repo_path = temp_dir.path().join("repo");
    assert_eq!(jj_entries(&repo_path), expected, "jj CLI");
    let summary = repo.show_summary("@").expect("show summary");
    assert_eq!(jayjay_entries(&summary.diff), expected, "show_summary");
    let full = repo.show("@").expect("show");
    assert_eq!(jayjay_entries(&full.diff), expected, "show");
}

#[test]
fn unrelated_file_with_the_same_basename_is_an_add_and_a_delete() {
    let (temp_dir, repo) = repo_with_change(&[("sub/AGENTS.md", TEN_LINES)], |repo_path| {
        fs::remove_file(repo_path.join("sub/AGENTS.md")).expect("delete nested file");
        fs::write(
            repo_path.join("AGENTS.md"),
            "# Overview\n\nUnrelated root guide\n",
        )
        .expect("write root file");
    });

    assert_matches_jj(
        &temp_dir,
        &repo,
        &[
            entry("added", "AGENTS.md", "AGENTS.md"),
            entry("removed", "sub/AGENTS.md", "sub/AGENTS.md"),
        ],
    );
}

#[test]
fn renames_are_paired_by_content_regardless_of_basename() {
    let (temp_dir, repo) = repo_with_change(&[("notes.txt", TEN_LINES)], |repo_path| {
        fs::rename(repo_path.join("hello.txt"), repo_path.join("greeting.txt"))
            .expect("rename hello.txt");
        fs::remove_file(repo_path.join("notes.txt")).expect("remove notes.txt");
        fs::write(repo_path.join("memo.txt"), TEN_LINES_ONE_EDIT).expect("write memo.txt");
    });

    assert_matches_jj(
        &temp_dir,
        &repo,
        &[
            entry("renamed", "hello.txt", "greeting.txt"),
            entry("renamed", "notes.txt", "memo.txt"),
        ],
    );
}

#[cfg(unix)]
#[test]
fn renamed_symlinks_stay_an_add_and_a_delete() {
    let temp_dir = init_jj_repo();
    let repo_path = temp_dir.path().join("repo");
    std::os::unix::fs::symlink("hello.txt", repo_path.join("link")).expect("create symlink");
    run_jj_in(&repo_path, &["new", "-m", "move link"]);
    fs::rename(repo_path.join("link"), repo_path.join("moved-link")).expect("move symlink");
    let repo = Repo::open(&repo_path).expect("open repo");
    repo.refresh_working_copy().expect("snapshot working copy");

    assert_matches_jj(
        &temp_dir,
        &repo,
        &[
            entry("added", "moved-link", "moved-link"),
            entry("removed", "link", "link"),
        ],
    );
}
