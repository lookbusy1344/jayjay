//! App-owned structural representation of a rendered DAG row.
//!
//! Mirrors the shape of `sapling_renderdag::GraphRow` without exposing any upstream type, so the pre-1.0 dependency stays fully behind `renderdag`.

/// Whether an edge is an immediate parent or a synthesized link to a further ancestor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DagEdgeKind {
    Direct,
    Indirect,
}

/// A cell in a row's node line or pad line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DagVerticalCell {
    Empty,
    Direct,
    Indirect,
}

/// A cell in a row's link line, describing every edge segment that can pass through it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DagLinkCell {
    pub vertical: Option<DagEdgeKind>,
    pub horizontal: Option<DagEdgeKind>,
    pub left_fork: Option<DagEdgeKind>,
    pub right_fork: Option<DagEdgeKind>,
    pub left_merge: Option<DagEdgeKind>,
    pub right_merge: Option<DagEdgeKind>,
    /// True when the node that owns this link row is the child column for this cell.
    pub is_child: bool,
}

/// A synthetic `(elided revisions)` row nested under the real row that owns it. Has no selection
/// revision, context menu, drag identity, bookmark target, or detail view; `target_commit_id`
/// identifies which real ancestor the omitted path leads to, purely for shell labelling/
/// accessibility, never for shell-side inference of *why* the band exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DagElisionBand {
    pub target_commit_id: String,
    pub node_column: u32,
    pub node_line: Vec<DagVerticalCell>,
    pub link_line: Option<Vec<DagLinkCell>>,
    pub termination_columns: Vec<u32>,
    pub pad_line: Vec<DagVerticalCell>,
}

/// The structural shape of one rendered row: the node's column and every band around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DagRowShape {
    pub commit_id: String,
    pub node_column: u32,
    /// Logical columns this row occupies, including its elision bands. `jj log` starts each
    /// change's text after that row's own graph prefix, not after the widest prefix in the log,
    /// so a shell sizes the row's graph gutter from this rather than `logical_column_count`.
    pub graph_column_count: u32,
    pub incoming: Option<DagEdgeKind>,
    pub node_line: Vec<DagVerticalCell>,
    pub link_line: Option<Vec<DagLinkCell>>,
    pub termination_columns: Vec<u32>,
    pub pad_line: Vec<DagVerticalCell>,
    /// Synthetic elision bands owned by this row, in `jj log`'s emission order (one per indirect
    /// edge this row had when `ui.log-synthetic-elided-nodes` is enabled).
    pub elisions_after: Vec<DagElisionBand>,
}

/// A full graph, ordered top to bottom, plus the logical column count needed to draw it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DagLayout {
    pub rows: Vec<DagRowShape>,
    pub logical_column_count: u32,
    /// The most elision bands any single row owns. A shell whose list gives every row the same
    /// height sizes that height from this once, rather than scanning the layout per frame.
    pub widest_elision_band_count: u32,
}

impl DagLayout {
    pub fn row(&self, commit_id: &str) -> Option<&DagRowShape> {
        self.rows.iter().find(|row| row.commit_id == commit_id)
    }
}

/// Logical columns one rendered band occupies. Every column a shell can draw into counts, not just
/// the node line, so a row's graph gutter is never sized narrower than the widest lane inside it.
pub(crate) fn band_column_count(
    node_column: u32,
    node_line: &[DagVerticalCell],
    link_line: Option<&[DagLinkCell]>,
    termination_columns: &[u32],
    pad_line: &[DagVerticalCell],
) -> u32 {
    [
        node_column + 1,
        node_line.len() as u32,
        link_line.map_or(0, |cells| cells.len() as u32),
        termination_columns
            .iter()
            .copied()
            .max()
            .map_or(0, |column| column + 1),
        pad_line.len() as u32,
    ]
    .into_iter()
    .max()
    .unwrap_or(1)
}
