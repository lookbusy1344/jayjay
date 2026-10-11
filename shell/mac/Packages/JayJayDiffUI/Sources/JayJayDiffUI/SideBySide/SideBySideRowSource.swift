import Foundation
import JayJayCore

/// Visual side-by-side rows: Rust wraps the rows to the pane widths and returns UTF-16 ranges, so both panes advance in lock-step and only integers cross per width change.
public enum SideBySideRowSource {
    public static func visualRows(rows: [SideBySideRow], layout: [SbsVisualLine]) -> [SideBySideRow] {
        var previousRow: UInt32?
        return layout.map { line in
            let row = rows[Int(line.rowIx)]
            let continuesRow = previousRow == line.rowIx
            previousRow = line.rowIx
            if let source = line.fullWidthSource {
                let side = source == .old ? row.old : row.new
                return SideBySideRow(
                    old: side.slice(line.old, lineNo: ""),
                    new: side.slice(line.new, lineNo: ""),
                    fullWidth: true,
                    contextRegion: nil
                )
            }
            return SideBySideRow(
                old: row.old.slice(line.old, lineNo: continuesRow ? "" : row.old.lineNo),
                new: row.new.slice(line.new, lineNo: continuesRow ? "" : row.new.lineNo),
                fullWidth: false,
                contextRegion: row.contextRegion
            )
        }
    }
}

private extension RowSide {
    func slice(_ range: Utf16Range, lineNo: String) -> RowSide {
        let start = Int(range.start)
        let end = Int(range.end)
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
