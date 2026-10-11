import JayJayCore

extension DiffSection {
    /// The displayed diff's basis, not the live controls: during a reload the old render stays visible, and expanding it must supersede when the replacement installs.
    var contextExpansionBasis: String? {
        loadedDiff?.basis
    }

    func expandAllContext() {
        guard renderedDiff?.flags.hasContextRegions == true else { return }
        expandContext(.allRegions)
    }

    func expandContext(_ request: ContextExpansionRequest) {
        guard let current = loadedDiff,
              let diff = current.diff,
              let basis = current.basis,
              let attempt = contextExpansion.begin(basis: basis, request: request)
        else { return }
        let session = contextExpansion
        let generation = attempt.generation()

        Task {
            let expanded = await Task.detached(priority: .userInitiated) {
                attempt.run(base: diff.loaded)
            }.value
            guard let outcome = expanded else { return }

            let currentBasis = contextExpansionBasis
            let prepared = await Task.detached(priority: .userInitiated) {
                PreparedContextExpansion(session.finish(basis: currentBasis ?? "", outcome: outcome), content: diff.content)
            }.value
            guard session.isCurrent(basis: contextExpansionBasis ?? "", generation: generation) else { return }
            install(prepared)
        }
    }

    private func install(_ prepared: PreparedContextExpansion) {
        switch prepared.finish {
            case .discarded:
                break
            case let .applied(_, reveal, selectionGeneration, next):
                guard var current = loadedDiff, let expanded = prepared.diff else { return }
                current.diff = expanded
                current.rows = prepared.rows
                selectedLineRange = nil
                loadedDiff = current
                contextExpansionDisplay.errorMessage = nil
                contextExpansionDisplay.selectionGeneration = selectionGeneration
                contextExpansionDisplay.revealFeedback = reveal
                refreshActiveNotes()
                if let reveal {
                    Task {
                        try? await Task.sleep(for: .milliseconds(300))
                        contextExpansionDisplay.clearRevealFeedback(generation: reveal.generation)
                    }
                }
                if let next {
                    expandContext(next)
                }
            case let .failed(message):
                contextExpansionDisplay.errorMessage = message
        }
    }
}

/// The core outcome with its rows read across the FFI off the main thread, since they are O(diff bytes).
private struct PreparedContextExpansion: Sendable {
    let finish: ContextExpansionFinish
    let diff: DiffStore.CachedDiff?
    let rows: DisplayRows?

    init(_ finish: ContextExpansionFinish, content: DiffLoadedContent) {
        self.finish = finish
        guard case let .applied(loaded, _, _, _) = finish else {
            diff = nil
            rows = nil
            return
        }
        diff = DiffStore.CachedDiff(loaded: loaded, content: content)
        rows = loaded.displayRows()
    }
}
