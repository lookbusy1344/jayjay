//! Cheap structural invariants for a computed [`DagLayout`], enforced with `debug_assert!` at the
//! layout boundary (see `docs/jj-log-dag-parity-plan.md`). Failing here means malformed data would
//! otherwise cross the UniFFI boundary and appear in a shell as misleading graph artwork, so this
//! fails loudly near its Rust source instead.

use std::collections::HashSet;

use super::layout::DagLayoutInput;
use super::row_shape::{DagLayout, DagLinkCell, DagVerticalCell, band_column_count};
use crate::types::EdgeType;

impl DagLayout {
    /// Validates every invariant a shell relies on without reinterpreting graph topology. Returns
    /// the first violation found, described for a test failure or a debug-build panic message.
    pub(crate) fn validate(
        &self,
        entries: &[DagLayoutInput],
        synthetic_elided_nodes: bool,
    ) -> Result<(), String> {
        if self.rows.len() != entries.len() {
            return Err(format!(
                "expected one real row per input entry: got {} rows for {} entries",
                self.rows.len(),
                entries.len()
            ));
        }
        for (row, entry) in self.rows.iter().zip(entries) {
            if row.commit_id != entry.commit_id {
                return Err(format!(
                    "real row order diverged from input order: expected {:?}, found {:?}",
                    entry.commit_id, row.commit_id
                ));
            }
        }

        let mut seen_commit_ids = HashSet::with_capacity(self.rows.len());
        for row in &self.rows {
            if !seen_commit_ids.insert(row.commit_id.as_str()) {
                return Err(format!(
                    "duplicate real commit id in layout: {:?}",
                    row.commit_id
                ));
            }
        }

        for (row, entry) in self.rows.iter().zip(entries) {
            // With synthetic elision off, `jj log` draws the indirect edge itself and emits no
            // synthetic row, so the row must own no bands at all.
            let expected_targets: Vec<&str> = entry
                .edges
                .iter()
                .filter(|edge| synthetic_elided_nodes && edge.edge_type == EdgeType::Indirect)
                .map(|edge| edge.target.as_str())
                .collect();
            let actual_targets = row
                .elisions_after
                .iter()
                .map(|band| band.target_commit_id.as_str())
                .collect::<Vec<_>>();
            if actual_targets != expected_targets {
                return Err(format!(
                    "row {:?} owns bands {actual_targets:?}, but its indirect edges call for \
                     {expected_targets:?} in that order",
                    row.commit_id
                ));
            }

            let mut widest_in_row = validate_band(
                &format!("row {:?}", row.commit_id),
                row.node_column,
                &row.node_line,
                row.link_line.as_deref(),
                &row.termination_columns,
                &row.pad_line,
                self.logical_column_count,
            )?;
            for band in &row.elisions_after {
                if band.target_commit_id.is_empty() {
                    return Err(format!(
                        "elision band owned by {:?} has an empty target commit id",
                        row.commit_id
                    ));
                }
                widest_in_row = widest_in_row.max(validate_band(
                    &format!(
                        "elision band owned by {:?} targeting {:?}",
                        row.commit_id, band.target_commit_id
                    ),
                    band.node_column,
                    &band.node_line,
                    band.link_line.as_deref(),
                    &band.termination_columns,
                    &band.pad_line,
                    self.logical_column_count,
                )?);
            }

            // A shell sizes the row's graph gutter from this, so it must cover every lane the row
            // and its bands draw — otherwise a lane falls outside the gutter and is never painted.
            if row.graph_column_count != widest_in_row {
                return Err(format!(
                    "row {:?} reports graph_column_count {} but its widest band occupies {}",
                    row.commit_id, row.graph_column_count, widest_in_row
                ));
            }
        }

        let widest_bands = self
            .rows
            .iter()
            .map(|row| row.elisions_after.len() as u32)
            .max()
            .unwrap_or(0);
        if self.widest_elision_band_count != widest_bands {
            return Err(format!(
                "layout reports widest_elision_band_count {} but its band-heaviest row owns {}",
                self.widest_elision_band_count, widest_bands
            ));
        }

        Ok(())
    }
}

fn validate_band(
    label: &str,
    node_column: u32,
    node_line: &[DagVerticalCell],
    link_line: Option<&[DagLinkCell]>,
    termination_columns: &[u32],
    pad_line: &[DagVerticalCell],
    logical_column_count: u32,
) -> Result<u32, String> {
    // The renderer draws the row's own node glyph at `node_column` separately from the node
    // line, so that cell always decodes as `Empty` there — a non-empty cell at `node_column`
    // would mean a pass-through connector is drawn on top of this row's own node.
    match node_line.get(node_column as usize) {
        Some(DagVerticalCell::Empty) => {}
        Some(_) => {
            return Err(format!(
                "{label}: node_column {node_column} carries a pass-through connector instead of \
                 the row's own node cell"
            ));
        }
        None => {
            return Err(format!(
                "{label}: node_column {node_column} is out of bounds for a node line of length {}",
                node_line.len()
            ));
        }
    }

    let columns = band_column_count(
        node_column,
        node_line,
        link_line,
        termination_columns,
        pad_line,
    );
    if columns > logical_column_count {
        return Err(format!(
            "{label}: column {} is not below logical_column_count {logical_column_count}",
            columns - 1
        ));
    }

    Ok(columns)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag::row_shape::{DagEdgeKind, DagElisionBand, DagRowShape};
    use crate::types::GraphEdge;

    /// Bandless inputs: the shapes under test are built by hand, so only their commit ids and
    /// (for the band-order case) their edges have to line up with the layout.
    fn inputs(commit_ids: &[&str]) -> Vec<DagLayoutInput> {
        commit_ids
            .iter()
            .map(|commit_id| DagLayoutInput {
                commit_id: (*commit_id).to_owned(),
                edges: vec![],
                is_working_copy: false,
            })
            .collect()
    }

    fn simple_row(commit_id: &str) -> DagRowShape {
        DagRowShape {
            commit_id: commit_id.to_owned(),
            node_column: 0,
            incoming: None,
            node_line: vec![DagVerticalCell::Empty],
            link_line: None,
            termination_columns: vec![],
            pad_line: vec![DagVerticalCell::Empty],
            elisions_after: vec![],
            graph_column_count: 1,
        }
    }

    fn band(target_commit_id: &str) -> DagElisionBand {
        DagElisionBand {
            target_commit_id: target_commit_id.to_owned(),
            node_column: 0,
            node_line: vec![DagVerticalCell::Empty],
            link_line: None,
            termination_columns: vec![],
            pad_line: vec![DagVerticalCell::Empty],
        }
    }

    fn indirect_edge(target: &str) -> GraphEdge {
        GraphEdge {
            target: target.to_owned(),
            edge_type: EdgeType::Indirect,
        }
    }

    /// Derives the two summary fields from the hand-built rows, so a test that widens a line or
    /// attaches a band fails on the invariant it targets rather than on a stale derived count.
    fn layout(mut rows: Vec<DagRowShape>, logical_column_count: u32) -> DagLayout {
        for row in &mut rows {
            row.graph_column_count = row.elisions_after.iter().fold(
                band_column_count(
                    row.node_column,
                    &row.node_line,
                    row.link_line.as_deref(),
                    &row.termination_columns,
                    &row.pad_line,
                ),
                |widest, band| {
                    widest.max(band_column_count(
                        band.node_column,
                        &band.node_line,
                        band.link_line.as_deref(),
                        &band.termination_columns,
                        &band.pad_line,
                    ))
                },
            );
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

    #[test]
    fn well_formed_layout_passes() {
        let layout = layout(vec![simple_row("a"), simple_row("b")], 1);
        assert_eq!(layout.validate(&inputs(&["a", "b"]), true), Ok(()));
    }

    #[test]
    fn a_graph_column_count_narrower_than_the_row_is_rejected() {
        let mut row = simple_row("a");
        row.pad_line = vec![DagVerticalCell::Direct; 2];
        let mut layout = layout(vec![row], 2);
        assert_eq!(layout.rows[0].graph_column_count, 2);

        layout.rows[0].graph_column_count = 1;
        assert!(layout.validate(&inputs(&["a"]), true).is_err());
    }

    #[test]
    fn a_widest_elision_band_count_that_understates_a_row_is_rejected() {
        let mut row = simple_row("a");
        row.elisions_after = vec![band("target")];
        let mut layout = layout(vec![row], 1);
        layout.widest_elision_band_count = 0;
        let mut input = inputs(&["a"]);
        input[0].edges = vec![indirect_edge("target")];
        assert!(layout.validate(&input, true).is_err());
    }

    #[test]
    fn row_count_mismatch_is_rejected() {
        let layout = layout(vec![simple_row("a")], 1);
        assert!(layout.validate(&inputs(&["a", "b"]), true).is_err());
    }

    #[test]
    fn row_order_diverging_from_input_order_is_rejected() {
        let layout = layout(vec![simple_row("b"), simple_row("a")], 1);
        assert!(layout.validate(&inputs(&["a", "b"]), true).is_err());
    }

    #[test]
    fn duplicate_commit_ids_are_rejected() {
        let layout = layout(vec![simple_row("a"), simple_row("a")], 1);
        assert!(layout.validate(&inputs(&["a", "a"]), true).is_err());
    }

    #[test]
    fn node_column_with_a_pass_through_connector_is_rejected() {
        let mut row = simple_row("a");
        row.node_line = vec![DagVerticalCell::Direct];
        let layout = layout(vec![row], 1);
        assert!(layout.validate(&inputs(&["a"]), true).is_err());
    }

    #[test]
    fn node_column_pointing_past_the_node_line_is_rejected() {
        let mut row = simple_row("a");
        row.node_column = 1;
        let layout = layout(vec![row], 2);
        assert!(layout.validate(&inputs(&["a"]), true).is_err());
    }

    #[test]
    fn column_reference_at_or_past_logical_column_count_is_rejected() {
        let mut row = simple_row("a");
        row.termination_columns = vec![2];
        let layout = layout(vec![row], 2);
        assert!(layout.validate(&inputs(&["a"]), true).is_err());
    }

    #[test]
    fn elision_band_with_empty_target_is_rejected() {
        let mut row = simple_row("a");
        row.elisions_after.push(band(""));
        let mut entries = inputs(&["a"]);
        entries[0].edges = vec![indirect_edge("")];
        let layout = layout(vec![row], 1);
        assert!(layout.validate(&entries, true).is_err());
    }

    /// The renderer attaches each synthetic row to whichever real row it followed. A band landing
    /// on the wrong owner would draw a real elision under an unrelated change.
    #[test]
    fn a_band_its_owner_has_no_indirect_edge_for_is_rejected() {
        let mut row = simple_row("a");
        row.elisions_after.push(band("t"));
        let layout = layout(vec![row], 1);
        assert!(layout.validate(&inputs(&["a"]), true).is_err());
    }

    #[test]
    fn bands_out_of_their_edge_order_are_rejected() {
        let mut row = simple_row("a");
        row.elisions_after = vec![band("second"), band("first")];
        let mut entries = inputs(&["a"]);
        entries[0].edges = vec![indirect_edge("first"), indirect_edge("second")];
        let layout = layout(vec![row], 1);
        assert!(layout.validate(&entries, true).is_err());
    }

    /// With `ui.log-synthetic-elided-nodes` off, `jj log` draws the indirect edge itself and emits
    /// no synthetic row, so a band would be JayJay's own invention.
    #[test]
    fn a_band_with_synthetic_elision_disabled_is_rejected() {
        let mut row = simple_row("a");
        row.elisions_after.push(band("t"));
        let mut entries = inputs(&["a"]);
        entries[0].edges = vec![indirect_edge("t")];
        let layout = layout(vec![row], 1);
        assert!(layout.validate(&entries, false).is_err());
    }

    /// A real large repository (see `docs/jj-log-dag-parity-plan.md`) produces rows where a
    /// column both terminates (a lane ends here) and carries a continuing vertical link — the
    /// renderer legitimately reuses a freed column within the same row for a different edge. An
    /// earlier version of this validator rejected that as a contradiction, which crashed the app
    /// on every real repository wide enough to reuse a column; this case guards against
    /// reintroducing that false positive.
    #[test]
    fn terminating_column_with_a_continuing_vertical_link_is_accepted() {
        let mut row = simple_row("a");
        row.termination_columns = vec![0];
        row.link_line = Some(vec![DagLinkCell {
            vertical: Some(DagEdgeKind::Direct),
            ..Default::default()
        }]);
        let layout = layout(vec![row], 1);
        assert_eq!(layout.validate(&inputs(&["a"]), true), Ok(()));
    }
}
