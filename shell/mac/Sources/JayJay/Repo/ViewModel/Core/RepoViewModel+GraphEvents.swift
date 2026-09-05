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
    private func applySnapshotEvent(_ snapshot: LogGraphSnapshot, context: RepoGraphRefreshContext) {
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
    }

    @MainActor
    func finishGraphLoad(generation: UInt64) {
        guard graphLoadGeneration == generation else { return }
        graphLoadToken = nil
        graphLoadGeneration = nil
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
        finishGraphLoad(generation: generation)
        applyRefreshFailure(error, presence: presence)
    }
}

private extension LogGraphEvent {
    var isGraphLoadUpdate: Bool {
        switch self {
            case .snapshot, .progress: true
            case .finished, .paused, .canceled, .failed: false
        }
    }
}
