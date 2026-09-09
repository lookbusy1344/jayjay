import Foundation
@testable import JayJay
import JayJayCore
import XCTest

@MainActor
final class RepoViewModelFocusTests: XCTestCase {
    private func makeViewModel(repo: JayJayRepo) -> RepoViewModel {
        RepoViewModel(path: "/tmp", repo: repo, workingCopyIsLarge: false, configWarning: nil, startsFileWatcher: false)
    }

    private func snapshot(entries: [GraphEntry], isComplete: Bool) -> LogGraphSnapshot {
        LogGraphSnapshot(
            entries: entries,
            layout: computeDagLayout(entries: entries, syntheticElidedNodes: true),
            loadedRows: UInt32(entries.count),
            isComplete: isComplete
        )
    }

    private func entry(changeId: String, commitId: String, isDivergent: Bool = false) -> GraphEntry {
        GraphEntry(
            change: mockChangeInfo(changeId: changeId, commitId: commitId, isDivergent: isDivergent),
            edges: []
        )
    }

    func testFocusSendsComposedEffectiveRevsetAndResetsPaging() async throws {
        let repo = FocusCaptureRepo(events: [])
        let viewModel = makeViewModel(repo: repo)
        viewModel.revset = "all()"
        viewModel.graphEntries = [entry(changeId: "T", commitId: "ttt")]
        viewModel.graphRowCeiling = 5
        viewModel.graphPaused = true

        viewModel.focus(on: "T")
        try await waitUntil("the focused graph request starts") { repo.requestCount == 1 }

        XCTAssertEqual(viewModel.focusedRevision, "T")
        XCTAssertEqual(viewModel.effectiveRevset, "(all()) & (::T | T::)")
        XCTAssertEqual(repo.requests.last?.revset, "(all()) & (::T | T::)")
        XCTAssertEqual(viewModel.graphRowCeiling, 0)
        XCTAssertFalse(viewModel.graphPaused)
        XCTAssertTrue(viewModel.isGraphAwaitingReplacement)
    }

    func testClearFocusSendsUnchangedBaseRevset() async throws {
        let repo = FocusCaptureRepo(events: [])
        let viewModel = makeViewModel(repo: repo)
        viewModel.revset = "all()"
        viewModel.graphEntries = [entry(changeId: "T", commitId: "ttt")]

        viewModel.focus(on: "T")
        try await waitUntil("the focused request starts") { repo.requestCount == 1 }
        viewModel.clearFocus()
        try await waitUntil("the cleared request starts") { repo.requestCount == 2 }

        XCTAssertNil(viewModel.focusedRevision)
        XCTAssertEqual(repo.requests.last?.revset, "all()")
        XCTAssertTrue(viewModel.isGraphAwaitingReplacement)
    }

    func testApplyRevsetClearsFocusBeforeSendingNewBase() async throws {
        let repo = FocusCaptureRepo(events: [])
        let viewModel = makeViewModel(repo: repo)
        viewModel.revset = "all()"
        viewModel.graphEntries = [entry(changeId: "T", commitId: "ttt")]

        viewModel.focus(on: "T")
        try await waitUntil("the focused request starts") { repo.requestCount == 1 }
        viewModel.applyRevset("mine()")
        try await waitUntil("the new base request starts") { repo.requestCount == 2 }

        XCTAssertNil(viewModel.focusedRevision)
        XCTAssertNil(viewModel.pendingFocusTarget)
        XCTAssertEqual(repo.requests.last?.revset, "mine()")
    }

    func testDivergentRowSuppliesItsCommitIdNotTheSharedChangeId() async throws {
        let repo = FocusCaptureRepo(events: [])
        let viewModel = makeViewModel(repo: repo)
        viewModel.revset = "all()"
        viewModel.graphEntries = [
            entry(changeId: "shared", commitId: "aaa", isDivergent: true),
            entry(changeId: "shared", commitId: "bbb", isDivergent: true)
        ]

        // selectionRevision of a divergent row is its commit id.
        viewModel.focus(on: "bbb")
        try await waitUntil("the focused request starts") { repo.requestCount == 1 }

        XCTAssertEqual(viewModel.pendingFocusTarget?.commitId, "bbb")
        XCTAssertEqual(repo.requests.last?.revset, "(all()) & (::bbb | bbb::)")
    }

    func testNonDivergentFocusAcceptsARewrittenCommitForTheSameChange() {
        let repo = FocusCaptureRepo(events: [])
        let viewModel = makeViewModel(repo: repo)
        viewModel.revset = "all()"
        viewModel.graphEntries = [entry(changeId: "X", commitId: "old")]

        viewModel.focus(on: "X")
        viewModel.applyGraphSnapshot(
            snapshot(entries: [entry(changeId: "X", commitId: "new")], isComplete: true),
            preferredCommitId: "old",
            preferredRev: "X"
        )

        XCTAssertEqual(viewModel.graphEntries.map(\.change.commitId.id), ["new"])
        XCTAssertEqual(viewModel.selectedChangeId, "X")
        XCTAssertNil(viewModel.error)
    }

    func testPinnedTargetHeldUntilItAppearsThenSelectedAndRevealedOnce() {
        let repo = FocusCaptureRepo(events: [])
        let viewModel = makeViewModel(repo: repo)
        let descendant = entry(changeId: "D", commitId: "ddd")
        let target = entry(changeId: "X", commitId: "xxx")
        viewModel.focusedRevision = "X"
        viewModel.pendingFocusTarget = PendingFocusTarget(revision: "X", commitId: "xxx")

        // First snapshot lacks X (a descendant is emitted above it): selection stays pending.
        viewModel.applyGraphSnapshot(
            snapshot(entries: [descendant], isComplete: false),
            preferredCommitId: "xxx",
            preferredRev: "X"
        )
        XCTAssertNotNil(viewModel.pendingFocusTarget)
        XCTAssertNil(viewModel.selectedChangeId)
        XCTAssertNil(viewModel.pendingDagReveal)
        XCTAssertFalse(viewModel.isGraphAwaitingReplacement)

        // X arrives: select and reveal it once.
        viewModel.applyGraphSnapshot(
            snapshot(entries: [descendant, target], isComplete: false),
            preferredCommitId: "xxx",
            preferredRev: "X"
        )
        XCTAssertNil(viewModel.pendingFocusTarget)
        XCTAssertEqual(viewModel.selectedChangeId, "X")
        let reveal = viewModel.pendingDagReveal
        XCTAssertEqual(reveal?.changeId, "X")

        // A later snapshot keeps X selected without issuing another reveal.
        viewModel.applyGraphSnapshot(
            snapshot(entries: [descendant, target, entry(changeId: "E", commitId: "eee")], isComplete: true),
            preferredCommitId: "xxx",
            preferredRev: "X"
        )
        XCTAssertEqual(viewModel.selectedChangeId, "X")
        XCTAssertEqual(viewModel.pendingDagReveal, reveal)
    }

    func testFocusPagingStaysDisabledWhenTheComposedResultCompletes() {
        let repo = FocusCaptureRepo(events: [])
        let viewModel = makeViewModel(repo: repo)
        viewModel.revset = RepoViewModel.buildDefaultRevset()
        viewModel.focusedRevision = "X"
        viewModel.pendingFocusTarget = PendingFocusTarget(revision: "X", commitId: "xxx")

        viewModel.applyGraphSnapshot(
            snapshot(entries: [entry(changeId: "X", commitId: "xxx")], isComplete: true),
            preferredCommitId: "xxx",
            preferredRev: "X"
        )
        XCTAssertFalse(viewModel.canLoadMore)
    }

    func testFailedFocusedRefreshPreservesPriorGraphAndFocusPill() async throws {
        let repo = FocusCaptureRepo(events: [.failed(message: "revision not found")])
        let viewModel = makeViewModel(repo: repo)
        viewModel.revset = "all()"
        let prior = [entry(changeId: "A", commitId: "aaa")]
        viewModel.graphEntries = prior

        viewModel.focus(on: "aaa")
        try await waitUntil("the focused refresh fails") { viewModel.error != nil }

        XCTAssertEqual(viewModel.graphEntries, prior)
        XCTAssertEqual(viewModel.focusedRevision, "aaa")
        XCTAssertEqual(viewModel.error, "revision not found")
    }

    func testCompletedFocusedRefreshWithoutTargetPreservesPriorGraphAndFocusPill() {
        let repo = FocusCaptureRepo(events: [])
        let viewModel = makeViewModel(repo: repo)
        viewModel.revset = "all()"
        let prior = [entry(changeId: "A", commitId: "aaa"), entry(changeId: "B", commitId: "bbb")]
        viewModel.graphEntries = prior
        viewModel.focusedRevision = "X"
        viewModel.pendingFocusTarget = PendingFocusTarget(revision: "X", commitId: "xxx")

        // The focused revset completed but the pinned target never appeared (a rewrite dropped it).
        viewModel.applyGraphSnapshot(
            snapshot(entries: [entry(changeId: "C", commitId: "ccc")], isComplete: true),
            preferredCommitId: "xxx",
            preferredRev: "X"
        )

        XCTAssertEqual(viewModel.graphEntries, prior)
        XCTAssertEqual(viewModel.focusedRevision, "X")
        XCTAssertNil(viewModel.pendingFocusTarget)
        XCTAssertNotNil(viewModel.error)
        XCTAssertTrue(viewModel.isGraphAwaitingReplacement)
    }

    func testProgressiveFocusedRefreshWithoutTargetRestoresPriorGraph() {
        let repo = FocusCaptureRepo(events: [])
        let viewModel = makeViewModel(repo: repo)
        viewModel.revset = "all()"
        let prior = [entry(changeId: "X", commitId: "xxx"), entry(changeId: "A", commitId: "aaa")]
        viewModel.graphEntries = prior

        viewModel.focus(on: "X")
        viewModel.applyGraphSnapshot(
            snapshot(entries: [entry(changeId: "D", commitId: "ddd")], isComplete: false),
            preferredCommitId: "xxx",
            preferredRev: "X"
        )
        viewModel.applyGraphSnapshot(
            snapshot(entries: [entry(changeId: "D", commitId: "ddd")], isComplete: true),
            preferredCommitId: "xxx",
            preferredRev: "X"
        )

        XCTAssertEqual(viewModel.graphEntries, prior)
        XCTAssertTrue(viewModel.isGraphAwaitingReplacement)
        XCTAssertNotNil(viewModel.error)
    }

    func testStreamedFocusedRefreshFinishingWithoutTargetRestoresPriorGraph() async throws {
        // The core omits the terminal is_complete snapshot when every row streamed already, so a
        // target-less focused result completes via Finished alone. Recovery must still fire.
        let partial = snapshot(entries: [entry(changeId: "D", commitId: "ddd")], isComplete: false)
        let repo = FocusCaptureRepo(events: [.snapshot(snapshot: partial), .finished])
        let viewModel = makeViewModel(repo: repo)
        viewModel.revset = "all()"
        let prior = [entry(changeId: "X", commitId: "xxx"), entry(changeId: "A", commitId: "aaa")]
        viewModel.graphEntries = prior

        viewModel.focus(on: "X")
        try await waitUntil("the focused refresh restores the prior graph") { viewModel.error != nil }

        XCTAssertEqual(viewModel.graphEntries, prior)
        XCTAssertEqual(viewModel.focusedRevision, "X")
        XCTAssertNil(viewModel.pendingFocusTarget)
        XCTAssertTrue(viewModel.isGraphAwaitingReplacement)
    }

    func testFailedProgressiveFocusedRefreshRestoresPriorGraph() async throws {
        let partial = snapshot(entries: [entry(changeId: "D", commitId: "ddd")], isComplete: false)
        let repo = FocusCaptureRepo(events: [.snapshot(snapshot: partial), .failed(message: "stream failed")])
        let viewModel = makeViewModel(repo: repo)
        viewModel.revset = "all()"
        let prior = [entry(changeId: "X", commitId: "xxx"), entry(changeId: "A", commitId: "aaa")]
        viewModel.graphEntries = prior

        viewModel.focus(on: "X")
        try await waitUntil("the progressive focused refresh fails") { viewModel.error != nil }

        XCTAssertEqual(viewModel.graphEntries, prior)
        XCTAssertTrue(viewModel.isGraphAwaitingReplacement)
        XCTAssertEqual(viewModel.error, "stream failed")
    }

    func testLaterFocusedRefreshWithoutTargetPreservesPriorGraph() {
        let repo = FocusCaptureRepo(events: [])
        let viewModel = makeViewModel(repo: repo)
        viewModel.revset = "all()"
        let prior = [entry(changeId: "X", commitId: "xxx")]
        viewModel.graphEntries = prior
        viewModel.focusedRevision = "X"
        viewModel.focusedCommitId = "xxx"
        viewModel.pendingFocusTarget = nil

        viewModel.applyGraphSnapshot(
            snapshot(entries: [], isComplete: true),
            preferredCommitId: "xxx",
            preferredRev: "X"
        )

        XCTAssertEqual(viewModel.graphEntries, prior)
        XCTAssertEqual(viewModel.focusedRevision, "X")
        XCTAssertNotNil(viewModel.error)
        XCTAssertTrue(viewModel.isGraphAwaitingReplacement)
    }

    func testLaterFocusedRefreshWithoutSelectionRepinsTheFocusRoot() async throws {
        let repo = FocusCaptureRepo(events: [])
        let viewModel = makeViewModel(repo: repo)
        viewModel.graphEntries = [entry(changeId: "X", commitId: "xxx")]
        viewModel.focusedRevision = "X"
        viewModel.focusedCommitId = "xxx"
        viewModel.pendingFocusTarget = nil

        viewModel.refresh()
        try await waitUntil("the focused refresh starts") { repo.requestCount == 1 }

        // No selection: fall back to the focus root, held silently (no scroll on a plain reload). X is
        // not divergent, so it is held by change id with no commit pin.
        XCTAssertEqual(
            viewModel.pendingFocusTarget,
            PendingFocusTarget(revision: "X", commitId: nil, revealsOnAppear: false)
        )
    }

    func testFocusedReloadHoldsCurrentSelectionWithoutRevealingOrDimming() {
        let repo = FocusCaptureRepo(events: [])
        let viewModel = makeViewModel(repo: repo)
        viewModel.revset = "all()"
        let target = entry(changeId: "X", commitId: "xxx")
        let descendant = entry(changeId: "Y", commitId: "yyy")
        viewModel.graphEntries = [descendant, target]
        viewModel.focusedRevision = "X"
        viewModel.focusedCommitId = "xxx"
        // The user navigated to a descendant within the focused subgraph.
        viewModel.selectedChangeId = "Y"
        viewModel.pendingFocusTarget = nil
        viewModel.pendingDagReveal = nil

        // A background filesystem event reloads the same focused revset.
        viewModel.refresh(isAutoTriggered: true)

        // The reload pins the current selection, not the focus root, and does not dim the graph. Y is
        // not divergent, so it is held by change id (no commit pin) and survives a later rewrite.
        XCTAssertEqual(
            viewModel.pendingFocusTarget,
            PendingFocusTarget(revision: "Y", commitId: nil, revealsOnAppear: false)
        )
        XCTAssertFalse(viewModel.isGraphAwaitingReplacement)

        // Y is present, so it stays selected with no forced scroll away from where the user is.
        viewModel.applyGraphSnapshot(
            snapshot(entries: [descendant, target], isComplete: true),
            preferredCommitId: "yyy",
            preferredRev: "Y"
        )
        XCTAssertEqual(viewModel.selectedChangeId, "Y")
        XCTAssertNil(viewModel.pendingDagReveal)
    }

    func testClearingFocusPinsTheSelectionAndRevealsItOnceItReappears() {
        let repo = FocusCaptureRepo(events: [])
        let viewModel = makeViewModel(repo: repo)
        viewModel.revset = "all()"
        // Focused on X; the user navigated to a descendant Y within the lineage.
        viewModel.graphEntries = [entry(changeId: "X", commitId: "xxx"), entry(changeId: "Y", commitId: "yyy")]
        viewModel.focusedRevision = "X"
        viewModel.focusedCommitId = "xxx"
        viewModel.selectedChangeId = "Y"

        viewModel.clearFocus()

        XCTAssertNil(viewModel.focusedRevision)
        XCTAssertEqual(
            viewModel.pendingFocusTarget,
            PendingFocusTarget(revision: "Y", commitId: nil, revealsOnAppear: true)
        )

        // Y is absent from the full graph's first prefix: selection stays pending, no reveal.
        viewModel.applyGraphSnapshot(
            snapshot(entries: [entry(changeId: "W", commitId: "www"), entry(changeId: "A", commitId: "aaa")], isComplete: false),
            preferredCommitId: nil,
            preferredRev: "Y"
        )
        XCTAssertNotNil(viewModel.pendingFocusTarget)
        XCTAssertNil(viewModel.pendingDagReveal)

        // Y streams in later: select and reveal it once.
        viewModel.applyGraphSnapshot(
            snapshot(
                entries: [
                    entry(changeId: "W", commitId: "www"),
                    entry(changeId: "A", commitId: "aaa"),
                    entry(changeId: "Y", commitId: "yyy")
                ],
                isComplete: true
            ),
            preferredCommitId: nil,
            preferredRev: "Y"
        )
        XCTAssertNil(viewModel.pendingFocusTarget)
        XCTAssertEqual(viewModel.selectedChangeId, "Y")
        XCTAssertEqual(viewModel.pendingDagReveal?.changeId, "Y")
    }

    private func waitUntil(_ what: String, _ condition: @escaping @MainActor () -> Bool) async throws {
        let deadline = Date().addingTimeInterval(10)
        while !condition() {
            if Date() >= deadline {
                XCTFail("timed out waiting until \(what)")
                throw CancellationError()
            }
            try await Task.sleep(for: .milliseconds(10))
        }
    }
}

/// Captures the revset of each graph request and drives a fixed script of events, so focus
/// composition and the failure-recovery path can be asserted without a live repository.
private final class FocusCaptureRepo: JayJayRepo, @unchecked Sendable {
    private let lock = NSLock()
    private let events: [LogGraphEvent]
    private var recordedRequests: [LogGraphRequest] = []

    init(events: [LogGraphEvent]) {
        self.events = events
        super.init(noHandle: .init())
    }

    required init(unsafeFromHandle handle: UInt64) {
        events = []
        super.init(unsafeFromHandle: handle)
    }

    var requestCount: Int {
        lock.withLock { recordedRequests.count }
    }

    var requests: [LogGraphRequest] {
        lock.withLock { recordedRequests }
    }

    override func refreshWorkingCopy() throws {}
    override func listBookmarks() throws -> [BookmarkInfo] {
        []
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
        throw FocusTestError.noSummary
    }

    override func startLogGraph(
        request: LogGraphRequest,
        token _: JayJayGraphLoadToken,
        observer: LogGraphObserver
    ) {
        lock.withLock { recordedRequests.append(request) }
        events.forEach { observer.onEvent(event: $0) }
    }
}

private enum FocusTestError: Error {
    case noSummary
}
