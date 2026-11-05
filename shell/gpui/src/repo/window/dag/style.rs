use std::f32::consts::FRAC_PI_2;

use jayjay_core::ChangeInfo;

use crate::app::theme::Theme;
use jayjay_core::trunk::is_trunk_bookmark;

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

#[derive(Clone, Copy)]
pub(crate) struct DagNodeStyle {
    pub shape: NodeShape,
    pub radius: f32,
    pub fill: NodeFill,
}

impl DagNodeStyle {
    pub fn resolve(change: &ChangeInfo, theme: &Theme, base_radius: f32) -> Self {
        let is_trunk = change.bookmarks.iter().any(|b| is_trunk_bookmark(b));
        let has_bookmark = !change.bookmarks.is_empty();
        let shape = if change.is_immutable {
            NodeShape::Diamond
        } else {
            NodeShape::Circle
        };
        let circle_radius = if is_trunk || has_bookmark {
            base_radius + 1.0
        } else {
            base_radius
        };
        // A diamond inscribed in a circle's box covers 2/π of its area; scaling by √(π/2) gives both shapes equal visual weight.
        let radius = match shape {
            NodeShape::Diamond => circle_radius * FRAC_PI_2.sqrt(),
            NodeShape::Circle => circle_radius,
        };
        let fill = if change.is_working_copy {
            NodeFill::Filled(theme.selected_accent)
        } else if change.has_conflict {
            NodeFill::Filled(theme.tag_conflict_fg)
        } else if change.is_empty {
            NodeFill::Outlined(theme.dag_node, 1.5)
        } else if is_trunk || has_bookmark {
            NodeFill::Outlined(theme.selected_accent, 1.8)
        } else {
            NodeFill::Filled(theme.dag_node)
        };
        Self {
            shape,
            radius,
            fill,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DagNodeStyle, NodeShape};
    use crate::app::theme::Theme;
    use jayjay_core::ChangeInfo;
    use jayjay_core::mock::change_info;

    const BASE_RADIUS: f32 = 4.0;

    fn shape(configure: impl FnOnce(&mut ChangeInfo)) -> NodeShape {
        let mut change = change_info("c", "a");
        configure(&mut change);
        DagNodeStyle::resolve(&change, &Theme::light(), BASE_RADIUS).shape
    }

    #[test]
    fn immutable_changes_draw_as_diamonds_like_jj_log() {
        assert_eq!(shape(|c| c.is_immutable = true), NodeShape::Diamond);
        assert_eq!(shape(|c| c.is_immutable = false), NodeShape::Circle);
    }

    #[test]
    fn a_mutable_change_with_a_trunk_bookmark_stays_a_circle() {
        assert_eq!(
            shape(|c| c.bookmarks = vec!["main".to_owned()]),
            NodeShape::Circle
        );
    }
}
