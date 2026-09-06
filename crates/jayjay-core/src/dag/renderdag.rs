//! Adapter from the `sapling-renderdag` crate's `GraphRowRenderer` to JayJay's app-owned row shapes.
//!
//! No upstream type crosses this module boundary.

use std::collections::{HashMap, HashSet};

use renderdag::{Ancestor, GraphRow, GraphRowRenderer, LinkLine, NodeLine, PadLine, Renderer};

use super::layout::DagLayoutInput;
use super::render_nodes::{self, DagRenderNodeId, DagRenderTarget};
use super::row_shape::{
    DagEdgeKind, DagElisionBand, DagLayout, DagLinkCell, DagRowShape, DagVerticalCell,
    band_column_count,
};
use crate::types::EdgeType;

pub(super) fn render(entries: &[DagLayoutInput], synthetic_elided_nodes: bool) -> DagLayout {
    if synthetic_elided_nodes {
        render_with_synthetic_elisions(entries)
    } else {
        render_raw(entries)
    }
}

/// `ui.log-synthetic-elided-nodes = false`: an indirect edge stays a single dashed ancestor jump
/// straight to its target, exactly as `jj log` renders it in that configuration.
fn render_raw(entries: &[DagLayoutInput]) -> DagLayout {
    let mut renderer = GraphRowRenderer::<String>::new();
    let mut rows = Vec::with_capacity(entries.len());
    let mut logical_column_count = 0;
    let mut incoming = HashMap::new();

    for entry in entries {
        let commit_id = entry.commit_id.clone();
        let parents = entry.edges.iter().map(to_ancestor).collect();
        let row = renderer.next_row(commit_id.clone(), parents, String::new(), String::new());
        debug_assert_eq!(
            row.node, commit_id,
            "renderer emitted a row for a different commit than requested"
        );
        let shape = to_row_shape(row, incoming.remove(&commit_id));
        logical_column_count = logical_column_count.max(shape.graph_column_count);
        rows.push(shape);
        register_incoming_edges(&mut incoming, entry);
    }

    DagLayout {
        rows,
        logical_column_count,
        widest_elision_band_count: 0,
    }
}

/// `ui.log-synthetic-elided-nodes = true`: every unprojected indirect edge is expanded into
/// `S --direct--> synthetic(T) --direct--> T` before rendering (see `render_nodes`), and the
/// synthetic row is folded into the owning real row's `elisions_after` rather than becoming its
/// own top-level `DagRowShape`. This includes indirect targets beyond a progressive prefix, matching
/// `jj log --limit N`.
fn render_with_synthetic_elisions(entries: &[DagLayoutInput]) -> DagLayout {
    let render_nodes = render_nodes::expand_indirect_edges(entries);

    let mut renderer = GraphRowRenderer::<String>::new();
    let mut rows: Vec<DagRowShape> = Vec::with_capacity(entries.len());
    let mut logical_column_count = 0;
    let mut incoming = HashMap::new();
    let mut source_index = 0usize;

    for node in render_nodes {
        let key = renderer_key(&node.id);
        let parents = node
            .parents
            .iter()
            .map(|target| match target {
                DagRenderTarget::Node(id) => Ancestor::Parent(renderer_key(id)),
                DagRenderTarget::Missing => Ancestor::Anonymous,
            })
            .collect();
        let row = renderer.next_row(key.clone(), parents, String::new(), String::new());
        debug_assert_eq!(
            row.node, key,
            "renderer emitted a row for a different node than requested"
        );

        match node.id {
            DagRenderNodeId::Change(commit_id) => {
                let shape = to_row_shape(row, incoming.remove(&commit_id));
                logical_column_count = logical_column_count.max(shape.graph_column_count);
                rows.push(shape);
                register_direct_incoming_edges(&mut incoming, &entries[source_index]);
                source_index += 1;
            }
            DagRenderNodeId::Elision {
                target_commit_id, ..
            } => {
                let (band, band_columns) = to_elision_band(row, target_commit_id.clone());
                logical_column_count = logical_column_count.max(band_columns);
                incoming
                    .entry(target_commit_id)
                    .and_modify(|current| *current = DagEdgeKind::Direct)
                    .or_insert(DagEdgeKind::Direct);
                let owner = rows
                    .last_mut()
                    .expect("an elision band always follows its owning real row");
                owner.graph_column_count = owner.graph_column_count.max(band_columns);
                owner.elisions_after.push(band);
            }
        }
    }

    let widest_elision_band_count = rows
        .iter()
        .map(|row| row.elisions_after.len() as u32)
        .max()
        .unwrap_or(0);

    DagLayout {
        rows,
        logical_column_count,
        widest_elision_band_count,
    }
}

/// Renders the unprojected sequence as `jj log`'s own ASCII text, for the real-CLI parity test
/// only (see `docs/jj-log-dag-parity-plan.md`'s "Comparing command output to JayJay structure"):
/// the same node/parent sequence `render` feeds `GraphRowRenderer`, fed instead to the identical
/// crate's `AsciiRenderer` with each node's own commit id (or `(elided revisions)`) as content, so
/// the result is byte-for-byte comparable with the real command's `-T 'commit_id ++ "\n"'` output.
pub(super) fn render_ascii_unprojected(
    entries: &[DagLayoutInput],
    synthetic_elided_nodes: bool,
) -> String {
    let mut renderer = GraphRowRenderer::<String>::new()
        .output()
        .with_min_row_height(1)
        .build_ascii();
    let mut text = String::new();
    if synthetic_elided_nodes {
        let working_copy: HashSet<&str> = entries
            .iter()
            .filter(|entry| entry.is_working_copy)
            .map(|entry| entry.commit_id.as_str())
            .collect();
        for node in render_nodes::expand_indirect_edges(entries) {
            let key = renderer_key(&node.id);
            let parents = node
                .parents
                .iter()
                .map(|target| match target {
                    DagRenderTarget::Node(id) => Ancestor::Parent(renderer_key(id)),
                    DagRenderTarget::Missing => Ancestor::Anonymous,
                })
                .collect();
            let glyph = glyph_for(&node.id, &working_copy);
            let message = match &node.id {
                DagRenderNodeId::Change(commit_id) => commit_id.clone(),
                DagRenderNodeId::Elision { .. } => "(elided revisions)".to_owned(),
            };
            text.push_str(&renderer.next_row(key, parents, glyph, message));
        }
    } else {
        for entry in entries {
            let parents = entry.edges.iter().map(to_ancestor).collect();
            let glyph = if entry.is_working_copy { "@" } else { "o" }.to_owned();
            text.push_str(&renderer.next_row(
                entry.commit_id.clone(),
                parents,
                glyph,
                entry.commit_id.clone(),
            ));
        }
    }
    text.lines().collect::<Vec<_>>().join("\n")
}

fn glyph_for(id: &DagRenderNodeId, working_copy: &HashSet<&str>) -> String {
    match id {
        DagRenderNodeId::Elision { .. } => "~",
        DagRenderNodeId::Change(commit_id) => {
            if working_copy.contains(commit_id.as_str()) {
                "@"
            } else {
                "o"
            }
        }
    }
    .to_owned()
}

fn renderer_key(id: &DagRenderNodeId) -> String {
    match id {
        DagRenderNodeId::Change(commit_id) => commit_id.clone(),
        DagRenderNodeId::Elision { occurrence, .. } => format!("\0elision:{occurrence}"),
    }
}

/// Incoming-edge bookkeeping for the synthetic path. An indirect edge's incoming registration
/// happens when its elision band is emitted instead.
fn register_direct_incoming_edges(
    incoming: &mut HashMap<String, DagEdgeKind>,
    entry: &DagLayoutInput,
) {
    for edge in &entry.edges {
        if edge.edge_type != EdgeType::Direct {
            continue;
        }
        incoming
            .entry(edge.target.clone())
            .and_modify(|current| *current = DagEdgeKind::Direct)
            .or_insert(DagEdgeKind::Direct);
    }
}

fn register_incoming_edges(incoming: &mut HashMap<String, DagEdgeKind>, entry: &DagLayoutInput) {
    for edge in &entry.edges {
        let kind = match edge.edge_type {
            EdgeType::Direct => DagEdgeKind::Direct,
            EdgeType::Indirect => DagEdgeKind::Indirect,
            EdgeType::Missing => continue,
        };
        incoming
            .entry(edge.target.clone())
            .and_modify(|current| {
                if kind == DagEdgeKind::Direct {
                    *current = kind;
                }
            })
            .or_insert(kind);
    }
}

fn to_ancestor(edge: &crate::types::GraphEdge) -> Ancestor<String> {
    match edge.edge_type {
        EdgeType::Direct => Ancestor::Parent(edge.target.clone()),
        EdgeType::Indirect => Ancestor::Ancestor(edge.target.clone()),
        EdgeType::Missing => Ancestor::Anonymous,
    }
}

struct BandGeometry {
    node_column: u32,
    node_line: Vec<DagVerticalCell>,
    link_line: Option<Vec<DagLinkCell>>,
    termination_columns: Vec<u32>,
    pad_line: Vec<DagVerticalCell>,
    column_count: u32,
}

fn band_geometry(row: &GraphRow<String>) -> BandGeometry {
    let node_column = node_column(&row.node_line);
    let node_line: Vec<DagVerticalCell> = row.node_line.iter().map(to_vertical_cell).collect();
    let pad_line = row
        .pad_lines
        .iter()
        .map(pad_to_vertical_cell)
        .collect::<Vec<_>>();
    let link_line = row
        .link_line
        .as_ref()
        .map(|cells| cells.iter().map(to_link_cell).collect());
    let termination_columns: Vec<u32> = row
        .term_line
        .as_ref()
        .map(|flags| {
            flags
                .iter()
                .enumerate()
                .filter_map(|(column, &terminates)| terminates.then_some(column as u32))
                .collect()
        })
        .unwrap_or_default();
    let column_count = band_column_count(
        node_column,
        &node_line,
        link_line.as_deref(),
        &termination_columns,
        &pad_line,
    );
    BandGeometry {
        node_column,
        node_line,
        link_line,
        termination_columns,
        pad_line,
        column_count,
    }
}

fn to_row_shape(row: GraphRow<String>, incoming: Option<DagEdgeKind>) -> DagRowShape {
    let BandGeometry {
        node_column,
        node_line,
        link_line,
        termination_columns,
        pad_line,
        column_count,
    } = band_geometry(&row);

    DagRowShape {
        commit_id: row.node,
        node_column,
        graph_column_count: column_count,
        incoming,
        node_line,
        link_line,
        termination_columns,
        pad_line,
        elisions_after: Vec::new(),
    }
}

/// Returns the band and the logical columns it occupies, which its owning row's gutter must cover.
fn to_elision_band(row: GraphRow<String>, target_commit_id: String) -> (DagElisionBand, u32) {
    let BandGeometry {
        node_column,
        node_line,
        link_line,
        termination_columns,
        pad_line,
        column_count,
    } = band_geometry(&row);
    (
        DagElisionBand {
            target_commit_id,
            node_column,
            node_line,
            link_line,
            termination_columns,
            pad_line,
        },
        column_count,
    )
}

/// The single column carrying the node glyph. The renderer emits exactly one `NodeLine::Node` per row; anything else is a renderer-contract violation, not runtime data, so we surface it in debug builds and fall back to the node column in release.
fn node_column(node_line: &[NodeLine]) -> u32 {
    let mut nodes = node_line
        .iter()
        .enumerate()
        .filter(|(_, cell)| **cell == NodeLine::Node)
        .map(|(column, _)| column as u32);
    let column = nodes.next();
    debug_assert!(column.is_some(), "renderer row has no node cell");
    debug_assert!(
        nodes.next().is_none(),
        "renderer row has more than one node cell"
    );
    column.unwrap_or(0)
}

fn to_vertical_cell(line: &NodeLine) -> DagVerticalCell {
    match line {
        NodeLine::Blank | NodeLine::Node => DagVerticalCell::Empty,
        NodeLine::Parent => DagVerticalCell::Direct,
        NodeLine::Ancestor => DagVerticalCell::Indirect,
    }
}

fn pad_to_vertical_cell(line: &PadLine) -> DagVerticalCell {
    match line {
        PadLine::Blank => DagVerticalCell::Empty,
        PadLine::Parent => DagVerticalCell::Direct,
        PadLine::Ancestor => DagVerticalCell::Indirect,
    }
}

fn to_link_cell(flags: &LinkLine) -> DagLinkCell {
    DagLinkCell {
        vertical: edge_kind(*flags, LinkLine::VERT_PARENT, LinkLine::VERT_ANCESTOR),
        horizontal: edge_kind(*flags, LinkLine::HORIZ_PARENT, LinkLine::HORIZ_ANCESTOR),
        left_fork: edge_kind(
            *flags,
            LinkLine::LEFT_FORK_PARENT,
            LinkLine::LEFT_FORK_ANCESTOR,
        ),
        right_fork: edge_kind(
            *flags,
            LinkLine::RIGHT_FORK_PARENT,
            LinkLine::RIGHT_FORK_ANCESTOR,
        ),
        left_merge: edge_kind(
            *flags,
            LinkLine::LEFT_MERGE_PARENT,
            LinkLine::LEFT_MERGE_ANCESTOR,
        ),
        right_merge: edge_kind(
            *flags,
            LinkLine::RIGHT_MERGE_PARENT,
            LinkLine::RIGHT_MERGE_ANCESTOR,
        ),
        is_child: flags.contains(LinkLine::CHILD),
    }
}

fn edge_kind(flags: LinkLine, direct: LinkLine, indirect: LinkLine) -> Option<DagEdgeKind> {
    if flags.contains(direct) {
        Some(DagEdgeKind::Direct)
    } else if flags.contains(indirect) {
        Some(DagEdgeKind::Indirect)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        ChangeInfo, CommitAuthor, GraphEdge, GraphEntry, NewChangeEligibility, ShortId,
    };

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

    fn direct(commit_id: &str, parents: &[&str]) -> GraphEntry {
        entry(
            commit_id,
            &parents
                .iter()
                .map(|p| (*p, EdgeType::Direct))
                .collect::<Vec<_>>(),
        )
    }

    fn compute(entries: &[GraphEntry]) -> DagLayout {
        DagLayout::compute(entries, false)
    }

    fn columns(layout: &DagLayout, commit_id: &str) -> u32 {
        layout
            .row(commit_id)
            .unwrap_or_else(|| panic!("no row for {commit_id}"))
            .node_column
    }

    #[test]
    fn linear_history_stays_in_one_column() {
        let entries = vec![direct("C", &["B"]), direct("B", &["A"]), direct("A", &[])];
        let layout = compute(&entries);

        assert_eq!(layout.logical_column_count, 1);
        assert_eq!(columns(&layout, "C"), 0);
        assert_eq!(columns(&layout, "B"), 0);
        assert_eq!(columns(&layout, "A"), 0);
    }

    #[test]
    fn disconnected_heads_reuse_the_freed_column() {
        let entries = vec![direct("D", &[]), direct("C", &[])];
        let layout = compute(&entries);

        // C's row is emitted after D's column is freed, so it reuses column 0 rather than opening a second lane — matching `jj log`'s own output for two adjacent, unrelated heads.
        assert_eq!(columns(&layout, "D"), 0);
        assert_eq!(columns(&layout, "C"), 0);
        assert_eq!(layout.logical_column_count, 1);
    }

    #[test]
    fn fork_then_merge_matches_renderer_column_transitions() {
        // D forks into B and C, both reconverge on A.
        let entries = vec![
            direct("D", &["B", "C"]),
            direct("B", &["A"]),
            direct("C", &["A"]),
            direct("A", &[]),
        ];
        let layout = compute(&entries);

        assert_eq!(columns(&layout, "D"), 0);
        assert_eq!(columns(&layout, "B"), 0);
        assert_eq!(columns(&layout, "C"), 1);
        assert_eq!(columns(&layout, "A"), 0);
        assert_eq!(layout.logical_column_count, 2);

        let d_row = &layout.rows[0];
        let link = d_row
            .link_line
            .as_ref()
            .expect("fork row needs a link line");
        assert!(
            link[1].left_fork.is_some(),
            "column 1 should fork left toward D"
        );
    }

    #[test]
    fn merge_keeps_an_off_prefix_parent_in_the_native_layout() {
        let entries = vec![
            entry(
                "merge",
                &[("parent", EdgeType::Direct), ("off-page", EdgeType::Direct)],
            ),
            direct("parent", &[]),
        ];

        let layout = compute(&entries);
        let row = layout.row("merge").expect("merge row");

        assert_eq!(
            row.pad_line[row.node_column as usize],
            DagVerticalCell::Direct
        );
        assert_eq!(layout.logical_column_count, 2);
        assert!(row.termination_columns.is_empty());
    }

    #[test]
    fn parent_outside_the_prefix_remains_a_native_lane() {
        let entries = vec![direct("C", &["outside-parent"])];

        let layout = compute(&entries);

        let row = &layout.rows[0];
        assert!(row.termination_columns.is_empty());
        assert_eq!(
            row.pad_line[row.node_column as usize],
            DagVerticalCell::Direct
        );
    }

    #[test]
    fn merge_parents_outside_the_prefix_remain_native_lanes() {
        let entries = vec![direct(
            "merge",
            &["outside-first-parent", "outside-second-parent"],
        )];

        let layout = compute(&entries);

        assert_eq!(layout.logical_column_count, 2);
    }

    #[test]
    fn a_long_connector_is_never_cut() {
        // The lane must stay reserved through every filler row, matching the real CLI's own
        // behaviour for an elided-then-unrelated-head adjacency.
        let mut entries = vec![direct("head", &[]), direct("long-source", &["target"])];
        entries.extend((0..12).map(|index| direct(&format!("filler-{index}"), &[])));
        entries.push(direct("target", &[]));

        let layout = compute(&entries);

        assert_eq!(columns(&layout, "long-source"), columns(&layout, "target"));
    }

    /// A graph wider than the eight lanes JayJay once projected onto keeps every native column
    /// and every long connector, exactly as `jj log` draws it.
    #[test]
    fn a_wide_graph_keeps_every_native_lane_and_long_connector() {
        let mut entries = (0..9)
            .map(|index| direct(&format!("source-{index}"), &[&format!("target-{index}")]))
            .collect::<Vec<_>>();
        entries.extend((0..9).map(|index| direct(&format!("target-{index}"), &[])));

        let layout = compute(&entries);

        assert_eq!(layout.rows.len(), entries.len());
        assert_eq!(layout.logical_column_count, 9);
        for index in 0..9 {
            assert_eq!(
                columns(&layout, &format!("source-{index}")),
                columns(&layout, &format!("target-{index}"))
            );
        }
    }

    #[test]
    fn working_copy_first_parent_spine_is_never_cut() {
        let mut entries = vec![direct("working-copy", &["parent"])];
        entries[0].change.is_working_copy = true;
        entries.extend((0..13).map(|index| direct(&format!("filler-{index}"), &[])));
        entries.push(direct("parent", &[]));

        let layout = compute(&entries);

        assert_eq!(
            layout.row("parent").expect("parent row").incoming,
            Some(DagEdgeKind::Direct)
        );
    }

    #[test]
    fn every_parent_remains_in_a_wide_mixed_edge_graph() {
        let extra_parents = (0..9)
            .map(|index| format!("extra-{index}"))
            .collect::<Vec<_>>();
        let mut edges = vec![
            ("indirect-first-parent", EdgeType::Indirect),
            ("direct-second-parent", EdgeType::Direct),
        ];
        edges.extend(extra_parents.iter().map(|p| (p.as_str(), EdgeType::Direct)));
        let mut entries = vec![entry("working-copy", &edges)];
        entries[0].change.is_working_copy = true;
        entries.push(direct("indirect-first-parent", &[]));
        entries.push(direct("direct-second-parent", &[]));
        entries.extend(extra_parents.iter().map(|p| direct(p, &[])));

        let layout = compute(&entries);

        assert_eq!(
            layout
                .row("direct-second-parent")
                .expect("direct parent")
                .incoming,
            Some(DagEdgeKind::Direct)
        );
    }

    #[test]
    fn adjacent_first_parent_edge_survives_a_wide_row() {
        let parents = (0..9).map(|index| format!("p{index}")).collect::<Vec<_>>();
        let parent_refs = parents.iter().map(String::as_str).collect::<Vec<_>>();
        let mut entries = vec![direct("head", &[]), direct("merge", &parent_refs)];
        entries.extend(parents.iter().map(|parent| direct(parent, &[])));

        let layout = compute(&entries);

        assert_eq!(
            layout.row("p0").expect("first parent row").incoming,
            Some(DagEdgeKind::Direct)
        );
    }

    #[test]
    fn native_layout_is_deterministic_and_preserves_semantic_edges() {
        let mut entries = (0..9)
            .map(|index| direct(&format!("source-{index}"), &[&format!("target-{index}")]))
            .collect::<Vec<_>>();
        entries.extend((0..9).map(|index| direct(&format!("target-{index}"), &[])));
        let original_edges = entries
            .iter()
            .map(|entry| {
                entry
                    .edges
                    .iter()
                    .map(|edge| (edge.target.clone(), edge.edge_type))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();

        let first = compute(&entries);
        let second = compute(&entries);

        assert_eq!(first, second);
        assert_eq!(
            entries
                .iter()
                .map(|entry| {
                    entry
                        .edges
                        .iter()
                        .map(|edge| (edge.target.clone(), edge.edge_type))
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>(),
            original_edges
        );
    }

    #[test]
    fn wide_direct_indirect_and_missing_edges_remain_distinguishable() {
        const INDIRECT_PARENT_COUNT: usize = 9;
        const DIRECT_PARENT_COUNT: usize = 9;

        let targets = (0..INDIRECT_PARENT_COUNT + DIRECT_PARENT_COUNT)
            .map(|index| format!("outside-{index}"))
            .collect::<Vec<_>>();
        let mut edges = targets
            .iter()
            .enumerate()
            .map(|(index, target)| {
                (
                    target.as_str(),
                    if index < INDIRECT_PARENT_COUNT {
                        EdgeType::Indirect
                    } else {
                        EdgeType::Direct
                    },
                )
            })
            .collect::<Vec<_>>();
        edges.push(("missing", EdgeType::Missing));
        let entries = vec![entry("source", &edges)];

        let layout = compute(&entries);
        let row = &layout.rows[0];

        assert!(row.pad_line.contains(&DagVerticalCell::Direct));
        assert!(row.pad_line.contains(&DagVerticalCell::Indirect));
        assert_eq!(row.termination_columns.len(), 1);
        assert_eq!(layout.logical_column_count as usize, targets.len() + 1);
    }

    #[test]
    fn wide_off_prefix_connectors_preserve_every_native_lane() {
        const PARENT_COUNT: usize = 12;

        let targets = (0..PARENT_COUNT)
            .map(|index| format!("target-{index}"))
            .collect::<Vec<_>>();
        let edges = targets
            .iter()
            .enumerate()
            .map(|(index, target)| {
                (
                    target.as_str(),
                    if index == 0 {
                        EdgeType::Direct
                    } else {
                        EdgeType::Indirect
                    },
                )
            })
            .collect::<Vec<_>>();
        let layout = compute(&[entry("source", &edges)]);

        assert_eq!(layout.logical_column_count as usize, PARENT_COUNT);
    }

    #[test]
    fn octopus_merge_keeps_every_parent_column() {
        let entries = vec![
            direct("merge", &["p0", "p1", "p2", "p3", "p4", "p5"]),
            direct("p5", &[]),
            direct("p4", &[]),
            direct("p3", &[]),
            direct("p2", &[]),
            direct("p1", &[]),
            direct("p0", &[]),
        ];
        let layout = compute(&entries);

        assert_eq!(layout.logical_column_count, 6);
        assert_eq!(columns(&layout, "p0"), 0);
        assert_eq!(columns(&layout, "p5"), 5);
    }

    #[test]
    fn under_budget_terminal_octopus_merge_keeps_off_prefix_parents() {
        let layout = compute(&[direct("merge", &["p0", "p1", "p2", "p3", "p4", "p5"])]);
        let row = &layout.rows[0];

        assert!(row.termination_columns.is_empty());
        assert_eq!(layout.logical_column_count, 6);
    }

    #[test]
    fn interleaved_heads_reuse_columns_per_renderer_not_first_free_heuristic() {
        // Two independent forks interleaved: X forks x0/x1, then Y forks y0/y1, both x0 and y0 free their columns before the other fork's second branch lands.
        let entries = vec![
            direct("X", &["x0", "x1"]),
            direct("x0", &[]),
            direct("Y", &["y0", "y1"]),
            direct("y0", &[]),
            direct("x1", &[]),
            direct("y1", &[]),
        ];
        let layout = compute(&entries);

        assert_eq!(columns(&layout, "X"), 0);
        assert_eq!(columns(&layout, "x0"), 0);
        assert_eq!(columns(&layout, "Y"), 0);
        // y0 reuses column 0, freed by x0; y1 keeps the column X's second parent left open.
        assert_eq!(columns(&layout, "y0"), 0);
        assert_eq!(columns(&layout, "x1"), 1);
        assert_eq!(columns(&layout, "y1"), 2);
    }

    #[test]
    fn single_parent_lane_move_is_expressed_by_the_link_line() {
        let entries = vec![
            direct("X", &["A", "P"]),
            direct("A", &[]),
            direct("B", &["P"]),
            direct("P", &[]),
        ];

        let layout = compute(&entries);
        let link = layout.rows[2]
            .link_line
            .as_ref()
            .expect("moving P from column 1 to B's column 0 needs a link line");

        assert_eq!(columns(&layout, "B"), 0);
        assert_eq!(columns(&layout, "P"), 0);
        assert_eq!(link[0].right_fork, Some(DagEdgeKind::Direct));
        assert_eq!(link[1].left_merge, Some(DagEdgeKind::Direct));
    }

    #[test]
    fn indirect_edges_stay_distinguishable_from_direct_edges() {
        let entries = vec![entry("C", &[("A", EdgeType::Indirect)]), entry("A", &[])];
        let layout = compute(&entries);

        let c_row = &layout.rows[0];
        assert_eq!(c_row.pad_line[0], DagVerticalCell::Indirect);
        assert_eq!(layout.rows[1].incoming, Some(DagEdgeKind::Indirect));
    }

    #[test]
    fn direct_incoming_edge_wins_when_a_target_is_also_an_indirect_ancestor() {
        let entries = vec![
            entry(
                "merge",
                &[("A", EdgeType::Indirect), ("A", EdgeType::Direct)],
            ),
            entry("A", &[]),
        ];

        let layout = compute(&entries);

        assert_eq!(layout.rows[1].incoming, Some(DagEdgeKind::Direct));
    }

    #[test]
    fn missing_edges_produce_a_termination_column() {
        let entries = vec![entry("A", &[("missing-parent", EdgeType::Missing)])];
        let layout = compute(&entries);

        let a_row = &layout.rows[0];
        assert_eq!(a_row.termination_columns, vec![0]);
        assert!(
            !layout
                .rows
                .iter()
                .any(|row| row.commit_id == "missing-parent"),
            "a missing parent must not produce its own row"
        );
    }

    #[test]
    fn omitted_synthetic_root_terminates_cleanly() {
        // The root commit is hidden from the stream; its child's edge must terminate rather than reserve a column that is never filled by a real row.
        let entries = vec![entry("only-commit", &[("hidden-root", EdgeType::Missing)])];
        let layout = compute(&entries);

        assert_eq!(layout.rows.len(), 1);
        assert_eq!(layout.rows[0].termination_columns, vec![0]);
    }

    fn compute_synthetic(entries: &[GraphEntry]) -> DagLayout {
        DagLayout::compute(entries, true)
    }

    #[test]
    fn synthetic_elided_nodes_disabled_keeps_the_raw_indirect_edge() {
        let entries = vec![entry("C", &[("A", EdgeType::Indirect)]), entry("A", &[])];
        let layout = compute(&entries);

        assert!(layout.row("C").expect("row").elisions_after.is_empty());
    }

    #[test]
    fn synthetic_elided_nodes_enabled_inserts_an_elision_band_owned_by_the_source() {
        let entries = vec![entry("C", &[("A", EdgeType::Indirect)]), entry("A", &[])];
        let layout = compute_synthetic(&entries);

        let c_row = layout.row("C").expect("source row");
        assert_eq!(c_row.elisions_after.len(), 1);
        assert_eq!(c_row.elisions_after[0].target_commit_id, "A");
        // The synthetic node is a direct hop, not a dashed ancestor jump: both C's edge into it
        // and its own edge into A draw as an ordinary parent line.
        assert_eq!(
            c_row.pad_line[c_row.node_column as usize],
            DagVerticalCell::Direct
        );
        let a_row = layout.row("A").expect("real target row");
        assert_eq!(a_row.incoming, Some(DagEdgeKind::Direct));
    }

    #[test]
    fn synthetic_elided_nodes_do_not_produce_their_own_top_level_row() {
        let entries = vec![entry("C", &[("A", EdgeType::Indirect)]), entry("A", &[])];
        let layout = compute_synthetic(&entries);

        assert_eq!(
            layout.rows.len(),
            2,
            "only real entries get a top-level row"
        );
        assert!(layout.row("\0elision:1").is_none());
    }

    #[test]
    fn an_elided_lineage_keeps_its_lane_alive_past_an_unrelated_interleaved_head() {
        // Mirrors the observed regression: an elided source's lane must stay reserved while an
        // unrelated visible head is emitted in between, rather than the unrelated head reusing
        // the freed column.
        let entries = vec![
            entry("source", &[("base", EdgeType::Indirect)]),
            direct("unrelated", &["base"]),
            direct("base", &[]),
        ];
        let layout = compute_synthetic(&entries);

        let source_row = layout.row("source").expect("source row");
        assert_eq!(source_row.elisions_after.len(), 1);
        let elision_column = source_row.elisions_after[0].node_column;
        assert_eq!(source_row.node_column, elision_column);
        assert_ne!(
            columns(&layout, "unrelated"),
            elision_column,
            "the unrelated head must not reuse the still-reserved elision lane"
        );
    }

    #[test]
    fn multiple_indirect_edges_on_one_source_get_bands_in_edge_order() {
        let entries = vec![
            entry(
                "merge",
                &[("A", EdgeType::Indirect), ("B", EdgeType::Indirect)],
            ),
            entry("A", &[]),
            entry("B", &[]),
        ];
        let layout = compute_synthetic(&entries);

        let merge_row = layout.row("merge").expect("merge row");
        let targets = merge_row
            .elisions_after
            .iter()
            .map(|band| band.target_commit_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(targets, vec!["A", "B"]);
    }

    #[test]
    fn width_never_removes_an_elision_band() {
        // Nine simultaneous indirect edges exceed the former lane budget; every one must retain
        // its synthetic elision band and native connector.
        let targets = (0..9)
            .map(|index| format!("target-{index}"))
            .collect::<Vec<_>>();
        let mut entries = (0..9)
            .map(|index| {
                entry(
                    &format!("source-{index}"),
                    &[(targets[index].as_str(), EdgeType::Indirect)],
                )
            })
            .collect::<Vec<_>>();
        entries.extend(targets.iter().map(|target| entry(target, &[])));

        let layout = compute_synthetic(&entries);

        for index in 0..9 {
            let row = layout
                .row(&format!("source-{index}"))
                .unwrap_or_else(|| panic!("missing source-{index}"));
            assert_eq!(
                row.elisions_after.len(),
                1,
                "source-{index} must keep its elision band regardless of width"
            );
        }
    }
}
