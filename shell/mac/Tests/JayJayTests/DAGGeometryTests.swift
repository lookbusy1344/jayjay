@testable import JayJay
import JayJayCore
import SwiftUI
import XCTest

final class DAGGeometryTests: XCTestCase {
    func testLinkComponentsPreserveEveryTypedRendererSegment() {
        let cell = DagLinkCell(
            vertical: .direct,
            horizontal: .indirect,
            leftFork: .direct,
            rightFork: .indirect,
            leftMerge: .direct,
            rightMerge: .indirect,
            isChild: true
        )

        XCTAssertEqual(
            cell.components,
            [
                .vertical(.direct),
                .horizontal(.indirect),
                .leftFork(.direct),
                .rightFork(.indirect),
                .leftMerge(.direct),
                .rightMerge(.indirect)
            ]
        )
    }

    func testForksAndMergesRetainRoundedElbows() {
        let bendComponents: [DAGLinkComponent] = [
            .leftFork(.direct),
            .rightFork(.direct),
            .leftMerge(.direct),
            .rightMerge(.direct)
        ]
        let straightComponents: [DAGLinkComponent] = [
            .vertical(.direct),
            .horizontal(.direct)
        ]

        XCTAssertTrue(bendComponents.allSatisfy(\.pathForTest.containsQuadraticCurve))
        XCTAssertTrue(straightComponents.allSatisfy { !$0.pathForTest.containsQuadraticCurve })
    }

    func testNodeColumnLinkStartsOutsideNode() {
        let geometry = DAGGeometry(logicalColumnCount: 3, availableSidebarWidth: 320)
        let nodeColumn = 1
        let otherColumn = 2
        let nodeY: CGFloat = 12
        let nodeRadius: CGFloat = 5

        XCTAssertEqual(
            geometry.linkTopY(forColumn: nodeColumn, nodeColumn: nodeColumn, nodeY: nodeY, nodeRadius: nodeRadius),
            nodeY + nodeRadius
        )
        XCTAssertEqual(
            geometry.linkTopY(forColumn: otherColumn, nodeColumn: nodeColumn, nodeY: nodeY, nodeRadius: nodeRadius),
            nodeY
        )
    }

    func testNarrowGraphUsesPreferredPitchAndGrowsOncePerColumn() {
        let one = DAGGeometry(logicalColumnCount: 1, availableSidebarWidth: 1000)
        let two = DAGGeometry(logicalColumnCount: 2, availableSidebarWidth: 1000)

        XCTAssertEqual(one.lanePitch, DAGGeometry.preferredLanePitch)
        XCTAssertEqual(two.lanePitch, DAGGeometry.preferredLanePitch)
        XCTAssertEqual(
            two.graphWidth(forColumnCount: 2),
            one.graphWidth(forColumnCount: 1) + DAGGeometry.preferredLanePitch
        )
    }

    func testWidthBudgetNeverCompressesBelowLegiblePitch() {
        let geometry = DAGGeometry(logicalColumnCount: 10, availableSidebarWidth: 200)

        XCTAssertEqual(geometry.lanePitch, DAGGeometry.minimumLegibleLanePitch)
        XCTAssertEqual(geometry.nodeRadius, DAGGeometry.preferredNodeRadius)
    }

    func testARowNarrowerThanTheLayoutGetsOnlyItsOwnLanes() {
        // 33 columns is `jayjay()` on the Rust checkout; most of its rows use one or two.
        let geometry = DAGGeometry(logicalColumnCount: 33, availableSidebarWidth: 360)

        XCTAssertFalse(geometry.isClipped(forColumnCount: 2))
        XCTAssertEqual(
            geometry.graphWidth(forColumnCount: 2),
            DAGGeometry.horizontalPadding + 2 * geometry.lanePitch
        )
        XCTAssertLessThan(geometry.graphWidth(forColumnCount: 2), geometry.graphWidth(forColumnCount: 33))
    }

    /// The width cap only means anything at the column counts real repositories reach: `jayjay()`
    /// on the Rust checkout is 33 native columns and `::trunk()` is 132 at the row ceiling. Both
    /// are far past the point where lane pitch has already compressed to its legibility floor, so
    /// the gutter can only be held to the budget by clipping.
    func testARowWiderThanTheBudgetIsCappedAndReportsClipping() {
        for (columns, sidebar) in [(33, CGFloat(360)), (132, 360), (132, 2000)] {
            let geometry = DAGGeometry(logicalColumnCount: columns, availableSidebarWidth: sidebar)
            let width = geometry.graphWidth(forColumnCount: columns)
            let label = "columns=\(columns) width=\(sidebar)"

            XCTAssertTrue(geometry.isClipped(forColumnCount: columns), label)
            XCTAssertEqual(width, geometry.widthBudget, label)
            XCTAssertLessThanOrEqual(width, DAGGeometry.absoluteGraphMaxWidth, label)
            XCTAssertLessThanOrEqual(width, sidebar * DAGGeometry.maxSidebarFraction, label)
        }
    }

    func testACappedRowStillLeavesTheSidebarRoomForChangeText() {
        let geometry = DAGGeometry(logicalColumnCount: 132, availableSidebarWidth: 360)

        let remaining = 360 - geometry.graphWidth(forColumnCount: 132)
        XCTAssertGreaterThanOrEqual(remaining, 360 * (1 - DAGGeometry.maxSidebarFraction) - 0.01)
    }

    /// An unclipped row draws every one of its columns, so each centre must land inside the frame.
    /// A clipped row deliberately paints only its leading lanes, so the invariant there is that the
    /// frame is the budget and the lanes it does fit are inside it.
    func testColumnCentresStayInsideTheFrameTheRowActuallyPaints() {
        for columns in [1, 2, 5, 12, 33, 132] {
            for width: CGFloat in [80, 200, 360, 1000] {
                let geometry = DAGGeometry(logicalColumnCount: columns, availableSidebarWidth: width)
                let frame = geometry.graphWidth(forColumnCount: columns)
                let label = "columns=\(columns) width=\(width)"

                XCTAssertGreaterThan(geometry.xPosition(forColumn: 0), 0, label)
                let lastVisible = (0 ..< columns).last { geometry.xPosition(forColumn: $0) < frame }
                XCTAssertNotNil(lastVisible, "the node's own lane must always be painted: \(label)")

                if geometry.isClipped(forColumnCount: columns) {
                    XCTAssertEqual(frame, geometry.widthBudget, label)
                    XCTAssertLessThan(lastVisible ?? 0, columns - 1, label)
                } else {
                    XCTAssertEqual(lastVisible, columns - 1, label)
                }
            }
        }
    }

    /// Rendering and rebase hit-testing both call `xPosition(forColumn:)` on the same `DAGGeometry` value for a row's `nodeColumn` — this only holds if the function is pure, so two identically-configured geometries must agree exactly.
    func testNodePositionIsDeterministicForRebaseHitTesting() {
        let a = DAGGeometry(logicalColumnCount: 6, availableSidebarWidth: 260)
        let b = DAGGeometry(logicalColumnCount: 6, availableSidebarWidth: 260)

        for column in 0 ..< 6 {
            XCTAssertEqual(a.xPosition(forColumn: column), b.xPosition(forColumn: column))
        }
    }

    func testChangingSidebarWidthCannotMakePitchSubLegible() {
        let narrow = DAGGeometry(logicalColumnCount: 5, availableSidebarWidth: 120)
        let wide = DAGGeometry(logicalColumnCount: 5, availableSidebarWidth: 1000)

        XCTAssertEqual(narrow.logicalColumnCount, wide.logicalColumnCount)
        XCTAssertNotEqual(narrow.lanePitch, wide.lanePitch)
        XCTAssertEqual(narrow.lanePitch, DAGGeometry.minimumLegibleLanePitch)
    }
}

private extension DAGLinkComponent {
    var pathForTest: Path {
        path(in: .init(x: 10, topY: 0, centerY: 10, bottomY: 20, halfPitch: 10, cornerRadius: 6))
    }
}

private extension Path {
    var containsQuadraticCurve: Bool {
        var result = false
        cgPath.applyWithBlock { element in
            result = result || element.pointee.type == .addQuadCurveToPoint
        }
        return result
    }
}
