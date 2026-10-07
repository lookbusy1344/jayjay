use gpui::SharedString;

/// One `(elided revisions)` band the user expanded: the revisions between `target` and the row that owns the band, `owner`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ElisionExpansion {
    pub owner: SharedString,
    pub target: SharedString,
}
