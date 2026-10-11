import JayJayCore

extension DiffStore {
    /// Throwing twin of the render path's load: the same fetch sequence, with primary-fetch errors propagated so destructive callers (Done's strict reload) can abort instead of seeing empty content.
    func loadContentStrict(
        hunk: DiffHunk,
        rev: String,
        repo: JayJayRepo
    ) async throws -> DiffLoadedContent {
        try await Self.fetchContent(
            repo: repo, hunk: hunk, rev: rev, fromRev: nil, projectionMode: nil
        )
    }

    /// The repository read for `hunk`; nil without a revision.
    nonisolated static func loadRequest(
        hunk: DiffHunk,
        rev: String?,
        fromRev: String?,
        projectionMode: DiffProjectionMode?,
        ignoreWhitespace: Bool
    ) -> DiffLoadRequest? {
        let raw = effectiveProjectionMode(hunk: hunk, mode: projectionMode) == .raw
        return rev.map { rev in
            DiffLoadRequest(
                rev: rev, fromRev: fromRev, path: hunk.path, oldPath: hunk.oldPath,
                hunkType: hunk.hunkType, projectionMode: raw ? .raw : .processed,
                ignoreWhitespace: ignoreWhitespace
            )
        }
    }

    /// Errors leave an empty diff, as an unreadable file shows no rows.
    nonisolated static func loadFromRepo(
        repo: JayJayRepo,
        request: DiffLoadRequest?,
        hunk: DiffHunk,
        ignoreWhitespace: Bool
    ) -> LoadedDiff {
        if let request, let loaded = try? repo.loadDiff(request: request) {
            return loaded
        }
        let empty = DiffHunk(
            path: hunk.path, oldPath: hunk.oldPath, oldContent: "", newContent: "",
            oldPreview: nil, newPreview: nil, hunkType: hunk.hunkType,
            reviewIdentity: hunk.reviewIdentity, projection: nil
        )
        return LoadedDiff.fromHunk(hunk: empty, ignoreWhitespace: ignoreWhitespace)
    }

    private static func fetchContent(
        repo: JayJayRepo,
        hunk: DiffHunk,
        rev: String?,
        fromRev: String?,
        projectionMode: DiffProjectionMode?
    ) async throws -> DiffLoadedContent {
        let path = hunk.path
        let hunkType = hunk.hunkType
        let oldPath = hunk.oldPath
        let raw = effectiveProjectionMode(hunk: hunk, mode: projectionMode) == .raw
        if let fromRev, let rev {
            if hunkType.hasSource, let oldPath {
                return try await loadedContent {
                    try repo.interdiffFileRenameWithMode(
                        fromRev: fromRev, toRev: rev, oldPath: oldPath, newPath: path,
                        projectionMode: raw ? .raw : .processed
                    )
                }
            }
            return try await loadedContent {
                raw
                    ? try repo.interdiffFileRaw(fromRev: fromRev, toRev: rev, path: path)
                    : try repo.interdiffFile(fromRev: fromRev, toRev: rev, path: path)
            }
        }
        if let rev, hunkType.hasSource, let oldPath {
            return try await loadedContent {
                raw
                    ? try repo.showFileRenameRaw(rev: rev, oldPath: oldPath, newPath: path)
                    : try repo.showFileRename(rev: rev, oldPath: oldPath, newPath: path)
            }
        }
        if let rev {
            let loaded = try await loadedContent {
                raw
                    ? try repo.showFileRaw(rev: rev, path: path)
                    : try repo.showFile(rev: rev, path: path)
            }
            if loaded.oldText.isEmpty, loaded.newText.isEmpty {
                // Best-effort enhancement, kept soft even for strict callers: image hunks land here with empty text sides, and a hard failure would drop their previews.
                let content = await Task.detached {
                    try? repo.fileContent(rev: rev, path: path)
                }.value
                if let content, !content.isEmpty {
                    return DiffLoadedContent(oldContent: nil, newContent: content)
                }
            }
            return loaded
        }
        return DiffLoadedContent(oldContent: "", newContent: "")
    }

    private static func loadedContent(
        loading: @escaping () throws -> DiffHunk
    ) async throws -> DiffLoadedContent {
        try await DiffLoadedContent(Task.detached { try loading() }.value)
    }
}
