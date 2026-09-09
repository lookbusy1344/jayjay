import Foundation
import JayJayCore

private struct RepoRefreshAncillary {
    let context: RepoRefreshContext
    let selectedChange: ChangeDetail?
}

enum BackgroundRefreshRequest {
    case checkOperation
    case reload
}

struct RepoGraphRefreshContext: Sendable {
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
    func handleOperationChange() {
        pendingBackgroundRefresh = pendingBackgroundRefresh ?? .checkOperation
        resumePendingBackgroundRefresh()
    }

    func handleWorkingCopyChange() {
        guard !isShuttingDown else { return }
        // Remember an event while editing even if a mutation also stamped the echo window; resume without re-checking the stamp once editing ends.
        if isBackgroundRefreshSuspended {
            pendingBackgroundRefresh = .reload
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

    func refresh(
        selecting preferredRev: String? = nil,
        isAutoTriggered: Bool = false,
        snapshotWorkingCopy: Bool = true
    ) {
        guard !isShuttingDown else { return }
        // Don't pile FS-triggered refreshes on an in-flight one — our own refreshWorkingCopy re-fires the watcher.
        if isAutoTriggered, isRefreshingInFlight {
            pendingBackgroundRefresh = .reload
            cancelGraphLoad()
            return
        }
        // A reload while focused (auto or manual) is not a revset replacement: it neither dims the graph
        // nor scrolls, but the composed revset can emit descendants above the selected row, so hold the
        // current selection across snapshots rather than letting the fallback snap to `@`. Focus/clear
        // transitions set their own pinned target first; leave it untouched.
        if focusedRevision != nil, pendingFocusTarget == nil {
            captureGraphReplacementBackup()
            pendingFocusTarget = (selectedChangeId ?? focusedRevision)
                .map { focusTarget(for: $0, revealsOnAppear: false) }
        }
        let generation = beginNewGraphRefresh()
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
            revset: effectiveRevset,
            isAutoTriggered: isAutoTriggered
        )
        let token = JayJayGraphLoadToken()
        graphLoadToken = token
        graphLoadGeneration = generation
        let request = Self.graphRequest(revset: context.revset, rowCeiling: graphRowCeiling)
        graphLoadSlowTask = scheduleSlowLoadTask(generation: generation, request: request)
        let observer = MainActorLogGraphObserver { [weak self] event in
            self?.applyLogGraphEvent(event, context: context)
        }
        startGraphRefresh(RepoGraphRefreshRun(
            context: context,
            snapshotWorkingCopy: snapshotWorkingCopy,
            includeSubmoduleStatuses: includeSubmoduleStatuses,
            token: token,
            observer: observer,
            request: request
        ))
    }

    private func beginNewGraphRefresh() -> UInt64 {
        refreshTask?.cancel()
        graphLoadToken?.cancel()
        graphLoadSlowTask?.cancel()
        graphRefreshGeneration &+= 1
        isRefreshingInFlight = true
        isLoading = graphEntries.isEmpty
        graphLoadCanceling = false
        graphLoadSlow = false
        graphPaused = false
        graphFirstSnapshotApplied = false
        graphPendingSelectedChange = nil
        canLoadMore = false
        return graphRefreshGeneration
    }

    private func scheduleSlowLoadTask(generation: UInt64, request: LogGraphRequest) -> Task<Void, Never> {
        Task { @MainActor [weak self] in
            try? await Task.sleep(for: .milliseconds(Int64(clamping: request.firstResultBudgetMs)))
            guard !Task.isCancelled,
                  let self,
                  graphRefreshGeneration == generation,
                  !self.graphFirstSnapshotApplied,
                  graphLoadToken != nil
            else { return }
            graphLoadSlow = true
        }
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
        apply(ancillary.context)
        graphPendingSelectedChange = ancillary.selectedChange
    }

    @MainActor
    func apply(_ context: RepoRefreshContext) {
        bookmarks = context.bookmarks
        if let workspaces = context.workspaces {
            self.workspaces = workspaces
        }
        prHostName = context.prHostName
        apply(context.statusBar)
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

    /// A mutation cannot share a repository generation with a pinned graph reader. Reject queued
    /// graph events before the write begins; the canceled worker still observes its core token.
    func cancelGraphLoadForMutation() {
        guard graphLoadToken != nil else { return }
        cancelGraphLoad()
        graphRefreshGeneration &+= 1
        isRefreshingInFlight = false
        isLoading = false
    }

    func continueLoading() {
        guard graphPaused, let graphLoadToken else { return }
        let currentCeiling = graphRowCeiling == 0
            ? defaultLogGraphRequest(revset: revset).rowCeiling
            : graphRowCeiling
        graphRowCeiling = currentCeiling.multipliedReportingOverflow(by: 2).overflow
            ? UInt32.max
            : currentCeiling * 2
        graphPaused = false
        isRefreshingInFlight = true
        graphLoadToken.continueLoading(rowCeiling: graphRowCeiling)
    }

    func loadMore() {
        guard !isShuttingDown, focusedRevision == nil, canLoadMore,
              let currentDepth = Self.defaultRevsetDepth(for: revset) else { return }
        revset = Self.buildDefaultRevset(depth: currentDepth + Self.defaultRevsetPageSize)
        refresh()
    }

    func resumePendingBackgroundRefresh(afterFailure: Bool = false) {
        if afterFailure, pendingBackgroundRefresh != nil {
            pendingBackgroundRefresh = .reload
        }
        guard !isShuttingDown, !isBackgroundRefreshSuspended, !isRefreshingInFlight,
              let pending = pendingBackgroundRefresh else { return }
        pendingBackgroundRefresh = nil
        // Compare only after a successful load; a failed load may have advanced the repo without updating the UI.
        if pending == .checkOperation, (try? repo.isAtOperationHead()) == true {
            return
        }
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
            context: RepoRefreshContext(repo: repo),
            selectedChange: try? loadSummaryWithConflicts(
                repo: repo,
                rev: preferredRev,
                includeSubmoduleStatuses: includeSubmoduleStatuses
            )
        )
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
        let boxIsClean = commitDraftIsClean(
            summary: commitSummaryDraft,
            body: commitDescriptionDraft,
            message: previousDescription
        )
        guard boxIsClean else { return }
        commitSummaryDraft = commitSummary(message: description)
        commitDescriptionDraft = commitBody(message: description)
    }
}
