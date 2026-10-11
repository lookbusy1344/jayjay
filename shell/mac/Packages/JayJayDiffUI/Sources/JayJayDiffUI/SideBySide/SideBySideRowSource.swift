import JayJayCore

/// Visual side-by-side rows for one diff at given pane widths, wrapped in Rust so both panes advance in lock-step.
public enum SideBySideRowSource {
    public static func visualRows(lines: [DiffLine], oldCols: UInt32, newCols: UInt32) -> [SideBySideRow] {
        let rows = buildSideBySideRows(lines: lines)
        return wrapSbsRows(rows: rows, oldCols: oldCols, newCols: newCols).map(\.row)
    }
}
