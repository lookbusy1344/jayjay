use gpui::{Context, SharedString};

use super::super::{ElisionExpansion, RepoViewModel};

impl RepoViewModel {
    /// Show the revisions hidden behind `owner`'s elision bands. Expansions layer over the base revset like focus does, so the base keeps paging and stays as the user typed it.
    pub fn expand_elisions(
        &mut self,
        owner: SharedString,
        targets: Vec<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let added: Vec<ElisionExpansion> = targets
            .into_iter()
            .map(|target| ElisionExpansion {
                owner: owner.clone(),
                target,
            })
            .filter(|expansion| !self.expanded_elisions.contains(expansion))
            .collect();
        if added.is_empty() {
            return;
        }
        self.expanded_elisions.extend(added);
        self.refresh(false, cx);
    }

    pub fn collapse_elisions(&mut self, owner: &str, cx: &mut Context<Self>) {
        if !self.has_expanded_elisions(owner) {
            return;
        }
        self.expanded_elisions
            .retain(|expansion| expansion.owner.as_ref() != owner);
        self.refresh(false, cx);
    }

    pub fn show_selected_elided_revisions(&mut self, cx: &mut Context<Self>) {
        let (Some(ix), Some(owner)) = (self.selected, self.selected_revision()) else {
            return;
        };
        let targets = self.elided_targets(ix);
        self.expand_elisions(owner.into(), targets, cx);
    }

    pub fn hide_selected_expanded_revisions(&mut self, cx: &mut Context<Self>) {
        if let Some(owner) = self.selected_revision() {
            self.collapse_elisions(&owner, cx);
        }
    }

    /// Only a row whose elisions were expanded offers to hide them again.
    pub(crate) fn has_expanded_elisions(&self, owner: &str) -> bool {
        self.expanded_elisions
            .iter()
            .any(|expansion| expansion.owner.as_ref() == owner)
    }

    /// Revisions the row's elision bands lead to. A target not loaded yet has no row, so its commit id stands in.
    pub(crate) fn elided_targets(&self, ix: usize) -> Vec<SharedString> {
        let Some(row) = self.graph.dag_layout.rows.get(ix) else {
            return Vec::new();
        };
        row.elisions_after
            .iter()
            .map(|band| {
                self.graph
                    .changes
                    .iter()
                    .find(|change| change.commit_id.id == band.target_commit_id)
                    .map_or(band.target_commit_id.clone(), |change| {
                        change.selection_revision().to_owned()
                    })
                    .into()
            })
            .collect()
    }

    /// The base revset with every expanded path added, before focus scopes it.
    pub(super) fn expanded_revset(&self) -> String {
        self.expanded_elisions
            .iter()
            .fold(self.revset().to_owned(), |base, expansion| {
                jayjay_core::expand_elision_revset(&base, &expansion.owner, &expansion.target)
            })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui::{AppContext, TestAppContext};
    use jayjay_core::dag::DagLayout;
    use jayjay_core::{
        ChangeInfo, CommitAuthor, EdgeType, GraphEdge, GraphEntry, NewChangeEligibility,
        RevsetFilterState, ShortId,
    };

    use super::super::super::RepoViewModel;

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

    #[gpui::test]
    fn expansions_union_each_hidden_path_once_beneath_focus(cx: &mut TestAppContext) {
        let vm = cx.new(|_| RepoViewModel::empty("/tmp".into()));
        vm.update(cx, |vm, cx| {
            vm.revset_filter = RevsetFilterState::new("all()");
            vm.expand_elisions("S".into(), vec!["T1".into(), "T2".into()], cx);
            vm.expand_elisions("S".into(), vec!["T1".into()], cx);

            assert_eq!(vm.expanded_elisions.len(), 2);
            assert_eq!(
                vm.effective_revset(),
                "((all()) | (present(T1)::present(S))) | (present(T2)::present(S))"
            );

            vm.focused_revision = Some("S".into());
            assert_eq!(
                vm.effective_revset(),
                "(((all()) | (present(T1)::present(S))) | (present(T2)::present(S))) & (::S | S::)"
            );
        });
    }

    #[gpui::test]
    fn collapsing_a_row_keeps_other_rows_expanded(cx: &mut TestAppContext) {
        let vm = cx.new(|_| RepoViewModel::empty("/tmp".into()));
        vm.update(cx, |vm, cx| {
            vm.revset_filter = RevsetFilterState::new("all()");
            vm.expand_elisions("S".into(), vec!["T".into()], cx);
            vm.expand_elisions("R".into(), vec!["Q".into()], cx);
            vm.collapse_elisions("S", cx);

            assert!(!vm.has_expanded_elisions("S"));
            assert!(vm.has_expanded_elisions("R"));
            assert_eq!(vm.effective_revset(), "(all()) | (present(Q)::present(R))");
        });
    }

    #[gpui::test]
    fn elided_targets_use_the_loaded_rows_revision_and_fall_back_to_the_commit_id(
        cx: &mut TestAppContext,
    ) {
        let owner = GraphEntry {
            change: change("S", "sss"),
            edges: vec![
                GraphEdge {
                    target: "bbb".to_string(),
                    edge_type: EdgeType::Indirect,
                },
                GraphEdge {
                    target: "unloaded".to_string(),
                    edge_type: EdgeType::Indirect,
                },
            ],
        };
        let base = GraphEntry {
            change: change("B", "bbb"),
            edges: Vec::new(),
        };
        let entries = vec![owner, base];
        let vm = cx.new(|_| RepoViewModel::empty("/tmp".into()));
        vm.update(cx, |vm, _| {
            vm.graph.dag_layout = Arc::new(DagLayout::compute(&entries, true));
            vm.graph.changes = Arc::new(entries.iter().map(|e| e.change.clone()).collect());
            vm.graph.entries = Arc::new(entries);

            assert_eq!(vm.elided_targets(0), vec!["B", "unloaded"]);
            assert!(vm.elided_targets(1).is_empty());
        });
    }

    #[gpui::test]
    fn repository_menu_shows_and_hides_the_selected_rows_elided_revisions(cx: &mut TestAppContext) {
        let owner = GraphEntry {
            change: change("S", "sss"),
            edges: vec![GraphEdge {
                target: "bbb".to_string(),
                edge_type: EdgeType::Indirect,
            }],
        };
        let base = GraphEntry {
            change: change("B", "bbb"),
            edges: Vec::new(),
        };
        let entries = vec![owner, base];
        let vm = cx.new(|_| RepoViewModel::empty("/tmp".into()));
        vm.update(cx, |vm, cx| {
            vm.revset_filter = RevsetFilterState::new("all()");
            vm.graph.dag_layout = Arc::new(DagLayout::compute(&entries, true));
            vm.graph.changes = Arc::new(entries.iter().map(|e| e.change.clone()).collect());
            vm.graph.entries = Arc::new(entries);

            vm.selected = Some(1);
            vm.show_selected_elided_revisions(cx);
            assert!(
                vm.expanded_elisions.is_empty(),
                "the base row owns no bands"
            );

            vm.selected = Some(0);
            vm.show_selected_elided_revisions(cx);
            assert_eq!(vm.effective_revset(), "(all()) | (present(B)::present(S))");

            vm.hide_selected_expanded_revisions(cx);
            assert_eq!(vm.effective_revset(), "all()");
        });
    }

    #[gpui::test]
    fn a_new_base_revset_drops_expansions(cx: &mut TestAppContext) {
        let vm = cx.new(|_| RepoViewModel::empty("/tmp".into()));
        vm.update(cx, |vm, cx| {
            vm.revset_filter = RevsetFilterState::new("all()");
            vm.expand_elisions("S".into(), vec!["T".into()], cx);
            vm.apply_revset("mine()", cx);

            assert!(vm.expanded_elisions.is_empty());
            assert_eq!(vm.effective_revset(), "mine()");
        });
    }
}
