import JayJayCore
@testable import JayJayDiffUI
import XCTest

final class SideBySideRowSourceTests: XCTestCase {
    func testLayoutRangesSliceSpansByUtf16Offsets() {
        let row = SideBySideRow(
            old: side("7", ["ab名", "前👨‍👩‍👧"]),
            new: side("7", ["x"]),
            fullWidth: false,
            contextRegion: nil
        )
        let layout = [
            SbsVisualLine(rowIx: 0, old: Utf16Range(start: 0, end: 4), new: Utf16Range(start: 0, end: 1), fullWidthSource: nil),
            SbsVisualLine(rowIx: 0, old: Utf16Range(start: 4, end: 12), new: Utf16Range(start: 0, end: 0), fullWidthSource: nil)
        ]

        let rows = SideBySideRowSource.visualRows(rows: [row], layout: layout).map(\.row)

        XCTAssertEqual(rows.map { $0.old.spans.map(\.text) }, [["ab名", "前"], ["👨‍👩‍👧"]])
        XCTAssertEqual(rows.map { $0.new.spans.map(\.text) }, [["x"], []])
        XCTAssertEqual(rows.map(\.old.lineNo), ["7", ""])
    }

    func testFullWidthLinesSliceTheSourceSideIntoBothPanes() {
        let row = SideBySideRow(old: side("", []), new: side("", ["<<<<<<< ours"]), fullWidth: true, contextRegion: nil)
        let layout = [SbsVisualLine(rowIx: 0, old: Utf16Range(start: 0, end: 10), new: Utf16Range(start: 10, end: 12), fullWidthSource: .new)]

        let rows = SideBySideRowSource.visualRows(rows: [row], layout: layout).map(\.row)

        XCTAssertEqual(rows.map { $0.old.spans.map(\.text) }, [["<<<<<<< ou"]])
        XCTAssertEqual(rows.map { $0.new.spans.map(\.text) }, [["rs"]])
        XCTAssertEqual(rows.map(\.fullWidth), [true])
    }

    private func side(_ lineNo: String, _ texts: [String]) -> RowSide {
        RowSide(
            lineNo: lineNo,
            spans: texts.map { DiffSpan(text: $0, style: .removed, token: .plain) },
            style: .removed,
            conflictKind: .none
        )
    }
}
