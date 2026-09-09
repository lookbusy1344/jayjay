use std::sync::Arc;

use gpui::SharedString;
use jayjay_core::ChangeInfo;
use jayjay_core::GraphEntry;
use jayjay_core::dag::{DagLayout, OrderedSelection};

/// A focus target pinned across progressive graph snapshots. Matched by exact commit id when known (so a divergent change's other versions never satisfy it), falling back to its revision string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PendingFocusTarget {
    pub revision: SharedString,
    pub commit_id: Option<String>,
    /// Whether appearing should scroll the target into view. A focus transition reveals its target; a plain reload only holds the existing selection and must not scroll away from where the user is.
    pub reveals_on_appear: bool,
}

impl PendingFocusTarget {
    pub(crate) fn matches(&self, change: &ChangeInfo) -> bool {
        match &self.commit_id {
            Some(commit_id) => &change.commit_id.id == commit_id,
            None => {
                change.change_id.id == self.revision.as_ref()
                    || change.commit_id.id == self.revision.as_ref()
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct GraphReplacementBackup {
    pub(super) changes: Arc<Vec<ChangeInfo>>,
    pub(super) entries: Arc<Vec<GraphEntry>>,
    pub(super) dag_layout: Arc<DagLayout>,
    pub(super) selected: Option<usize>,
    pub(super) selected_changes: OrderedSelection,
}
