use jayjay_core::dag::LaneSpan;

/// Which of one row's cells carry the selected change's lane: its column, plus the segments above the node, below it, and passing beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LaneEmphasis {
    column: u32,
    node_column: u32,
    incoming: bool,
    outgoing: bool,
    pass_through: bool,
}

impl LaneEmphasis {
    pub(crate) fn new(span: LaneSpan, row: usize, node_column: u32) -> Self {
        let LaneSpan {
            lane,
            first_row,
            last_row,
        } = span;
        Self {
            column: lane as u32,
            node_column,
            incoming: first_row < row && row <= last_row,
            outgoing: first_row <= row && row < last_row,
            pass_through: first_row < row && row < last_row,
        }
    }

    /// A cell above the node: the node's own column is its incoming edge, any other is a lane passing by.
    pub(crate) fn above(self, column: u32) -> bool {
        column == self.column
            && if column == self.node_column {
                self.incoming
            } else {
                self.pass_through
            }
    }

    /// A cell below the node: the node's own column is its edge to the parent, any other is a lane passing by.
    pub(crate) fn below(self, column: u32) -> bool {
        column == self.column
            && if column == self.node_column {
                self.outgoing
            } else {
                self.pass_through
            }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPAN: LaneSpan = LaneSpan {
        lane: 1,
        first_row: 2,
        last_row: 5,
    };

    #[test]
    fn the_lane_runs_from_the_first_node_down_to_the_last() {
        let first = LaneEmphasis::new(SPAN, 2, 1);
        assert_eq!((first.above(1), first.below(1)), (false, true));
        let middle = LaneEmphasis::new(SPAN, 3, 1);
        assert_eq!((middle.above(1), middle.below(1)), (true, true));
        let last = LaneEmphasis::new(SPAN, 5, 1);
        assert_eq!((last.above(1), last.below(1)), (true, false));
        assert!(
            !middle.above(0) && !middle.below(2),
            "other columns stay faded"
        );
    }

    #[test]
    fn a_row_off_the_lane_shows_it_passing_only_between_its_ends() {
        let between = LaneEmphasis::new(SPAN, 4, 0);
        assert_eq!((between.above(1), between.below(1)), (true, true));
        assert!(
            !between.above(0),
            "the row's own node column is not the lane"
        );
        let at_first = LaneEmphasis::new(SPAN, 2, 0);
        assert_eq!((at_first.above(1), at_first.below(1)), (false, false));
        let at_last = LaneEmphasis::new(SPAN, 5, 0);
        assert_eq!((at_last.above(1), at_last.below(1)), (false, false));
        let before = LaneEmphasis::new(SPAN, 1, 0);
        assert_eq!((before.above(1), before.below(1)), (false, false));
        let after = LaneEmphasis::new(SPAN, 6, 0);
        assert_eq!((after.above(1), after.below(1)), (false, false));
    }
}
