@testable import JayJay
import XCTest

final class DAGNodeLayerContentTests: XCTestCase {
    func testOverflowReplacesOnlyNodesThatReachItsMarker() {
        XCTAssertEqual(
            dagNodeLayerContent(isGraphClipped: false, nodeTrailingX: 120, overflowLeadingX: 90),
            .nodeOnly
        )
        XCTAssertEqual(
            dagNodeLayerContent(isGraphClipped: true, nodeTrailingX: 87, overflowLeadingX: 90),
            .nodeAndOverflow
        )
        XCTAssertEqual(
            dagNodeLayerContent(isGraphClipped: true, nodeTrailingX: 88, overflowLeadingX: 90),
            .overflowOnly
        )
    }

    func testFocusTapTargetExistsOnlyWhereTheBadgeIsDrawn() {
        let width: CGFloat = 200

        // No badge on an unclipped row: no tap target.
        let unclipped = dagOverflowBadgeLayout(isGraphClipped: false, nodeTrailingX: 20, width: width)
        XCTAssertFalse(unclipped.content.includesOverflow)

        // Clipped row: the badge is drawn, its tap region stays within the sidebar, and its left edge
        // clears the row's node (which ends at nodeTrailingX).
        let clipped = dagOverflowBadgeLayout(isGraphClipped: true, nodeTrailingX: 20, width: width)
        XCTAssertTrue(clipped.content.includesOverflow)
        XCTAssertEqual(clipped.haloCenterX, width - dagOverflowMarkerInset - dagOverflowMarkerHaloRadius)
        XCTAssertLessThanOrEqual(clipped.haloCenterX + dagOverflowMarkerTapSize / 2, width)
        XCTAssertGreaterThan(clipped.haloCenterX - dagOverflowMarkerTapSize / 2, 20)
    }

    func testFocusTapTargetConsumesTheRowGestureOnlyInsideItsHitRegion() {
        let width: CGFloat = 200
        let layout = dagOverflowBadgeLayout(isGraphClipped: true, nodeTrailingX: 20, width: width)

        XCTAssertTrue(dagFocusBadgeContains(x: layout.haloCenterX, layout: layout))
        XCTAssertFalse(dagFocusBadgeContains(x: 20, layout: layout))
    }
}
