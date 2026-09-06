@testable import JayJay
import JayJayCore
import XCTest

@MainActor
final class RepoViewModelEmptyStatesTests: RepoViewModelTestCase {
    func testEmptyStatesEventCorrectsOnlyMatchingRows() throws {
        let viewModel = try XCTUnwrap(viewModel)
        viewModel.graphEntries = [
            GraphEntry(change: mockChangeInfo(commitId: "merge", isEmpty: false), edges: []),
            GraphEntry(change: mockChangeInfo(commitId: "other", isEmpty: false), edges: [])
        ]

        let context = RepoGraphRefreshContext(
            generation: viewModel.graphRefreshGeneration,
            preferredCommitId: nil,
            preferredRev: nil,
            revset: "all()",
            isAutoTriggered: false
        )
        viewModel.applyLogGraphEvent(
            .emptyStates(updates: [EmptyStateUpdate(commitId: "merge", isEmpty: true)]),
            context: context
        )

        XCTAssertTrue(viewModel.graphEntries[0].change.isEmpty)
        XCTAssertFalse(viewModel.graphEntries[1].change.isEmpty)
    }

    func testEmptyStatesFromASupersededGenerationIsIgnored() throws {
        let viewModel = try XCTUnwrap(viewModel)
        viewModel.graphEntries = [
            GraphEntry(change: mockChangeInfo(commitId: "merge", isEmpty: false), edges: [])
        ]

        let staleContext = RepoGraphRefreshContext(
            generation: viewModel.graphRefreshGeneration &- 1,
            preferredCommitId: nil,
            preferredRev: nil,
            revset: "all()",
            isAutoTriggered: false
        )
        viewModel.applyLogGraphEvent(
            .emptyStates(updates: [EmptyStateUpdate(commitId: "merge", isEmpty: true)]),
            context: staleContext
        )

        XCTAssertFalse(viewModel.graphEntries[0].change.isEmpty)
    }
}
