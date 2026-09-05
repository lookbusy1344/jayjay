use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

use super::renderdag;
use super::row_shape::{DagContinuation, DagContinuationDirection, DagEdgeKind, DagLayout};
use crate::types::{EdgeType, GraphEdge, GraphEntry};

pub const MAX_CONTINUOUS_CONNECTOR_ROWS: usize = 12;
pub const MAX_VISIBLE_DAG_LANES: u32 = 8;

#[derive(Debug, Clone)]
pub(crate) struct DagLayoutInput {
    pub commit_id: String,
    pub parents: Vec<String>,
    pub edges: Vec<GraphEdge>,
    pub is_working_copy: bool,
    pub has_ref: bool,
}

impl From<&GraphEntry> for DagLayoutInput {
    fn from(entry: &GraphEntry) -> Self {
        Self {
            commit_id: entry.change.commit_id.id.clone(),
            parents: entry.change.parents.clone(),
            edges: entry.edges.clone(),
            is_working_copy: entry.change.is_working_copy,
            has_ref: entry.change.is_working_copy
                || !entry.change.bookmarks.is_empty()
                || !entry.change.workspaces.is_empty(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct EdgeId {
    pub source_index: usize,
    pub edge_index: usize,
}

#[derive(Debug)]
struct EdgeCandidate<'a> {
    id: EdgeId,
    source_id: &'a str,
    target_id: &'a str,
    target_index: Option<usize>,
    kind: DagEdgeKind,
    span: usize,
    has_ref: bool,
    is_adjacent_first_parent: bool,
}

impl DagLayout {
    pub fn compute(entries: &[GraphEntry]) -> Self {
        let inputs = entries.iter().map(DagLayoutInput::from).collect::<Vec<_>>();
        Self::compute_inputs(&inputs)
    }

    pub(crate) fn compute_inputs(entries: &[DagLayoutInput]) -> Self {
        let span = tracing::debug_span!("dag.layout_projection", rows = entries.len());
        let _entered = span.enter();
        let commit_rows = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| (entry.commit_id.as_str(), index))
            .collect::<HashMap<_, _>>();
        let protected_spine = protected_first_parent_spine(entries, &commit_rows);
        let candidates = edge_candidates(entries, &commit_rows);
        let mut cut_edges = candidates
            .iter()
            .filter(|candidate| {
                candidate.target_index.is_none()
                    || (candidate.target_index.is_some()
                        && candidate.span > MAX_CONTINUOUS_CONNECTOR_ROWS
                        && !protected_spine.contains(&candidate.id))
            })
            .map(|candidate| candidate.id)
            .collect::<HashSet<_>>();

        let empty_continuations = vec![Vec::new(); entries.len()];
        let mut layout = renderdag::render(entries, &cut_edges, &empty_continuations);
        if layout.logical_column_count > MAX_VISIBLE_DAG_LANES {
            let mut budget_candidates = candidates
                .iter()
                .filter(|candidate| {
                    !cut_edges.contains(&candidate.id)
                        && !protected_spine.contains(&candidate.id)
                        && !candidate.is_adjacent_first_parent
                })
                .collect::<Vec<_>>();
            budget_candidates.sort_by_key(|candidate| {
                (
                    candidate.has_ref,
                    candidate.kind == DagEdgeKind::Direct,
                    Reverse(candidate.span),
                    candidate.target_id,
                    candidate.source_id,
                )
            });

            for candidate in budget_candidates {
                cut_edges.insert(candidate.id);
                layout = renderdag::render(entries, &cut_edges, &empty_continuations);
                if layout.logical_column_count <= MAX_VISIBLE_DAG_LANES {
                    break;
                }
            }
        }

        let continuations = build_continuations(entries, &candidates, &cut_edges);
        let layout = renderdag::render(entries, &cut_edges, &continuations);
        tracing::debug!(
            projected_lanes = layout.logical_column_count,
            cut_connectors = cut_edges.len(),
            "DAG projection complete"
        );
        layout
    }
}

fn edge_candidates<'a>(
    entries: &'a [DagLayoutInput],
    commit_rows: &HashMap<&str, usize>,
) -> Vec<EdgeCandidate<'a>> {
    entries
        .iter()
        .enumerate()
        .flat_map(|(source_index, entry)| {
            let first_parent_id = entry.parents.first().map(String::as_str);
            entry
                .edges
                .iter()
                .enumerate()
                .filter_map(move |(edge_index, edge)| {
                    let kind = match edge.edge_type {
                        EdgeType::Direct => DagEdgeKind::Direct,
                        EdgeType::Indirect => DagEdgeKind::Indirect,
                        EdgeType::Missing => return None,
                    };
                    let target_index = commit_rows.get(edge.target.as_str()).copied();
                    let span = target_index
                        .map(|target_index| source_index.abs_diff(target_index))
                        .unwrap_or(usize::MAX);
                    let target_has_ref = target_index
                        .map(|index| entries[index].has_ref)
                        .unwrap_or(false);
                    Some(EdgeCandidate {
                        id: EdgeId {
                            source_index,
                            edge_index,
                        },
                        source_id: entry.commit_id.as_str(),
                        target_id: edge.target.as_str(),
                        target_index,
                        kind,
                        span,
                        has_ref: entry.has_ref || target_has_ref,
                        is_adjacent_first_parent: edge.edge_type == EdgeType::Direct
                            && first_parent_id == Some(edge.target.as_str())
                            && span == 1,
                    })
                })
        })
        .collect()
}

fn protected_first_parent_spine(
    entries: &[DagLayoutInput],
    commit_rows: &HashMap<&str, usize>,
) -> HashSet<EdgeId> {
    let mut protected = HashSet::new();
    let mut current = entries
        .iter()
        .position(|entry| entry.is_working_copy)
        .or((!entries.is_empty()).then_some(0));

    while let Some(source_index) = current {
        let Some(first_parent_id) = entries[source_index].parents.first() else {
            break;
        };
        let Some((edge_index, edge)) =
            entries[source_index]
                .edges
                .iter()
                .enumerate()
                .find(|(_, edge)| {
                    edge.edge_type == EdgeType::Direct && edge.target == *first_parent_id
                })
        else {
            break;
        };
        let Some(&target_index) = commit_rows.get(edge.target.as_str()) else {
            break;
        };
        if !protected.insert(EdgeId {
            source_index,
            edge_index,
        }) {
            break;
        }
        current = Some(target_index);
    }
    protected
}

fn build_continuations(
    entries: &[DagLayoutInput],
    candidates: &[EdgeCandidate<'_>],
    cut_edges: &HashSet<EdgeId>,
) -> Vec<Vec<DagContinuation>> {
    let mut rows = vec![Vec::new(); entries.len()];
    for candidate in candidates
        .iter()
        .filter(|candidate| cut_edges.contains(&candidate.id))
    {
        let kind = match candidate.kind {
            DagEdgeKind::Direct => "direct",
            DagEdgeKind::Indirect => "indirect",
        };
        let key = format!("{}:{}:{kind}", candidate.source_id, candidate.target_id);
        rows[candidate.id.source_index].push(DagContinuation {
            key: key.clone(),
            edge_kind: candidate.kind,
            direction: DagContinuationDirection::Outgoing,
            related_commit_id: candidate.target_id.to_owned(),
        });
        if let Some(target_index) = candidate.target_index {
            rows[target_index].push(DagContinuation {
                key,
                edge_kind: candidate.kind,
                direction: DagContinuationDirection::Incoming,
                related_commit_id: candidate.source_id.to_owned(),
            });
        }
    }
    for continuations in &mut rows {
        continuations.sort_by(|left, right| left.key.cmp(&right.key));
    }
    rows
}
