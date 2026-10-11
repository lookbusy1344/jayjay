use unicode_segmentation::UnicodeSegmentation;

use super::chunks::{SpanChunk, grapheme_cells, side_extents};
use super::side_by_side::full_width_source;
use crate::side_by_side::{RowSide, SideBySideRow};
use crate::types::{DiffSide, DiffSpanStyle};

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

/// The geometry of `wrap_sbs_rows` in UTF-16 offsets, for rows that wrap or span full width; any other row is one visual line holding the whole row. Both walk the same chunker, and this one builds no text.
pub fn wrap_sbs_layout(rows: &[SideBySideRow], old_cols: u32, new_cols: u32) -> Vec<SbsVisualLine> {
    let old_cols = old_cols.max(1) as usize;
    let new_cols = new_cols.max(1) as usize;
    let mut layout = Vec::new();
    for (row_ix, row) in rows.iter().enumerate() {
        let row_ix = row_ix as u32;
        if row.old.style == DiffSpanStyle::Separator {
            continue;
        }
        if row.full_width {
            push_full_width(row_ix, row, old_cols, new_cols, &mut layout);
            continue;
        }
        let old = side_extents(&row.old.spans, old_cols);
        let new = side_extents(&row.new.spans, new_cols);
        let count = old.len().max(new.len());
        if count > 1 {
            layout.extend((0..count).map(|ix| SbsVisualLine {
                row_ix,
                old: old.get(ix).map(chunk_range).unwrap_or_default(),
                new: new.get(ix).map(chunk_range).unwrap_or_default(),
                full_width_source: None,
            }));
        }
    }
    layout
}

/// A full-width row wraps at both panes' width, then splits each line at the old pane's width; a glyph straddling the split stays on the old side.
fn push_full_width(
    row_ix: u32,
    row: &SideBySideRow,
    old_cols: usize,
    new_cols: usize,
    layout: &mut Vec<SbsVisualLine>,
) {
    let side = full_width_source(row);
    let source = match side {
        DiffSide::Old => &row.old,
        DiffSide::New => &row.new,
    };
    let offsets = CellOffsets::of(source);
    for chunk in side_extents(&source.spans, old_cols.saturating_add(new_cols)) {
        let split = offsets.offset(chunk.start.saturating_add(old_cols).min(chunk.end));
        layout.push(SbsVisualLine {
            row_ix,
            old: utf16_range(chunk.utf16_start, split),
            new: utf16_range(split, chunk.utf16_end),
            full_width_source: Some(side),
        });
    }
}

fn chunk_range(chunk: &SpanChunk) -> Utf16Range {
    utf16_range(chunk.utf16_start, chunk.utf16_end)
}

fn utf16_range(start: usize, end: usize) -> Utf16Range {
    Utf16Range {
        start: start as u32,
        end: end as u32,
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
}
