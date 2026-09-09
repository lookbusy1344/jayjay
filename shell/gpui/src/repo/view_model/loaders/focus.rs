use gpui::{Context, SharedString};
use jayjay_core::GraphEntry;

use super::super::{GraphReplacementBackup, PendingFocusTarget, RepoViewModel};

impl RepoViewModel {
    /// A new base filter shows in full: a stale focus would silently scope it to an unrelated change.
    pub(super) fn clear_focus_for_new_filter(&mut self) {
        self.focused_revision = None;
        self.focused_commit_id = None;
        self.pending_focus_target = None;
        self.mark_graph_awaiting_replacement();
        self.reset_graph_paging();
    }

    /// A fresh query disables paging and drops any raised Continue Loading ceiling.
    pub(super) fn reset_graph_paging(&mut self) {
        self.can_load_more = false;
        self.loading.graph_row_ceiling = 0;
    }

    /// Retain the current graph so a focused refresh that loses its target can restore it. Cheap: the rows are behind `Arc`, so nothing is deep-copied.
    pub(super) fn capture_graph_replacement_backup(&mut self) {
        if self.graph_replacement_backup.is_none() && !self.graph.entries.is_empty() {
            self.graph_replacement_backup = Some(GraphReplacementBackup {
                changes: self.graph.changes.clone(),
                entries: self.graph.entries.clone(),
                dag_layout: self.graph.dag_layout.clone(),
                selected: self.selected,
                selected_changes: self.selected_changes.clone(),
            });
        }
    }

    pub(super) fn mark_graph_awaiting_replacement(&mut self) {
        self.capture_graph_replacement_backup();
        self.graph_awaiting_replacement = !self.graph.entries.is_empty();
    }

    pub(super) fn restore_graph_replacement(&mut self) -> bool {
        let Some(backup) = self.graph_replacement_backup.take() else {
            return false;
        };
        self.graph.changes = backup.changes;
        self.graph.entries = backup.entries;
        self.graph.dag_layout = backup.dag_layout;
        self.selected = backup.selected;
        self.selected_changes = backup.selected_changes;
        self.graph_awaiting_replacement = !self.graph.entries.is_empty();
        true
    }

    /// Restore the prior graph and keep the focus pill when a focused refresh no longer contains its focus root (a rewrite invalidated the pinned commit id), rather than presenting a target-less graph as current. No-op when unfocused or the root is present. Returns whether recovery ran.
    pub(super) fn recover_missing_focus_target(
        &mut self,
        entries: &[GraphEntry],
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(revision) = self.focused_revision.clone() else {
            return false;
        };
        let root = PendingFocusTarget {
            revision,
            commit_id: self.focused_commit_id.clone(),
            reveals_on_appear: false,
        };
        if entries.iter().any(|e| root.matches(&e.change)) {
            return false;
        }
        self.pending_focus_target = None;
        if !self.restore_graph_replacement() {
            self.graph_awaiting_replacement = !self.graph.entries.is_empty();
        }
        self.present_error("Focused change is no longer in the graph.");
        cx.notify();
        true
    }

    /// The revset actually queried: the base scoped to the focus target's lineage, or the base itself.
    pub(crate) fn effective_revset(&self) -> String {
        match &self.focused_revision {
            Some(target) => jayjay_core::focus_revset(self.revset(), target.as_ref()),
            None => self.revset().to_owned(),
        }
    }

    /// Scope the graph to `revision`'s connected lineage. Composition is always over the base revset, not the current effective one, so focusing a second change replaces the first.
    pub fn focus_on(&mut self, revision: SharedString, cx: &mut Context<Self>) {
        if self.focused_revision.as_ref() == Some(&revision) {
            return;
        }
        let target =
            self.graph.changes.iter().find(|c| {
                c.change_id.id == revision.as_ref() || c.commit_id.id == revision.as_ref()
            });
        let commit_id = target
            .filter(|change| change.is_divergent)
            .map(|change| change.commit_id.id.clone());
        self.pending_focus_target = Some(PendingFocusTarget {
            revision: revision.clone(),
            commit_id: commit_id.clone(),
            reveals_on_appear: true,
        });
        self.focused_revision = Some(revision);
        self.focused_commit_id = commit_id;
        self.mark_graph_awaiting_replacement();
        self.reset_graph_paging();
        self.refresh(false, cx);
    }

    /// Keyboard/menu path to focus the current selection. Reachable when no row is clipped, so focus is not badge-only. No-op without a selection or when already focused on that revision.
    pub fn focus_selected(&mut self, cx: &mut Context<Self>) {
        let Some(revision) = self
            .selected
            .and_then(|ix| self.graph.changes.get(ix))
            .map(|change| change.selection_revision().to_owned())
        else {
            return;
        };
        self.focus_on(revision.into(), cx);
    }

    pub fn clear_focus(&mut self, cx: &mut Context<Self>) {
        if self.focused_revision.is_none() {
            return;
        }
        self.focused_revision = None;
        self.focused_commit_id = None;
        // Pin the current selection so it survives the full graph's progressive snapshots and scrolls back into view; without the pin the taller base graph keeps its offset and leaves the selected row below the fold, or a row streamed after the first prefix is never reselected.
        self.pending_focus_target =
            self.selected
                .and_then(|ix| self.graph.changes.get(ix))
                .map(|change| PendingFocusTarget {
                    revision: change.selection_revision().into(),
                    commit_id: change.is_divergent.then(|| change.commit_id.id.clone()),
                    reveals_on_appear: true,
                });
        self.mark_graph_awaiting_replacement();
        self.reset_graph_paging();
        self.refresh(false, cx);
    }

    /// Consumed by the window after a focus target is selected, to scroll it into view once.
    pub(crate) fn take_pending_focus_reveal(&mut self) -> Option<SharedString> {
        self.pending_focus_reveal.take()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui::{AppContext, TestAppContext};
    use jayjay_core::dag::DagLayout;
    use jayjay_core::{
        ChangeInfo, CommitAuthor, DEFAULT_REVSET_DEPTH, GraphEntry, LogGraphSnapshot,
        NewChangeEligibility, RevsetFilterState, ShortId, build_default_revset,
    };

    use super::super::super::{PendingFocusTarget, RepoViewModel};

    fn change(change_id: &str, commit_id: &str) -> ChangeInfo {
        ChangeInfo {
            change_id: ShortId::new(change_id.to_string(), 1),
            commit_id: ShortId::new(commit_id.to_string(), 1),
            description: "entry".to_string(),
            author: CommitAuthor::empty(0),
            parents: Vec::new(),
            bookmarks: Vec::new(),
            tags: Vec::new(),
            workspaces: Vec::new(),
            is_working_copy: false,
            has_conflict: false,
            is_empty: false,
            is_immutable: false,
            is_divergent: false,
            new_change: NewChangeEligibility {
                on_top: true,
                before: true,
                after: true,
            },
        }
    }

    fn entry(change_id: &str, commit_id: &str) -> GraphEntry {
        GraphEntry {
            change: change(change_id, commit_id),
            edges: Vec::new(),
        }
    }

    fn snapshot(entries: Vec<GraphEntry>, is_complete: bool) -> LogGraphSnapshot {
        let layout = DagLayout::compute(&entries, true);
        let loaded_rows = entries.len() as u32;
        LogGraphSnapshot {
            entries,
            layout,
            loaded_rows,
            is_complete,
        }
    }

    #[gpui::test]
    fn effective_revset_composes_focus_over_base(cx: &mut TestAppContext) {
        let vm = cx.new(|_| RepoViewModel::empty("/tmp".into()));
        vm.update(cx, |vm, _| {
            vm.revset_filter = RevsetFilterState::new("all()");
            assert_eq!(vm.effective_revset(), "all()");
            vm.focused_revision = Some("abc".into());
            assert_eq!(vm.effective_revset(), "(all()) & (::abc | abc::)");
        });
    }

    #[gpui::test]
    fn pinned_target_held_until_it_appears_then_selected_and_revealed_once(
        cx: &mut TestAppContext,
    ) {
        let vm = cx.new(|_| RepoViewModel::empty("/tmp".into()));
        vm.update(cx, |vm, cx| {
            vm.focused_revision = Some("X".into());
            vm.graph_awaiting_replacement = true;
            vm.pending_focus_target = Some(PendingFocusTarget {
                revision: "X".into(),
                commit_id: Some("xxx".into()),
                reveals_on_appear: true,
            });

            // First snapshot lacks X: selection stays pending, no reveal.
            vm.apply_graph_snapshot(snapshot(vec![entry("D", "ddd")], false), &None, cx);
            assert!(vm.pending_focus_target.is_some());
            assert_eq!(vm.selected, None);
            assert!(vm.pending_focus_reveal.is_none());

            // X arrives: select and reveal it once.
            vm.apply_graph_snapshot(
                snapshot(vec![entry("D", "ddd"), entry("X", "xxx")], false),
                &None,
                cx,
            );
            assert!(vm.pending_focus_target.is_none());
            assert_eq!(vm.selected, Some(1));
            assert_eq!(vm.take_pending_focus_reveal().as_deref(), Some("X"));

            // A later snapshot keeps X selected without another reveal.
            vm.apply_graph_snapshot(
                snapshot(
                    vec![entry("D", "ddd"), entry("X", "xxx"), entry("E", "eee")],
                    true,
                ),
                &None,
                cx,
            );
            assert_eq!(vm.selected, Some(1));
            assert!(vm.pending_focus_reveal.is_none());
        });
    }

    #[gpui::test]
    fn divergent_target_matches_only_its_commit_id(cx: &mut TestAppContext) {
        let vm = cx.new(|_| RepoViewModel::empty("/tmp".into()));
        vm.update(cx, |vm, cx| {
            vm.focused_revision = Some("bbb".into());
            vm.pending_focus_target = Some(PendingFocusTarget {
                revision: "bbb".into(),
                commit_id: Some("bbb".into()),
                reveals_on_appear: true,
            });
            // Two divergent rows share the change id "shared"; only the bbb commit satisfies the pin.
            vm.apply_graph_snapshot(
                snapshot(vec![entry("shared", "aaa"), entry("shared", "bbb")], true),
                &None,
                cx,
            );
            assert_eq!(vm.selected, Some(1));
        });
    }

    #[gpui::test]
    fn non_divergent_focus_accepts_rewritten_commit_for_same_change(cx: &mut TestAppContext) {
        let vm = cx.new(|_| RepoViewModel::empty("/tmp".into()));
        vm.update(cx, |vm, cx| {
            vm.revset_filter = RevsetFilterState::new("all()");
            vm.graph.changes = Arc::new(vec![change("X", "old")]);
            vm.graph.entries = Arc::new(vec![entry("X", "old")]);

            vm.focus_on("X".into(), cx);
            vm.apply_graph_snapshot(snapshot(vec![entry("X", "new")], true), &None, cx);

            assert_eq!(vm.graph.entries[0].change.commit_id.id, "new");
            assert_eq!(vm.selected, Some(0));
            assert!(vm.error.is_none());
        });
    }

    #[gpui::test]
    fn focus_disables_paging_when_the_composed_result_completes(cx: &mut TestAppContext) {
        let vm = cx.new(|_| RepoViewModel::empty("/tmp".into()));
        vm.update(cx, |vm, cx| {
            vm.revset_filter = RevsetFilterState::new(&build_default_revset(DEFAULT_REVSET_DEPTH));
            vm.focused_revision = Some("c0".into());
            vm.pending_focus_target = Some(PendingFocusTarget {
                revision: "c0".into(),
                commit_id: Some("h0".into()),
                reveals_on_appear: true,
            });
            let entries: Vec<GraphEntry> = (0..=DEFAULT_REVSET_DEPTH)
                .map(|i| entry(&format!("c{i}"), &format!("h{i}")))
                .collect();
            vm.apply_graph_snapshot(snapshot(entries, true), &None, cx);
            assert!(!vm.can_load_more);
        });
    }
}
