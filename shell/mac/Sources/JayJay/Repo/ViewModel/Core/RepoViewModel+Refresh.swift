import Foundation
import JayJayCore

private struct RepoRefreshAncillary {
    let bookmarks: [BookmarkInfo]
    let workspaces: [WorkspaceInfo]?
    let prHostName: String?
    let statusBar: StatusBarSnapshot
    let selectedChange: ChangeDetail?
}

private struct RepoGraphRefreshContext: Sendable {
    let generation: UInt64
    let preferredCommitId: String?
    let preferredRev: String?
    let revset: String
    let isAutoTriggered: Bool
}

private struct RepoGraphRefreshRun: Sendable {
    let context: RepoGraphRefreshContext
    let snapshotWorkingCopy: Bool
    let includeSubmoduleStatuses: Bool
    let token: JayJayGraphLoadToken
    let observer: MainActorLogGraphObserver
    let request: LogGraphRequest
}

extension RepoViewModel {
    func handleWorkingCopyChange() {
        guard !isShuttingDown else { return }
        // Remember an event while editing even if a mutation also stamped the echo window; resume without re-checking the stamp once editing ends.
        if isBackgroundRefreshSuspended {
            hasPendingBackgroundRefresh = true
            return
        }
        // Ignore the FS echo from our own mutations — perform() already refreshed.
        if let last = lastInternalMutationAt, Date().timeIntervalSince(last) < 5 {
            return
        }
        refresh(isAutoTriggered: true)
    }

    func setBackgroundRefreshSuspended(_ suspended: Bool) {
        guard isBackgroundRefreshSuspended != suspended else { return }
        isBackgroundRefreshSuspended = suspended
        resumePendingBackgroundRefresh()
    }

    func fetchPrInfo(bookmarks: [String]) {
        clearPrInfo()
        guard !isShuttingDown, let bookmark = bookmarks.first else { return }
        prFetchTask = startRepoTask { [weak self, repo] in
            let info = repo.pullRequestInfo(bookmark: bookmark)
            guard !Task.isCancelled else { return }
            await self?.applyPrInfo(info)
        }
    }

    func clearPrInfo() {
        prFetchTask?.cancel()
        prFetchTask = nil
        prInfo = nil
    }

    @MainActor
    private func applyPrInfo(_ info: PrInfo?) {
        guard !isShuttingDown else { return }
        prInfo = info
    }

    func applyRevset(_ newRevset: String) {
        revset = newRevset
        graphRowCeiling = 0
        graphPaused = false
        refresh(selecting: "@")
    }

    func refresh(
        selecting preferredRev: String? = nil,
        isAutoTriggered: Bool = false,
        snapshotWorkingCopy: Bool = true
    ) {
        guard !isShuttingDown else { return }
        // Don't pile FS-triggered refreshes on an in-flight one — our own refreshWorkingCopy re-fires the watcher.
        if isAutoTriggered, isRefreshingInFlight {
            hasPendingBackgroundRefresh = true
            cancelGraphLoad()
            return
        }
        refreshTask?.cancel()
        graphLoadToken?.cancel()
        graphRefreshGeneration &+= 1
        let generation = graphRefreshGeneration
        isRefreshingInFlight = true
        isLoading = graphEntries.isEmpty
        graphLoadCanceling = false
        graphPaused = false
        graphFirstSnapshotApplied = false
        graphPendingSelectedChange = nil
        canLoadMore = false
        // A background refresh must not dismiss an error the user is still reading; manual refresh is an explicit retry.
        if !isAutoTriggered {
            error = nil
        }
        let preferredSelection = preferredRev ?? selectedChangeId
        let preferredCommitId = graphEntries.first(where: {
            guard let preferredSelection else { return false }
            return $0.change.matchesRevision(preferredSelection)
        })?.change.commitId.id
        let context = RepoGraphRefreshContext(
            generation: generation,
            preferredCommitId: preferredCommitId,
            preferredRev: preferredSelection,
            revset: revset,
            isAutoTriggered: isAutoTriggered
        )
        let token = JayJayGraphLoadToken()
        graphLoadToken = token
        let observer = MainActorLogGraphObserver { [weak self] event in
            self?.applyLogGraphEvent(event, context: context)
        }
        startGraphRefresh(RepoGraphRefreshRun(
            context: context,
            snapshotWorkingCopy: snapshotWorkingCopy,
            includeSubmoduleStatuses: includeSubmoduleStatuses,
            token: token,
            observer: observer,
            request: Self.graphRequest(revset: context.revset, rowCeiling: graphRowCeiling)
        ))
    }

    private func startGraphRefresh(_ run: RepoGraphRefreshRun) {
        refreshTask = startRepoTask { [weak self, repo] in
            await withTaskCancellationHandler {
                do {
                    if run.snapshotWorkingCopy {
                        try repo.refreshWorkingCopy()
                    }
                    guard !Task.isCancelled else {
                        await self?.finishCanceledGraphLoad(generation: run.context.generation)
                        return
                    }
                    let ancillary = try Self.loadRefreshAncillary(
                        repo: repo,
                        preferredRev: run.context.preferredRev ?? "@",
                        includeSubmoduleStatuses: run.includeSubmoduleStatuses
                    )
                    guard !Task.isCancelled else {
                        await self?.finishCanceledGraphLoad(generation: run.context.generation)
                        return
                    }
                    await self?.applyRefreshAncillary(ancillary, generation: run.context.generation)
                    guard !Task.isCancelled else {
                        await self?.finishCanceledGraphLoad(generation: run.context.generation)
                        return
                    }
                    repo.startLogGraph(request: run.request, token: run.token, observer: run.observer)
                } catch {
                    guard !Task.isCancelled else {
                        await self?.finishCanceledGraphLoad(generation: run.context.generation)
                        return
                    }
                    let presence = repo.workspacePresence()
                    await self?.applyGraphLoadFailure(error, presence: presence, generation: run.context.generation)
                }
            } onCancel: {
                run.token.cancel()
            }
        }
    }

    @MainActor
    private func applyRefreshAncillary(_ ancillary: RepoRefreshAncillary, generation: UInt64) {
        guard !isShuttingDown, graphRefreshGeneration == generation else { return }
        bookmarks = ancillary.bookmarks
        if let workspaces = ancillary.workspaces {
            self.workspaces = workspaces
        }
        prHostName = ancillary.prHostName
        apply(ancillary.statusBar)
        graphPendingSelectedChange = ancillary.selectedChange
    }

    @MainActor
    func applyGraphSnapshot(
        _ snapshot: LogGraphSnapshot,
        preferredCommitId: String?,
        preferredRev: String?
    ) {
        let isFirst = !graphFirstSnapshotApplied
        graphFirstSnapshotApplied = true
        dagLayout = DAGLayout(computed: snapshot.layout)

        if isFirst {
            graphEntries = snapshot.entries
        } else {
            assert(snapshot.entries.count >= graphEntries.count)
            assert(zip(graphEntries, snapshot.entries).allSatisfy { pair in
                pair.0.change.commitId == pair.1.change.commitId
            })
            graphEntries.append(contentsOf: snapshot.entries.dropFirst(graphEntries.count))
        }

        if snapshot.isComplete {
            canLoadMore = Self.canLoadMore(revset: revset, loadedCount: graphEntries.count)
        }
        if let workingCopy = snapshot.entries.first(where: { $0.change.isWorkingCopy })?.change {
            applyWorkingCopy(changeId: workingCopy.changeId.id, description: workingCopy.description)
        }
        isLoading = false

        guard isFirst else { return }
        let selected = preferredCommitId.flatMap { commitId in
            snapshot.entries.first(where: { $0.change.commitId.id == commitId })
        } ?? preferredRev.flatMap { rev in
            snapshot.entries.first(where: { $0.change.matchesRevision(rev) })
        } ?? snapshot.entries.first(where: { $0.change.isWorkingCopy }) ?? snapshot.entries.first
        if let selected,
           let detail = graphPendingSelectedChange,
           detail.info.commitId == selected.change.commitId
        {
            applySingleSelectedChange(detail)
            fetchPrInfo(bookmarks: detail.info.bookmarks)
        } else {
            select(changeId: selected?.change.selectionRevision)
        }
        graphPendingSelectedChange = nil
    }

    @MainActor
    private func applyLogGraphEvent(
        _ event: LogGraphEvent,
        context: RepoGraphRefreshContext
    ) {
        guard !isShuttingDown, graphRefreshGeneration == context.generation else { return }

        switch event {
            case let .snapshot(snapshot):
                if context.isAutoTriggered, isBackgroundRefreshSuspended {
                    hasPendingBackgroundRefresh = true
                    cancelGraphLoad()
                    return
                }
                applyGraphSnapshot(
                    snapshot,
                    preferredCommitId: context.preferredCommitId,
                    preferredRev: context.preferredRev
                )
            case .progress:
                break
            case .paused:
                finishGraphLoad(generation: context.generation)
                graphPaused = true
                resumePendingBackgroundRefresh()
            case .finished:
                finishGraphLoad(generation: context.generation)
                canLoadMore = Self.canLoadMore(revset: context.revset, loadedCount: graphEntries.count)
                if context.isAutoTriggered, isBackgroundRefreshSuspended {
                    hasPendingBackgroundRefresh = true
                }
                resumePendingBackgroundRefresh()
            case .canceled:
                finishGraphLoad(generation: context.generation)
                resumePendingBackgroundRefresh()
            case let .failed(message):
                finishGraphLoad(generation: context.generation)
                error = message
                resumePendingBackgroundRefresh()
        }
    }

    @MainActor
    private func finishGraphLoad(generation: UInt64) {
        guard graphRefreshGeneration == generation else { return }
        graphLoadToken = nil
        graphLoadCanceling = false
        isLoading = false
        isRefreshingInFlight = false
    }

    @MainActor
    private func finishCanceledGraphLoad(generation: UInt64) {
        finishGraphLoad(generation: generation)
        resumePendingBackgroundRefresh()
    }

    func refreshOrCancel() {
        if graphLoadToken != nil {
            cancelGraphLoad()
        } else {
            refresh()
        }
    }

    func cancelGraphLoad() {
        guard let graphLoadToken else { return }
        graphLoadToken.cancel()
        refreshTask?.cancel()
        graphLoadCanceling = true
    }

    func continueLoading() {
        guard graphPaused else { return }
        let currentCeiling = graphRowCeiling == 0
            ? defaultLogGraphRequest(revset: revset).rowCeiling
            : graphRowCeiling
        graphRowCeiling = currentCeiling.multipliedReportingOverflow(by: 2).overflow
            ? UInt32.max
            : currentCeiling * 2
        refresh()
    }

    func loadMore() {
        guard !isShuttingDown, canLoadMore, let currentDepth = Self.defaultRevsetDepth(for: revset) else { return }
        revset = Self.buildDefaultRevset(depth: currentDepth + Self.defaultRevsetPageSize)
        refresh()
    }

    func resumePendingBackgroundRefresh() {
        guard !isBackgroundRefreshSuspended, hasPendingBackgroundRefresh else { return }
        hasPendingBackgroundRefresh = false
        refresh(isAutoTriggered: true)
    }

    private static func graphRequest(revset: String, rowCeiling: UInt32) -> LogGraphRequest {
        let defaults = defaultLogGraphRequest(revset: revset)
        return LogGraphRequest(
            revset: defaults.revset,
            initialRows: defaults.initialRows,
            backgroundBatchRows: defaults.backgroundBatchRows,
            firstResultBudgetMs: defaults.firstResultBudgetMs,
            rowCeiling: rowCeiling == 0 ? defaults.rowCeiling : rowCeiling
        )
    }

    private static func loadRefreshAncillary(
        repo: JayJayRepo,
        preferredRev: String,
        includeSubmoduleStatuses: Bool
    ) throws -> RepoRefreshAncillary {
        try RepoRefreshAncillary(
            bookmarks: repo.listBookmarks(),
            workspaces: try? repo.workspaceList(),
            prHostName: repo.prHostName(),
            statusBar: StatusBarSnapshot.load(from: repo),
            selectedChange: try? loadSummaryWithConflicts(
                repo: repo,
                rev: preferredRev,
                includeSubmoduleStatuses: includeSubmoduleStatuses
            )
        )
    }

    @MainActor
    private func applyGraphLoadFailure(
        _ error: any Error,
        presence: WorkspacePresence,
        generation: UInt64
    ) {
        guard graphRefreshGeneration == generation else { return }
        finishGraphLoad(generation: generation)
        applyRefreshFailure(error, presence: presence)
    }
}

extension RepoViewModel {
    /// A clean box follows the working copy; a typed draft is never replaced, even when @ moves to a described change.
    func applyWorkingCopy(changeId: String, description: String) {
        let previousDescription = workingCopyDescription
        workingCopyDescription = description
        guard !changeId.isEmpty else { return }
        let identityChanged = changeId != workingCopyChangeId
        let descriptionChanged = description != previousDescription
        guard identityChanged || descriptionChanged else { return }
        workingCopyChangeId = changeId
        let boxIsClean = commitSummaryDraft == commitSummary(message: previousDescription)
            && commitDescriptionDraft == commitBody(message: previousDescription)
        guard boxIsClean else { return }
        commitSummaryDraft = commitSummary(message: description)
        commitDescriptionDraft = commitBody(message: description)
    }
}
