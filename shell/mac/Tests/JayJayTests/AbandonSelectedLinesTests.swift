@testable import JayJay
import JayJayCore
import XCTest

final class AbandonSelectedLinesTests: XCTestCase {
    func testSelectionMapsRenderedRowsToTheRawDiffByLineNotByIndex() {
        let old = "let a = 1;\nlet b = 2;\n"
        let new = "let a = 10;\nlet b = 20;\n"
        let hunk = testHunk(path: "f.rs", oldContent: old, newContent: new, hunkType: .modified)
        let loaded = DiffSectionLoadedDiff(
            DiffStore.CachedDiff(LoadedDiff.fromHunk(hunk: hunk, ignoreWhitespace: false)),
            path: hunk.path, basis: "basis", hunk: hunk
        )
        let fullLines = computeFileDiffFull(path: hunk.path, oldContent: old, newContent: new, ignoreWhitespace: false).lines
        // The raw diff pairs each edited line; the rendered rows list both removals before both additions.
        XCTAssertEqual(fullLines.map(\.style), [.removed, .added, .removed, .added])
        XCTAssertEqual(loaded.rows?.lines.map(\.style), [.removed, .removed, .added, .added])

        // Rendered row 3 is the addition `let a = 10;`, raw line 2.
        let keys = loaded.changedLineKeys(selecting: 3 ... 3)
        XCTAssertEqual(DiffEditRange.ranges(holding: keys, in: fullLines), [DiffEditRange(startLine: 2, endLine: 2)])

        // Rendered rows 2–3 are the removal of `let b = 2;` and the addition `let a = 10;`, raw lines 2 and 3.
        let pair = loaded.changedLineKeys(selecting: 2 ... 3)
        XCTAssertEqual(DiffEditRange.ranges(holding: pair, in: fullLines), [DiffEditRange(startLine: 2, endLine: 3)])
    }
}
