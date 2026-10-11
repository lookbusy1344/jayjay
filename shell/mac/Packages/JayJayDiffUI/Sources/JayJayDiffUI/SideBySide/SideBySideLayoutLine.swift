import Foundation
import JayJayCore

/// One visual line of a side-by-side row that wraps or spans both panes, as UTF-16 ranges into the row's text.
public struct SideBySideLayoutLine: Equatable, Sendable {
    public let rowIx: Int
    public let old: Range<Int>
    public let new: Range<Int>
    /// For a full-width row, the side whose text both ranges index.
    public let fullWidthSource: DiffSide?
}

public extension LoadedDiff {
    func sideBySideLayout(oldCols: UInt32, newCols: UInt32, movedLabelCols: UInt32) -> [SideBySideLayoutLine] {
        SideBySideLayoutLine.decode(packedSideBySideLayout(oldCols: oldCols, newCols: newCols, movedLabelCols: movedLabelCols))
    }
}

extension SideBySideLayoutLine {
    /// Word order and source tags of `pack_sbs_layout` in jayjay-uniffi.
    private enum Word: Int, CaseIterable {
        case rowIx, oldStart, oldEnd, newStart, newEnd, source
    }

    private enum Source: UInt32 {
        case regular, old, new

        var side: DiffSide? {
            switch self {
                case .regular: nil
                case .old: .old
                case .new: .new
            }
        }
    }

    /// Reads `pack_sbs_layout` (jayjay-uniffi): six little-endian `UInt32` words per line, so the layout crosses as one buffer.
    static func decode(_ data: Data) -> [SideBySideLayoutLine] {
        let lineBytes = Word.allCases.count * MemoryLayout<UInt32>.size
        return data.withUnsafeBytes { raw in
            (0 ..< raw.count / lineBytes).map { line in
                func word(_ field: Word) -> UInt32 {
                    let offset = line * lineBytes + field.rawValue * MemoryLayout<UInt32>.size
                    return UInt32(littleEndian: raw.loadUnaligned(fromByteOffset: offset, as: UInt32.self))
                }
                guard let source = Source(rawValue: word(.source)) else {
                    preconditionFailure("unknown full-width source tag \(word(.source))")
                }
                return SideBySideLayoutLine(
                    rowIx: Int(word(.rowIx)),
                    old: Int(word(.oldStart)) ..< Int(word(.oldEnd)),
                    new: Int(word(.newStart)) ..< Int(word(.newEnd)),
                    fullWidthSource: source.side
                )
            }
        }
    }
}
