use std::sync::Arc;

use jayjay_primitives::{DiffContent, DiffHunk};
use jj_diff::{
    ChangeGroup, ConflictLineKind, ContextExpansionSource, DiffLine, DiffSpanStyle, FileDiff,
    ReviewDisplayParts, SbsVisualLine, SideBySideRow, build_diff_display_lines,
    build_side_by_side_rows, change_groups, compute_file_diff, review_display_parts,
    wrap_sbs_layout,
};

use super::packed::PackedDiff;
use crate::review_snapshot::review_text_pair;

/// Rendered unified rows and their change groups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayRows {
    pub lines: Vec<DiffLine>,
    pub groups: Vec<ChangeGroup>,
}

impl DisplayRows {
    fn of(diff: &FileDiff) -> Self {
        let lines = build_diff_display_lines(&diff.lines);
        Self {
            groups: change_groups(&lines),
            lines,
        }
    }
}

/// One file's diff, computed once from the loaded content, with the review state of its display rows.
///
/// Display rows are rebuilt on request: a shell reads them once per open, and a cached diff holding them twice costs more than the rebuild.
#[derive(Debug, Clone)]
pub struct LoadedDiff {
    /// Previews, projection and paths; the texts live in `old` and `new`.
    meta: Arc<DiffHunk>,
    old: Option<Arc<str>>,
    new: Option<Arc<str>>,
    diff: PackedDiff,
    review: Option<ReviewDisplayParts>,
}

impl LoadedDiff {
    pub fn from_hunk(mut hunk: DiffHunk, ignore_whitespace: bool) -> Self {
        let path = hunk
            .projection
            .as_ref()
            .map_or(hunk.path.as_str(), |projection| {
                projection.virtual_path.as_str()
            });
        let old_text = hunk.old.content.as_deref().unwrap_or_default();
        let new_text = hunk.new.content.as_deref().unwrap_or_default();
        let mut file_diff = compute_file_diff(path, old_text, new_text, ignore_whitespace);
        file_diff.detect_moves(old_text, new_text);
        let review = review_text_pair(&hunk)
            .map(|(old, new)| {
                review_display_parts(old, new, &build_diff_display_lines(&file_diff.lines))
            })
            .filter(|review| !review.snapshot.fingerprints.is_empty());
        let old = hunk.old.content.take().map(Arc::from);
        let new = hunk.new.content.take().map(Arc::from);
        Self {
            meta: Arc::new(hunk),
            old,
            new,
            diff: PackedDiff::pack(file_diff),
            review,
        }
    }

    /// The same file with `diff` (an expansion of this diff) in place of its rows.
    pub fn with_expanded(&self, diff: FileDiff) -> Self {
        Self {
            meta: Arc::clone(&self.meta),
            old: self.old.clone(),
            new: self.new.clone(),
            diff: PackedDiff::pack(diff),
            review: self.review.clone(),
        }
    }

    pub fn expansion_source(&self) -> ContextExpansionSource {
        let text = |side: &Option<Arc<str>>| side.clone().unwrap_or_else(|| Arc::from(""));
        ContextExpansionSource {
            diff: self.diff.unpack(),
            old_content: text(&self.old),
            new_content: text(&self.new),
        }
    }

    /// The loaded hunk with its texts; absent sides stay `None`.
    pub fn hunk(&self) -> DiffHunk {
        let content = |side: &DiffContent, text: &Option<Arc<str>>| {
            DiffContent::new(text.as_deref().map(str::to_owned), side.preview.clone())
        };
        DiffHunk {
            old: content(&self.meta.old, &self.old),
            new: content(&self.meta.new, &self.new),
            ..(*self.meta).clone()
        }
    }

    pub fn display_rows(&self) -> DisplayRows {
        DisplayRows::of(&self.diff.unpack())
    }

    /// Absent for projections, previews, placeholders and diffs without changed text.
    pub fn review(&self) -> Option<&ReviewDisplayParts> {
        self.review.as_ref()
    }

    pub fn side_by_side_rows(&self) -> Vec<SideBySideRow> {
        build_side_by_side_rows(&self.diff.unpack().lines)
    }

    /// Visual lines of `side_by_side_rows` wrapped to the pane widths, with `moved_label_cols` reserved on each moved block's first row.
    pub fn side_by_side_layout(
        &self,
        old_cols: u32,
        new_cols: u32,
        moved_label_cols: u32,
    ) -> Vec<SbsVisualLine> {
        wrap_sbs_layout(
            &self.side_by_side_rows(),
            old_cols,
            new_cols,
            moved_label_cols,
        )
    }

    pub fn whitespace_only_hidden(&self) -> bool {
        self.diff.whitespace_only_hidden()
    }

    pub fn has_context_regions(&self) -> bool {
        self.diff
            .rows()
            .iter()
            .any(|row| row.context_region.is_some())
    }

    pub fn has_conflicts(&self) -> bool {
        self.diff
            .rows()
            .iter()
            .any(|row| row.conflict_kind != ConflictLineKind::None)
    }

    /// Two columns need both removed and added rows, and conflict sections have no column of their own.
    pub fn supports_side_by_side(&self) -> bool {
        let has = |style| self.diff.rows().iter().any(|row| row.style == style);
        has(DiffSpanStyle::Added) && has(DiffSpanStyle::Removed) && !self.has_conflicts()
    }

    /// Text bytes plus the packed diff.
    pub fn retained_bytes(&self) -> u64 {
        let text = |side: &Option<Arc<str>>| side.as_deref().map_or(0, str::len);
        (text(&self.old) + text(&self.new) + self.diff.retained_bytes()) as u64
    }
}
