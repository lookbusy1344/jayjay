import JayJayCore

struct DiffLoadedContent {
    var old: DiffContent
    var new: DiffContent
    var projection: DiffProjection?
    var supportsFileEditor: Bool

    init(
        old: DiffContent = DiffContent(content: nil, preview: nil),
        new: DiffContent = DiffContent(content: nil, preview: nil),
        projection: DiffProjection? = nil,
        supportsFileEditor: Bool = false
    ) {
        self.old = old
        self.new = new
        self.projection = projection
        self.supportsFileEditor = supportsFileEditor
    }

    init(
        oldContent: String?,
        newContent: String?,
        oldPreview: DiffPreview? = nil,
        newPreview: DiffPreview? = nil,
        projection: DiffProjection? = nil,
        supportsFileEditor: Bool = false
    ) {
        self.init(
            old: DiffContent(content: oldContent, preview: oldPreview),
            new: DiffContent(content: newContent, preview: newPreview),
            projection: projection,
            supportsFileEditor: supportsFileEditor
        )
    }

    /// Absent sides stay nil: core's staleness guard materializes them as None, and Some("") would abort the apply as stale.
    init(_ hunk: DiffHunk) {
        self.init(
            old: hunk.old,
            new: hunk.new,
            projection: hunk.projection,
            supportsFileEditor: hunk.supportsFileEditor
        )
    }

    var oldContent: String? {
        old.content
    }

    var newContent: String? {
        new.content
    }

    var oldPreview: DiffPreview? {
        old.preview
    }

    var newPreview: DiffPreview? {
        new.preview
    }

    var oldText: String {
        old.content ?? ""
    }

    var newText: String {
        new.content ?? ""
    }
}

struct DiffSectionLoadedDiff {
    var path: String
    /// Absent for placeholders, which render `content` alone.
    var diff: DiffStore.CachedDiff?
    var rows: DisplayRows?
    var content: DiffLoadedContent
    var basis: String?
    var reviewQuery: ReviewDisplayQuery?

    init(placeholder content: DiffLoadedContent, path: String) {
        self.path = path
        self.content = content
    }

    /// Reads the display rows across the FFI; call off the main thread.
    init(_ diff: DiffStore.CachedDiff, path: String, basis: String, hunk: DiffHunk) {
        self.path = path
        self.diff = diff
        rows = diff.loaded.displayRows()
        content = diff.content
        self.basis = basis
        reviewQuery = diff.review.map {
            ReviewDisplayQuery(path: hunk.path, identity: hunk.reviewIdentity, snapshot: $0.snapshot, mapping: $0.groupMap)
        }
    }
}
