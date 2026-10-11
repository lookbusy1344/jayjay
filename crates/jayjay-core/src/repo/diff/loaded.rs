use super::Repo;
use crate::loaded_diff::{DiffLoadRequest, LoadedDiff};
use crate::types::*;

impl Repo {
    /// Read one file card's sides and compute its diff, display rows, change groups and review state once.
    pub fn load_diff(&self, request: &DiffLoadRequest) -> JayResult<LoadedDiff> {
        let mut hunk = self.load_diff_hunk(request)?;
        // Review eligibility follows the card's type, not the type of the tree entry read for it.
        hunk.hunk_type = request.hunk_type;
        Ok(LoadedDiff::from_hunk(hunk, request.ignore_whitespace))
    }

    fn load_diff_hunk(&self, request: &DiffLoadRequest) -> JayResult<DiffHunk> {
        let trees = match &request.from_rev {
            Some(from_rev) => self.interdiff_tree_pair(from_rev, &request.rev)?,
            None => self.commit_tree_pair(&request.rev)?,
        };
        let source = request
            .old_path
            .as_deref()
            .filter(|_| request.hunk_type.has_source());
        if let Some(old_path) = source {
            return self.diff_sourced_file_with_mode(
                &trees,
                old_path,
                &request.path,
                request.projection_mode,
            );
        }
        let hunk =
            self.diff_single_file_with_mode(&trees, &request.path, request.projection_mode)?;
        if request.from_rev.is_some() || has_text(&hunk) {
            return Ok(hunk);
        }
        Ok(self.file_text_hunk(hunk, &request.rev))
    }

    /// Best effort for a card whose diff has no text on either side: show the file's text at `rev` as the new side.
    fn file_text_hunk(&self, hunk: DiffHunk, rev: &str) -> DiffHunk {
        match self.file_content(rev, &hunk.path) {
            Ok(content) if !content.is_empty() => DiffHunk {
                old: DiffContent::default(),
                new: DiffContent::new(Some(content), None),
                supports_file_editor: false,
                projection: None,
                ..hunk
            },
            _ => hunk,
        }
    }
}

fn has_text(hunk: &DiffHunk) -> bool {
    [&hunk.old, &hunk.new]
        .iter()
        .any(|side| side.content.as_deref().is_some_and(|text| !text.is_empty()))
}
