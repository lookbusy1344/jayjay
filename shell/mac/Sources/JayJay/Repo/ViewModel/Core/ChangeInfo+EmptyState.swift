import JayJayCore

extension ChangeInfo {
    /// A copy with `isEmpty` replaced. Deferred graph loading publishes merge rows as non-empty and
    /// corrects them once their parent-tree merge finishes; the record's fields are immutable, so the
    /// correction rebuilds the value.
    func withIsEmpty(_ isEmpty: Bool) -> ChangeInfo {
        ChangeInfo(
            changeId: changeId,
            commitId: commitId,
            description: description,
            author: author,
            parents: parents,
            bookmarks: bookmarks,
            tags: tags,
            workspaces: workspaces,
            isWorkingCopy: isWorkingCopy,
            hasConflict: hasConflict,
            isEmpty: isEmpty,
            isImmutable: isImmutable,
            isDivergent: isDivergent,
            newChange: newChange
        )
    }
}

extension GraphEntry {
    func withChange(_ change: ChangeInfo) -> GraphEntry {
        GraphEntry(change: change, edges: edges)
    }
}
