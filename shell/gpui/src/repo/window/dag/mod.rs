//! DAG row renderer for the repo sidebar: draws the row shapes the Rust renderer computes.

mod paint;
mod style;

use gpui::{
    AnyElement, Bounds, ContentMask, IntoElement, Pixels, Styled, canvas, fill, linear_color_stop,
    linear_gradient, point, px, rgb, size,
};
use jayjay_core::GraphEntry;
use jayjay_core::dag::{DagEdgeKind, DagLinkCell, DagRowShape, DagVerticalCell};

use crate::app::theme::Theme;

use paint::{LinePattern, paint_node, stroke_line_pattern, stroke_rounded_elbow_pattern};
use style::DagNodeStyle;

const PREFERRED_LANE_PITCH: f32 = 13.5;
const MINIMUM_LEGIBLE_LANE_PITCH: f32 = 10.0;
const ABSOLUTE_GRAPH_MAX_WIDTH: f32 = 74.0;
const MAX_SIDEBAR_FRACTION: f32 = 0.45;
const LEADING_PAD: f32 = 8.0;
const TRAILING_PAD: f32 = 6.0;
const HORIZONTAL_PADDING: f32 = LEADING_PAD + TRAILING_PAD;
const PREFERRED_NODE_RADIUS: f32 = 4.5;
/// Aligns with the first text line in the DAG row.
const NODE_TOP_OFFSET: f32 = 15.0;
const LINK_CENTER_FRACTION: f32 = 0.45;
const TERMINATION_STUB_FRACTION: f32 = 0.55;
const INDIRECT_EDGE_DASH_PATTERN: &[f32] = &[3.0, 3.0];
const MISSING_EDGE_DASH_PATTERN: &[f32] = &[2.0, 2.0];
/// Compact height for one synthetic `(elided revisions)` band, composed into its owning row below
/// the row's normal content. `uniform_list` gives every row the same height, so the sidebar sizes
/// that height for the widest band stack in the layout (see `dag_row_height`); a row with fewer
/// bands leaves the surplus above its own bands, keeping lanes continuous into the next row.
pub(in crate::repo::window) const ELISION_BAND_HEIGHT: f32 = 18.0;
const ELISION_GLYPH_RADIUS: f32 = 2.0;
/// Trailing-edge chevron marking a row whose native lanes are wider than the gutter budget.
const OVERFLOW_MARKER_SIZE: f32 = 6.0;
const OVERFLOW_MARKER_INSET: f32 = 4.0;
/// The clipped lanes dissolve into the row background across this trailing band instead of ending
/// on a hard vertical wall, and the chevron sits on the solid end of it.
const OVERFLOW_FADE_WIDTH: f32 = 22.0;
/// Horizontal offset between the two nested chevrons of the "lanes continue" marker.
const OVERFLOW_CHEVRON_GAP: f32 = 3.5;
/// Clear space between a visible node and the overflow marker.
const OVERFLOW_NODE_GAP: f32 = 2.0;

/// Where a row's own graph content ends and its trailing elision bands begin, given the row's
/// total canvas height. Pure so band placement can be unit-tested without a GPUI window.
pub(in crate::repo::window) fn main_row_bottom(total_height: f32, band_count: usize) -> f32 {
    total_height - ELISION_BAND_HEIGHT * band_count as f32
}

/// Maps logical columns to pixel positions for the sidebar's current width. One value is built per render and shared by every visible row, so column pitch never drifts row to row.
#[derive(Clone, Copy)]
pub(super) struct DagGeometry {
    pub lane_pitch: f32,
    pub node_radius: f32,
    /// The widest gutter a row may claim. Lanes past it are clipped behind an overflow marker so
    /// the change text is always legible, however wide the native graph gets.
    pub width_budget: f32,
}

/// How much gutter one row needs, and whether its native lanes outran the budget.
#[derive(Clone, Copy)]
pub(super) struct RowGraphWidth {
    pub width: f32,
    pub is_clipped: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeLayerContent {
    NodeOnly,
    NodeAndOverflow,
    OverflowOnly,
}

impl NodeLayerContent {
    fn includes_node(self) -> bool {
        self != Self::OverflowOnly
    }

    fn includes_overflow(self) -> bool {
        self != Self::NodeOnly
    }
}

fn node_layer_content(
    is_graph_clipped: bool,
    node_trailing_x: Pixels,
    overflow_leading_x: Pixels,
) -> NodeLayerContent {
    if !is_graph_clipped {
        return NodeLayerContent::NodeOnly;
    }
    if node_trailing_x + px(OVERFLOW_NODE_GAP) < overflow_leading_x {
        NodeLayerContent::NodeAndOverflow
    } else {
        NodeLayerContent::OverflowOnly
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinkComponent {
    Vertical(DagEdgeKind),
    Horizontal(DagEdgeKind),
    LeftFork(DagEdgeKind),
    RightFork(DagEdgeKind),
    LeftMerge(DagEdgeKind),
    RightMerge(DagEdgeKind),
}

#[derive(Clone, Copy)]
struct LinkBand {
    top: Pixels,
    center: Pixels,
    bottom: Pixels,
    half_pitch: Pixels,
}

impl LinkComponent {
    fn edge_kind(self) -> DagEdgeKind {
        match self {
            Self::Vertical(kind)
            | Self::Horizontal(kind)
            | Self::LeftFork(kind)
            | Self::RightFork(kind)
            | Self::LeftMerge(kind)
            | Self::RightMerge(kind) => kind,
        }
    }

    fn rounded_elbow(self, x: Pixels, band: LinkBand) -> Option<[gpui::Point<Pixels>; 3]> {
        match self {
            Self::LeftFork(_) => Some([
                point(x - band.half_pitch, band.center),
                point(x, band.center),
                point(x, band.bottom),
            ]),
            Self::RightFork(_) => Some([
                point(x + band.half_pitch, band.center),
                point(x, band.center),
                point(x, band.bottom),
            ]),
            Self::LeftMerge(_) => Some([
                point(x, band.top),
                point(x, band.center),
                point(x - band.half_pitch, band.center),
            ]),
            Self::RightMerge(_) => Some([
                point(x, band.top),
                point(x, band.center),
                point(x + band.half_pitch, band.center),
            ]),
            Self::Vertical(_) | Self::Horizontal(_) => None,
        }
    }
}

impl DagGeometry {
    pub(super) fn new(logical_column_count: u32, available_sidebar_width: f32) -> Self {
        let columns = logical_column_count.max(1) as f32;
        let width_budget =
            ABSOLUTE_GRAPH_MAX_WIDTH.min(available_sidebar_width * MAX_SIDEBAR_FRACTION);
        let compressed_pitch = (width_budget - HORIZONTAL_PADDING) / columns;
        let lane_pitch = compressed_pitch.clamp(MINIMUM_LEGIBLE_LANE_PITCH, PREFERRED_LANE_PITCH);
        Self {
            lane_pitch,
            node_radius: PREFERRED_NODE_RADIUS,
            width_budget,
        }
    }

    /// `jj log` starts each change's text after that row's own graph prefix, not after the widest
    /// prefix in the log, so the gutter is sized per row. The budget caps it: a row wider than the
    /// budget keeps every lane and column the renderer gave it — topology is never touched — but
    /// only the leading `width_budget` pixels of them are painted.
    pub(super) fn row_graph_width(&self, row_column_count: u32) -> RowGraphWidth {
        let natural = HORIZONTAL_PADDING + row_column_count.max(1) as f32 * self.lane_pitch;
        RowGraphWidth {
            width: natural.min(self.width_budget),
            is_clipped: natural > self.width_budget,
        }
    }
}

fn link_top(column: u32, node_column: u32, node_y: Pixels, node_radius: Pixels) -> Pixels {
    if column == node_column {
        node_y + node_radius
    } else {
        node_y
    }
}

pub(super) fn dag_column(
    entry: &GraphEntry,
    row: &DagRowShape,
    geometry: &DagGeometry,
    theme: &Theme,
    row_bg: u32,
) -> AnyElement {
    debug_assert_eq!(
        row.commit_id, entry.change.commit_id.id,
        "DAG row shape does not correspond to its entry; row index and entry index diverged"
    );
    let style = DagNodeStyle::resolve(&entry.change, theme, geometry.node_radius);
    let line_color = theme.dag_line;
    let edge_color = theme.dag_edge;
    let elision_glyph_color = theme.fg_dim;

    let node_column = row.node_column;
    let incoming = row.incoming;
    let node_line = row.node_line.clone();
    let link_line = row.link_line.clone();
    let pad_line = row.pad_line.clone();
    let termination_columns = row.termination_columns.clone();
    let bands = row.elisions_after.clone();

    let RowGraphWidth {
        width: graph_width,
        is_clipped,
    } = geometry.row_graph_width(row.graph_column_count);
    let lane_pitch = geometry.lane_pitch;
    let node_radius = style.radius;
    let node_top_offset = NODE_TOP_OFFSET + (theme.scaled_font_size(10.) - 10.) / 2.;
    let x_position =
        move |column: u32| -> f32 { LEADING_PAD + column as f32 * lane_pitch + lane_pitch / 2.0 };

    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let h = bounds.size.height;
            let oy = bounds.origin.y;
            let ox = bounds.origin.x;
            let column_center_x = |column: u32| -> Pixels { ox + px(x_position(column)) };

            let my_x = column_center_x(node_column);
            let node_y = oy + px(node_top_offset);
            let radius_px = px(node_radius);
            let row_bottom = oy + px(main_row_bottom(h.into(), bands.len()));
            let start_y = node_y + radius_px;
            let link_center_y = if link_line.is_some() {
                node_y + (row_bottom - node_y) * LINK_CENTER_FRACTION
            } else {
                node_y
            };
            let link_bottom_y = if link_line.is_some() {
                row_bottom.min(link_center_y + px(paint::CORNER_RADIUS))
            } else {
                node_y
            };

            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                // The node line is the renderer state above this row's transition band.
                for (column, cell) in node_line.iter().enumerate() {
                    let column = column as u32;
                    if column == node_column {
                        continue;
                    }
                    let Some(pattern) = line_pattern_for(cell) else {
                        continue;
                    };
                    stroke_line_pattern(
                        window,
                        column_center_x(column),
                        oy,
                        column_center_x(column),
                        node_y,
                        line_color,
                        pattern,
                    );
                }

                if let Some(kind) = incoming {
                    stroke_line_pattern(
                        window,
                        my_x,
                        oy,
                        my_x,
                        node_y - radius_px,
                        line_color,
                        line_pattern_for_kind(kind),
                    );
                }

                if let Some(link_line) = &link_line {
                    for (column, cell) in link_line.iter().enumerate() {
                        let column = column as u32;
                        let x = column_center_x(column);
                        let top = link_top(column, node_column, node_y, radius_px);
                        for component in link_components(cell) {
                            paint_link_component(
                                window,
                                component,
                                x,
                                LinkBand {
                                    top,
                                    center: link_center_y,
                                    bottom: link_bottom_y,
                                    half_pitch: px(lane_pitch / 2.0),
                                },
                                edge_color,
                            );
                        }
                    }
                }

                // The pad line is the renderer state below the transition band.
                for (column, cell) in pad_line.iter().enumerate() {
                    let Some(pattern) = line_pattern_for(cell) else {
                        continue;
                    };
                    let column = column as u32;
                    let pad_start = if link_line.is_some() {
                        link_bottom_y
                    } else if column == node_column {
                        start_y
                    } else {
                        node_y
                    };
                    stroke_line_pattern(
                        window,
                        column_center_x(column),
                        pad_start,
                        column_center_x(column),
                        row_bottom,
                        line_color,
                        pattern,
                    );
                }

                for &column in &termination_columns {
                    let x = column_center_x(column);
                    let start = if link_line.is_some() {
                        link_bottom_y
                    } else if column == node_column {
                        start_y
                    } else {
                        node_y
                    };
                    let end = start + (row_bottom - start) * TERMINATION_STUB_FRACTION;
                    stroke_line_pattern(
                        window,
                        x,
                        start,
                        x,
                        end,
                        edge_color,
                        LinePattern::Dashed(MISSING_EDGE_DASH_PATTERN),
                    );
                }

                // Synthetic elision bands, drawn in the row's own canvas so their lanes connect
                // seamlessly to the row above rather than resetting per band. Never selectable,
                // never their own list item — see the DAG parity plan's GPUI section.
                let mut band_top = row_bottom;
                for band in &bands {
                    let band_node_column = band.node_column;
                    let band_bottom = band_top + px(ELISION_BAND_HEIGHT);
                    let label_y = band_top + px(ELISION_BAND_HEIGHT / 2.0);
                    let band_x = column_center_x(band_node_column);

                    for (column, cell) in band.node_line.iter().enumerate() {
                        let column = column as u32;
                        if column == band_node_column {
                            continue;
                        }
                        let Some(pattern) = line_pattern_for(cell) else {
                            continue;
                        };
                        stroke_line_pattern(
                            window,
                            column_center_x(column),
                            band_top,
                            column_center_x(column),
                            label_y,
                            line_color,
                            pattern,
                        );
                    }
                    stroke_line_pattern(
                        window,
                        band_x,
                        band_top,
                        band_x,
                        label_y,
                        line_color,
                        LinePattern::Solid,
                    );

                    if let Some(band_link_line) = &band.link_line {
                        let degenerate = LinkBand {
                            top: label_y,
                            center: label_y,
                            bottom: label_y,
                            half_pitch: px(lane_pitch / 2.0),
                        };
                        for (column, cell) in band_link_line.iter().enumerate() {
                            let x = column_center_x(column as u32);
                            for component in link_components(cell) {
                                paint_link_component(window, component, x, degenerate, edge_color);
                            }
                        }
                    }

                    paint_node(
                        window,
                        band_x,
                        label_y,
                        DagNodeStyle {
                            shape: style::NodeShape::Circle,
                            radius: ELISION_GLYPH_RADIUS,
                            fill: style::NodeFill::Filled(elision_glyph_color),
                        },
                    );

                    for (column, cell) in band.pad_line.iter().enumerate() {
                        let Some(pattern) = line_pattern_for(cell) else {
                            continue;
                        };
                        let x = column_center_x(column as u32);
                        stroke_line_pattern(
                            window,
                            x,
                            label_y,
                            x,
                            band_bottom,
                            line_color,
                            pattern,
                        );
                    }

                    band_top = band_bottom;
                }

                let marker_leading_x = bounds.origin.x + bounds.size.width
                    - px(OVERFLOW_MARKER_INSET)
                    - px(OVERFLOW_MARKER_SIZE / 2.0)
                    - px(OVERFLOW_CHEVRON_GAP);
                let content = node_layer_content(is_clipped, my_x + radius_px, marker_leading_x);
                if content.includes_overflow() {
                    paint_overflow_marker(window, bounds, node_y, elision_glyph_color, row_bg);
                }
                // Node on top, painted after the overflow fade so the clip never dims the circle.
                if content.includes_node() {
                    paint_node(window, my_x, node_y, style);
                }
            });
        },
    )
    .flex_none()
    .w(px(graph_width))
    .overflow_hidden()
    .h_full()
    .into_any_element()
}

/// Marks a row whose native lanes ran past the gutter budget: the lanes are still in the layout,
/// they are simply not painted past this edge. The trailing band fades the lanes into the row
/// background so they dissolve rather than end on a hard wall, and a double chevron at the node's
/// own height reads as "lanes continue" instead of a stray glyph.
fn paint_overflow_marker(
    window: &mut gpui::Window,
    bounds: Bounds<Pixels>,
    y: Pixels,
    color: u32,
    row_bg: u32,
) {
    let right = bounds.origin.x + bounds.size.width;
    let fade = Bounds::new(
        point(right - px(OVERFLOW_FADE_WIDTH), bounds.origin.y),
        size(px(OVERFLOW_FADE_WIDTH), bounds.size.height),
    );
    let bg: gpui::Hsla = rgb(row_bg).into();
    window.paint_quad(fill(
        fade,
        linear_gradient(
            90.0,
            linear_color_stop(bg.opacity(0.0), 0.0),
            linear_color_stop(bg, 0.8),
        ),
    ));

    let tip_x = right - px(OVERFLOW_MARKER_INSET);
    let arm = px(OVERFLOW_MARKER_SIZE / 2.0);
    for chevron in 0..2 {
        let x = tip_x - px(chevron as f32 * OVERFLOW_CHEVRON_GAP);
        stroke_line_pattern(window, x - arm, y - arm, x, y, color, LinePattern::Solid);
        stroke_line_pattern(window, x, y, x - arm, y + arm, color, LinePattern::Solid);
    }
}

fn line_pattern_for(cell: &DagVerticalCell) -> Option<LinePattern> {
    match cell {
        DagVerticalCell::Empty => None,
        DagVerticalCell::Direct => Some(LinePattern::Solid),
        DagVerticalCell::Indirect => Some(LinePattern::Dashed(INDIRECT_EDGE_DASH_PATTERN)),
    }
}

fn line_pattern_for_kind(kind: DagEdgeKind) -> LinePattern {
    match kind {
        DagEdgeKind::Direct => LinePattern::Solid,
        DagEdgeKind::Indirect => LinePattern::Dashed(INDIRECT_EDGE_DASH_PATTERN),
    }
}

fn link_components(cell: &DagLinkCell) -> impl Iterator<Item = LinkComponent> + '_ {
    [
        cell.vertical.map(LinkComponent::Vertical),
        cell.horizontal.map(LinkComponent::Horizontal),
        cell.left_fork.map(LinkComponent::LeftFork),
        cell.right_fork.map(LinkComponent::RightFork),
        cell.left_merge.map(LinkComponent::LeftMerge),
        cell.right_merge.map(LinkComponent::RightMerge),
    ]
    .into_iter()
    .flatten()
}

fn paint_link_component(
    window: &mut gpui::Window,
    component: LinkComponent,
    x: Pixels,
    band: LinkBand,
    color: u32,
) {
    let pattern = line_pattern_for_kind(component.edge_kind());
    let radius = px(paint::CORNER_RADIUS)
        .min(band.half_pitch)
        .min(band.center - band.top)
        .min(band.bottom - band.center);
    if let Some([start, corner, end]) = component.rounded_elbow(x, band) {
        stroke_rounded_elbow_pattern(window, start, corner, end, radius, color, pattern);
        return;
    }

    match component {
        LinkComponent::Vertical(_) => {
            stroke_line_pattern(window, x, band.top, x, band.bottom, color, pattern)
        }
        LinkComponent::Horizontal(_) => stroke_line_pattern(
            window,
            x - band.half_pitch,
            band.center,
            x + band.half_pitch,
            band.center,
            color,
            pattern,
        ),
        LinkComponent::LeftFork(_)
        | LinkComponent::RightFork(_)
        | LinkComponent::LeftMerge(_)
        | LinkComponent::RightMerge(_) => unreachable!("elbows returned above"),
    }
}

#[cfg(test)]
mod tests {
    use jayjay_core::dag::{DagEdgeKind, DagLinkCell};

    use super::{
        DagGeometry, LinkBand, LinkComponent, MINIMUM_LEGIBLE_LANE_PITCH, NodeLayerContent,
        PREFERRED_LANE_PITCH, link_components, link_top, node_layer_content,
    };

    #[test]
    fn link_components_preserve_every_typed_renderer_segment() {
        let cell = DagLinkCell {
            vertical: Some(DagEdgeKind::Direct),
            horizontal: Some(DagEdgeKind::Indirect),
            left_fork: Some(DagEdgeKind::Direct),
            right_fork: Some(DagEdgeKind::Indirect),
            left_merge: Some(DagEdgeKind::Direct),
            right_merge: Some(DagEdgeKind::Indirect),
            is_child: true,
        };

        assert_eq!(
            link_components(&cell).collect::<Vec<_>>(),
            vec![
                LinkComponent::Vertical(DagEdgeKind::Direct),
                LinkComponent::Horizontal(DagEdgeKind::Indirect),
                LinkComponent::LeftFork(DagEdgeKind::Direct),
                LinkComponent::RightFork(DagEdgeKind::Indirect),
                LinkComponent::LeftMerge(DagEdgeKind::Direct),
                LinkComponent::RightMerge(DagEdgeKind::Indirect),
            ]
        );
    }

    #[test]
    fn forks_and_merges_retain_rounded_elbows() {
        let bends = [
            LinkComponent::LeftFork(DagEdgeKind::Direct),
            LinkComponent::RightFork(DagEdgeKind::Direct),
            LinkComponent::LeftMerge(DagEdgeKind::Direct),
            LinkComponent::RightMerge(DagEdgeKind::Direct),
        ];
        let straights = [
            LinkComponent::Vertical(DagEdgeKind::Direct),
            LinkComponent::Horizontal(DagEdgeKind::Direct),
        ];

        let band = LinkBand {
            top: gpui::px(0.0),
            center: gpui::px(10.0),
            bottom: gpui::px(20.0),
            half_pitch: gpui::px(10.0),
        };

        assert!(
            bends
                .into_iter()
                .all(|component| component.rounded_elbow(gpui::px(10.0), band).is_some())
        );
        assert!(
            straights
                .into_iter()
                .all(|component| component.rounded_elbow(gpui::px(10.0), band).is_none())
        );
    }

    #[test]
    fn node_column_link_starts_outside_node() {
        let node_column = 1;
        let other_column = 2;
        let node_y = gpui::px(12.0);
        let node_radius = gpui::px(5.0);

        assert_eq!(
            link_top(node_column, node_column, node_y, node_radius),
            node_y + node_radius
        );
        assert_eq!(
            link_top(other_column, node_column, node_y, node_radius),
            node_y
        );
    }

    #[test]
    fn narrow_sidebar_never_compresses_lanes_or_nodes_below_legible_sizes() {
        let geometry = DagGeometry::new(10, 200.0);

        assert_eq!(geometry.lane_pitch, MINIMUM_LEGIBLE_LANE_PITCH);
        assert_eq!(geometry.node_radius, super::PREFERRED_NODE_RADIUS);
    }

    #[test]
    fn ordinary_graph_uses_preferred_pitch() {
        // The gutter cap holds the preferred pitch up to four lanes; a four-lane graph is at that
        // edge, so it must never fall back to the compressed floor.
        let geometry = DagGeometry::new(4, 1_000.0);

        assert_eq!(geometry.lane_pitch, PREFERRED_LANE_PITCH);
    }

    #[test]
    fn a_row_narrower_than_the_layout_gets_only_its_own_lanes() {
        // 33 columns is `jayjay()` on the Rust checkout; most of its rows use one or two.
        let geometry = DagGeometry::new(33, 360.0);

        let narrow = geometry.row_graph_width(2);
        assert!(!narrow.is_clipped);
        assert_eq!(
            narrow.width,
            super::HORIZONTAL_PADDING + 2.0 * geometry.lane_pitch,
            "a two-lane row must not reserve the whole layout's width"
        );
        assert!(narrow.width < geometry.row_graph_width(33).width);
    }

    /// The width cap is only meaningful at the column counts real repositories reach: `jayjay()`
    /// on the Rust checkout is 33 native columns and `::trunk()` is 132 at the row ceiling. Both
    /// are far past the point where lane pitch has already compressed to its legibility floor, so
    /// the gutter can only be held to the budget by clipping.
    #[test]
    fn a_row_wider_than_the_budget_is_capped_and_reports_clipping() {
        for (columns, sidebar) in [(33u32, 360.0f32), (132, 360.0), (132, 2_000.0)] {
            let geometry = DagGeometry::new(columns, sidebar);
            let row = geometry.row_graph_width(columns);

            assert!(row.is_clipped, "columns={columns} sidebar={sidebar}");
            assert_eq!(
                row.width, geometry.width_budget,
                "columns={columns} sidebar={sidebar}"
            );
            assert!(row.width <= super::ABSOLUTE_GRAPH_MAX_WIDTH);
            assert!(row.width <= sidebar * super::MAX_SIDEBAR_FRACTION);
        }
    }

    #[test]
    fn a_capped_row_still_leaves_the_sidebar_room_for_change_text() {
        let geometry = DagGeometry::new(132, 360.0);

        let remaining = 360.0 - geometry.row_graph_width(132).width;
        assert!(
            remaining >= 360.0 * (1.0 - super::MAX_SIDEBAR_FRACTION),
            "the gutter must never crowd out the change text: {remaining}px left"
        );
    }

    #[test]
    fn overflow_replaces_only_nodes_that_reach_its_marker() {
        assert_eq!(
            node_layer_content(false, gpui::px(120.0), gpui::px(90.0)),
            NodeLayerContent::NodeOnly
        );
        assert_eq!(
            node_layer_content(true, gpui::px(87.0), gpui::px(90.0)),
            NodeLayerContent::NodeAndOverflow
        );
        assert_eq!(
            node_layer_content(true, gpui::px(88.0), gpui::px(90.0)),
            NodeLayerContent::OverflowOnly
        );
    }
}
