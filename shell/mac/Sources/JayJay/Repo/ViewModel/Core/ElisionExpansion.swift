/// One `(elided revisions)` band the user expanded: the revisions between `target` and the row that owns the band, `owner`.
struct ElisionExpansion: Equatable {
    let owner: String
    let target: String
}
