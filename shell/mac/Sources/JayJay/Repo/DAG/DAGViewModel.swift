import JayJayCore
import SwiftUI

struct DAGViewModel {
    private static let downArrowKeyCode: UInt16 = 125
    private static let upArrowKeyCode: UInt16 = 126

    let entries: [GraphEntry]
    let selectedId: String?
    let selectedIds: [String]
    let compareFromId: String?
    let rebaseDrag: DAGRebaseDragState?
    let bookmarkDrag: BookmarkDragState?
    let colorScheme: ColorScheme
    let layout: DAGLayout
    let capabilities: DAGSelectionCapabilities
    var isActivePane = true
    let geometry: DAGGeometry

    // Derived state built once per view model, not per row. macOS SwiftUI evaluates each row's
    // `.contextMenu` content eagerly on every body update, so any O(entries) work these lookups back
    // would otherwise run per visible row — quadratic on a large graph.
    private let changeByRevision: [String: ChangeInfo]
    private let selectedChanges: [ChangeInfo]

    init(
        entries: [GraphEntry],
        selectedId: String?,
        selectedIds: [String],
        compareFromId: String?,
        rebaseDrag: DAGRebaseDragState?,
        bookmarkDrag: BookmarkDragState?,
        colorScheme: ColorScheme,
        layout: DAGLayout,
        capabilities: DAGSelectionCapabilities,
        geometry: DAGGeometry,
        isActivePane: Bool = true
    ) {
        self.entries = entries
        self.selectedId = selectedId
        self.selectedIds = selectedIds
        self.compareFromId = compareFromId
        self.rebaseDrag = rebaseDrag
        self.bookmarkDrag = bookmarkDrag
        self.colorScheme = colorScheme
        self.layout = layout
        self.capabilities = capabilities
        self.geometry = geometry
        self.isActivePane = isActivePane

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

        let selectedIdSet = Set(selectedIds)
        selectedChanges = entries.map(\.change).filter { change in
            let revision = change.selectionRevision
            return selectedIdSet.contains(revision) || (selectedIds.isEmpty && selectedId == revision)
        }
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
        capabilities.canAbandon
    }

    var canSquashSelection: Bool {
        capabilities.canSquash
    }

    func canRebaseSelection(onto target: ChangeInfo) -> Bool {
        capabilities.canRebase(onto: target)
    }

    var canMergeSelection: Bool {
        capabilities.canMerge
    }

    func canMergeSelectedChange(with target: ChangeInfo) -> Bool {
        capabilities.canMerge(with: target)
    }

    var canDiffSelection: Bool {
        isContiguousLinearSelection && Self.rangeHasSingleParentBase(selectedChanges)
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

    static func formsConsecutiveLinearRange(_ changes: [ChangeInfo]) -> Bool {
        changes.count > 1 && zip(changes, changes.dropFirst()).allSatisfy { newer, older in
            newer.parents == [older.commitId.id]
        }
    }

    /// The combined diff bases on the oldest change's single parent; squashing the same range into a merge commit is still legal.
    static func rangeHasSingleParentBase(_ changes: [ChangeInfo]) -> Bool {
        changes.last?.parents.count == 1
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
            rebaseDrag: rebaseDrag,
            rebasePreviewText: rebasePreviewText,
            bookmarkDrag: bookmarkDrag,
            bookmarkPreviewText: bookmarkPreviewText,
            colorScheme: colorScheme,
            isActivePane: isActivePane
        )
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
