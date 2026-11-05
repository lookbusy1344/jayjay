@testable import JayJay
import XCTest

final class DAGNodeStyleTests: XCTestCase {
    private let baseRadius: CGFloat = 4

    func testImmutableChangesDrawAsDiamondsLikeJjLog() {
        XCTAssertEqual(DAGNodeStyle.resolve(change: mockChangeInfo(isImmutable: true), radius: baseRadius).shape, .diamond)
        XCTAssertEqual(DAGNodeStyle.resolve(change: mockChangeInfo(isImmutable: false), radius: baseRadius).shape, .circle)
    }

    func testMutableChangeWithTrunkBookmarkStaysACircle() {
        XCTAssertEqual(DAGNodeStyle.resolve(change: mockChangeInfo(bookmarks: ["main"]), radius: baseRadius).shape, .circle)
    }
}
