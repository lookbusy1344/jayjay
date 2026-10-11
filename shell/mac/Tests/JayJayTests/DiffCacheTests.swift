@testable import JayJay
import JayJayCore
import XCTest

final class DiffCacheTests: XCTestCase {
    // MARK: - Content-addressed key

    func testKeyUsesCommitIdOverRev() {
        // Same working-copy rev, different content hash (e.g. after an edit/amend) must produce different keys so a stale diff is never served.
        let a = key(commitId: "c1", rev: "@", ignoreWhitespace: false)
        let b = key(commitId: "c2", rev: "@", ignoreWhitespace: false)
        XCTAssertNotEqual(a, b)
    }

    func testKeyIgnoresRevWhenCommitIdPresent() {
        // A change keeps its content hash across selection-revision spellings.
        let a = key(commitId: "c1", rev: "abc", ignoreWhitespace: false)
        let b = key(commitId: "c1", rev: "xyz", ignoreWhitespace: false)
        XCTAssertEqual(a, b)
    }

    func testKeyFallsBackToRevWhenCommitIdMissing() {
        let a = key(commitId: nil, rev: "r1", ignoreWhitespace: false)
        let b = key(commitId: "", rev: "r1", ignoreWhitespace: false)
        XCTAssertEqual(a, b, "empty commit id should fall back to rev")
    }

    func testKeyDistinguishesWhitespaceMode() {
        let on = key(commitId: "c1", rev: nil, ignoreWhitespace: true)
        let off = key(commitId: "c1", rev: nil, ignoreWhitespace: false)
        XCTAssertNotEqual(on, off)
    }

    func testKeyDistinguishesCompareSide() {
        let plain = key(commitId: "c1", rev: nil, compareFromRev: nil, ignoreWhitespace: false)
        let compare = key(commitId: "c1", rev: nil, compareFromRev: "c0", ignoreWhitespace: false)
        XCTAssertNotEqual(plain, compare)
    }

    func testKeyDistinguishesProjectionMode() {
        let processed = key(
            commitId: "c1", rev: nil, ignoreWhitespace: false,
            projectionKey: "ipynb:v1:processed"
        )
        let raw = key(
            commitId: "c1", rev: nil, ignoreWhitespace: false,
            projectionKey: "ipynb:v1:raw"
        )
        XCTAssertNotEqual(processed, raw)
    }

    func testProjectionModeChangeReloadsEvenWithInlineContent() {
        XCTAssertTrue(DiffStore.shouldLoadFileContent(
            oldContent: "processed old",
            newContent: "processed new",
            projectionModeChanged: true
        ))
    }

    func testInlineContentDoesNotReloadWhenProjectionModeIsUnchanged() {
        XCTAssertFalse(DiffStore.shouldLoadFileContent(
            oldContent: "old",
            newContent: "new",
            projectionModeChanged: false
        ))
    }

    func testDefaultProjectionModeUsesHunkProjectionMode() {
        let rawHunk = projectedHunk(mode: .raw)

        XCTAssertEqual(DiffStore.effectiveProjectionMode(hunk: rawHunk, mode: nil), .raw)
        XCTAssertEqual(DiffStore.effectiveProjectionMode(hunk: rawHunk, mode: .processed), .processed)
        XCTAssertNil(DiffStore.effectiveProjectionMode(hunk: plainHunk(), mode: nil))
    }

    // MARK: - LRU eviction

    func testEvictsLeastRecentlyUsedPastBudget() async {
        let cache = DiffCache(budgetBytes: 100)
        await cache.set("A", value: entry(bytes: 60))
        await cache.set("B", value: entry(bytes: 30)) // total 90, both fit

        let a = await cache.get("A") // touch A so B is now least recently used
        XCTAssertNotNil(a)

        await cache.set("C", value: entry(bytes: 30)) // total 120 > 100, evict LRU (B)

        let evicted = await cache.get("B")
        let keptA = await cache.get("A")
        let keptC = await cache.get("C")
        XCTAssertNil(evicted, "B was least recently used and should be evicted")
        XCTAssertNotNil(keptA)
        XCTAssertNotNil(keptC)
    }

    func testBudgetCountsTheRetainedDiffNotJustItsTexts() async {
        // Two 100-byte files: their texts alone fit 300 bytes; with their rows and Rust-side copies they do not.
        let text = String(repeating: "xxxxxxxxx\n", count: 10)
        let cache = DiffCache(budgetBytes: 300)
        await cache.set("old", value: entry(newText: text))
        await cache.set("new", value: entry(newText: text))

        let evicted = await cache.get("old")
        let kept = await cache.get("new")
        XCTAssertNil(evicted)
        XCTAssertNotNil(kept)
    }

    func testBudgetCountsTheReviewStateCopiedIntoSwift() async {
        let first = entry(newText: "a\n")
        let second = entry(newText: "b\n")
        // Each added file is one change group, so each carries one fingerprint lifted into Swift.
        XCTAssertEqual(first.review?.snapshot.fingerprints.count, 1)
        // The budget fits both entries exactly by Rust's count plus their two-byte texts; their Swift review copies push it over.
        let budget = Int(first.loaded.retainedBytes()) + 2 + Int(second.loaded.retainedBytes()) + 2
        let cache = DiffCache(budgetBytes: budget)
        await cache.set("first", value: first)
        await cache.set("second", value: second)

        let evicted = await cache.get("first")
        let kept = await cache.get("second")
        XCTAssertNil(evicted)
        XCTAssertNotNil(kept)
    }

    func testKeepsSingleEntryLargerThanBudget() async {
        let cache = DiffCache(budgetBytes: 100)
        await cache.set("big", value: entry(bytes: 500))
        let stored = await cache.get("big")
        XCTAssertNotNil(stored, "the only entry is kept even when it exceeds the budget")
    }

    // MARK: - Fixtures

    private func entry(bytes: Int) -> DiffStore.CachedDiff {
        DiffStore.CachedDiff(
            loaded: LoadedDiff.fromHunk(hunk: testHunk(projection: nil), ignoreWhitespace: false),
            content: DiffLoadedContent(oldContent: "", newContent: String(repeating: "x", count: bytes))
        )
    }

    private func entry(newText: String) -> DiffStore.CachedDiff {
        DiffStore.CachedDiff(LoadedDiff.fromHunk(
            hunk: testHunk(path: "f.txt", newContent: newText),
            ignoreWhitespace: false
        ))
    }

    private func plainHunk() -> DiffHunk {
        testHunk(projection: nil)
    }

    private func projectedHunk(mode: DiffProjectionMode) -> DiffHunk {
        testHunk(projection: testProjection(
            mode: mode,
            virtualPath: mode == .raw ? "results.sarif" : "results.sarif.md"
        ))
    }

    private func key(
        commitId: String?,
        rev: String?,
        compareFromRev: String? = nil,
        ignoreWhitespace: Bool,
        projectionKey: String = "raw"
    ) -> String {
        DiffStore.key(DiffStore.CacheKeyParts(
            commitId: commitId,
            rev: rev,
            compareFromRev: compareFromRev,
            ignoreWhitespace: ignoreWhitespace,
            path: "f",
            projectionKey: projectionKey
        ))
    }
}
