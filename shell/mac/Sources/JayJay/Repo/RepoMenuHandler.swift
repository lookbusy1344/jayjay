import SwiftUI

@MainActor
final class RepoMenuHandler: RepositoryMenuHandler {
    var onAction: ((MenuAction) -> Void)?
    var graphMenuEligibility = GraphMenuEligibility.none

    enum MenuAction {
        case commandPalette, undo, revsetFilter, bookmarkManager, overview, newWorkspace, pullRequestImport, focusSelectedChange,
            showSelectedElidedRevisions, hideSelectedExpandedRevisions
    }

    func showCommandPalette() {
        onAction?(.commandPalette)
    }

    func showUndo() {
        onAction?(.undo)
    }

    func showRevsetFilter() {
        onAction?(.revsetFilter)
    }

    func showBookmarkManager() {
        onAction?(.bookmarkManager)
    }

    func showOverview() {
        onAction?(.overview)
    }

    func showNewWorkspace() {
        onAction?(.newWorkspace)
    }

    func showPullRequestImport() {
        onAction?(.pullRequestImport)
    }

    func focusSelectedChange() {
        onAction?(.focusSelectedChange)
    }

    func showSelectedElidedRevisions() {
        onAction?(.showSelectedElidedRevisions)
    }

    func hideSelectedExpandedRevisions() {
        onAction?(.hideSelectedExpandedRevisions)
    }
}
