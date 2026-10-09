use jj_lib::backend::MergedTreeValue;
use jj_lib::copies::{CopiesTreeDiffEntry, CopyOperation};
use jj_lib::merge::{Diff, Merge};
use jj_lib::repo_path::RepoPathBuf;

use super::entry::resolve_diff_values;
use crate::types::*;
use jayjay_primitives::hex_sha256;

/// A copy-aware tree-diff entry reduced to the per-path diffs jayjay builds hunks from.
pub(super) enum CopyEntry {
    Path(RepoPathBuf, Diff<MergedTreeValue>),
    /// Each side carries one present value, so it builds as an ordinary removal and addition before `rename_hunk` joins them.
    Rename {
        source: (RepoPathBuf, Diff<MergedTreeValue>),
        target: (RepoPathBuf, Diff<MergedTreeValue>),
    },
}

impl CopyEntry {
    pub(super) fn from_jj(entry: CopiesTreeDiffEntry) -> JayResult<Self> {
        let CopiesTreeDiffEntry { path, values } = entry;
        let values = resolve_diff_values(&path.target, values)?;
        Ok(match path.source {
            None => Self::Path(path.target, values),
            // HunkType has no copy variant; the copy's source is unchanged, so the target reads as an addition.
            Some((_, CopyOperation::Copy)) => {
                Self::Path(path.target, Diff::new(Merge::absent(), values.after))
            }
            Some((source, CopyOperation::Rename)) => Self::Rename {
                source: (source, Diff::new(values.before, Merge::absent())),
                target: (path.target, Diff::new(Merge::absent(), values.after)),
            },
        })
    }
}

/// Joins the source's removal and the target's addition into one renamed hunk.
pub(super) fn rename_hunk(removed: DiffHunk, mut added: DiffHunk) -> DiffHunk {
    // Combine both sides so removed-side changes also invalidate the mark.
    added.review_identity = hex_sha256(
        format!(
            "rename|{}|{}",
            removed.review_identity, added.review_identity
        )
        .as_bytes(),
    );
    if removed.old.content == added.new.content {
        added.old.content = None;
        added.new.content = None;
    } else {
        added.old.content = removed.old.content;
    }
    added.old.preview = removed.old.preview;
    added.old_path = Some(removed.path);
    added.hunk_type = HunkType::Renamed;
    added
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hunk(path: &str, hunk_type: HunkType, old: Option<&str>, new: Option<&str>) -> DiffHunk {
        hunk_with_identity(path, hunk_type, old, new, "")
    }

    fn hunk_with_identity(
        path: &str,
        hunk_type: HunkType,
        old: Option<&str>,
        new: Option<&str>,
        review_identity: &str,
    ) -> DiffHunk {
        DiffHunk {
            path: path.to_owned(),
            old_path: None,
            old: DiffContent::new(old.map(str::to_owned), None),
            new: DiffContent::new(new.map(str::to_owned), None),
            hunk_type,
            supports_conflict_editor: false,
            supports_file_editor: false,
            review_identity: review_identity.to_owned(),
            projection: None,
        }
    }

    #[test]
    fn byte_equal_rename_clears_both_sides() {
        let renamed = rename_hunk(
            hunk("old.rs", HunkType::Removed, Some("body\n"), None),
            hunk("new.rs", HunkType::Added, None, Some("body\n")),
        );
        assert_eq!(renamed.hunk_type, HunkType::Renamed);
        assert_eq!(renamed.path, "new.rs");
        assert_eq!(renamed.old_path.as_deref(), Some("old.rs"));
        assert!(renamed.is_content_free_rename());
    }

    #[test]
    fn rename_with_changed_bytes_keeps_both_sides() {
        // Reordered lines and a dropped duplicate are line-set equal but not byte-equal, so the diff must survive.
        for (old, new) in [("a\nb\nc\n", "c\nb\na\n"), ("x\nx\ny\n", "x\ny\n")] {
            let renamed = rename_hunk(
                hunk("a/x.rs", HunkType::Removed, Some(old), None),
                hunk("b/x.rs", HunkType::Added, None, Some(new)),
            );
            assert_eq!(renamed.old.content.as_deref(), Some(old));
            assert_eq!(renamed.new.content.as_deref(), Some(new));
        }
    }

    #[test]
    fn rename_review_identity_combines_both_sides() {
        let rename = |old_identity| {
            rename_hunk(
                hunk_with_identity(
                    "old.rs",
                    HunkType::Removed,
                    Some("body"),
                    None,
                    old_identity,
                ),
                hunk_with_identity("new.rs", HunkType::Added, None, Some("body"), "id-new"),
            )
            .review_identity
        };
        let renamed_v1 = rename("id-old-v1");
        assert_ne!(renamed_v1, rename("id-old-v2"));
        assert_ne!(renamed_v1, "id-new");
    }

    #[test]
    fn rename_carries_old_preview_from_removed_hunk() {
        let mut removed = hunk(
            "old/icon.png",
            HunkType::Removed,
            Some("<image (100 bytes)>"),
            None,
        );
        removed.old.preview = Some(DiffPreview::Image {
            path: "/tmp/jayjay-images/abc123.png".to_owned(),
        });
        let added = hunk(
            "new/icon.png",
            HunkType::Added,
            None,
            Some("<image (100 bytes)>"),
        );

        let renamed = rename_hunk(removed, added);

        assert!(renamed.old.content.is_none());
        assert!(renamed.new.content.is_none());
        let Some(DiffPreview::Image { path }) = renamed.old.preview.as_ref() else {
            panic!(
                "renamed hunk should carry the removed side's preview, got {:?}",
                renamed.old.preview
            );
        };
        assert_eq!(path, "/tmp/jayjay-images/abc123.png");
    }
}
