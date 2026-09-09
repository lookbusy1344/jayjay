import SwiftUI

@MainActor
final class RepoMenuHandler: RepositoryMenuHandler {
    var onAction: ((MenuAction) -> Void)?
    var canFocusSelectedChange = false

    enum MenuAction {
        case commandPalette, undo, bookmarkManager, newWorkspace, pullRequestImport, focusSelectedChange
    }

    func showCommandPalette() {
        onAction?(.commandPalette)
    }

    func showUndo() {
        onAction?(.undo)
    }

    func showBookmarkManager() {
        onAction?(.bookmarkManager)
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
}
