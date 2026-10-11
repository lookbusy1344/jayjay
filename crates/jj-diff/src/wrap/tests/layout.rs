use crate::side_by_side::{RowSide, SideBySideRow};
use crate::types::{ConflictLineKind, DiffSide, DiffSpanStyle, MovedRow};

use super::super::{SbsVisualLine, Utf16Range, wrap_sbs_layout, wrap_sbs_rows};
use super::fixtures::{row_side, span};

fn row(old: &str, new: &str) -> SideBySideRow {
    SideBySideRow {
        old: row_side("1", old, DiffSpanStyle::Removed),
        new: row_side("1", new, DiffSpanStyle::Added),
        full_width: false,
        context_region: None,
    }
}

fn range(start: u32, end: u32) -> Utf16Range {
    Utf16Range { start, end }
}

/// Old-side ranges of a row whose new side is empty.
fn old_ranges(text: &str, cols: u32) -> Vec<(u32, u32)> {
    wrap_sbs_layout(&[row(text, "")], cols, cols, 0)
        .iter()
        .map(|line| (line.old.start, line.old.end))
        .collect()
}

#[test]
fn ranges_follow_display_cells_in_utf16_units() {
    assert_eq!(old_ranges("abcdefghij", 4), vec![(0, 4), (4, 8), (8, 10)]);
    // Five CJK characters, two cells and one UTF-16 unit each.
    assert_eq!(old_ranges("名前の変更", 4), vec![(0, 2), (2, 4), (4, 5)]);
    // The family emoji is one 2-cell cluster of 8 UTF-16 units; it moves whole to the next line.
    assert_eq!(old_ranges("ab👨‍👩‍👧", 3), vec![(0, 2), (2, 10)]);
    // A combining accent stays with its base: "e\u{301}" is one cell, two units.
    assert_eq!(old_ranges("abe\u{301}d", 3), vec![(0, 4), (4, 5)]);
    // "𝒳" is one cell and a surrogate pair.
    assert_eq!(old_ranges("a𝒳b", 2), vec![(0, 3), (3, 4)]);
    assert_eq!(old_ranges("ab", 0), vec![(0, 1), (1, 2)]);
}

#[test]
fn rows_on_one_line_are_left_out_and_full_width_rows_stay() {
    let separator = SideBySideRow {
        old: row_side("", "40 unmodified lines", DiffSpanStyle::Separator),
        new: row_side("", "40 unmodified lines", DiffSpanStyle::Separator),
        full_width: false,
        context_region: None,
    };
    let conflict = SideBySideRow {
        old: RowSide::default(),
        new: RowSide {
            line_no: String::new(),
            spans: vec![span("<<<<<<< ours", DiffSpanStyle::Context)],
            style: DiffSpanStyle::Context,
            conflict_kind: ConflictLineKind::Start,
            moved: None,
        },
        full_width: true,
        context_region: None,
    };

    assert_eq!(
        wrap_sbs_layout(
            &[row("", ""), separator, row("abcdefghij", "x"), conflict],
            10,
            10,
            0
        ),
        vec![SbsVisualLine {
            row_ix: 3,
            old: range(0, 10),
            new: range(10, 12),
            full_width_source: Some(DiffSide::New),
        },]
    );
}

#[test]
fn uneven_sides_pad_the_shorter_one_with_empty_ranges() {
    assert_eq!(
        wrap_sbs_layout(&[row("abcdefgh", "xy")], 3, 3, 0),
        vec![
            SbsVisualLine {
                row_ix: 0,
                old: range(0, 3),
                new: range(0, 2),
                full_width_source: None
            },
            SbsVisualLine {
                row_ix: 0,
                old: range(3, 6),
                new: range(0, 0),
                full_width_source: None
            },
            SbsVisualLine {
                row_ix: 0,
                old: range(6, 8),
                new: range(0, 0),
                full_width_source: None
            },
        ]
    );
}

const MOVED_LABEL_COLS: u32 = 9;

fn utf16_slice(text: &str, range: Utf16Range) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    String::from_utf16(&units[range.start as usize..range.end as usize])
        .expect("slice on a character boundary")
}

#[test]
fn slicing_rows_by_the_layout_reproduces_wrapped_rows() {
    let wide: String = ["token ", "名前の変更 ", "👨‍👩‍👧 ", "café ", "e\u{301} ", "𝒳 "]
        .repeat(20)
        .concat();
    let conflict = SideBySideRow {
        old: RowSide::default(),
        new: RowSide {
            line_no: String::new(),
            spans: vec![span(&wide, DiffSpanStyle::Added)],
            style: DiffSpanStyle::Added,
            conflict_kind: ConflictLineKind::Added,
            moved: None,
        },
        full_width: true,
        context_region: None,
    };
    let mut moved = row(&wide, "short");
    moved.old.moved = Some(MovedRow {
        counterpart_line: 1,
        starts_block: true,
    });
    let rows = vec![
        row(&wide, &wide[..wide.len() / 2 + 1]),
        row("short", "fits"),
        row("short", &wide),
        moved,
        conflict,
    ];

    for cols in [40, 63, 89] {
        let layout = wrap_sbs_layout(&rows, cols, cols + 7, MOVED_LABEL_COLS);
        let wrapped = wrap_sbs_rows(&rows, cols, cols + 7, MOVED_LABEL_COLS);
        let visual: Vec<(u32, String, String)> = rows
            .iter()
            .enumerate()
            .flat_map(|(row_ix, source)| {
                let lines: Vec<_> = layout
                    .iter()
                    .filter(|line| line.row_ix == row_ix as u32)
                    .collect();
                if lines.is_empty() {
                    return vec![(row_ix as u32, source.old.text(), source.new.text())];
                }
                lines
                    .into_iter()
                    .map(|line| {
                        let (old_text, new_text) = match line.full_width_source {
                            Some(DiffSide::Old) => (source.old.text(), source.old.text()),
                            Some(DiffSide::New) => (source.new.text(), source.new.text()),
                            None => (source.old.text(), source.new.text()),
                        };
                        (
                            line.row_ix,
                            utf16_slice(&old_text, line.old),
                            utf16_slice(&new_text, line.new),
                        )
                    })
                    .collect()
            })
            .collect();
        let expected: Vec<(u32, String, String)> = wrapped
            .iter()
            .map(|line| (line.row_ix, line.row.old.text(), line.row.new.text()))
            .collect();
        assert_eq!(visual, expected, "cols {cols}");
    }
}
