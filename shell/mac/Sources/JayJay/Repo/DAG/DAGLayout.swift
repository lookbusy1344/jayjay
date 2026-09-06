import JayJayCore
import SwiftUI

let dagRowLeadingPadding: CGFloat = 4
let dagRowVerticalPadding: CGFloat = 8
let dagNodeCenterY: CGFloat = 12
let dagIndirectEdgeStroke = StrokeStyle(lineWidth: 1, dash: [3, 3])
let dagSolidStroke = StrokeStyle(lineWidth: 1)
let dagMissingEdgeStroke = StrokeStyle(lineWidth: 1, lineCap: .round, dash: [2, 2])
let dagGraphCornerRadius: CGFloat = 6
let dagLinkCenterFraction: CGFloat = 0.45
let dagTerminationStubFraction: CGFloat = 0.55
/// Compact height for a synthetic `(elided revisions)` band composed into its owning row, below
/// the row's normal content — never a `ForEach` item or a selectable row of its own.
let dagElisionBandHeight: CGFloat = 18
/// Trailing-edge chevron marking a row whose native lanes are wider than the gutter budget.
let dagOverflowMarkerSize: CGFloat = 6
let dagOverflowMarkerInset: CGFloat = 4
/// The clipped lanes dissolve into the backdrop across this trailing band rather than ending on a
/// hard vertical wall, and the chevron sits over its solid end.
let dagOverflowFadeWidth: CGFloat = 22
/// Radius of the backdrop-filled badge disc behind the chevron: it frames the marker and hides any
/// lane line under it without blanking the whole column.
let dagOverflowMarkerHaloRadius: CGFloat = 8
/// Horizontal offset between the two nested chevrons of the "lanes continue" marker.
let dagOverflowChevronGap: CGFloat = 3.5
/// Clear space between a visible node and the overflow marker.
let dagOverflowNodeGap: CGFloat = 2
/// The overflow chevron is an affordance, not a graph edge, so it carries a heavier rounded stroke
/// than the neutral graph lines it sits beside.
let dagOverflowMarkerStroke = StrokeStyle(lineWidth: 1.5, lineCap: .round, lineJoin: .round)

/// Where the real row's own graph content ends and its trailing elision bands begin, given the
/// row's total measured height. Pure so band placement can be unit-tested without SwiftUI layout.
func dagMainRowBottomY(totalHeight: CGFloat, bandCount: Int) -> CGFloat {
    totalHeight - CGFloat(bandCount) * dagElisionBandHeight
}

/// Thin Swift wrapper over `jayjay_core::dag::DagLayout` (via uniffi): the renderer-computed row shapes, indexed by commit id so rows never need to consult their neighbors to draw.
struct DAGLayout: Sendable {
    let rows: [DagRowShape]
    let logicalColumnCount: Int
    private let rowsByCommitId: [String: DagRowShape]

    /// Wraps a layout computed by the Rust renderer.
    init(computed layout: JayJayCore.DagLayout) {
        rows = layout.rows
        logicalColumnCount = max(1, Int(layout.logicalColumnCount))
        rowsByCommitId = Dictionary(rows.map { ($0.commitId, $0) }, uniquingKeysWith: { first, _ in first })
    }

    static let empty = DAGLayout(computed: .init(rows: [], logicalColumnCount: 1, widestElisionBandCount: 0))

    func row(for commitId: String) -> DagRowShape? {
        rowsByCommitId[commitId]
    }
}
