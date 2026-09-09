import AppKit
import JayJayCore
import SwiftUI

enum DAGRebaseCoordinateSpace {
    static let name = "dag-rebase"
}

struct DAGRebaseRowFramePreferenceKey: PreferenceKey {
    static let defaultValue: [String: CGRect] = [:]

    static func reduce(value: inout [String: CGRect], nextValue: () -> [String: CGRect]) {
        value.merge(nextValue(), uniquingKeysWith: { _, next in next })
    }
}

extension DAGView {
    func rebaseGesture(for entry: GraphEntry, layout: DAGLayout, geometry: DAGGeometry) -> some Gesture {
        DragGesture(minimumDistance: 0, coordinateSpace: .named(DAGRebaseCoordinateSpace.name))
            .onChanged { value in
                handleRebaseGestureChanged(entry: entry, layout: layout, geometry: geometry, value: value)
            }
            .onEnded { value in
                handleRebaseGestureEnded(entry: entry, layout: layout, geometry: geometry, value: value)
            }
    }

    @ViewBuilder
    var rebaseDragOverlay: some View {
        if let rebaseDrag, rebaseDrag.phase == .dragging {
            DAGRebaseGhost(label: rebaseDrag.sourceLabel)
                .position(x: rebaseDrag.location.x + 68, y: rebaseDrag.location.y - 18)
                .allowsHitTesting(false)
        }
    }

    func rebasePreviewText(for change: ChangeInfo) -> String? {
        guard rebasePreviewTargetId == change.commitId.id,
              let rebaseDrag
        else { return nil }
        if let refusal = rebaseDrag.targetRefusal {
            return refusal
        }
        return "Rebase \(rebaseDrag.sourceLabel) onto \(DAGRebaseGesturePolicy.displayLabel(for: change))?"
    }

    private func handleRebaseGestureChanged(
        entry: GraphEntry,
        layout: DAGLayout,
        geometry: DAGGeometry,
        value: DragGesture.Value
    ) {
        guard !focusBadgeContains(
            entry: entry,
            layout: layout,
            geometry: geometry,
            location: value.startLocation
        ) else { return }
        // A bookmark-chip drag (started on a child view) wins over the row rebase.
        guard bookmarkDrag == nil else { return }
        let action = DAGRebaseGesturePolicy.changeAction(
            entryIsImmutable: entry.change.isImmutable,
            sourceCommitId: entry.change.commitId.id,
            rebaseDrag: rebaseDrag,
            location: value.location
        )

        switch action {
            case .ignore:
                break
            case .beginPress:
                beginRebasePress(for: entry, layout: layout, geometry: geometry, location: value.location)
            case .cancelPress:
                cancelRebaseDrag()
            case .beginDragging:
                beginDraggingIfNeeded()
                updateRebaseDrag(location: value.location)
            case .updateDragging:
                updateRebaseDrag(location: value.location)
        }
    }

    private func handleRebaseGestureEnded(
        entry: GraphEntry,
        layout: DAGLayout,
        geometry: DAGGeometry,
        value: DragGesture.Value
    ) {
        guard !focusBadgeContains(
            entry: entry,
            layout: layout,
            geometry: geometry,
            location: value.startLocation
        ) else { return }
        guard bookmarkDrag == nil else { return }
        let action = DAGRebaseGesturePolicy.endAction(
            entryIsImmutable: entry.change.isImmutable,
            sourceCommitId: entry.change.commitId.id,
            rebaseDrag: rebaseDrag,
            startLocation: value.startLocation,
            location: value.location
        )

        switch action {
            case .ignore:
                break
            case .select:
                cancelRebaseDrag()
                selectEntry(entry)
            case .cancel:
                cancelRebaseDrag()
            case .confirmDrop:
                updateRebaseDrag(location: value.location)
                confirmRebaseDrop()
        }
    }

    private func focusBadgeContains(
        entry: GraphEntry,
        layout: DAGLayout,
        geometry: DAGGeometry,
        location: CGPoint
    ) -> Bool {
        guard let frame = rebaseRowFrames[entry.change.commitId.id],
              let row = layout.row(for: entry.change.commitId.id)
        else { return false }
        let columnCount = Int(row.graphColumnCount)
        let width = geometry.graphWidth(forColumnCount: columnCount)
        let nodeX = geometry.xPosition(forColumn: Int(row.nodeColumn))
        let nodeRadius = DAGNodeStyle.resolve(change: entry.change, radius: geometry.nodeRadius).radius
        let badge = dagOverflowBadgeLayout(
            isGraphClipped: geometry.isClipped(forColumnCount: columnCount),
            nodeTrailingX: nodeX + nodeRadius,
            width: width
        )
        let localX = location.x - frame.minX - dagRowLeadingPadding
        let localY = location.y - frame.minY
        return dagFocusBadgeContains(x: localX, layout: badge)
            && abs(localY - dagNodeCenterY) <= dagOverflowMarkerTapSize / 2
    }

    private func beginRebasePress(for entry: GraphEntry, layout: DAGLayout, geometry: DAGGeometry, location: CGPoint) {
        guard rebaseDrag?.sourceCommitId != entry.change.commitId.id else { return }
        // Row frames only mount once a drag state exists, so the first press seeds from the pointer; the ghost re-anchors from live frames on the first drag movement.
        let seedLocation = rebaseDragSeedLocation(for: entry, layout: layout, geometry: geometry) ?? location

        activePane = .dag
        rebaseArmTask?.cancel()
        rebaseDrag = DAGRebaseDragState(
            sourceCommitId: entry.change.commitId.id,
            sourceChangeId: entry.change.changeId.id,
            sourceRev: DAGRebaseGesturePolicy.revision(for: entry.change),
            sourceLabel: DAGRebaseGesturePolicy.displayLabel(for: entry.change),
            sourceParents: entry.change.parents,
            startLocation: location,
            armedAt: nil,
            phase: .pressing,
            location: seedLocation,
            hoveredCommitId: nil
        )
        scheduleRebaseArm(for: entry)
    }

    private func scheduleRebaseArm(for entry: GraphEntry) {
        let sourceCommitId = entry.change.commitId.id
        rebaseArmTask = Task {
            try? await Task.sleep(for: .seconds(DAGRebaseGesturePolicy.armDuration))
            guard !Task.isCancelled else { return }
            await MainActor.run {
                guard var rebaseDrag,
                      rebaseDrag.sourceCommitId == sourceCommitId,
                      rebaseDrag.phase == .pressing
                else { return }
                rebaseDrag.phase = .armed
                rebaseDrag.armedAt = .now
                self.rebaseDrag = rebaseDrag
            }
        }
    }

    private func beginDraggingIfNeeded() {
        guard var rebaseDrag, rebaseDrag.phase != .dragging else { return }
        rebaseDrag.phase = .dragging
        rebaseDrag.descendantCommitIds = Set(descendantCommitIds(entries: entries, commitId: rebaseDrag.sourceCommitId))
        self.rebaseDrag = rebaseDrag
    }

    private func updateRebaseDrag(location: CGPoint) {
        guard var rebaseDrag else { return }
        let hoveredCommitId = rebaseRowFrames.first(where: { $0.value.contains(location) })?.key
        let normalizedTarget = DAGRebaseGesturePolicy.normalizedTargetCommitId(
            sourceCommitId: rebaseDrag.sourceCommitId,
            hoveredCommitId: hoveredCommitId
        )
        rebaseDrag.location = location
        if normalizedTarget != rebaseDrag.hoveredCommitId {
            rebaseDrag.hoveredCommitId = normalizedTarget
            rebaseDrag.targetRefusal = normalizedTarget.flatMap {
                DAGRebaseGesturePolicy.targetRefusal(rebaseDrag: rebaseDrag, targetCommitId: $0)
            }
        }
        self.rebaseDrag = rebaseDrag
        // A refusal shows at once; only a valid target waits for the preview delay.
        updateRebasePreviewTarget(normalizedTarget, delayed: rebaseDrag.targetRefusal == nil)
    }

    private func updateRebasePreviewTarget(_ commitId: String?, delayed: Bool) {
        if commitId == rebasePreviewTargetId {
            return
        }

        rebasePreviewTask?.cancel()
        rebasePreviewTask = nil
        rebasePreviewTargetId = nil

        guard let commitId else { return }
        guard delayed else {
            rebasePreviewTargetId = commitId
            return
        }

        rebasePreviewTask = Task {
            try? await Task.sleep(for: .milliseconds(DAGRebaseGesturePolicy.previewDelayMs))
            guard !Task.isCancelled else { return }
            await MainActor.run {
                guard rebaseDrag?.hoveredCommitId == commitId else { return }
                rebasePreviewTargetId = commitId
            }
        }
    }

    func confirmRebaseDrop() {
        guard let request = DAGRebaseGesturePolicy.dropRequest(
            rebaseDrag: rebaseDrag,
            previewTargetCommitId: rebasePreviewTargetId,
            hoveredCommitId: rebaseDrag?.hoveredCommitId,
            entries: entries
        ) else {
            cancelRebaseDrag()
            return
        }

        cancelRebaseDrag()
        onRequest?(.rebase(request))
    }

    func cancelRebaseDrag() {
        rebaseArmTask?.cancel()
        rebaseArmTask = nil
        rebasePreviewTask?.cancel()
        rebasePreviewTask = nil
        rebasePreviewTargetId = nil
        rebaseDrag = nil
    }

    private func selectEntry(_ entry: GraphEntry) {
        activePane = .dag
        NSApp.keyWindow?.makeFirstResponder(nil)
        let rev = entry.change.selectionRevision
        let click = OrderedSelectionClick(modifiers: NSEvent.modifierFlags)
        switch click {
            case .toggle, .extend:
                actions?.updateSelection(changeId: rev, click: click)
            case .replace:
                actions?.select(changeId: rev)
        }
    }

    private func rebaseMovementDistance(for rebaseDrag: DAGRebaseDragState, to location: CGPoint) -> CGFloat {
        DAGRebaseGesturePolicy.movementDistance(from: rebaseDrag.startLocation, to: location)
    }

    private func rebaseDragSeedLocation(for entry: GraphEntry, layout: DAGLayout, geometry: DAGGeometry) -> CGPoint? {
        guard let rowFrame = rebaseRowFrames[entry.change.commitId.id] else { return nil }
        guard let row = layout.row(for: entry.change.commitId.id) else { return nil }
        return CGPoint(
            // rowFrame.minX is the row's left edge before its own leading padding; the graph column's Canvas starts right after that padding, so skip it once here before adding the geometry's own (separate) leading margin inside the column.
            x: rowFrame.minX + dagRowLeadingPadding + geometry.xPosition(forColumn: Int(row.nodeColumn)),
            y: rowFrame.midY
        )
    }
}

private struct DAGRebaseGhost: View {
    let label: String

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: "arrow.up.forward.app")
                .jayjayFont(11, weight: .semibold)
            Text(label)
                .jayjayFont(11, weight: .medium)
                .lineLimit(1)
        }
        .foregroundStyle(.primary)
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .glassEffect(in: Capsule())
        .overlay(
            Capsule()
                .stroke(Color.accentColor.opacity(0.25), lineWidth: 1)
        )
        .shadow(color: .black.opacity(0.12), radius: 8, y: 4)
    }
}
