import Foundation
import JayJayCore

extension RepoViewModel {
    @MainActor
    func applyGraphSnapshot(
        _ snapshot: LogGraphSnapshot,
        preferredCommitId: String?,
        preferredRev: String?
    ) {
        // A focused refresh that completes without surfacing the focus root means the root left the
        // graph. Snapshot entries are cumulative, so a complete snapshot missing the root is
        // authoritative. The core omits this terminal snapshot when every row was already streamed, so
        // applyGraphFinished repeats the check for that path.
        if snapshot.isComplete, recoverMissingFocusTarget(in: snapshot.entries) {
            return
        }

        isGraphAwaitingReplacement = false

        let isFirst = !graphFirstSnapshotApplied
        graphFirstSnapshotApplied = true
        graphLoadSlowTask?.cancel()
        graphLoadSlowTask = nil
        graphLoadSlow = false

        let mergedEntries: [GraphEntry]
        if isFirst {
            mergedEntries = snapshot.entries
        } else {
            assert(snapshot.entries.count >= graphEntries.count)
            assert(zip(graphEntries, snapshot.entries).allSatisfy { pair in
                pair.0.change.commitId == pair.1.change.commitId
            })
            mergedEntries = graphEntries + snapshot.entries.dropFirst(graphEntries.count)
        }
        setStreamedGraph(mergedEntries, layout: snapshot.layout)

        if snapshot.isComplete {
            // Focus disables infinite-scroll paging: the composed revset is not a pageable default.
            canLoadMore = focusedRevision == nil
                && Self.canLoadMore(revset: revset, loadedCount: graphEntries.count)
            graphReplacementBackup = nil
        }
        if let workingCopy = snapshot.entries.first(where: { $0.change.isWorkingCopy })?.change {
            applyWorkingCopy(changeId: workingCopy.changeId.id, description: workingCopy.description)
        }
        isLoading = false

        // A pinned focus target may surface in any snapshot, not just the first. Select and reveal it
        // once it appears; until then hold selection rather than falling back to `@` or the first row.
        if let target = pendingFocusTarget {
            guard let appeared = snapshot.entries.first(where: { target.matches($0.change) }) else { return }
            pendingFocusTarget = nil
            graphPendingSelectedChange = nil
            select(changeId: appeared.change.selectionRevision)
            if target.revealsOnAppear {
                pendingDagReveal = DAGRevealRequest(changeId: appeared.change.selectionRevision)
            }
            return
        }

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
}
