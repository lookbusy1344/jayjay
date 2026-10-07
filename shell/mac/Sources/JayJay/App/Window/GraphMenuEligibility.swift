/// Which Repository menu graph commands apply to the active window's selected change.
struct GraphMenuEligibility: Equatable {
    var canFocusSelectedChange = false
    var canShowElidedRevisions = false
    var canHideExpandedRevisions = false

    static let none = GraphMenuEligibility()
}
