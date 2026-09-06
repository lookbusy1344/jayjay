@testable import JayJay
import JayJayCore
import XCTest

@MainActor
final class DAGPerformanceTests: XCTestCase {
    private static let rowCount = 12000
    private static let headCount = 32
    private static let sidebarWidth: CGFloat = 320
    private static let measurementIterations = 3
    private static let sampledTargetCount = 20
    private static let maximumMenuEligibilityDuration = Duration.milliseconds(200)

    func testLargeGraphRowMenuEligibility() {
        let selectionGraph = DagSelectionGraph(entries: Self.entries)
        let layout = DAGLayout(entries: Self.entries)
        let geometry = DAGGeometry(
            logicalColumnCount: layout.logicalColumnCount,
            availableSidebarWidth: Self.sidebarWidth
        )
        let options = XCTMeasureOptions()
        options.iterationCount = Self.measurementIterations

        measure(metrics: [XCTClockMetric()], options: options) {
            let start = ContinuousClock.now
            let viewModel = DAGViewModel(
                entries: Self.entries,
                selectedId: "change-0",
                selectedIds: ["change-0"],
                compareFromId: nil,
                rebaseDrag: nil,
                bookmarkDrag: nil,
                colorScheme: .light,
                layout: layout,
                capabilities: DAGSelectionCapabilities(
                    graph: selectionGraph,
                    entries: Self.entries,
                    selectedCommitIds: ["commit-0"]
                ),
                geometry: geometry
            )
            for target in Self.entries.prefix(Self.sampledTargetCount) {
                XCTAssertTrue(viewModel.canMergeSelectedChange(with: target.change))
            }
            XCTAssertLessThan(start.duration(to: .now), Self.maximumMenuEligibilityDuration)
        }
    }

    private static let entries: [GraphEntry] = {
        let heads = (0 ..< headCount).map { entry("head-\($0)", parents: ["commit-\(rowCount - 1 - $0)"]) }
        let chain = (0 ..< rowCount).map { index in
            entry("\(index)", parents: index + 1 < rowCount ? ["commit-\(index + 1)"] : [])
        }
        return heads + chain
    }()

    private static func entry(_ id: String, parents: [String]) -> GraphEntry {
        GraphEntry(
            change: mockChangeInfo(changeId: "change-\(id)", commitId: "commit-\(id)", parents: parents),
            edges: parents.map { GraphEdge(target: $0, edgeType: .direct) }
        )
    }
}
