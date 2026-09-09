import Foundation
import JayJayCore

struct GraphReplacementBackup {
    let entries: [GraphEntry]
    let layout: DAGLayout
}

extension RepoViewModel {
    func applyRevset(_ newRevset: String, selecting revision: String = "@") {
        // A new base filter shows in full: a stale focus would silently scope it to an unrelated change.
        focusedRevision = nil
        focusedCommitId = nil
        pendingFocusTarget = nil
        revset = newRevset
        markGraphAwaitingReplacement()
        resetGraphPaging()
        refresh(selecting: revision)
    }

    /// Pin `revision` across progressive snapshots, resolving a divergent row's commit id so its other
    /// versions can't satisfy the pin.
    func focusTarget(for revision: String, revealsOnAppear: Bool) -> PendingFocusTarget {
        let row = graphEntries.first(where: { $0.change.matchesRevision(revision) })?.change
        return PendingFocusTarget(
            revision: revision,
            commitId: row?.isDivergent == true ? row?.commitId.id : nil,
            revealsOnAppear: revealsOnAppear
        )
    }

    /// Scope the graph to `revision`'s connected lineage. Composition is always over the base `revset`,
    /// not the current effective one, so focusing a second change replaces the first.
    func focus(on revision: String) {
        guard focusedRevision != revision else { return }
        let target = focusTarget(for: revision, revealsOnAppear: true)
        focusedRevision = revision
        focusedCommitId = target.commitId
        pendingFocusTarget = target
        markGraphAwaitingReplacement()
        resetGraphPaging()
        refresh(selecting: revision)
    }

    /// Keyboard/menu path to focus the current selection. Reachable when no row is clipped, so focus
    /// is not badge-only. No-op without a selection or when already focused on that revision.
    func focusSelectedChange() {
        guard let revision = selectedChangeId, revision != focusedRevision else { return }
        focus(on: revision)
    }

    func clearFocus() {
        guard focusedRevision != nil else { return }
        focusedRevision = nil
        focusedCommitId = nil
        // Pin the current selection so it survives the full graph's progressive snapshots and scrolls back into view; without the pin the taller base graph keeps its offset and leaves the selected row below the fold, or a row streamed after the first prefix is never reselected.
        pendingFocusTarget = selectedChangeId.map { focusTarget(for: $0, revealsOnAppear: true) }
        markGraphAwaitingReplacement()
        resetGraphPaging()
        refresh(selecting: selectedChangeId)
    }

    /// The revset actually queried: the base scoped to the focus target's lineage, or the base itself.
    var effectiveRevset: String {
        focusedRevision.map { focusRevset(base: revset, target: $0) } ?? revset
    }

    func resetGraphPaging() {
        graphRowCeiling = 0
        graphPaused = false
    }

    /// Retain the current graph so a focused refresh that loses its target can restore it. Cheap: the
    /// array is copy-on-write, so nothing is duplicated until the graph is next mutated.
    func captureGraphReplacementBackup() {
        if graphReplacementBackup == nil, !graphEntries.isEmpty {
            graphReplacementBackup = GraphReplacementBackup(entries: graphEntries, layout: dagLayout)
        }
    }

    func markGraphAwaitingReplacement() {
        captureGraphReplacementBackup()
        isGraphAwaitingReplacement = !graphEntries.isEmpty
    }

    @discardableResult
    func restoreGraphReplacement() -> Bool {
        guard let backup = graphReplacementBackup else { return false }
        restoreGraph(backup)
        graphReplacementBackup = nil
        isGraphAwaitingReplacement = !graphEntries.isEmpty
        return true
    }

    /// Restore the prior graph and keep the focus pill when a focused refresh no longer contains its
    /// focus root (a rewrite invalidated the pinned commit id), rather than presenting a target-less
    /// graph as current. No-op when unfocused or the root is present. Returns whether recovery ran.
    func recoverMissingFocusTarget(in entries: [GraphEntry]) -> Bool {
        guard let revision = focusedRevision else { return false }
        let root = PendingFocusTarget(revision: revision, commitId: focusedCommitId)
        guard !entries.contains(where: { root.matches($0.change) }) else { return false }
        pendingFocusTarget = nil
        isLoading = false
        if !restoreGraphReplacement() {
            isGraphAwaitingReplacement = !graphEntries.isEmpty
        }
        error = "Focused change is no longer in the graph."
        return true
    }
}
