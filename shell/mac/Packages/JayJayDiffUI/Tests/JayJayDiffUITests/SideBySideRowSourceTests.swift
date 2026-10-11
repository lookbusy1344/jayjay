import JayJayCore
@testable import JayJayDiffUI
import XCTest

final class SideBySideRowSourceTests: XCTestCase {
    func testLayoutRangesSliceWrappedRowsAndRowsWithoutLinesStayWhole() {
        let row = SideBySideRow(
            old: side("7", ["ab名", "前👨‍👩‍👧"]),
            new: side("7", ["x"]),
            fullWidth: false,
            contextRegion: nil
        )
        let fitting = SideBySideRow(old: side("8", ["cd"]), new: side("8", ["yz"]), fullWidth: false, contextRegion: nil)
        let layout = [
            SideBySideLayoutLine(rowIx: 1, old: 0 ..< 4, new: 0 ..< 1, fullWidthSource: nil),
            SideBySideLayoutLine(rowIx: 1, old: 4 ..< 12, new: 0 ..< 0, fullWidthSource: nil)
        ]

        let visual = SideBySideRowSource.visualRows(rows: [fitting, row, fitting], layout: layout)
        let rows = visual.map(\.row)

        XCTAssertEqual(visual.map(\.rowIx), [0, 1, 1, 2])
        XCTAssertEqual(rows.map { $0.old.spans.map(\.text) }, [["cd"], ["ab名", "前"], ["👨‍👩‍👧"], ["cd"]])
        XCTAssertEqual(rows.map { $0.new.spans.map(\.text) }, [["yz"], ["x"], [], ["yz"]])
        XCTAssertEqual(rows.map(\.old.lineNo), ["8", "7", "", "8"])
    }

    func testFullWidthLinesSliceTheSourceSideIntoBothPanes() {
        let row = SideBySideRow(old: side("", []), new: side("", ["<<<<<<< ours"]), fullWidth: true, contextRegion: nil)
        let layout = [SideBySideLayoutLine(rowIx: 0, old: 0 ..< 10, new: 10 ..< 12, fullWidthSource: .new)]

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
