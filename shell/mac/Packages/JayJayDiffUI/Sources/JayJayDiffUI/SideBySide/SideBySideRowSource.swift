import Foundation
import JayJayCore

/// Visual side-by-side rows: Rust wraps the rows to the pane widths and returns UTF-16 ranges for the rows that wrap, so both panes advance in lock-step and only integers cross per width change.
public enum SideBySideRowSource {
    /// A row absent from `layout` is one visual line.
    public static func visualRows(rows: [SideBySideRow], layout: [SideBySideLayoutLine]) -> [SideBySideRow] {
        var visual: [SideBySideRow] = []
        visual.reserveCapacity(rows.count + layout.count)
        var next = layout.startIndex
        for (rowIx, row) in rows.enumerated() {
            let first = next
            while next < layout.endIndex, layout[next].rowIx == rowIx {
                visual.append(row.slice(layout[next], continuesRow: next > first))
                next += 1
            }
            if next == first {
                visual.append(row)
            }
        }
        return visual
    }
}

private extension SideBySideRow {
    func slice(_ line: SideBySideLayoutLine, continuesRow: Bool) -> SideBySideRow {
        if let source = line.fullWidthSource {
            let side = source == .old ? old : new
            return SideBySideRow(
                old: side.slice(line.old, lineNo: ""),
                new: side.slice(line.new, lineNo: ""),
                fullWidth: true,
                contextRegion: nil
            )
        }
        return SideBySideRow(
            old: old.slice(line.old, lineNo: continuesRow ? "" : old.lineNo),
            new: new.slice(line.new, lineNo: continuesRow ? "" : new.lineNo),
            fullWidth: false,
            contextRegion: contextRegion
        )
    }
}

private extension RowSide {
    func slice(_ range: Range<Int>, lineNo: String) -> RowSide {
        let (start, end) = (range.lowerBound, range.upperBound)
        var offset = 0
        var sliced: [DiffSpan] = []
        for span in spans {
            let length = span.text.utf16.count
            let lower = max(start, offset)
            let upper = min(end, offset + length)
            if lower < upper {
                let text = lower == offset && upper == offset + length
                    ? span.text
                    : (span.text as NSString).substring(with: NSRange(location: lower - offset, length: upper - lower))
                sliced.append(DiffSpan(text: text, style: span.style, token: span.token))
            }
            offset += length
        }
        return RowSide(lineNo: lineNo, spans: sliced, style: style, conflictKind: conflictKind)
    }
}
