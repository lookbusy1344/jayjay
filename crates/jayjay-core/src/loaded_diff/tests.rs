use std::mem::size_of;

use jayjay_primitives::{DiffContent, DiffHunk, HunkType};

use super::LoadedDiff;
use super::packed::{PackedRow, PackedSpan};

#[test]
fn retained_bytes_count_texts_span_text_and_packed_metadata() {
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
    let expected = 5 + (1 + 2) + 2 * size_of::<PackedRow>() + 2 * size_of::<PackedSpan>();
    assert_eq!(loaded.retained_bytes(), expected as u64);
}
