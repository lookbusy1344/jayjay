import Foundation

/// Maps logical DAG columns and row bands to pixel positions for a given sidebar width.
///
/// One value is built per visible width and shared by every row, so column pitch never drifts row to row. Lanes never compress below a legible pitch, so a wide graph is held to `widthBudget` by clipping its trailing lanes behind an overflow marker rather than by dropping columns.
struct DAGGeometry: Equatable {
    static let preferredLanePitch: CGFloat = 12
    static let minimumLegibleLanePitch: CGFloat = 10
    static let absoluteGraphMaxWidth: CGFloat = 68
    static let maxSidebarFraction: CGFloat = 0.45
    static let horizontalPadding: CGFloat = 8
    static let preferredNodeRadius: CGFloat = 4

    let logicalColumnCount: Int
    let lanePitch: CGFloat
    let nodeRadius: CGFloat
    /// The widest gutter a row may claim. Lanes past it are clipped behind an overflow marker so the change text stays legible however wide the native graph gets.
    let widthBudget: CGFloat

    init(logicalColumnCount: Int, availableSidebarWidth: CGFloat) {
        let columns = max(1, logicalColumnCount)
        self.logicalColumnCount = columns

        widthBudget = min(
            Self.absoluteGraphMaxWidth,
            availableSidebarWidth * Self.maxSidebarFraction
        )
        let compressedPitch = (widthBudget - Self.horizontalPadding) / CGFloat(columns)
        lanePitch = min(Self.preferredLanePitch, max(Self.minimumLegibleLanePitch, compressedPitch))
        nodeRadius = Self.preferredNodeRadius
    }

    func xPosition(forColumn column: Int) -> CGFloat {
        dagRowLeadingPadding + CGFloat(column) * lanePitch + lanePitch / 2
    }

    /// `jj log` starts each change's text after that row's graph prefix, rather than after the widest prefix in the entire log. Every row still shares this geometry's lane pitch, and `widthBudget` caps how much of that prefix is painted.
    func graphWidth(forColumnCount columnCount: Int) -> CGFloat {
        min(naturalGraphWidth(forColumnCount: columnCount), widthBudget)
    }

    /// True when the row's native lanes run past the gutter budget. The lanes stay in the layout — topology is never touched — but only the leading `widthBudget` pixels of them are drawn.
    func isClipped(forColumnCount columnCount: Int) -> Bool {
        naturalGraphWidth(forColumnCount: columnCount) > widthBudget
    }

    private func naturalGraphWidth(forColumnCount columnCount: Int) -> CGFloat {
        Self.horizontalPadding + CGFloat(max(1, columnCount)) * lanePitch
    }

    func linkTopY(forColumn column: Int, nodeColumn: Int, nodeY: CGFloat, nodeRadius: CGFloat) -> CGFloat {
        column == nodeColumn ? nodeY + nodeRadius : nodeY
    }
}
