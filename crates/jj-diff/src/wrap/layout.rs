use unicode_segmentation::UnicodeSegmentation;

use super::chunks::grapheme_cells;
use super::side_by_side::{full_width_source, wrap_sbs_rows};
use super::types::WrappedSide;
use crate::side_by_side::{RowSide, SideBySideRow};
use crate::types::DiffSide;

/// UTF-16 code-unit range `[start, end)` into one side's row text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Utf16Range {
    pub start: u32,
    pub end: u32,
}

/// One visual line of a wrapped side-by-side diff as ranges into its row's text, so a shell slices text it already holds instead of receiving every wrapped row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SbsVisualLine {
    pub row_ix: u32,
    pub old: Utf16Range,
    pub new: Utf16Range,
    /// For a full-width row, the side whose text both ranges index.
    pub full_width_source: Option<DiffSide>,
}

/// The geometry of `wrap_sbs_rows`, in UTF-16 offsets.
pub fn wrap_sbs_layout(rows: &[SideBySideRow], old_cols: u32, new_cols: u32) -> Vec<SbsVisualLine> {
    let wrapped = wrap_sbs_rows(rows, old_cols, new_cols);
    let mut offsets: Option<(u32, CellOffsets, CellOffsets)> = None;
    wrapped
        .iter()
        .map(|line| {
            let row = &rows[line.row_ix as usize];
            let source = row.full_width.then(|| full_width_source(row));
            if offsets
                .as_ref()
                .is_none_or(|(row_ix, _, _)| *row_ix != line.row_ix)
            {
                let (old, new) = match source {
                    Some(side) => (side_of(row, side), side_of(row, side)),
                    None => (&row.old, &row.new),
                };
                offsets = Some((line.row_ix, CellOffsets::of(old), CellOffsets::of(new)));
            }
            let (_, old, new) = offsets.as_ref().expect("offsets for the current row");
            SbsVisualLine {
                row_ix: line.row_ix,
                old: old.range(&line.old),
                new: new.range(&line.new),
                full_width_source: source,
            }
        })
        .collect()
}

fn side_of(row: &SideBySideRow, side: DiffSide) -> &RowSide {
    match side {
        DiffSide::Old => &row.old,
        DiffSide::New => &row.new,
    }
}

/// `(first cell, first UTF-16 unit)` of each grapheme cluster in one side's text.
struct CellOffsets {
    clusters: Vec<(usize, usize)>,
    utf16_len: usize,
}

impl CellOffsets {
    fn of(side: &RowSide) -> Self {
        let mut clusters = Vec::new();
        let (mut cells, mut units) = (0, 0);
        for span in &side.spans {
            for cluster in span.text.graphemes(true) {
                clusters.push((cells, units));
                cells += grapheme_cells(cluster);
                units += cluster.encode_utf16().count();
            }
        }
        Self {
            clusters,
            utf16_len: units,
        }
    }

    /// A cluster belongs to the line holding its first cell, as `wrap_sbs_rows` assigns a straddling glyph.
    fn offset(&self, cell: usize) -> usize {
        let ix = self.clusters.partition_point(|&(start, _)| start < cell);
        self.clusters
            .get(ix)
            .map_or(self.utf16_len, |&(_, units)| units)
    }

    fn range(&self, side: &WrappedSide) -> Utf16Range {
        Utf16Range {
            start: self.offset(side.col_start as usize) as u32,
            end: self.offset(side.col_end as usize) as u32,
        }
    }
}
