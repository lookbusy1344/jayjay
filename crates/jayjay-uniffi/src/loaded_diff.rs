use std::sync::Arc;

use jayjay_core::diff::{ReviewDisplayParts, SideBySideRow};
use jayjay_core::{DiffHunk, DisplayRows, LoadedDiff as CoreLoadedDiff};

use crate::packed_layout::pack_sbs_layout;

#[derive(uniffi::Object)]
pub struct LoadedDiff(pub(crate) CoreLoadedDiff);

#[derive(uniffi::Record)]
pub struct LoadedDiffFlags {
    pub whitespace_only_hidden: bool,
    pub has_context_regions: bool,
    pub has_conflicts: bool,
    pub supports_side_by_side: bool,
}

#[uniffi::export]
impl LoadedDiff {
    /// For hunks that already carry their texts, such as shell-built placeholders.
    #[uniffi::constructor]
    fn from_hunk(hunk: DiffHunk, ignore_whitespace: bool) -> Arc<Self> {
        Arc::new(Self(CoreLoadedDiff::from_hunk(hunk, ignore_whitespace)))
    }

    fn hunk(&self) -> DiffHunk {
        self.0.hunk()
    }

    fn display_rows(&self) -> DisplayRows {
        self.0.display_rows()
    }

    fn review_display(&self) -> Option<ReviewDisplayParts> {
        self.0.review().cloned()
    }

    fn side_by_side_rows(&self) -> Vec<SideBySideRow> {
        self.0.side_by_side_rows()
    }

    /// The side-by-side layout at these pane widths, packed by `pack_sbs_layout`.
    fn packed_side_by_side_layout(&self, old_cols: u32, new_cols: u32) -> Vec<u8> {
        pack_sbs_layout(&self.0.side_by_side_layout(old_cols, new_cols))
    }

    fn flags(&self) -> LoadedDiffFlags {
        LoadedDiffFlags {
            whitespace_only_hidden: self.0.whitespace_only_hidden(),
            has_context_regions: self.0.has_context_regions(),
            has_conflicts: self.0.has_conflicts(),
            supports_side_by_side: self.0.supports_side_by_side(),
        }
    }

    fn retained_bytes(&self) -> u64 {
        self.0.retained_bytes()
    }
}
