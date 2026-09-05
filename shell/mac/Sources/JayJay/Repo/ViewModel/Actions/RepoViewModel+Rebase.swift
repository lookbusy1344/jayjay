import Foundation
import JayJayCore

struct RepoRebaseFeedback {
    let message: String
    let undoOperationId: String?
}

private struct RepoRebaseRefreshResult {
    let hadConflicts: Bool
    let undoOperationId: String?
}

extension RepoViewModel {
    func rebase(
        request: DAGRebaseRequest,
        onSuccess: @escaping @MainActor (RepoViewModel, RepoRebaseFeedback) -> Void,
        onFailure: @escaping @MainActor (RepoViewModel, String) -> Void = { viewModel, message in
            viewModel.error = message
        }
    ) {
        lastInternalMutationAt = Date()
        isRefreshingInFlight = true
        error = nil
        runRepoTask { repo in
            try Self.performRebase(
                repo: repo,
                request: request
            )
        } onSuccess: { viewModel, result in
            viewModel.successActionSignal += 1
            viewModel.refresh(selecting: request.sourceChangeId, snapshotWorkingCopy: false)

            onSuccess(viewModel, RepoRebaseFeedback(
                message: Self.rebaseMessage(for: request, hadConflicts: result.hadConflicts),
                undoOperationId: result.undoOperationId
            ))
        } onFailure: { viewModel, error in
            viewModel.isLoading = false
            viewModel.isRefreshingInFlight = false
            viewModel.resumePendingBackgroundRefresh()
            onFailure(viewModel, error.friendlyDescription)
        }
    }

    private static func performRebase(
        repo: JayJayRepo,
        request: DAGRebaseRequest
    ) throws -> RepoRebaseRefreshResult {
        let undoOperationId = try repo.opLog().first(where: { $0.isCurrent })?.id.id
        try repo.rebase(rev: request.sourceRev, dest: request.destRev)
        try repo.refreshWorkingCopy()

        let hadConflicts = (try? repo.showSummary(rev: request.sourceChangeId).info.hasConflict) ?? false

        return RepoRebaseRefreshResult(
            hadConflicts: hadConflicts,
            undoOperationId: undoOperationId
        )
    }

    private static func rebaseMessage(for request: DAGRebaseRequest, hadConflicts: Bool) -> String {
        let base = "Rebased \(request.sourceLabel) onto \(request.destLabel)."
        guard hadConflicts else { return base }
        return "\(base) Conflicts need resolution."
    }
}
