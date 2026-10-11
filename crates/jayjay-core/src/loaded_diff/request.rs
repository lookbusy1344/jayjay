use jayjay_primitives::{DiffProjectionMode, HunkType};

/// One file card's diff: which revisions and paths to read, and how to present them.
#[derive(Debug, Clone)]
pub struct DiffLoadRequest {
    pub rev: String,
    /// Diff `from_rev` against `rev` instead of `rev` against its parents.
    pub from_rev: Option<String>,
    pub path: String,
    /// Source path of a rename or copy.
    pub old_path: Option<String>,
    pub hunk_type: HunkType,
    pub projection_mode: DiffProjectionMode,
    pub ignore_whitespace: bool,
}
