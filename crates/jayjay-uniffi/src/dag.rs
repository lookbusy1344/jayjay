use std::sync::Arc;

use jayjay_core as core;
use jayjay_core::GraphEntry;
use jayjay_core::dag::{
    self, DagEdgeKind, DagElisionBand, DagLayout, DagLinkCell, DagRowShape, DagVerticalCell,
    SelectionGraph, SelectionState,
};

#[uniffi::remote(Record)]
pub struct DagLayout {
    pub rows: Vec<core::dag::DagRowShape>,
    pub logical_column_count: u32,
    pub widest_elision_band_count: u32,
}

#[uniffi::remote(Record)]
pub struct DagRowShape {
    pub commit_id: String,
    pub node_column: u32,
    pub graph_column_count: u32,
    pub incoming: Option<core::dag::DagEdgeKind>,
    pub node_line: Vec<core::dag::DagVerticalCell>,
    pub link_line: Option<Vec<core::dag::DagLinkCell>>,
    pub termination_columns: Vec<u32>,
    pub pad_line: Vec<core::dag::DagVerticalCell>,
    pub elisions_after: Vec<core::dag::DagElisionBand>,
}

#[uniffi::remote(Record)]
pub struct DagElisionBand {
    pub target_commit_id: String,
    pub node_column: u32,
    pub node_line: Vec<core::dag::DagVerticalCell>,
    pub link_line: Option<Vec<core::dag::DagLinkCell>>,
    pub termination_columns: Vec<u32>,
    pub pad_line: Vec<core::dag::DagVerticalCell>,
}

#[uniffi::remote(Enum)]
pub enum DagVerticalCell {
    Empty,
    Direct,
    Indirect,
}

#[uniffi::remote(Record)]
pub struct DagLinkCell {
    pub vertical: Option<core::dag::DagEdgeKind>,
    pub horizontal: Option<core::dag::DagEdgeKind>,
    pub left_fork: Option<core::dag::DagEdgeKind>,
    pub right_fork: Option<core::dag::DagEdgeKind>,
    pub left_merge: Option<core::dag::DagEdgeKind>,
    pub right_merge: Option<core::dag::DagEdgeKind>,
    pub is_child: bool,
}

#[uniffi::remote(Enum)]
pub enum DagEdgeKind {
    Direct,
    Indirect,
}

#[uniffi::export]
fn compute_dag_layout(entries: Vec<GraphEntry>, synthetic_elided_nodes: bool) -> DagLayout {
    layout_data(&entries, synthetic_elided_nodes)
}

pub(crate) fn layout_data(entries: &[GraphEntry], synthetic_elided_nodes: bool) -> DagLayout {
    DagLayout::compute(entries, synthetic_elided_nodes)
}

/// Everything the shell needs from one load; sending the entries back into Rust would copy every string again.
#[derive(uniffi::Record, Debug, Clone)]
pub struct GraphWithLayout {
    pub entries: Vec<GraphEntry>,
    pub layout: DagLayout,
    pub selection: Arc<DagSelectionGraph>,
}

#[derive(uniffi::Object, Debug)]
pub struct DagSelectionGraph(SelectionGraph);

impl DagSelectionGraph {
    pub(crate) fn from_entries(entries: &[GraphEntry]) -> Self {
        Self(SelectionGraph::new(entries))
    }
}

#[uniffi::export]
impl DagSelectionGraph {
    /// Only for a graph the shell patched itself; a loaded graph arrives with one already built.
    #[uniffi::constructor]
    pub fn new(entries: Vec<GraphEntry>) -> Self {
        Self::from_entries(&entries)
    }

    pub fn selection_state(&self, selected_commit_ids: Vec<String>) -> SelectionState {
        self.0.state(&selected_commit_ids)
    }
}

#[uniffi::export]
fn descendant_commit_ids(entries: Vec<GraphEntry>, commit_id: &str) -> Vec<String> {
    dag::descendant_commit_ids(&entries, commit_id)
}

#[uniffi::export]
fn can_rebase_onto(
    entries: Vec<GraphEntry>,
    source_commit_id: &str,
    target_commit_id: &str,
) -> bool {
    dag::can_rebase_onto(&entries, source_commit_id, target_commit_id)
}

#[cfg(test)]
mod tests {
    use jayjay_core::{
        ChangeInfo, CommitAuthor, EdgeType, GraphEdge, GraphEntry, NewChangeEligibility, ShortId,
    };

    use jayjay_core::dag::DagVerticalCell;

    use super::compute_dag_layout;

    fn entry(commit_id: &str, edges: &[(&str, EdgeType)]) -> GraphEntry {
        GraphEntry {
            change: ChangeInfo {
                change_id: ShortId::new(format!("change-{commit_id}"), 1),
                commit_id: ShortId::new(commit_id.to_owned(), 1),
                description: String::new(),
                author: CommitAuthor::empty(0),
                parents: edges.iter().map(|(p, _)| (*p).to_owned()).collect(),
                bookmarks: Vec::new(),
                tags: Vec::new(),
                workspaces: Vec::new(),
                is_working_copy: false,
                has_conflict: false,
                is_empty: false,
                is_immutable: false,
                is_divergent: false,
                new_change: NewChangeEligibility {
                    on_top: true,
                    before: true,
                    after: true,
                },
            },
            edges: edges
                .iter()
                .map(|(target, edge_type)| GraphEdge {
                    target: (*target).to_owned(),
                    edge_type: *edge_type,
                })
                .collect(),
        }
    }

    /// Exercises the exported `compute_dag_layout` boundary function — the same function SwiftUI and GPUI call — over a row with a fork, a merge, an indirect edge, and a missing-edge termination, proving every field survives the FFI-facing conversion.
    #[test]
    fn compute_dag_layout_round_trips_fork_merge_indirect_and_termination() {
        let entries = vec![
            entry(
                "merge",
                &[
                    ("direct-parent", EdgeType::Direct),
                    ("indirect-parent", EdgeType::Indirect),
                    ("missing-parent", EdgeType::Missing),
                    ("outside-parent", EdgeType::Direct),
                ],
            ),
            entry("direct-parent", &[]),
            entry("indirect-parent", &[]),
        ];

        let layout = compute_dag_layout(entries, false);

        assert_eq!(layout.rows.len(), 3);
        assert_eq!(layout.logical_column_count, 4);

        let merge_row = &layout.rows[0];
        assert_eq!(merge_row.commit_id, "merge");
        assert_eq!(merge_row.node_column, 0);
        // The direct first parent keeps the node column, the missing parent terminates in its lane, and the off-page parent forks aside.
        assert_eq!(merge_row.pad_line[0], DagVerticalCell::Direct);
        assert_eq!(merge_row.termination_columns, vec![2]);
        // `outside-parent` is not in this prefix, so its lane simply continues below the row —
        // width never turns an unloaded parent into a marker of its own.
        assert!(
            merge_row
                .pad_line
                .iter()
                .filter(|cell| **cell == DagVerticalCell::Direct)
                .count()
                >= 2
        );

        let link_line = merge_row
            .link_line
            .as_ref()
            .expect("fork/merge row needs a link line");
        assert!(
            link_line.iter().any(|cell| {
                cell.left_fork.is_some()
                    || cell.right_fork.is_some()
                    || cell.left_merge.is_some()
                    || cell.right_merge.is_some()
            }),
            "expected at least one fork or merge segment linking the parents: {link_line:?}"
        );
        assert!(
            link_line.iter().any(|cell| cell.is_child),
            "expected one cell to mark the merge node as the link line's child column"
        );

        let direct_row = &layout.rows[1];
        assert_eq!(direct_row.node_column, 0);
        assert_eq!(
            direct_row.incoming,
            Some(jayjay_core::dag::DagEdgeKind::Direct)
        );
        let indirect_row = &layout.rows[2];
        assert_eq!(indirect_row.node_column, 1);
        assert_eq!(
            indirect_row.incoming,
            Some(jayjay_core::dag::DagEdgeKind::Indirect)
        );
        assert!(
            merge_row.pad_line[1] == DagVerticalCell::Indirect
                || merge_row.node_line[1] == DagVerticalCell::Indirect,
            "the indirect edge should stay visually distinguishable from the direct one"
        );
    }

    /// With `synthetic_elided_nodes` enabled, the boundary must carry: a real row with a synthetic
    /// elision band nested under it (never a top-level row of its own), the band's target identity
    /// and renderer columns, a missing-edge termination, and more than one real row — proving row
    /// lookup stays keyed on real commit ids even with a synthetic band present.
    #[test]
    fn compute_dag_layout_round_trips_a_synthetic_elision_band() {
        let entries = vec![
            entry(
                "source",
                &[
                    ("base", EdgeType::Indirect),
                    ("outside-parent", EdgeType::Direct),
                    ("missing-parent", EdgeType::Missing),
                ],
            ),
            entry("base", &[]),
        ];

        let layout = compute_dag_layout(entries, true);

        assert_eq!(
            layout.rows.len(),
            2,
            "only real entries get a top-level row"
        );
        let source_row = layout
            .rows
            .iter()
            .find(|row| row.commit_id == "source")
            .expect("source row");
        assert_eq!(source_row.elisions_after.len(), 1);
        let band = &source_row.elisions_after[0];
        assert_eq!(band.target_commit_id, "base");
        assert!(band.node_column < layout.logical_column_count);

        // The two summary fields a shell sizes its gutter and its row height from: the row's own
        // width covers the lanes its band draws, and the layout reports the deepest band stack.
        assert!(
            source_row.graph_column_count > band.node_column,
            "the row's gutter must cover the column its elision band paints in"
        );
        assert!(source_row.graph_column_count <= layout.logical_column_count);
        assert_eq!(layout.widest_elision_band_count, 1);

        assert!(
            !source_row.termination_columns.is_empty(),
            "the missing parent must still terminate its lane"
        );

        let base_row = layout
            .rows
            .iter()
            .find(|row| row.commit_id == "base")
            .expect("base row, looked up by its real commit id");
        assert_eq!(
            base_row.incoming,
            Some(jayjay_core::dag::DagEdgeKind::Direct),
            "the elision band's own edge into base draws as a direct line, not a dashed jump"
        );
    }
}
