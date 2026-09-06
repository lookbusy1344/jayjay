import JayJayCore

/// Which of one row's cells carry the selected change's lane: its column, plus the segments above the node, below it, and passing beside it.
struct DAGLaneEmphasis: Equatable {
    let column: Int
    let nodeColumn: Int
    let incoming: Bool
    let outgoing: Bool
    let passThrough: Bool

    init?(span: DagLaneSpan?, rowIndex: Int?, nodeColumn: Int) {
        guard let span, let rowIndex else { return nil }
        let first = Int(span.firstRow)
        let last = Int(span.lastRow)
        column = Int(span.lane)
        self.nodeColumn = nodeColumn
        incoming = first < rowIndex && rowIndex <= last
        outgoing = first <= rowIndex && rowIndex < last
        passThrough = first < rowIndex && rowIndex < last
    }

    /// A cell above the node: the node's own column is its incoming edge, any other is a lane passing by.
    func emphasizesAbove(column: Int) -> Bool {
        column == self.column && (column == nodeColumn ? incoming : passThrough)
    }

    /// A cell below the node: the node's own column is its edge to the parent, any other is a lane passing by.
    func emphasizesBelow(column: Int) -> Bool {
        column == self.column && (column == nodeColumn ? outgoing : passThrough)
    }
}
