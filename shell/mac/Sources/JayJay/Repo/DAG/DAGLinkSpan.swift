import Foundation

/// Vertical extent of a link line: forks and merges bend at `centerY` and lanes leave it at `bottomY`. Elbows above and below `centerY` get the full corner radius in both main rows and elision bands.
struct DAGLinkSpan: Equatable {
    let nodeY: CGFloat
    let centerY: CGFloat
    let bottomY: CGFloat

    init(nodeY: CGFloat, rowBottomY: CGFloat) {
        self.nodeY = nodeY
        centerY = nodeY + (rowBottomY - nodeY) * dagLinkCenterFraction
        bottomY = min(rowBottomY, centerY + dagGraphCornerRadius)
    }

    /// The band is too short for the main row's proportional bend, so it bends one corner radius above its bottom.
    init(elisionBandTopY: CGFloat) {
        nodeY = elisionBandTopY + dagElisionBandHeight / 2
        bottomY = elisionBandTopY + dagElisionBandHeight
        centerY = bottomY - dagGraphCornerRadius
    }
}
