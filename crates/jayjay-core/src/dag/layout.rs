use super::renderdag;
use super::row_shape::DagLayout;
use crate::types::{GraphEdge, GraphEntry};

#[derive(Debug, Clone)]
pub(crate) struct DagLayoutInput {
    pub commit_id: String,
    pub edges: Vec<GraphEdge>,
    pub is_working_copy: bool,
}

impl From<&GraphEntry> for DagLayoutInput {
    fn from(entry: &GraphEntry) -> Self {
        Self {
            commit_id: entry.change.commit_id.id.clone(),
            edges: entry.edges.clone(),
            is_working_copy: entry.change.is_working_copy,
        }
    }
}

impl DagLayout {
    pub fn compute(entries: &[GraphEntry], synthetic_elided_nodes: bool) -> Self {
        let inputs = entries.iter().map(DagLayoutInput::from).collect::<Vec<_>>();
        Self::compute_inputs(&inputs, synthetic_elided_nodes)
    }

    pub(crate) fn compute_inputs(entries: &[DagLayoutInput], synthetic_elided_nodes: bool) -> Self {
        let span = tracing::debug_span!("dag.layout", rows = entries.len());
        let _entered = span.enter();
        let layout = renderdag::render(entries, synthetic_elided_nodes);
        #[cfg(debug_assertions)]
        {
            if let Err(violation) = layout.validate(entries, synthetic_elided_nodes) {
                panic!("DagLayout structural validation failed: {violation}");
            }
        }
        layout
    }
}
