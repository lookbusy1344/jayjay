import JayJayCore

extension RepoViewModel {
    /// Show the revisions hidden behind `owner`'s elision bands. Expansions layer over the base `revset` like focus does, so the base keeps paging and stays as the user typed it.
    func expandElisions(owner: String, targets: [String]) {
        let added = targets
            .map { ElisionExpansion(owner: owner, target: $0) }
            .filter { !expandedElisions.contains($0) }
        guard !added.isEmpty else { return }
        expandedElisions += added
        refresh(selecting: owner)
    }

    func collapseElisions(owner: String) {
        guard expandedElisionOwners.contains(owner) else { return }
        expandedElisions.removeAll { $0.owner == owner }
        refresh(selecting: owner)
    }

    func showSelectedElidedRevisions() {
        guard let owner = selectedChangeId else { return }
        expandElisions(owner: owner, targets: selectedElidedTargets)
    }

    func hideSelectedExpandedRevisions() {
        guard let owner = selectedChangeId else { return }
        collapseElisions(owner: owner)
    }

    var graphMenuEligibility: GraphMenuEligibility {
        guard let selected = selectedChangeId else { return .none }
        return GraphMenuEligibility(
            canFocusSelectedChange: selected != focusedRevision,
            canShowElidedRevisions: !selectedElidedTargets.isEmpty,
            canHideExpandedRevisions: expandedElisionOwners.contains(selected)
        )
    }

    private var selectedElidedTargets: [String] {
        guard let selected = selectedChangeId,
              let change = graphEntries.first(where: { $0.change.matchesRevision(selected) })?.change
        else { return [] }
        return dagLayout.elidedTargets(of: change.commitId.id) { commitId in
            graphEntries.first { $0.change.commitId.id == commitId }?.change.selectionRevision
        }
    }

    /// Rows whose elisions are expanded; only these offer to hide them again.
    var expandedElisionOwners: Set<String> {
        Set(expandedElisions.map(\.owner))
    }

    /// The base revset with every expanded path added, before focus scopes it.
    var expandedRevset: String {
        expandedElisions.reduce(revset) { base, expansion in
            expandElisionRevset(base: base, owner: expansion.owner, target: expansion.target)
        }
    }
}
