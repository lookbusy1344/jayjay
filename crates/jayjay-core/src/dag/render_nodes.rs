//! Pure expansion of indirect edges into the synthetic render-node sequence `jj log` feeds its
//! renderer when `ui.log-synthetic-elided-nodes` is enabled (see
//! `docs/jj-log-dag-parity-plan.md`): each indirect edge from real source `S` to real target `T`
//! becomes `S --direct--> synthetic(T) --direct--> T`. Both hops are direct/`Parent` edges to the
//! renderer; the elision is conveyed by the synthetic row's presence and content, not by dashed
//! ancestor styling. No `renderdag` type appears here — this module is renderer-agnostic.

use super::layout::DagLayoutInput;
use crate::types::EdgeType;

/// Identifies a render node: a real change, or a synthetic elision standing in for the omitted
/// path to `target_commit_id`. An `Elision` id is never equal to any real commit id, so it can
/// never resolve through `DagLayout::row()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DagRenderNodeId {
    Change(String),
    Elision {
        target_commit_id: String,
        /// Distinguishes multiple elisions that happen to share a target (e.g. two source rows
        /// with independent indirect edges into the same real ancestor), since each is a
        /// separate renderer row with its own reserved lane.
        occurrence: u32,
    },
}

/// One parent edge of a render node, after indirect edges have been expanded away.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DagRenderTarget {
    Node(DagRenderNodeId),
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DagRenderNode {
    pub id: DagRenderNodeId,
    pub parents: Vec<DagRenderTarget>,
    /// The real change this node is displayed under. Equal to the node's own commit id for a
    /// real node; for a synthetic node, the real source row whose indirect edge produced it.
    pub owner_commit_id: String,
}

/// Expands every entry's edges into the real node plus its owned synthetic elisions, in the same
/// order `jj log` emits them: the real row, then one row per indirect edge in edge order.
pub(crate) fn expand_indirect_edges(entries: &[DagLayoutInput]) -> Vec<DagRenderNode> {
    let mut occurrence = 0u32;
    let mut nodes = Vec::with_capacity(entries.len());

    for entry in entries {
        let mut parents = Vec::with_capacity(entry.edges.len());
        let mut elisions = Vec::new();

        for edge in &entry.edges {
            match edge.edge_type {
                EdgeType::Direct => {
                    parents.push(DagRenderTarget::Node(DagRenderNodeId::Change(
                        edge.target.clone(),
                    )));
                }
                EdgeType::Missing => parents.push(DagRenderTarget::Missing),
                EdgeType::Indirect => {
                    let elision_id = DagRenderNodeId::Elision {
                        target_commit_id: edge.target.clone(),
                        occurrence,
                    };
                    occurrence += 1;
                    parents.push(DagRenderTarget::Node(elision_id.clone()));
                    elisions.push(DagRenderNode {
                        id: elision_id,
                        parents: vec![DagRenderTarget::Node(DagRenderNodeId::Change(
                            edge.target.clone(),
                        ))],
                        owner_commit_id: entry.commit_id.clone(),
                    });
                }
            }
        }

        nodes.push(DagRenderNode {
            id: DagRenderNodeId::Change(entry.commit_id.clone()),
            parents,
            owner_commit_id: entry.commit_id.clone(),
        });
        nodes.extend(elisions);
    }

    nodes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::GraphEdge;

    fn input(commit_id: &str, edges: &[(&str, EdgeType)]) -> DagLayoutInput {
        DagLayoutInput {
            commit_id: commit_id.to_owned(),
            edges: edges
                .iter()
                .map(|(target, edge_type)| GraphEdge {
                    target: (*target).to_owned(),
                    edge_type: *edge_type,
                })
                .collect(),
            is_working_copy: false,
        }
    }

    #[test]
    fn a_direct_edge_produces_no_synthetic_node() {
        let entries = vec![input("S", &[("T", EdgeType::Direct)]), input("T", &[])];
        let nodes = expand_indirect_edges(&entries);

        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].id, DagRenderNodeId::Change("S".to_owned()));
        assert_eq!(
            nodes[0].parents,
            vec![DagRenderTarget::Node(DagRenderNodeId::Change(
                "T".to_owned()
            ))]
        );
    }

    #[test]
    fn one_indirect_edge_inserts_one_synthetic_node_immediately_after_the_source() {
        let entries = vec![input("S", &[("T", EdgeType::Indirect)]), input("T", &[])];
        let nodes = expand_indirect_edges(&entries);

        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0].id, DagRenderNodeId::Change("S".to_owned()));
        assert!(matches!(nodes[1].id, DagRenderNodeId::Elision { .. }));
        assert_eq!(nodes[2].id, DagRenderNodeId::Change("T".to_owned()));
    }

    #[test]
    fn multiple_indirect_targets_keep_stable_insertion_order() {
        let entries = vec![
            input("S", &[("A", EdgeType::Indirect), ("B", EdgeType::Indirect)]),
            input("A", &[]),
            input("B", &[]),
        ];
        let nodes = expand_indirect_edges(&entries);

        let elision_targets = nodes
            .iter()
            .filter_map(|node| match &node.id {
                DagRenderNodeId::Elision {
                    target_commit_id, ..
                } => Some(target_commit_id.as_str()),
                DagRenderNodeId::Change(_) => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(elision_targets, vec!["A", "B"]);
        // Synthetic rows for a merge's edges appear together, right after their owning source.
        assert!(matches!(nodes[1].id, DagRenderNodeId::Elision { .. }));
        assert!(matches!(nodes[2].id, DagRenderNodeId::Elision { .. }));
    }

    #[test]
    fn source_links_through_synthetic_to_the_real_target() {
        let entries = vec![input("S", &[("T", EdgeType::Indirect)]), input("T", &[])];
        let nodes = expand_indirect_edges(&entries);

        let DagRenderTarget::Node(source_edge) = &nodes[0].parents[0] else {
            panic!("source must have a node parent");
        };
        assert_eq!(
            source_edge, &nodes[1].id,
            "source's edge must point at the synthetic row it owns"
        );
        assert_eq!(
            nodes[1].parents,
            vec![DagRenderTarget::Node(DagRenderNodeId::Change(
                "T".to_owned()
            ))],
            "the synthetic row's own edge must point directly at the real target"
        );
        assert_eq!(nodes[1].owner_commit_id, "S");
    }

    #[test]
    fn missing_edges_are_preserved_untouched() {
        let entries = vec![input("S", &[("ghost", EdgeType::Missing)])];
        let nodes = expand_indirect_edges(&entries);

        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].parents, vec![DagRenderTarget::Missing]);
    }

    #[test]
    fn two_elisions_sharing_a_target_get_distinct_synthetic_identities() {
        // S1 and S2 each have their own indirect edge to the same real target T.
        let entries = vec![
            input("S1", &[("T", EdgeType::Indirect)]),
            input("S2", &[("T", EdgeType::Indirect)]),
            input("T", &[]),
        ];
        let nodes = expand_indirect_edges(&entries);

        let elisions = nodes
            .iter()
            .filter(|node| matches!(node.id, DagRenderNodeId::Elision { .. }))
            .collect::<Vec<_>>();
        assert_eq!(elisions.len(), 2);
        assert_ne!(
            elisions[0].id, elisions[1].id,
            "each elision row is a distinct renderer node, even with the same target"
        );
    }
}
