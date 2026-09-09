import CoreGraphics

enum DAGNodeLayerContent: Equatable {
    case nodeOnly
    case nodeAndOverflow
    case overflowOnly

    var includesNode: Bool {
        self != .overflowOnly
    }

    var includesOverflow: Bool {
        self != .nodeOnly
    }
}

func dagNodeLayerContent(
    isGraphClipped: Bool,
    nodeTrailingX: CGFloat,
    overflowLeadingX: CGFloat
) -> DAGNodeLayerContent {
    guard isGraphClipped else { return .nodeOnly }
    return nodeTrailingX + dagOverflowNodeGap < overflowLeadingX ? .nodeAndOverflow : .overflowOnly
}

/// Placement of the trailing overflow badge for a row of the given width and node position. The badge
/// drawing and its focus tap target both read this, so they never drift apart. Pure, so the tap-region
/// contract is unit-testable without SwiftUI layout.
struct DAGOverflowBadgeLayout {
    let content: DAGNodeLayerContent
    let haloCenterX: CGFloat
    let tipX: CGFloat
    let arm: CGFloat
}

func dagOverflowBadgeLayout(isGraphClipped: Bool, nodeTrailingX: CGFloat, width: CGFloat) -> DAGOverflowBadgeLayout {
    let arm = dagOverflowMarkerSize / 2
    // Anchor the badge by its disc so the whole circle clears the column edge, then place the chevron
    // centred in that disc.
    let haloCenterX = width - dagOverflowMarkerInset - dagOverflowMarkerHaloRadius
    let tipX = haloCenterX + (arm + dagOverflowChevronGap) / 2
    let overflowLeadingX = tipX - arm - dagOverflowChevronGap
    let content = dagNodeLayerContent(
        isGraphClipped: isGraphClipped,
        nodeTrailingX: nodeTrailingX,
        overflowLeadingX: overflowLeadingX
    )
    return DAGOverflowBadgeLayout(content: content, haloCenterX: haloCenterX, tipX: tipX, arm: arm)
}

func dagFocusBadgeContains(x: CGFloat, layout: DAGOverflowBadgeLayout) -> Bool {
    layout.content.includesOverflow
        && abs(x - layout.haloCenterX) <= dagOverflowMarkerTapSize / 2
}
