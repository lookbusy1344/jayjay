//! DAG layout for the jj log graph.

mod layout;
mod rebase;
mod render_nodes;
mod renderdag;
mod row_shape;
mod selection;
/// Only ever invoked behind `debug_assertions` (see `DagLayout::compute_inputs`), so a release
/// build has no caller for it.
#[cfg(any(debug_assertions, test))]
mod validate;

pub(crate) use layout::DagLayoutInput;
pub use rebase::{can_rebase_onto, descendant_commit_ids};
pub use row_shape::{
    DagEdgeKind, DagElisionBand, DagLayout, DagLinkCell, DagRowShape, DagVerticalCell,
};
pub use selection::{SelectionGraph, SelectionState};

/// Renders `entries` as `jj log`'s own ASCII graph text via the identical `sapling-renderdag`
/// renderer JayJay's production layout uses, for the real-CLI parity test only (see
/// `docs/jj-log-dag-parity-plan.md`). Not for shell or production use — shells consume
/// [`DagLayout`], never raw graph text.
pub fn debug_render_log_graph_ascii(
    entries: &[crate::types::GraphEntry],
    synthetic_elided_nodes: bool,
) -> String {
    let inputs = entries.iter().map(DagLayoutInput::from).collect::<Vec<_>>();
    renderdag::render_ascii_unprojected(&inputs, synthetic_elided_nodes)
}

#[cfg(test)]
mod tests {
    use crate::types::{
        ChangeInfo, CommitAuthor, EdgeType, GraphEdge, GraphEntry, NewChangeEligibility, ShortId,
    };

    pub(super) fn entry(commit_id: &str, parents: &[&str]) -> GraphEntry {
        GraphEntry {
            change: ChangeInfo {
                change_id: ShortId::new(format!("change-{commit_id}"), 1),
                commit_id: ShortId::new(commit_id.to_owned(), 1),
                description: String::new(),
                author: CommitAuthor::empty(0),
                parents: parents.iter().map(|id| (*id).to_owned()).collect(),
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
            edges: parents
                .iter()
                .map(|target| GraphEdge {
                    target: (*target).to_owned(),
                    edge_type: EdgeType::Direct,
                })
                .collect(),
        }
    }
}
