import Foundation
import JayJayCore

extension RepoViewModel {
    func fetchPrInfo(bookmarks: [String], keepingShown: Bool = false) {
        let bookmark = isShuttingDown ? nil : bookmarks.first
        if keepingShown, bookmark != nil {
            prFetchTask?.cancel()
        } else {
            clearPrInfo()
        }
        guard let bookmark else { return }
        prFetchTask = startRepoTask { [weak self, repo] in
            let info = repo.pullRequestInfo(bookmark: bookmark)
            guard !Task.isCancelled else { return }
            await self?.applyPrInfo(info)
        }
    }

    @MainActor
    func refreshPrInfo(replacing shown: ChangeInfo?, isAutoTriggered: Bool) {
        let info = selectedChange?.info
        let bookmarks = info?.bookmarks ?? []
        guard let shown, shown.commitId.id == info?.commitId.id else {
            fetchPrInfo(bookmarks: bookmarks)
            return
        }
        if !isAutoTriggered || shown.bookmarks.first != bookmarks.first {
            fetchPrInfo(bookmarks: bookmarks, keepingShown: true)
        }
    }

    func clearPrInfo() {
        prFetchTask?.cancel()
        prFetchTask = nil
        prInfo = nil
    }

    @MainActor
    private func applyPrInfo(_ info: PrInfo?) {
        guard !isShuttingDown else { return }
        prInfo = info
    }
}
