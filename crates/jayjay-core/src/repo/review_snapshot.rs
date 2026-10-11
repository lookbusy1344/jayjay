use jj_diff::ReviewFileSnapshot;

use crate::review_snapshot::review_snapshot_from_hunk;
use crate::types::{DiffHunk, JayResult};

impl super::Repo {
    pub fn review_file_snapshot(
        &self,
        rev: &str,
        path: &str,
        old_path: Option<&str>,
    ) -> JayResult<ReviewFileSnapshot> {
        Ok(review_snapshot_from_hunk(
            &self.load_review_hunk(rev, path, old_path)?,
        ))
    }

    pub(super) fn load_review_hunk(
        &self,
        rev: &str,
        path: &str,
        old_path: Option<&str>,
    ) -> JayResult<DiffHunk> {
        match old_path {
            Some(old) if old != path => self.show_file_rename(rev, old, path),
            _ => self.show_file(rev, path),
        }
    }
}
