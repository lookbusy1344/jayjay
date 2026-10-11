import JayJayCore

/// Visual side-by-side rows for one diff at given pane widths, wrapped in Rust so both panes advance in lock-step.
public enum SideBySideRowSource {
    public static func visualRows(rows: [SideBySideRow], oldCols: UInt32, newCols: UInt32, movedLabelCols: UInt32) -> [WrappedSbsRow] {
        wrapSbsRows(rows: rows, oldCols: oldCols, newCols: newCols, movedLabelCols: movedLabelCols)
    }
}
