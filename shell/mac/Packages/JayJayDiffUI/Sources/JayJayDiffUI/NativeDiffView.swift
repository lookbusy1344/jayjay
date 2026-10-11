import AppKit
import JayJayCore
import SwiftUI

public struct NativeDiffView: NSViewRepresentable {
    public typealias Coordinator = NativeDiffContextCoordinator

    /// Raw diff rows, or display rows and groups the owner computed once per loaded diff; `updateNSView` re-runs on every observed change and the display-line/group FFI is O(diff bytes).
    enum Rows {
        case raw([DiffLine])
        case display([DiffLine], groups: [ChangeGroup])
    }

    let rows: Rows
    public var gutterActions: (any DiffGutterContextActions)?
    /// Passed as a property (not pulled through `gutterActions`) so SwiftUI re-runs `updateNSView` when the note list changes.
    public var reviewNotes: [DiffReviewNoteSummary]
    public var reserveNoteColumn: Bool
    public var compactGutterWidth: Bool
    /// Shows a dedicated +/- column for compact merge-hunk comparisons where the change type must never be confused with editable text.
    public var showsChangeMarkers: Bool
    public var onExpandContext: ((ContextExpansionRequest) -> Void)?
    public var resetSelectionGeneration: UInt64
    /// Enables constant-time selection refreshes when the owner increments this value for every rendered-content change.
    public var contentGeneration: UInt64?
    /// Passed as a property so SwiftUI re-runs `updateNSView` when hunk/file review marks change.
    public var reviewStateGeneration: UInt64
    public var revealFeedback: ContextExpansionReveal?
    /// When set, the view sizes to its full content: inner scrolling is disabled and every laid-out height change is reported so the host can match its frame.
    public var onContentHeightChanged: ((CGFloat) -> Void)?

    @Environment(\.colorScheme) var colorScheme
    @Environment(\.diffPalettes) var diffPalettes
    @Environment(\.diffTypography) var typography
    @Environment(\.accessibilityReduceMotion) var reduceMotion

    public init(
        diff: FileDiff,
        gutterActions: (any DiffGutterContextActions)? = nil,
        reviewNotes: [DiffReviewNoteSummary] = [],
        reserveNoteColumn: Bool = false,
        compactGutterWidth: Bool = false,
        showsChangeMarkers: Bool = false,
        onExpandContext: ((ContextExpansionRequest) -> Void)? = nil,
        resetSelectionGeneration: UInt64 = 0,
        contentGeneration: UInt64? = nil,
        reviewStateGeneration: UInt64 = 0,
        revealFeedback: ContextExpansionReveal? = nil,
        onContentHeightChanged: ((CGFloat) -> Void)? = nil
    ) {
        self.init(
            rows: .raw(diff.lines),
            gutterActions: gutterActions,
            reviewNotes: reviewNotes,
            reserveNoteColumn: reserveNoteColumn,
            compactGutterWidth: compactGutterWidth,
            showsChangeMarkers: showsChangeMarkers,
            onExpandContext: onExpandContext,
            resetSelectionGeneration: resetSelectionGeneration,
            contentGeneration: contentGeneration,
            reviewStateGeneration: reviewStateGeneration,
            revealFeedback: revealFeedback,
            onContentHeightChanged: onContentHeightChanged
        )
    }

    public init(
        displayLines: [DiffLine],
        displayGroups: [ChangeGroup],
        gutterActions: (any DiffGutterContextActions)? = nil,
        reviewNotes: [DiffReviewNoteSummary] = [],
        reserveNoteColumn: Bool = false,
        compactGutterWidth: Bool = false,
        showsChangeMarkers: Bool = false,
        onExpandContext: ((ContextExpansionRequest) -> Void)? = nil,
        resetSelectionGeneration: UInt64 = 0,
        contentGeneration: UInt64? = nil,
        reviewStateGeneration: UInt64 = 0,
        revealFeedback: ContextExpansionReveal? = nil,
        onContentHeightChanged: ((CGFloat) -> Void)? = nil
    ) {
        self.init(
            rows: .display(displayLines, groups: displayGroups),
            gutterActions: gutterActions,
            reviewNotes: reviewNotes,
            reserveNoteColumn: reserveNoteColumn,
            compactGutterWidth: compactGutterWidth,
            showsChangeMarkers: showsChangeMarkers,
            onExpandContext: onExpandContext,
            resetSelectionGeneration: resetSelectionGeneration,
            contentGeneration: contentGeneration,
            reviewStateGeneration: reviewStateGeneration,
            revealFeedback: revealFeedback,
            onContentHeightChanged: onContentHeightChanged
        )
    }

    private init(
        rows: Rows,
        gutterActions: (any DiffGutterContextActions)?,
        reviewNotes: [DiffReviewNoteSummary],
        reserveNoteColumn: Bool,
        compactGutterWidth: Bool,
        showsChangeMarkers: Bool,
        onExpandContext: ((ContextExpansionRequest) -> Void)?,
        resetSelectionGeneration: UInt64,
        contentGeneration: UInt64?,
        reviewStateGeneration: UInt64,
        revealFeedback: ContextExpansionReveal?,
        onContentHeightChanged: ((CGFloat) -> Void)?
    ) {
        self.rows = rows
        self.gutterActions = gutterActions
        self.reviewNotes = reviewNotes
        self.reserveNoteColumn = reserveNoteColumn
        self.compactGutterWidth = compactGutterWidth
        self.showsChangeMarkers = showsChangeMarkers
        self.onExpandContext = onExpandContext
        self.resetSelectionGeneration = resetSelectionGeneration
        self.contentGeneration = contentGeneration
        self.reviewStateGeneration = reviewStateGeneration
        self.revealFeedback = revealFeedback
        self.onContentHeightChanged = onContentHeightChanged
    }
}
