import Foundation
@testable import JayJay
import JayJayCore
import XCTest

@MainActor
final class RepoViewModelRefreshTests: RepoViewModelTestCase {
    func testObserverAppliesSnapshotsOnMainActorUsingCoreLayout() async throws {
        let viewModel = try XCTUnwrap(viewModel)
        let entries = try viewModel.repo.logGraph(revset: "all()")
        let coreLayout = JayJayCore.DagLayout(rows: [], logicalColumnCount: 7)
        let firstSnapshot = LogGraphSnapshot(
            entries: [],
            layout: .init(rows: [], logicalColumnCount: 3),
            loadedRows: 0,
            isComplete: false
        )
        let finalSnapshot = LogGraphSnapshot(
            entries: entries,
            layout: coreLayout,
            loadedRows: UInt32(entries.count),
            isComplete: true
        )
        let applied = expectation(description: "snapshot applied")
        applied.expectedFulfillmentCount = 2
        let observer = MainActorLogGraphObserver { event in
            MainActor.preconditionIsolated()
            guard case let .snapshot(received) = event else { return }
            viewModel.applyGraphSnapshot(received, preferredCommitId: nil, preferredRev: nil)
            applied.fulfill()
        }

        await Task.detached {
            observer.onEvent(event: .snapshot(snapshot: firstSnapshot))
            observer.onEvent(event: .snapshot(snapshot: finalSnapshot))
        }.value
        await fulfillment(of: [applied], timeout: 1)

        XCTAssertEqual(viewModel.graphEntries, entries)
        XCTAssertEqual(viewModel.dagLayout.logicalColumnCount, 7)
        XCTAssertTrue(viewModel.dagLayout.rows.isEmpty)
    }

    func testObserverDrainsAnEventQueuedImmediatelyBeforeRelease() async {
        let delivered = expectation(description: "queued event delivered")

        do {
            let observer = MainActorLogGraphObserver { event in
                guard case .finished = event else { return }
                delivered.fulfill()
            }
            await Task.detached {
                observer.onEvent(event: .finished)
            }.value
        }

        await fulfillment(of: [delivered], timeout: 1)
    }

    func testFirstSnapshotRestoresSelectionByCommitIdAndLaterSnapshotKeepsIt() throws {
        let viewModel = try XCTUnwrap(viewModel)
        let entries = try viewModel.repo.logGraph(revset: "all()")
        let selected = try XCTUnwrap(entries.last)
        viewModel.selectedChangeId = selected.change.selectionRevision

        viewModel.applyGraphSnapshot(
            snapshot(entries: entries, isComplete: false),
            preferredCommitId: selected.change.commitId.id,
            preferredRev: selected.change.selectionRevision
        )
        XCTAssertEqual(viewModel.selectedChangeId, selected.change.selectionRevision)

        let selectionAfterFirstSnapshot = viewModel.selectedChangeId
        viewModel.applyGraphSnapshot(
            snapshot(entries: entries, isComplete: true),
            preferredCommitId: entries[0].change.commitId.id,
            preferredRev: entries[0].change.selectionRevision
        )
        XCTAssertEqual(viewModel.selectedChangeId, selectionAfterFirstSnapshot)
    }

    func testContinueLoadingKeepsThePublishedPrefixWhileTheSessionResumes() throws {
        let viewModel = try XCTUnwrap(viewModel)
        let entries = [
            GraphEntry(change: mockChangeInfo(changeId: "c-2", commitId: "222"), edges: []),
            GraphEntry(change: mockChangeInfo(changeId: "c-1", commitId: "111"), edges: [])
        ]
        viewModel.applyGraphSnapshot(
            snapshot(entries: entries, isComplete: false),
            preferredCommitId: nil,
            preferredRev: nil
        )
        viewModel.graphPaused = true
        viewModel.graphLoadToken = JayJayGraphLoadToken()

        viewModel.continueLoading()

        XCTAssertEqual(viewModel.graphEntries, entries)
        XCTAssertFalse(viewModel.graphPaused)
        XCTAssertTrue(viewModel.isRefreshingInFlight)
    }

    func testCancelingRefreshTaskLatchesCoreGraphToken() async throws {
        let repo = BlockingGraphRepo()
        let viewModel = RepoViewModel(
            path: "/tmp",
            repo: repo,
            workingCopyIsLarge: false,
            configWarning: nil
        )

        viewModel.refresh(snapshotWorkingCopy: false)
        try await waitUntil("the graph session starts") { repo.hasStarted }
        let token = try XCTUnwrap(viewModel.graphLoadToken)

        viewModel.refreshTask?.cancel()
        try await waitUntil("the core token is canceled") { token.isCanceled() }
        repo.finish()
    }

    func testStartingNewRefreshCancelsPreviousCoreGraphToken() async throws {
        let repo = BlockingGraphRepo()
        let viewModel = RepoViewModel(
            path: "/tmp",
            repo: repo,
            workingCopyIsLarge: false,
            configWarning: nil
        )

        viewModel.refresh(snapshotWorkingCopy: false)
        try await waitUntil("the first graph session starts") { repo.requestCount == 1 }
        let firstToken = try XCTUnwrap(viewModel.graphLoadToken)

        viewModel.refresh(snapshotWorkingCopy: false)
        XCTAssertTrue(firstToken.isCanceled())

        repo.finish(count: 2)
    }

    func testPausedSessionContinuesWithoutStartingAnotherRequest() async throws {
        let repo = BlockingGraphRepo(events: [.paused])
        let viewModel = RepoViewModel(
            path: "/tmp",
            repo: repo,
            workingCopyIsLarge: false,
            configWarning: nil
        )

        viewModel.refresh(snapshotWorkingCopy: false)
        try await waitUntil("the graph session pauses") { viewModel.graphPaused }
        let initialRequest = try XCTUnwrap(repo.requests.first)
        XCTAssertEqual(repo.ancillaryLoadCount, 1)

        viewModel.continueLoading()

        XCTAssertEqual(repo.requestCount, 1)
        XCTAssertEqual(viewModel.graphRowCeiling, initialRequest.rowCeiling * 2)
        XCTAssertEqual(repo.ancillaryLoadCount, 1)
        XCTAssertFalse(viewModel.graphPaused)
        XCTAssertTrue(viewModel.isRefreshingInFlight)
    }

    func testExpiredFirstResultBudgetShowsSlowStateUntilSnapshotArrives() throws {
        let viewModel = try XCTUnwrap(viewModel)
        let generation: UInt64 = 7
        viewModel.graphRefreshGeneration = generation
        let context = RepoGraphRefreshContext(
            generation: generation,
            preferredCommitId: nil,
            preferredRev: nil,
            revset: "all()",
            isAutoTriggered: false
        )

        viewModel.applyLogGraphEvent(
            .progress(
                consumedRows: 0,
                materializedRows: 0,
                elapsedMs: 10000,
                firstResultBudgetExpired: true
            ),
            context: context
        )
        XCTAssertTrue(viewModel.graphLoadSlow)

        viewModel.applyLogGraphEvent(
            .snapshot(snapshot: snapshot(entries: [], isComplete: false)),
            context: context
        )
        XCTAssertFalse(viewModel.graphLoadSlow)
    }

    func testRefreshAppliesGraphEntriesAndTheirLayoutTogether() async throws {
        let viewModel = try XCTUnwrap(viewModel)

        viewModel.refresh()
        for _ in 0 ..< 200 where viewModel.isRefreshingInFlight {
            try await Task.sleep(for: .milliseconds(20))
        }

        XCTAssertEqual(
            viewModel.dagLayout.rows.map(\.commitId),
            viewModel.graphEntries.map(\.change.commitId.id)
        )
    }

    func testWorkingCopyChangeWaitsForEditingAndDefersAnInFlightResult() async throws {
        let viewModel = try XCTUnwrap(viewModel)
        viewModel.refresh()
        try await waitUntil("the refresh finishes") { !viewModel.isRefreshingInFlight }
        XCTAssertTrue(viewModel.selectedChange?.info.isWorkingCopy == true)

        try "refresh me\n".write(
            to: URL(fileURLWithPath: viewModel.repoPath).appending(path: "late-edit.txt"),
            atomically: true,
            encoding: .utf8
        )
        viewModel.setBackgroundRefreshSuspended(true)
        viewModel.lastInternalMutationAt = Date()
        viewModel.handleWorkingCopyChange()
        XCTAssertFalse(viewModel.isRefreshingInFlight)
        XCTAssertTrue(viewModel.hasPendingBackgroundRefresh)

        viewModel.setBackgroundRefreshSuspended(false)
        XCTAssertTrue(viewModel.isRefreshingInFlight)
        viewModel.setBackgroundRefreshSuspended(true)

        try await waitUntil("the refresh finishes") { !viewModel.isRefreshingInFlight }
        XCTAssertTrue(viewModel.hasPendingBackgroundRefresh)
        XCTAssertFalse(viewModel.selectedChange?.diff.contains { $0.path == "late-edit.txt" } == true)

        viewModel.setBackgroundRefreshSuspended(false)
        XCTAssertTrue(viewModel.isRefreshingInFlight)
        try await waitUntil("the refresh finishes") { !viewModel.isRefreshingInFlight }
        XCTAssertNil(viewModel.error)
        XCTAssertTrue(viewModel.selectedChange?.diff.contains { $0.path == "late-edit.txt" } == true)
    }

    func testCancelledFailureProbeCannotOverwriteNewerRefreshState() async throws {
        let viewModel = try XCTUnwrap(viewModel)
        let probe = BlockingWorkspacePresenceProbe()
        viewModel.isLoading = true
        viewModel.isRefreshingInFlight = true

        let staleRefresh = viewModel.startRepoTask { [viewModel] in
            await viewModel.handleRefreshFailure(TestRefreshError.failed) {
                probe.run()
            }
        }
        while !probe.hasStarted {
            await Task.yield()
        }

        staleRefresh.cancel()
        viewModel.isLoading = false
        viewModel.isRefreshingInFlight = false
        viewModel.error = "newer refresh"
        probe.finish()
        await staleRefresh.value

        XCTAssertFalse(viewModel.workspaceVanished)
        XCTAssertEqual(viewModel.error, "newer refresh")
    }

    private func snapshot(entries: [GraphEntry], isComplete: Bool) -> LogGraphSnapshot {
        LogGraphSnapshot(
            entries: entries,
            layout: computeDagLayout(entries: entries),
            loadedRows: UInt32(entries.count),
            isComplete: isComplete
        )
    }
}

private enum TestRefreshError: Error {
    case failed
}

private final class BlockingWorkspacePresenceProbe: @unchecked Sendable {
    private let lock = NSLock()
    private let release = DispatchSemaphore(value: 0)
    private var started = false

    var hasStarted: Bool {
        lock.lock()
        defer { lock.unlock() }
        return started
    }

    func run() -> WorkspacePresence {
        lock.lock()
        started = true
        lock.unlock()
        release.wait()
        return .gone
    }

    func finish() {
        release.signal()
    }
}

private final class BlockingGraphRepo: JayJayRepo, @unchecked Sendable {
    private let lock = NSLock()
    private let release = DispatchSemaphore(value: 0)
    private let events: [LogGraphEvent]?
    private var recordedRequests: [LogGraphRequest] = []
    private var recordedAncillaryLoadCount = 0

    init(events: [LogGraphEvent]? = nil) {
        self.events = events
        super.init(noHandle: .init())
    }

    required init(unsafeFromHandle handle: UInt64) {
        events = nil
        super.init(unsafeFromHandle: handle)
    }

    var hasStarted: Bool {
        requestCount > 0
    }

    var requestCount: Int {
        lock.withLock { recordedRequests.count }
    }

    var requests: [LogGraphRequest] {
        lock.withLock { recordedRequests }
    }

    var ancillaryLoadCount: Int {
        lock.withLock { recordedAncillaryLoadCount }
    }

    override func refreshWorkingCopy() throws {}
    override func listBookmarks() throws -> [BookmarkInfo] {
        lock.withLock { recordedAncillaryLoadCount += 1 }
        return []
    }

    override func workspaceList() throws -> [WorkspaceInfo] {
        []
    }

    override func prHostName() -> String? {
        nil
    }

    override func diffStats(rev: String) throws -> DiffStats {
        DiffStats(filesChanged: 0, insertions: 0, deletions: 0)
    }

    override func currentOperationDescription() -> String {
        ""
    }

    override func showSummary(rev: String) throws -> ChangeDetail {
        throw TestRefreshError.failed
    }

    override func startLogGraph(
        request: LogGraphRequest,
        token _: JayJayGraphLoadToken,
        observer: LogGraphObserver
    ) {
        lock.withLock { recordedRequests.append(request) }
        if let events {
            events.forEach { observer.onEvent(event: $0) }
            return
        }
        release.wait()
    }

    func finish(count: Int = 1) {
        for _ in 0 ..< count {
            release.signal()
        }
    }
}
