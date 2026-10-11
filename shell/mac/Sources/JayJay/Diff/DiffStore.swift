import JayJayCore
import Observation

@Observable
final class DiffStore {
    /// One loaded diff with the state read across the FFI at load; display rows are read when a section shows it.
    struct CachedDiff {
        let loaded: LoadedDiff
        let content: DiffLoadedContent
        let review: ReviewDisplayParts?
        let flags: LoadedDiffFlags

        init(_ loaded: LoadedDiff) {
            self.init(loaded: loaded, content: DiffLoadedContent(loaded.hunk()))
        }

        init(loaded: LoadedDiff, content: DiffLoadedContent) {
            self.loaded = loaded
            self.content = content
            review = loaded.reviewDisplay()
            flags = loaded.flags()
        }

        var oldContent: String {
            content.oldText
        }

        var newContent: String {
            content.newText
        }

        var projection: DiffProjection? {
            content.projection
        }
    }

    struct CacheKeyParts {
        let commitId: String?
        let rev: String?
        let compareFromRev: String?
        let ignoreWhitespace: Bool
        let path: String
        let projectionKey: String
    }

    private let cache = DiffCache()
    @ObservationIgnored private var preloadTask: Task<Void, Never>?

    func clear() {
        Task { await cache.clear() }
    }

    func cachedDiff(
        hunk: DiffHunk,
        rev: String?,
        commitId: String? = nil,
        compareFromRev: String? = nil,
        ignoreWhitespace: Bool = false,
        projectionMode: DiffProjectionMode? = nil
    ) async -> CachedDiff? {
        guard !hunk.isSubmodulePlaceholder else { return nil }
        guard !hunk.isContentFreeRenameOrCopy else { return nil }
        let key = Self.key(CacheKeyParts(
            commitId: commitId, rev: rev, compareFromRev: compareFromRev,
            ignoreWhitespace: ignoreWhitespace, path: hunk.path,
            projectionKey: Self.projectionKey(hunk: hunk, mode: projectionMode)
        ))
        return await cache.get(key)
    }

    /// `commitId` is the immutable content hash used as the cache identity; `rev` (the mutable selection revision) is what jj resolves to fetch content, but keying on it would serve stale diffs after an amend/rebase reuses the id.
    func loadDiff(
        hunk: DiffHunk,
        rev: String?,
        commitId: String? = nil,
        repo: JayJayRepo?,
        compareFromRev: String? = nil,
        ignoreWhitespace: Bool = false,
        projectionMode: DiffProjectionMode? = nil
    ) async -> CachedDiff? {
        guard let repo else { return nil }
        guard !hunk.isSubmodulePlaceholder else { return nil }
        // A byte-identical rename has no content to load; loading by the new path alone would render every line as added.
        guard !hunk.isContentFreeRenameOrCopy else { return nil }

        let key = Self.key(CacheKeyParts(
            commitId: commitId, rev: rev, compareFromRev: compareFromRev,
            ignoreWhitespace: ignoreWhitespace, path: hunk.path,
            projectionKey: Self.projectionKey(hunk: hunk, mode: projectionMode)
        ))

        if let cached = await cache.get(key) {
            return cached
        }

        let needsProjectionModeReload = hunk.projection != nil
            && projectionMode != nil
            && hunk.projection?.mode != projectionMode
        let loadsFromRepo = Self.shouldLoadFileContent(
            oldContent: hunk.oldContent ?? "",
            newContent: hunk.newContent ?? "",
            projectionModeChanged: needsProjectionModeReload
        )
        let request = Self.loadRequest(
            hunk: hunk, rev: rev, fromRev: compareFromRev,
            projectionMode: projectionMode, ignoreWhitespace: ignoreWhitespace
        )
        let cached = await Task.detached {
            let loaded = loadsFromRepo
                ? Self.loadFromRepo(repo: repo, request: request, hunk: hunk, ignoreWhitespace: ignoreWhitespace)
                : LoadedDiff.fromHunk(hunk: hunk, ignoreWhitespace: ignoreWhitespace)
            return CachedDiff(loaded)
        }.value
        await cache.set(key, value: cached)
        return cached
    }

    func preload(
        hunks: [DiffHunk],
        rev: String?,
        commitId: String? = nil,
        repo: JayJayRepo?,
        compareFromRev: String? = nil,
        ignoreWhitespace: Bool = false,
        projectionMode: DiffProjectionMode? = nil
    ) {
        guard let repo, let rev else { return }
        // Cancel any in-flight preload so rapid commit navigation doesn't pile up detached tasks all racing the FFI.
        preloadTask?.cancel()
        preloadTask = Task.detached(priority: .utility) { [weak self] in
            for hunk in hunks {
                if Task.isCancelled {
                    return
                }
                _ = await self?.loadDiff(
                    hunk: hunk, rev: rev, commitId: commitId, repo: repo,
                    compareFromRev: compareFromRev, ignoreWhitespace: ignoreWhitespace,
                    projectionMode: projectionMode
                )
            }
        }
    }

    nonisolated static func key(_ parts: CacheKeyParts) -> String {
        let base = (parts.commitId?.isEmpty == false ? parts.commitId : parts.rev) ?? ""
        let identity = parts.compareFromRev.map { "\($0)→\(base)" } ?? base
        return "\(identity)|\(parts.ignoreWhitespace ? "iw" : "")|\(parts.projectionKey)|\(parts.path)"
    }

    nonisolated static func shouldLoadFileContent(
        oldContent: String,
        newContent: String,
        projectionModeChanged: Bool
    ) -> Bool {
        (oldContent.isEmpty && newContent.isEmpty) || projectionModeChanged
    }

    nonisolated static func effectiveProjectionMode(
        hunk: DiffHunk,
        mode: DiffProjectionMode?
    ) -> DiffProjectionMode? {
        mode ?? hunk.projection?.mode
    }

    nonisolated private static func projectionKey(
        hunk: DiffHunk,
        mode: DiffProjectionMode?
    ) -> String {
        guard let projection = hunk.projection else { return "raw" }
        let activeMode = effectiveProjectionMode(hunk: hunk, mode: mode) ?? projection.mode
        return projection.identityPart(mode: activeMode)
    }
}

actor DiffCache {
    private var entries: [String: DiffStore.CachedDiff] = [:]
    private var order: [String] = [] // LRU recency; front = least recently used
    private var totalBytes = 0
    private let budgetBytes: Int

    init(budgetBytes: Int = 64 * 1024 * 1024) {
        self.budgetBytes = budgetBytes
    }

    func get(_ key: String) -> DiffStore.CachedDiff? {
        guard let value = entries[key] else { return nil }
        touch(key)
        return value
    }

    func set(_ key: String, value: DiffStore.CachedDiff) {
        if let existing = entries[key] {
            totalBytes -= bytes(existing)
            order.removeAll { $0 == key }
        }
        entries[key] = value
        order.append(key)
        totalBytes += bytes(value)
        evict()
    }

    func clear() {
        entries.removeAll()
        order.removeAll()
        totalBytes = 0
    }

    private func touch(_ key: String) {
        order.removeAll { $0 == key }
        order.append(key)
    }

    /// Drop least-recently-used entries until under budget, always keeping the most recent one (so a single oversized file is still cached for its view).
    private func evict() {
        while totalBytes > budgetBytes, order.count > 1, let oldest = order.first {
            order.removeFirst()
            if let removed = entries.removeValue(forKey: oldest) {
                totalBytes -= bytes(removed)
            }
        }
    }

    /// Rust's copy of the diff plus the texts held here.
    private func bytes(_ diff: DiffStore.CachedDiff) -> Int {
        Int(diff.loaded.retainedBytes()) + diff.content.oldText.utf8.count + diff.content.newText.utf8.count
    }
}
