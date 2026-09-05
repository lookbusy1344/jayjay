import JayJayCore
import SwiftUI

struct DAGViewModel {
    private static let downArrowKeyCode: UInt16 = 125
    private static let upArrowKeyCode: UInt16 = 126

    let entries: [GraphEntry]
    let selectedId: String?
    let selectedIds: [String]
    let compareFromId: String?
    let contextTargetId: String?
    let rebaseDrag: DAGRebaseDragState?
    let bookmarkDrag: BookmarkDragState?
    let colorScheme: ColorScheme
    let layout: DAGLayout
    let geometry: DAGGeometry

    // Derived state built once per view model, not per row. macOS SwiftUI evaluates each row's
    // `.contextMenu` content eagerly on every body update, so any O(entries) work these lookups back
    // would otherwise run per visible row — quadratic on a large graph.
    private let changeByRevision: [String: ChangeInfo]
    private let parentIdsByCommitId: [String: [String]]
    private let selectedChanges: [ChangeInfo]
    private let selectedCommitIds: Set<String>
    /// Commit ids strictly above/below the selection. A row can merge with a single selected change
    /// only when it is independent of it — in neither set — so per-row merge eligibility is an O(1)
    /// lookup rather than an ancestor walk.
    private let selectionAncestors: Set<String>
    private let selectionDescendants: Set<String>

    init(
        entries: [GraphEntry],
        selectedId: String?,
        selectedIds: [String],
        compareFromId: String?,
        contextTargetId: String?,
        rebaseDrag: DAGRebaseDragState?,
        bookmarkDrag: BookmarkDragState?,
        colorScheme: ColorScheme,
        layout: DAGLayout,
        geometry: DAGGeometry
    ) {
        self.entries = entries
        self.selectedId = selectedId
        self.selectedIds = selectedIds
        self.compareFromId = compareFromId
        self.contextTargetId = contextTargetId
        self.rebaseDrag = rebaseDrag
        self.bookmarkDrag = bookmarkDrag
        self.colorScheme = colorScheme
        self.layout = layout
        self.geometry = geometry

        // `matchesRevision` matches either id, so `change(for:)` becomes an O(1) lookup keyed by both,
        // keeping the first entry for a key to mirror the previous `first(where:)` scan.
        var lookup: [String: ChangeInfo] = [:]
        lookup.reserveCapacity(entries.count * 2)
        for entry in entries {
            let change = entry.change
            if lookup[change.commitId.id] == nil {
                lookup[change.commitId.id] = change
            }
            if lookup[change.changeId.id] == nil {
                lookup[change.changeId.id] = change
            }
        }
        changeByRevision = lookup

        parentIdsByCommitId = Dictionary(
            uniqueKeysWithValues: entries.map { entry in
                (
                    entry.change.commitId.id,
                    entry.edges.filter { $0.edgeType != .missing }.map(\.target)
                )
            }
        )

        let selectedIdSet = Set(selectedIds)
        let selected = entries.map(\.change).filter { change in
            let revision = change.selectionRevision
            return selectedIdSet.contains(revision) || (selectedIds.isEmpty && selectedId == revision)
        }
        selectedChanges = selected
        let selectedCommits = Set(selected.map(\.commitId.id))
        selectedCommitIds = selectedCommits

        var childIdsByCommitId: [String: [String]] = [:]
        for (commitId, parentIds) in parentIdsByCommitId {
            for parentId in parentIds {
                childIdsByCommitId[parentId, default: []].append(commitId)
            }
        }
        selectionAncestors = Self.reachable(from: selectedCommits, via: parentIdsByCommitId)
        selectionDescendants = Self.reachable(from: selectedCommits, via: childIdsByCommitId)
    }

    /// Commit ids reachable from `starts` by following `edges`, excluding the starts themselves.
    private static func reachable(
        from starts: Set<String>,
        via edges: [String: [String]]
    ) -> Set<String> {
        var reached: Set<String> = []
        var pending = Array(starts.flatMap { edges[$0] ?? [] })
        while let next = pending.popLast() {
            if reached.insert(next).inserted {
                pending.append(contentsOf: edges[next] ?? [])
            }
        }
        return reached
    }

    var isEmpty: Bool {
        entries.isEmpty
    }

    var hasMultipleSelection: Bool {
        selectedIds.count > 1
    }

    var selectedRevisions: [String] {
        selectedChanges.map(\.selectionRevision)
    }

    var canAbandonSelection: Bool {
        hasMutableSelection
    }

    var canDiffSelection: Bool {
        isContiguousLinearSelection && Self.rangeHasSingleParentBase(selectedChanges)
    }

    var canSquashSelection: Bool {
        hasMutableSelection && isContiguousLinearSelection
    }

    private var isContiguousLinearSelection: Bool {
        let selectedEntries = entries.enumerated().filter { isSelected($0.element.change) }
        guard let first = selectedEntries.first?.offset,
              let last = selectedEntries.last?.offset,
              selectedEntries.count == last - first + 1
        else {
            return false
        }
        return Self.formsConsecutiveLinearRange(selectedChanges)
    }

    func canRebaseSelection(onto target: ChangeInfo) -> Bool {
        guard hasMutableSelection, !isSelected(target) else { return false }
        return !hasSelectedAncestor(
            startingAt: target.commitId.id,
            parentIds: parentIdsByCommitId,
            selectedCommitIds: selectedCommitIds
        )
    }

    var canMergeSelection: Bool {
        canMerge(selectedChanges)
    }

    func canMergeSelectedChange(with target: ChangeInfo) -> Bool {
        // Single selection is the per-row hot path: a row can merge with the selected change when the
        // two are independent, i.e. the row is neither an ancestor nor a descendant of it.
        if let selected = selectedChanges.first, selectedChanges.count == 1 {
            let targetId = target.commitId.id
            return targetId != selected.commitId.id
                && !selectionAncestors.contains(targetId)
                && !selectionDescendants.contains(targetId)
        }
        return canMerge(selectedChanges + [target])
    }

    private func canMerge(_ changes: [ChangeInfo]) -> Bool {
        let selection = Set(changes.map(\.commitId.id))
        guard selection.count > 1 else { return false }
        let parentIds = parentIdsByCommitId
        return !selection.contains {
            hasSelectedAncestor(
                startingAt: $0,
                parentIds: parentIds,
                selectedCommitIds: selection
            )
        }
    }

    static func formsConsecutiveLinearRange(_ changes: [ChangeInfo]) -> Bool {
        changes.count > 1 && zip(changes, changes.dropFirst()).allSatisfy { newer, older in
            newer.parents == [older.commitId.id]
        }
    }

    /// The combined diff bases on the oldest change's single parent; squashing the same range into a merge commit is still legal.
    static func rangeHasSingleParentBase(_ changes: [ChangeInfo]) -> Bool {
        changes.last?.parents.count == 1
    }

    private var hasMutableSelection: Bool {
        selectedChanges.count == selectedIds.count
            && selectedChanges.count > 1
            && selectedChanges.allSatisfy { !$0.isImmutable }
    }

    private func hasSelectedAncestor(
        startingAt commitId: String,
        parentIds: [String: [String]],
        selectedCommitIds: Set<String>
    ) -> Bool {
        var pending = parentIds[commitId, default: []]
        var visited: Set<String> = []
        while let parentId = pending.popLast() {
            if selectedCommitIds.contains(parentId) {
                return true
            }
            if visited.insert(parentId).inserted {
                pending.append(contentsOf: parentIds[parentId, default: []])
            }
        }
        return false
    }

    func rowViewModel(
        for entry: GraphEntry,
        rebasePreviewText: String?,
        bookmarkPreviewText: String?
    ) -> DAGRowViewModel {
        DAGRowViewModel(
            entry: entry,
            layout: layout,
            geometry: geometry,
            selectedId: selectedId,
            selectedIds: selectedIds,
            compareFromId: compareFromId,
            contextTargetId: contextTargetId,
            rebaseDrag: rebaseDrag,
            rebasePreviewText: rebasePreviewText,
            bookmarkDrag: bookmarkDrag,
            bookmarkPreviewText: bookmarkPreviewText,
            colorScheme: colorScheme
        )
    }

    func nextContextTargetId(hovering: Bool, entry: GraphEntry) -> String? {
        let rowId = entry.change.selectionRevision
        if hovering, !isSelected(entry.change) {
            return rowId
        }
        if !hovering, contextTargetId == rowId {
            return nil
        }
        return contextTargetId
    }

    func shouldCancelRebaseDrag(for hoveredCommitId: String?) -> Bool {
        guard let hoveredCommitId else { return false }
        return !entries.contains(where: { $0.change.commitId.id == hoveredCommitId })
    }

    func selectedChangeId(afterMovingBy delta: Int) -> String? {
        Self.selectedChangeId(in: entries, selectedId: selectedId, afterMovingBy: delta)
    }

    /// Static so keyboard navigation resolves the next selection without building a whole view model
    /// (and its per-view-model precompute) on every arrow press.
    static func selectedChangeId(in entries: [GraphEntry], selectedId: String?, afterMovingBy delta: Int) -> String? {
        guard !entries.isEmpty else { return nil }
        let currentIdx: Int = if let selectedId,
                                 let idx = entries.firstIndex(where: { $0.change.selectionRevision == selectedId })
        {
            idx
        } else {
            delta > 0 ? -1 : entries.count
        }
        let newIdx = max(0, min(entries.count - 1, currentIdx + delta))
        guard newIdx != currentIdx else { return nil }
        return entries[newIdx].change.selectionRevision
    }

    func isSelected(_ change: ChangeInfo) -> Bool {
        let revision = change.selectionRevision
        return selectedIds.contains(revision) || (selectedIds.isEmpty && selectedId == revision)
    }

    func selectedRevision(for changeId: String) -> String {
        change(for: changeId)?.selectionRevision ?? changeId
    }

    func change(for changeId: String) -> ChangeInfo? {
        changeByRevision[changeId]
    }

    func canSquashIntoParent(_ target: ChangeInfo) -> Bool {
        guard let parentId = target.parents.first else { return false }
        return change(for: parentId).map { !$0.isImmutable } ?? true
    }

    func bookmarkDiffRequest(from selectedId: String, to target: ChangeInfo) -> BookmarkDiffRequest? {
        guard let selectedChange = changeByRevision[selectedId],
              let base = RevsetExpressions.primaryBaseBookmarkEndpoint(for: selectedChange),
              let head = RevsetExpressions.primaryHeadBookmarkEndpoint(for: target),
              base.label != head.label
        else {
            return nil
        }
        return BookmarkDiffRequest(base: base, head: head)
    }

    func scrollId(for rev: String) -> String {
        changeByRevision[rev]?.selectionRevision ?? rev
    }

    /// Other visible commits that share this change's id — the siblings of a divergent change. Empty unless `change` is divergent. Used to offer an interdiff between two versions of the same change so the user can see which is safer to abandon.
    func divergentSiblings(of change: ChangeInfo) -> [ChangeInfo] {
        guard change.isDivergent else { return [] }
        return entries
            .map(\.change)
            .filter { $0.changeId.id == change.changeId.id && $0.commitId.id != change.commitId.id }
    }

    static func selectionDelta(
        keyCode: UInt16,
        charactersIgnoringModifiers: String?,
        controlPressed: Bool
    ) -> Int? {
        switch keyCode {
            case downArrowKeyCode:
                return 1
            case upArrowKeyCode:
                return -1
            default:
                break
        }

        switch charactersIgnoringModifiers {
            case "j":
                return 1
            case "k":
                return -1
            case "n" where controlPressed:
                return 1
            case "p" where controlPressed:
                return -1
            default:
                return nil
        }
    }
}
