import Foundation
import JayJayCore

extension RepoViewModel {
    func revealAncestors(of headChangeId: String, selecting revision: String) {
        reloadFilter(revsetFilterShowAncestors(state: revsetFilter, changeId: headChangeId), selecting: revision)
    }

    func applyFilter(_ newRevset: String, selecting revision: String) {
        let isEmpty = newRevset.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        let filter = revsetFilterApply(state: revsetFilter, revset: isEmpty ? defaultRevsetPreset.revset : newRevset)
        let recent = recentRevsetsRemember(repositoryPath: primaryRoot, revset: filter.revset)
        reloadFilter(RevsetFilterState(revset: filter.revset, previous: filter.previous, recent: recent), selecting: revision)
    }

    func returnToPreviousRevset() {
        reloadFilter(revsetFilterBack(state: revsetFilter), selecting: "@")
    }

    func forgetRecentRevset(_ revset: String) {
        let recent = recentRevsetsForget(repositoryPath: primaryRoot, revset: revset)
        revsetFilter = RevsetFilterState(revset: revsetFilter.revset, previous: revsetFilter.previous, recent: recent)
    }

    private func reloadFilter(_ filter: RevsetFilterState, selecting revision: String) {
        // A new base filter shows in full: a stale focus or expansion would silently reshape it around unrelated changes.
        expandedElisions = []
        focusedRevision = nil
        focusedCommitId = nil
        pendingFocusTarget = nil
        revsetFilter = filter
        markGraphAwaitingReplacement()
        resetGraphPaging()
        refresh(selecting: revision)
    }

    func visibleRevision(of rev: String) -> String? {
        guard let commitId = (try? repo.log(revset: rev))?.first?.commitId.id else { return nil }
        return changes.first { $0.commitId.id == commitId }?.selectionRevision
    }

    func revealInGraph(_ revision: String) {
        dagRevealRequest = DAGRevealRequest(changeId: revision)
        select(changeId: revision)
    }

    func revealBookmark(_ target: BookmarkFilterTarget) {
        if let revision = visibleRevision(of: target.head) {
            revealInGraph(revision)
        } else {
            applyFilter(target.revset, selecting: target.head)
        }
    }

    func filterByBookmark(_ target: BookmarkFilterTarget) {
        applyFilter(target.revset)
    }

    func filterByWorkspace(name: String) {
        applyFilter(workspaceFilterRevset(name: name))
    }

    func revsetError(_ revset: String) -> String? {
        do {
            try repo.checkRevset(revset: revset)
            return nil
        } catch {
            return error.friendlyDescription
        }
    }
}
