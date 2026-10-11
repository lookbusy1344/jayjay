import Foundation
import JayJayCore
@testable import JayJayDiffUI
import XCTest

final class SideBySideLayoutLineTests: XCTestCase {
    func testDecodesSixLittleEndianWordsPerLine() {
        let bytes: [UInt8] = [
            3, 0, 0, 0, 0, 0, 0, 0, 40, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            4, 3, 2, 1, 5, 0, 0, 0, 6, 0, 0, 0, 6, 0, 0, 0, 44, 1, 0, 0, 2, 0, 0, 0,
            9, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 1, 0, 0, 0
        ]

        XCTAssertEqual(SideBySideLayoutLine.decode(Data(bytes)), [
            SideBySideLayoutLine(rowIx: 3, old: 0 ..< 40, new: 0 ..< 0, fullWidthSource: nil),
            SideBySideLayoutLine(rowIx: 0x0102_0304, old: 5 ..< 6, new: 6 ..< 300, fullWidthSource: .new),
            SideBySideLayoutLine(rowIx: 9, old: 1 ..< 2, new: 2 ..< 3, fullWidthSource: .old)
        ])
    }
}
