use std::f32::consts::FRAC_PI_2;

use jayjay_core::ChangeInfo;

use crate::app::theme::Theme;

pub(crate) const PREFERRED_NODE_RADIUS: f32 = 4.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NodeShape {
    Circle,
    Diamond,
}

#[derive(Clone, Copy)]
pub(crate) enum NodeFill {
    Filled(u32),
    Outlined(u32, f32),
}

#[derive(Clone, Copy, Default)]
pub(crate) struct NodeFlags {
    pub is_working_copy: bool,
    pub is_immutable: bool,
    pub has_conflict: bool,
    pub is_empty: bool,
    pub has_bookmark: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct DagNodeStyle {
    pub shape: NodeShape,
    pub radius: f32,
    pub fill: NodeFill,
}

impl DagNodeStyle {
    pub fn resolve(change: &ChangeInfo, theme: &Theme, base_radius: f32) -> Self {
        Self::new(
            NodeFlags {
                is_working_copy: change.is_working_copy,
                is_immutable: change.is_immutable,
                has_conflict: change.has_conflict,
                is_empty: change.is_empty,
                has_bookmark: !change.bookmarks.is_empty(),
            },
            theme,
            base_radius,
        )
    }

    pub fn new(flags: NodeFlags, theme: &Theme, base_radius: f32) -> Self {
        let shape = if flags.is_immutable {
            NodeShape::Diamond
        } else {
            NodeShape::Circle
        };
        let fill = if flags.is_working_copy {
            NodeFill::Filled(theme.selected_accent)
        } else if flags.has_conflict {
            NodeFill::Filled(theme.tag_conflict_fg)
        } else if flags.is_empty {
            NodeFill::Outlined(theme.dag_node, 1.5)
        } else if flags.has_bookmark {
            NodeFill::Outlined(theme.selected_accent, 1.8)
        } else {
            NodeFill::Filled(theme.dag_node)
        };
        Self {
            shape,
            radius: Self::radius(shape, flags.has_bookmark, base_radius),
            fill,
        }
    }

    pub fn radius(shape: NodeShape, has_bookmark: bool, base_radius: f32) -> f32 {
        let circle_radius = if has_bookmark {
            base_radius + 1.0
        } else {
            base_radius
        };
        match shape {
            NodeShape::Diamond => circle_radius * FRAC_PI_2.sqrt(),
            NodeShape::Circle => circle_radius,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DagNodeStyle, NodeShape, PREFERRED_NODE_RADIUS};
    use crate::app::theme::Theme;
    use jayjay_core::mock::change_info;

    #[test]
    fn node_shape_follows_immutability_regardless_of_bookmarks() {
        for bookmarks in [vec![], vec!["main".to_owned()]] {
            let mut change = change_info("c", "a");
            change.bookmarks = bookmarks;
            for (immutable, expected) in [(false, NodeShape::Circle), (true, NodeShape::Diamond)] {
                change.is_immutable = immutable;
                assert_eq!(
                    DagNodeStyle::resolve(&change, &Theme::light(), PREFERRED_NODE_RADIUS).shape,
                    expected
                );
            }
        }
    }
}
