import JayJayCore

/// A focus target pinned across progressive graph snapshots. Matched by exact commit id when known
/// (so a divergent change's other versions never satisfy it), falling back to its revision string.
struct PendingFocusTarget: Equatable {
    let revision: String
    let commitId: String?
    /// Whether appearing should scroll the target into view. A focus transition reveals its target; a
    /// plain reload only holds the existing selection and must not scroll away from where the user is.
    var revealsOnAppear = true

    func matches(_ change: ChangeInfo) -> Bool {
        if let commitId {
            return change.commitId.id == commitId
        }
        return change.matchesRevision(revision)
    }
}
