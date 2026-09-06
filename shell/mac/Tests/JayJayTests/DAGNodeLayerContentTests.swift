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
}
