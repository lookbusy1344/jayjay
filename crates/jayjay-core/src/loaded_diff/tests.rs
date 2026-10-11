use std::mem::size_of;

use jayjay_primitives::{DiffContent, DiffHunk, HunkType};
use jj_diff::{DiffSpanStyle, ReviewGroupFingerprint};

use super::LoadedDiff;
use super::packed::{PackedRow, PackedSpan};

#[test]
fn retained_bytes_count_texts_span_text_packed_metadata_and_review_state() {
    let hunk = DiffHunk {
        path: "a.txt".into(),
        old_path: None,
        old: DiffContent::default(),
        new: DiffContent::new(Some("a\nbc\n".into()), None),
        hunk_type: HunkType::Added,
        supports_conflict_editor: false,
        supports_file_editor: true,
        review_identity: "identity".into(),
        projection: None,
        matches_source: false,
    };
    let loaded = LoadedDiff::from_hunk(hunk, false);

    // Texts 0 + 5 bytes; two added rows "a" and "bc", one unhighlighted span each.
    let diff = 5 + (1 + 2) + 2 * size_of::<PackedRow>() + 2 * size_of::<PackedSpan>();
    // One change group: a 64-hex-char SHA-256 digest, and one display group mapped to one canonical index.
    let review =
        size_of::<ReviewGroupFingerprint>() + 64 + size_of::<Vec<u32>>() + size_of::<u32>();
    assert_eq!(
        loaded
            .review()
            .map(|review| review.snapshot.fingerprints.len()),
        Some(1)
    );
    assert_eq!(loaded.retained_bytes(), (diff + review) as u64);
}

#[test]
fn display_rows_keep_moved_block_links() {
    const SMALL: &str = "fn small() {\n    x(1);\n}\n";
    const BIG: &str = "fn big() {\n    let a = compute(1, 2);\n    let b = compute(3, 4);\n    let c = compute(5, 6);\n}\n";
    let hunk = DiffHunk {
        path: "moved.rs".into(),
        old_path: None,
        old: DiffContent::new(Some(format!("{SMALL}{BIG}")), None),
        new: DiffContent::new(Some(format!("{BIG}{SMALL}")), None),
        hunk_type: HunkType::Modified,
        supports_conflict_editor: false,
        supports_file_editor: true,
        review_identity: "identity".into(),
        projection: None,
        matches_source: false,
    };
    let moved: Vec<_> = LoadedDiff::from_hunk(hunk, false)
        .display_rows()
        .lines
        .iter()
        .filter_map(|line| {
            let moved = line.moved?;
            let own = match line.style {
                DiffSpanStyle::Removed => line.old_line_no?,
                _ => line.new_line_no?,
            };
            Some((line.style, own, moved.counterpart_line, moved.starts_block))
        })
        .collect();
    assert_eq!(
        moved,
        [
            (DiffSpanStyle::Removed, 1, 6, true),
            (DiffSpanStyle::Removed, 2, 6, false),
            (DiffSpanStyle::Removed, 3, 6, false),
            (DiffSpanStyle::Added, 6, 1, true),
            (DiffSpanStyle::Added, 7, 1, false),
            (DiffSpanStyle::Added, 8, 1, false),
        ]
    );
}
