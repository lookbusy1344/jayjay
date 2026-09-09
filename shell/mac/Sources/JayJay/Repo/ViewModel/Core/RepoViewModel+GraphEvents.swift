import JayJayCore

extension RepoViewModel {
    @MainActor
    func applyLogGraphEvent(_ event: LogGraphEvent, context: RepoGraphRefreshContext) {
        guard !isShuttingDown else { return }
        guard graphRefreshGeneration == context.generation else {
            if event.isGraphLoadUpdate {
                return
            }
            finishGraphLoad(generation: context.generation)
            return
        }

        switch event {
            case let .snapshot(snapshot):
                applySnapshotEvent(snapshot, context: context)
            case let .progress(_, _, _, firstResultBudgetExpired):
                applyGraphProgress(firstResultBudgetExpired: firstResultBudgetExpired)
            case let .emptyStates(updates):
                applyEmptyStates(updates)
            case .paused:
                graphPaused = true
                isLoading = false
                isRefreshingInFlight = false
                resumePendingBackgroundRefresh()
            case .finished:
                applyGraphFinished(context: context)
            case .canceled:
                restoreGraphReplacement()
                finishGraphLoad(generation: context.generation)
                resumePendingBackgroundRefresh()
            case let .failed(message):
                restoreGraphReplacement()
                finishGraphLoad(generation: context.generation)
                error = message
                resumePendingBackgroundRefresh()
        }
    }

    @MainActor
    private func applyGraphFinished(context: RepoGraphRefreshContext) {
        finishGraphLoad(generation: context.generation)
        // The core omits the terminal is_complete snapshot when every row was already streamed, so the
        // focus-root recovery that applyGraphSnapshot runs on completion must also run here.
        if !recoverMissingFocusTarget(in: graphEntries) {
            canLoadMore = Self.canLoadMore(revset: context.revset, loadedCount: graphEntries.count)
            if context.isAutoTriggered, isBackgroundRefreshSuspended {
                pendingBackgroundRefresh = .reload
            }
        }
        resumePendingBackgroundRefresh()
    }

    @MainActor
    private func applyGraphProgress(firstResultBudgetExpired: Bool) {
        guard !graphFirstSnapshotApplied, firstResultBudgetExpired else { return }
        graphLoadSlow = true
    }

    /// Apply deferred `is_empty` corrections to already-published rows. Merge and off-page rows are
    /// published as non-empty and refined once their parent-tree merge completes off the first-paint
    /// path; corrections arrive after every snapshot, so `graphEntries` is stable here.
    @MainActor
    private func applyEmptyStates(_ updates: [EmptyStateUpdate]) {
        guard !updates.isEmpty else { return }
        var correctionsByCommitId: [String: Bool] = [:]
        correctionsByCommitId.reserveCapacity(updates.count)
        for update in updates {
            correctionsByCommitId[update.commitId] = update.isEmpty
        }
        applyEmptyStateCorrections(correctionsByCommitId)
    }

    @MainActor
    private func applySnapshotEvent(_ snapshot: LogGraphSnapshot, context: RepoGraphRefreshContext) {
        if context.isAutoTriggered, isBackgroundRefreshSuspended {
            pendingBackgroundRefresh = .reload
            cancelGraphLoad()
            return
        }
        applyGraphSnapshot(
            snapshot,
            preferredCommitId: context.preferredCommitId,
            preferredRev: context.preferredRev
        )
    }

    @MainActor
    func finishGraphLoad(generation: UInt64) {
        guard graphLoadGeneration == generation else { return }
        graphLoadToken = nil
        graphLoadGeneration = nil
        graphLoadSlowTask?.cancel()
        graphLoadSlowTask = nil
        graphLoadSlow = false
        graphLoadCanceling = false
        if graphRefreshGeneration == generation {
            isLoading = false
            isRefreshingInFlight = false
        }
    }

    @MainActor
    func finishCanceledGraphLoad(generation: UInt64) {
        finishGraphLoad(generation: generation)
        resumePendingBackgroundRefresh()
    }

    @MainActor
    func applyGraphLoadFailure(
        _ error: any Error,
        presence: WorkspacePresence,
        generation: UInt64
    ) {
        guard graphRefreshGeneration == generation else { return }
        restoreGraphReplacement()
        finishGraphLoad(generation: generation)
        applyRefreshFailure(error, presence: presence)
    }
}

private extension LogGraphEvent {
    var isGraphLoadUpdate: Bool {
        switch self {
            case .snapshot, .progress, .emptyStates: true
            case .finished, .paused, .canceled, .failed: false
        }
    }
}
