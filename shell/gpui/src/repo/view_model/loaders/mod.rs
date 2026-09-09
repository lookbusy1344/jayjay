mod diff;
mod diff_compute;
mod review_notes;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use gpui::{Context, SharedString};
use jayjay_core::{
    BookmarkInfo, ChangeInfo, CoreResult, DEFAULT_REVSET_DEPTH, DiffStats, FIRST_RESULT_BUDGET,
    GraphEntry, GraphLoadToken, LogGraphEvent, LogGraphRequest, LogGraphSnapshot,
    MAX_AUTO_LOADED_ROWS, Repo, WorkspaceInfo, build_default_revset, default_revset_depth,
};

use super::{PendingFocusTarget, PendingRefresh, RepoViewModel};
use crate::repo::revset;

/// Window during which FS echoes from our own mutations are ignored.
const MUTATION_ECHO_WINDOW: Duration = Duration::from_secs(5);

impl RepoViewModel {
    pub(in crate::repo) fn load_annotate(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let Some(rev) = self
            .selected
            .and_then(|i| self.graph.changes.get(i))
            .map(revset::change_revision)
        else {
            return;
        };
        let Some(path) = self.selected_hunk().map(|h| h.path.clone()) else {
            return;
        };

        self.loading.annotate_gen = self.loading.annotate_gen.wrapping_add(1);
        let generation = self.loading.annotate_gen;
        self.annotate_lines = None;
        self.loading.annotate = true;
        cx.notify();

        Self::background_update(
            cx,
            async move { repo.annotate_file(&rev, &path).ok() },
            move |vm, result, cx| {
                if vm.loading.annotate_gen != generation {
                    return;
                }
                vm.loading.annotate = false;
                vm.annotate_lines = result.map(Arc::new);
                cx.notify();
            },
        );
    }

    pub(in crate::repo) fn refresh_pr_info(&mut self, change: &ChangeInfo, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let Some(bookmark) = change.bookmarks.first().cloned() else {
            return;
        };
        self.loading.pr_gen = self.loading.pr_gen.wrapping_add(1);
        let generation = self.loading.pr_gen;
        self.loading.pr = true;
        Self::background_update(
            cx,
            async move { repo.pull_request_info(&bookmark) },
            move |vm, info, cx| {
                // A newer selection's fetch superseded this one; its result lands later.
                if vm.loading.pr_gen != generation {
                    return;
                }
                vm.loading.pr = false;
                vm.pr_info = info;
                cx.notify();
            },
        );
    }

    pub fn handle_operation_change(&mut self, cx: &mut Context<Self>) {
        self.loading
            .pending_auto_refresh
            .get_or_insert(PendingRefresh::CheckOperation);
        self.resume_pending_refresh(cx);
    }

    pub fn handle_working_copy_change(&mut self, cx: &mut Context<Self>) {
        // Gate before the echo check: an event remembered here must survive even if a mutation stamps the echo window before the overlay closes.
        if self.refresh_suspended {
            self.loading.pending_auto_refresh = Some(PendingRefresh::Reload);
            return;
        }
        // Ignore the FS echo from our own mutations — the mutation path already refreshed.
        if self.is_internal_mutation_echo() {
            return;
        }
        self.refresh(true, cx);
    }

    /// The owed refresh runs without an echo re-check: the deferred event was external when it arrived.
    pub fn set_refresh_suspended(&mut self, suspended: bool, cx: &mut Context<Self>) {
        if self.refresh_suspended == suspended {
            return;
        }
        self.refresh_suspended = suspended;
        self.resume_pending_refresh(cx);
    }

    /// The at-head check waits for in-flight work: that refresh is what moves the loaded repo to the head it compares against. Returns whether it ran, so a caller can distinguish "resumed" from "still owed" (e.g. while suspended).
    pub(crate) fn resume_pending_refresh(&mut self, cx: &mut Context<Self>) -> bool {
        if self.refresh_suspended || self.loading.refreshing {
            return false;
        }
        let Some(pending) = self.loading.pending_auto_refresh.take() else {
            return false;
        };
        if pending == PendingRefresh::CheckOperation
            && let Some(repo) = self.repo.as_ref()
            && repo.is_at_operation_head().unwrap_or(false)
        {
            return false;
        }
        self.refresh(true, cx);
        true
    }

    pub(in crate::repo) fn is_internal_mutation_echo(&self) -> bool {
        self.last_internal_mutation_at
            .is_some_and(|at| at.elapsed() < MUTATION_ECHO_WINDOW)
    }

    /// Refresh only the workspace picker without reloading the graph or selected change.
    pub(crate) fn refresh_workspaces(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        self.loading.workspaces_gen = self.loading.workspaces_gen.wrapping_add(1);
        let generation = self.loading.workspaces_gen;
        Self::background_update(
            cx,
            async move { repo.workspace_list() },
            move |vm, workspaces, cx| {
                if vm.loading.workspaces_gen != generation {
                    return;
                }
                if let Ok(workspaces) = workspaces {
                    vm.graph.workspaces = Arc::new(workspaces);
                    cx.notify();
                }
            },
        );
    }

    pub fn refresh(&mut self, is_auto_triggered: bool, cx: &mut Context<Self>) {
        self.refresh_with_working_copy_snapshot(is_auto_triggered, true, cx);
    }

    pub(super) fn refresh_with_working_copy_snapshot(
        &mut self,
        is_auto_triggered: bool,
        snapshot_working_copy: bool,
        cx: &mut Context<Self>,
    ) {
        let selection = self
            .selected
            .and_then(|ix| self.graph.changes.get(ix))
            .map(|c| (c.change_id.id.clone(), c.commit_id.id.clone()));
        self.refresh_preferring(is_auto_triggered, snapshot_working_copy, selection, cx);
    }

    /// `selection` is (change id, commit id): the commit wins, the change id is the fallback once a rewrite retired that commit.
    pub(super) fn refresh_preferring(
        &mut self,
        is_auto_triggered: bool,
        snapshot_working_copy: bool,
        selection: Option<(String, String)>,
        cx: &mut Context<Self>,
    ) {
        // FS event mid-refresh: defer it and re-run from the completion so the user's latest write isn't lost.
        if is_auto_triggered && self.loading.refreshing {
            self.loading.pending_auto_refresh = Some(PendingRefresh::Reload);
            // An external change makes the streaming snapshot stale; cancel the active session so the
            // deferred refresh starts against fresh state instead of waiting for the whole stale
            // stream to drain. The Canceled terminal event runs the pending refresh.
            self.cancel_graph_session(cx);
            return;
        }
        let Some(repo) = self.repo.clone() else {
            return;
        };
        // A reload while focused (auto or manual) is not a revset replacement: it neither dims the graph
        // nor scrolls, but the composed revset can emit descendants above the selected row, so hold the
        // current selection across snapshots rather than letting the fallback snap to `@`. Focus/clear
        // transitions set their own pinned target first; leave it untouched.
        if self.focused_revision.is_some() && self.pending_focus_target.is_none() {
            self.capture_graph_replacement_backup();
            let held_row = selection.as_ref().and_then(|(_, commit_id)| {
                self.graph
                    .changes
                    .iter()
                    .find(|c| &c.commit_id.id == commit_id)
            });
            self.pending_focus_target = match held_row {
                Some(change) => Some(PendingFocusTarget {
                    revision: crate::repo::revset::change_revision(change).into(),
                    commit_id: change.is_divergent.then(|| change.commit_id.id.clone()),
                    reveals_on_appear: false,
                }),
                None => self
                    .focused_revision
                    .clone()
                    .map(|revision| PendingFocusTarget {
                        revision,
                        commit_id: None,
                        reveals_on_appear: false,
                    }),
            };
        }
        self.loading.pending_auto_refresh = None;
        // A background refresh must not dismiss an error the user is still reading; manual refresh is an explicit retry.
        if !is_auto_triggered {
            self.clear_error();
        }
        // A manual refresh (e.g. a revset change) can reach here while an older session is still
        // streaming; cancel it so it stops consuming CPU instead of running to completion unseen.
        if let Some(old_token) = self.loading.graph_session.take() {
            old_token.cancel();
        }
        self.begin_refreshing(cx);
        self.loading.refresh_gen = self.loading.refresh_gen.wrapping_add(1);
        let generation = self.loading.refresh_gen;
        let revset = self.effective_revset();
        let previous_selection = selection;
        let token = GraphLoadToken::new();
        self.loading.graph_session = Some(token.clone());
        self.loading.graph_session_gen = Some(generation);
        self.loading.graph_in_flight_generations.insert(generation);
        self.loading.graph_session_canceling = false;
        self.loading.graph_first_snapshot_applied = false;
        self.loading.graph_load_slow = false;
        self.loading.graph_paused = false;
        let row_ceiling = self.effective_row_ceiling();
        Self::delayed_update(cx, FIRST_RESULT_BUDGET, move |vm, cx| {
            if vm.loading.refresh_gen == generation
                && !vm.loading.graph_first_snapshot_applied
                && vm.loading.graph_session.is_some()
            {
                vm.loading.graph_load_slow = true;
                cx.notify();
            }
        });

        Self::background_stream(
            cx,
            move |tx| {
                let ancillary = refresh_ancillary_blocking(&repo, snapshot_working_copy);
                let is_err = ancillary.is_err();
                let _ = tx.send(RefreshUpdate::Ancillary(ancillary));
                if is_err {
                    return;
                }
                let request = LogGraphRequest {
                    row_ceiling,
                    ..LogGraphRequest::new(revset)
                };
                repo.start_log_graph(request, token, |event| {
                    let _ = tx.send(RefreshUpdate::Graph(event));
                });
            },
            move |vm, update, cx| {
                vm.apply_refresh_update(
                    update,
                    is_auto_triggered,
                    &previous_selection,
                    generation,
                    cx,
                );
            },
        );
    }

    /// Cancel any in-flight graph load session, or start one if none is running. Wired to the
    /// toolbar refresh/cancel control so it never enqueues a second overlapping refresh.
    pub fn refresh_or_cancel(&mut self, cx: &mut Context<Self>) {
        if self.cancel_graph_session(cx) {
            return;
        }
        self.refresh(false, cx);
    }

    /// Row ceiling for the next session, resolving the `0` sentinel to the core default.
    fn effective_row_ceiling(&self) -> u32 {
        if self.loading.graph_row_ceiling == 0 {
            MAX_AUTO_LOADED_ROWS
        } else {
            self.loading.graph_row_ceiling
        }
    }

    /// Resume the session paused at the row ceiling without restarting its repository graph stream.
    pub fn continue_loading(&mut self, cx: &mut Context<Self>) {
        if !self.loading.graph_paused {
            return;
        }
        let Some(token) = self.loading.graph_session.clone() else {
            return;
        };
        let row_ceiling = self.effective_row_ceiling().saturating_mul(2);
        self.loading.graph_row_ceiling = row_ceiling;
        self.loading.graph_paused = false;
        self.begin_refreshing(cx);
        self.loading
            .graph_in_flight_generations
            .insert(self.loading.refresh_gen);
        token.continue_loading(row_ceiling);
    }

    /// Latch cancellation of the active graph session, if one is running. Returns whether a session
    /// was present, so callers can distinguish "canceled the running load" from "nothing to cancel".
    fn cancel_graph_session(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(token) = self.loading.graph_session.clone() else {
            return false;
        };
        if !self.loading.graph_session_canceling {
            token.cancel();
            self.loading.graph_session_canceling = true;
            cx.notify();
        }
        true
    }

    /// A write must never race a pinned graph snapshot. Invalidate its generation immediately so
    /// any event already queued for the UI cannot overwrite mutation-era state; retain the token
    /// until that worker's terminal event performs its own task bookkeeping.
    pub(in crate::repo) fn cancel_graph_session_for_mutation(&mut self, cx: &mut Context<Self>) {
        if self.cancel_graph_session(cx) {
            self.loading.refresh_gen = self.loading.refresh_gen.wrapping_add(1);
        }
    }

    fn apply_refresh_update(
        &mut self,
        update: RefreshUpdate,
        is_auto_triggered: bool,
        previous_selection: &Option<(String, String)>,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        match update {
            RefreshUpdate::Ancillary(Ok(data)) => {
                if self.loading.refresh_gen != generation {
                    return;
                }
                self.graph.bookmarks = Arc::new(data.bookmarks);
                if let Some(workspaces) = data.workspaces {
                    self.graph.workspaces = Arc::new(workspaces);
                }
                self.pr_host_name = data.pr_host_name.map(SharedString::from);
                self.working_copy_stats = data.working_copy_stats;
                self.current_operation_description = data.current_operation_description;
                cx.notify();
            }
            RefreshUpdate::Ancillary(Err(error)) => {
                self.finish_graph_session(generation, cx);
                if self.loading.refresh_gen == generation {
                    self.restore_graph_replacement();
                    self.present_error(error);
                    cx.notify();
                }
            }
            RefreshUpdate::Graph(LogGraphEvent::Snapshot(snapshot)) => {
                if self.loading.refresh_gen != generation {
                    return;
                }
                self.apply_graph_snapshot(snapshot, previous_selection, cx);
            }
            RefreshUpdate::Graph(LogGraphEvent::Progress(progress)) => {
                if self.loading.refresh_gen == generation
                    && !self.loading.graph_first_snapshot_applied
                    && progress.first_result_budget_expired
                {
                    self.loading.graph_load_slow = true;
                    cx.notify();
                }
            }
            RefreshUpdate::Graph(LogGraphEvent::EmptyStates(updates)) => {
                if self.loading.refresh_gen != generation {
                    return;
                }
                self.apply_empty_states(&updates, cx);
            }
            RefreshUpdate::Graph(LogGraphEvent::Paused) => {
                if self.loading.refresh_gen != generation {
                    return;
                }
                if self.loading.graph_in_flight_generations.remove(&generation) {
                    self.finish_repo_task(cx);
                }
                // An owed refresh supersedes the paused prefix; otherwise expose Continue Loading.
                if self.resume_pending_refresh(cx) {
                    return;
                }
                self.loading.graph_paused = true;
                cx.notify();
            }
            RefreshUpdate::Graph(LogGraphEvent::Finished) => {
                self.finish_graph_session(generation, cx);
                if self.loading.refresh_gen != generation {
                    return;
                }
                // The core omits the terminal is_complete snapshot when every row was already streamed,
                // so the focus-root recovery that apply_graph_snapshot runs on completion must also run
                // here.
                let entries = self.graph.entries.clone();
                if self.recover_missing_focus_target(&entries, cx) {
                    return;
                }
                if is_auto_triggered && self.refresh_suspended {
                    self.loading.pending_auto_refresh = Some(PendingRefresh::Reload);
                    return;
                }
                self.resume_pending_refresh(cx);
            }
            RefreshUpdate::Graph(LogGraphEvent::Canceled) => {
                self.finish_graph_session(generation, cx);
                // A stale-session cancel from an FS event leaves a deferred refresh owed; run it now
                // against fresh state. A user-initiated cancel leaves no pending refresh, so this is
                // inert. While suspended, keep it owed for `set_refresh_suspended` to run later.
                if self.loading.refresh_gen == generation {
                    self.restore_graph_replacement();
                    self.resume_pending_refresh(cx);
                }
            }
            RefreshUpdate::Graph(LogGraphEvent::Failed(error)) => {
                self.finish_graph_session(generation, cx);
                if self.loading.refresh_gen == generation {
                    self.restore_graph_replacement();
                    self.present_error(error);
                    cx.notify();
                }
            }
        }
    }

    /// Clears the session token and repo-task bookkeeping for `generation`'s terminal event,
    /// regardless of whether `generation` is still current — every `begin_refreshing()` needs
    /// exactly one matching `finish_repo_task()`, even for a superseded run.
    fn finish_graph_session(&mut self, generation: u64, cx: &mut Context<Self>) {
        if self.loading.graph_in_flight_generations.remove(&generation) {
            self.finish_repo_task(cx);
        }
        if self.loading.graph_session_gen == Some(generation) {
            self.loading.graph_session = None;
            self.loading.graph_session_gen = None;
            self.loading.graph_session_canceling = false;
            self.loading.more = false;
            self.loading.graph_load_slow = false;
        }
    }

    /// Applies one published graph prefix. The first snapshot of a session restores selection from
    /// `previous_selection`; later snapshots only append rows, since a session's prefixes share a
    /// stable ordering and never renumber an already-published row.
    fn apply_graph_snapshot(
        &mut self,
        snapshot: LogGraphSnapshot,
        previous_selection: &Option<(String, String)>,
        cx: &mut Context<Self>,
    ) {
        // A focused refresh that completes without surfacing the focus root means the root left the
        // graph. Snapshot entries are cumulative, so a complete snapshot missing the root is
        // authoritative. The core omits this terminal snapshot when every row was already streamed, so
        // the Finished handler repeats the check for that path.
        if snapshot.is_complete && self.recover_missing_focus_target(&snapshot.entries, cx) {
            return;
        }
        if snapshot.is_complete {
            self.graph_replacement_backup = None;
        }

        self.graph_awaiting_replacement = false;

        let is_first = !self.loading.graph_first_snapshot_applied;
        self.loading.graph_first_snapshot_applied = true;
        self.loading.graph_load_slow = false;

        if snapshot.is_complete {
            // Focus disables infinite-scroll paging: the composed revset is not a pageable default.
            self.can_load_more = self.focused_revision.is_none()
                && self
                    .revset_depth()
                    .is_some_and(|depth| snapshot.entries.len() >= depth as usize);
        }
        self.graph.dag_layout = Arc::new(snapshot.layout);
        let changes: Vec<ChangeInfo> = snapshot.entries.iter().map(|e| e.change.clone()).collect();

        // A pinned focus target may surface in any snapshot, not just the first. Select and reveal it
        // once it appears; until then hold selection rather than falling back to `@` or the first row.
        if let Some(target) = &self.pending_focus_target {
            let appeared = changes.iter().position(|c| target.matches(c));
            let reveals_on_appear = target.reveals_on_appear;
            self.graph.changes = Arc::new(changes);
            self.graph.entries = Arc::new(snapshot.entries);
            if let Some(ix) = appeared {
                self.pending_focus_target = None;
                if reveals_on_appear {
                    let revision = crate::repo::revset::change_revision(&self.graph.changes[ix]);
                    self.pending_focus_reveal = Some(revision.into());
                }
                self.select_change(ix, cx);
            } else {
                // Selection is index-based, so a snapshot without the target has no valid row to keep;
                // clear until it appears. Matching by id then re-selects the correct row, not a stale index.
                self.selected = None;
                self.selected_changes.clear();
                cx.notify();
            }
            return;
        }

        if !is_first {
            self.graph.changes = Arc::new(changes);
            self.graph.entries = Arc::new(snapshot.entries);
            cx.notify();
            return;
        }

        let new_selected = previous_selection
            .as_ref()
            .and_then(|(_, commit_id)| changes.iter().position(|c| &c.commit_id.id == commit_id))
            .or_else(|| {
                previous_selection.as_ref().and_then(|(change_id, _)| {
                    changes.iter().position(|c| &c.change_id.id == change_id)
                })
            })
            .or_else(|| changes.iter().position(|c| c.is_working_copy))
            .or(if changes.is_empty() { None } else { Some(0) });
        self.graph.changes = Arc::new(changes);
        self.graph.entries = Arc::new(snapshot.entries);
        // Re-select even if the index is unchanged — file contents may have.
        if let Some(ix) = new_selected {
            // Keep the user's place in the file column across a background reload; mutation paths may have staked a restore target already.
            if self.pending_file_selection.is_none() {
                self.pending_file_selection = self
                    .selected_file_ix
                    .and_then(|file_ix| self.files.as_ref()?.get(file_ix))
                    .map(|file| file.path.clone());
            }
            self.select_change(ix, cx);
        } else {
            self.loading.change_gen = self.loading.change_gen.wrapping_add(1);
            self.loading.pr_gen = self.loading.pr_gen.wrapping_add(1);
            self.selected = None;
            self.selected_changes.clear();
            self.clear_detail_state();
            self.compare = None;
            self.pr_info = None;
        }
        cx.notify();
    }

    /// Apply a batch of deferred `is_empty` corrections to the already-published rows. Merge and
    /// off-page rows are published as non-empty; these refine them once their parent-tree merge
    /// completes off the first-paint path.
    fn apply_empty_states(
        &mut self,
        updates: &[jayjay_core::EmptyStateUpdate],
        cx: &mut Context<Self>,
    ) {
        if updates.is_empty() {
            return;
        }
        let corrections: HashMap<&str, bool> = updates
            .iter()
            .map(|update| (update.commit_id.as_str(), update.is_empty))
            .collect();
        let entries = Arc::make_mut(&mut self.graph.entries);
        let changes = Arc::make_mut(&mut self.graph.changes);
        for (entry, change) in entries.iter_mut().zip(changes.iter_mut()) {
            if let Some(&is_empty) = corrections.get(entry.change.commit_id.id.as_str()) {
                entry.change.is_empty = is_empty;
                change.is_empty = is_empty;
            }
        }
        cx.notify();
    }

    pub fn apply_revset(&mut self, revset: &str, cx: &mut Context<Self>) {
        let trimmed = revset.trim();
        self.revset = if trimmed.is_empty() {
            build_default_revset(DEFAULT_REVSET_DEPTH).into()
        } else {
            trimmed.to_owned().into()
        };
        // A new base filter shows in full: a stale focus would silently scope it to an unrelated change.
        self.focused_revision = None;
        self.focused_commit_id = None;
        self.pending_focus_target = None;
        self.mark_graph_awaiting_replacement();
        self.reset_graph_paging();
        self.refresh(false, cx);
    }

    /// A fresh query disables paging and drops any raised Continue Loading ceiling.
    fn reset_graph_paging(&mut self) {
        self.can_load_more = false;
        self.loading.graph_row_ceiling = 0;
    }

    /// Retain the current graph so a focused refresh that loses its target can restore it. Cheap: the
    /// rows are behind `Arc`, so nothing is deep-copied.
    fn capture_graph_replacement_backup(&mut self) {
        if self.graph_replacement_backup.is_none() && !self.graph.entries.is_empty() {
            self.graph_replacement_backup = Some(super::GraphReplacementBackup {
                changes: self.graph.changes.clone(),
                entries: self.graph.entries.clone(),
                dag_layout: self.graph.dag_layout.clone(),
                selected: self.selected,
                selected_changes: self.selected_changes.clone(),
            });
        }
    }

    fn mark_graph_awaiting_replacement(&mut self) {
        self.capture_graph_replacement_backup();
        self.graph_awaiting_replacement = !self.graph.entries.is_empty();
    }

    fn restore_graph_replacement(&mut self) -> bool {
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

    /// Restore the prior graph and keep the focus pill when a focused refresh no longer contains its
    /// focus root (a rewrite invalidated the pinned commit id), rather than presenting a target-less
    /// graph as current. No-op when unfocused or the root is present. Returns whether recovery ran.
    fn recover_missing_focus_target(
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
            Some(target) => jayjay_core::focus_revset(self.revset.as_ref(), target.as_ref()),
            None => self.revset.to_string(),
        }
    }

    /// Scope the graph to `revision`'s connected lineage. Composition is always over the base `revset`,
    /// not the current effective one, so focusing a second change replaces the first.
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

    /// Keyboard/menu path to focus the current selection. Reachable when no row is clipped, so focus
    /// is not badge-only. No-op without a selection or when already focused on that revision.
    pub fn focus_selected(&mut self, cx: &mut Context<Self>) {
        let Some(revision) = self
            .selected
            .and_then(|ix| self.graph.changes.get(ix))
            .map(crate::repo::revset::change_revision)
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
                    revision: crate::repo::revset::change_revision(change).into(),
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

    pub(crate) fn revset_depth(&self) -> Option<u32> {
        default_revset_depth(&self.revset)
    }

    pub(crate) fn ensure_avatar(&mut self, email: String, cx: &mut Context<Self>) {
        if email.trim().is_empty() {
            return;
        }
        if self.avatar_in_flight.contains(&email) {
            return;
        }
        if let Some(path) = crate::ui::avatar::cache_path(&email)
            && path.exists()
        {
            return;
        }
        self.avatar_in_flight.insert(email.clone());
        let email_for_remove = email.clone();
        Self::background_update(
            cx,
            async move {
                crate::ui::avatar::fetch_blocking(&email);
            },
            move |vm, (), cx| {
                vm.avatar_in_flight.remove(&email_for_remove);
                cx.notify();
            },
        );
    }
}

/// One item flowing back from a refresh's background thread: the ancillary read (once, first),
/// then the graph session's events, in that order.
enum RefreshUpdate {
    Ancillary(CoreResult<AncillaryRefreshData>),
    Graph(LogGraphEvent),
}

struct AncillaryRefreshData {
    bookmarks: Vec<BookmarkInfo>,
    workspaces: Option<Vec<WorkspaceInfo>>,
    pr_host_name: Option<String>,
    working_copy_stats: Option<DiffStats>,
    current_operation_description: String,
}

fn refresh_ancillary_blocking(
    repo: &Repo,
    snapshot_working_copy: bool,
) -> CoreResult<AncillaryRefreshData> {
    if snapshot_working_copy {
        repo.refresh_working_copy()?;
    }
    let bookmarks = repo.list_bookmarks().unwrap_or_default();
    let workspaces = repo.workspace_list().ok();
    let pr_host_name = repo.pr_host_name();
    let working_copy_stats = repo.diff_stats("@").ok();
    let current_operation_description = repo.current_operation_description();
    Ok(AncillaryRefreshData {
        bookmarks,
        workspaces,
        pr_host_name,
        working_copy_stats,
        current_operation_description,
    })
}

#[cfg(test)]
mod focus_tests {
    use std::sync::Arc;

    use gpui::{AppContext, TestAppContext};
    use jayjay_core::dag::DagLayout;
    use jayjay_core::{
        ChangeInfo, CommitAuthor, DEFAULT_REVSET_DEPTH, GraphEntry, LogGraphSnapshot,
        NewChangeEligibility, ShortId, build_default_revset,
    };

    use super::super::{PendingFocusTarget, RepoViewModel};

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
            vm.revset = "all()".into();
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
            vm.revset = "all()".into();
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
            vm.revset = build_default_revset(DEFAULT_REVSET_DEPTH).into();
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

    #[gpui::test]
    fn completed_focus_without_target_preserves_prior_graph_and_pill(cx: &mut TestAppContext) {
        let vm = cx.new(|_| RepoViewModel::empty("/tmp".into()));
        vm.update(cx, |vm, cx| {
            vm.revset = "all()".into();
            let prior = vec![entry("A", "aaa"), entry("B", "bbb")];
            vm.graph.entries = std::sync::Arc::new(prior.clone());
            vm.graph.changes =
                std::sync::Arc::new(prior.iter().map(|e| e.change.clone()).collect());
            vm.focused_revision = Some("X".into());
            vm.pending_focus_target = Some(PendingFocusTarget {
                revision: "X".into(),
                commit_id: Some("xxx".into()),
                reveals_on_appear: true,
            });

            // The focused revset completed but the pinned target never appeared (a rewrite dropped it).
            vm.apply_graph_snapshot(snapshot(vec![entry("C", "ccc")], true), &None, cx);

            let commit_ids: Vec<&str> = vm
                .graph
                .entries
                .iter()
                .map(|e| e.change.commit_id.id.as_str())
                .collect();
            assert_eq!(commit_ids, vec!["aaa", "bbb"]);
            assert_eq!(vm.focused_revision.as_deref(), Some("X"));
            assert!(vm.pending_focus_target.is_none());
            assert!(vm.error.is_some());
            assert!(vm.graph_awaiting_replacement);
        });
    }

    #[gpui::test]
    fn progressive_focus_without_target_restores_prior_graph(cx: &mut TestAppContext) {
        let vm = cx.new(|_| RepoViewModel::empty("/tmp".into()));
        vm.update(cx, |vm, cx| {
            vm.revset = "all()".into();
            let prior = vec![entry("X", "xxx"), entry("A", "aaa")];
            vm.graph.entries = Arc::new(prior.clone());
            vm.graph.changes = Arc::new(prior.iter().map(|e| e.change.clone()).collect());

            vm.focus_on("X".into(), cx);
            vm.apply_graph_snapshot(snapshot(vec![entry("D", "ddd")], false), &None, cx);
            vm.apply_graph_snapshot(snapshot(vec![entry("D", "ddd")], true), &None, cx);

            let commit_ids: Vec<&str> = vm
                .graph
                .entries
                .iter()
                .map(|entry| entry.change.commit_id.id.as_str())
                .collect();
            assert_eq!(commit_ids, vec!["xxx", "aaa"]);
            assert!(vm.graph_awaiting_replacement);
            assert!(vm.error.is_some());
        });
    }

    #[gpui::test]
    fn failed_progressive_focus_restores_prior_graph(cx: &mut TestAppContext) {
        let vm = cx.new(|_| RepoViewModel::empty("/tmp".into()));
        vm.update(cx, |vm, cx| {
            let prior = vec![entry("X", "xxx"), entry("A", "aaa")];
            vm.graph.entries = Arc::new(prior.clone());
            vm.graph.changes = Arc::new(prior.iter().map(|e| e.change.clone()).collect());
            vm.focused_revision = Some("X".into());
            vm.mark_graph_awaiting_replacement();
            vm.pending_focus_target = Some(PendingFocusTarget {
                revision: "X".into(),
                commit_id: None,
                reveals_on_appear: true,
            });

            vm.apply_graph_snapshot(snapshot(vec![entry("D", "ddd")], false), &None, cx);
            let generation = vm.loading.refresh_gen;
            vm.apply_refresh_update(
                super::RefreshUpdate::Graph(jayjay_core::LogGraphEvent::Failed(
                    jayjay_core::CoreError::Review {
                        message: "stream failed".to_owned(),
                    },
                )),
                false,
                &None,
                generation,
                cx,
            );

            let commit_ids: Vec<&str> = vm
                .graph
                .entries
                .iter()
                .map(|entry| entry.change.commit_id.id.as_str())
                .collect();
            assert_eq!(commit_ids, vec!["xxx", "aaa"]);
            assert!(vm.graph_awaiting_replacement);
            assert!(
                vm.error
                    .as_deref()
                    .is_some_and(|error| error.contains("stream failed"))
            );
        });
    }

    #[gpui::test]
    fn streamed_focus_finishing_without_target_restores_prior_graph(cx: &mut TestAppContext) {
        // The core omits the terminal is_complete snapshot when every row streamed already, so a
        // target-less focused result completes via Finished alone. Recovery must still fire.
        let vm = cx.new(|_| RepoViewModel::empty("/tmp".into()));
        vm.update(cx, |vm, cx| {
            let prior = vec![entry("X", "xxx"), entry("A", "aaa")];
            vm.graph.entries = Arc::new(prior.clone());
            vm.graph.changes = Arc::new(prior.iter().map(|e| e.change.clone()).collect());
            vm.focused_revision = Some("X".into());
            vm.mark_graph_awaiting_replacement();
            vm.pending_focus_target = Some(PendingFocusTarget {
                revision: "X".into(),
                commit_id: None,
                reveals_on_appear: true,
            });

            vm.apply_graph_snapshot(snapshot(vec![entry("D", "ddd")], false), &None, cx);
            let generation = vm.loading.refresh_gen;
            vm.apply_refresh_update(
                super::RefreshUpdate::Graph(jayjay_core::LogGraphEvent::Finished),
                false,
                &None,
                generation,
                cx,
            );

            let commit_ids: Vec<&str> = vm
                .graph
                .entries
                .iter()
                .map(|entry| entry.change.commit_id.id.as_str())
                .collect();
            assert_eq!(commit_ids, vec!["xxx", "aaa"]);
            assert_eq!(vm.focused_revision.as_deref(), Some("X"));
            assert!(vm.pending_focus_target.is_none());
            assert!(vm.error.is_some());
            assert!(vm.graph_awaiting_replacement);
        });
    }

    #[gpui::test]
    fn later_focused_refresh_without_target_preserves_prior_graph(cx: &mut TestAppContext) {
        let vm = cx.new(|_| RepoViewModel::empty("/tmp".into()));
        vm.update(cx, |vm, cx| {
            let prior = vec![entry("X", "xxx")];
            vm.graph.entries = Arc::new(prior.clone());
            vm.graph.changes = Arc::new(prior.iter().map(|e| e.change.clone()).collect());
            vm.focused_revision = Some("X".into());
            vm.focused_commit_id = Some("xxx".into());
            vm.pending_focus_target = None;

            vm.apply_graph_snapshot(snapshot(Vec::new(), true), &None, cx);

            let commit_ids: Vec<&str> = vm
                .graph
                .entries
                .iter()
                .map(|entry| entry.change.commit_id.id.as_str())
                .collect();
            assert_eq!(commit_ids, vec!["xxx"]);
            assert!(vm.error.is_some());
            assert!(vm.graph_awaiting_replacement);
        });
    }

    #[gpui::test]
    fn pending_focus_does_not_retarget_an_existing_selection_by_index(cx: &mut TestAppContext) {
        let vm = cx.new(|_| RepoViewModel::empty("/tmp".into()));
        vm.update(cx, |vm, cx| {
            vm.graph.changes = Arc::new(vec![change("A", "aaa"), change("X", "xxx")]);
            vm.graph.entries = Arc::new(vec![entry("A", "aaa"), entry("X", "xxx")]);
            vm.selected = Some(1);
            vm.focused_revision = Some("X".into());
            vm.focused_commit_id = Some("xxx".into());
            vm.pending_focus_target = Some(PendingFocusTarget {
                revision: "X".into(),
                commit_id: Some("xxx".into()),
                reveals_on_appear: true,
            });

            vm.apply_graph_snapshot(snapshot(vec![entry("D", "ddd")], false), &None, cx);

            assert_eq!(vm.selected, None);
        });
    }

    #[gpui::test]
    fn clearing_focus_pins_the_selection_and_reveals_it_once_it_reappears(cx: &mut TestAppContext) {
        let vm = cx.new(|_| RepoViewModel::empty("/tmp".into()));
        vm.update(cx, |vm, cx| {
            vm.revset = "all()".into();
            // Focused on X; the user navigated to a descendant Y within the lineage.
            let focused = vec![entry("X", "xxx"), entry("Y", "yyy")];
            vm.graph.entries = Arc::new(focused.clone());
            vm.graph.changes = Arc::new(focused.iter().map(|e| e.change.clone()).collect());
            vm.focused_revision = Some("X".into());
            vm.focused_commit_id = Some("xxx".into());
            vm.selected = Some(1);

            vm.clear_focus(cx);

            assert!(vm.focused_revision.is_none());
            assert_eq!(
                vm.pending_focus_target,
                Some(PendingFocusTarget {
                    revision: "Y".into(),
                    commit_id: None,
                    reveals_on_appear: true,
                })
            );

            // Y is absent from the full graph's first prefix: selection stays pending, no reveal.
            vm.apply_graph_snapshot(
                snapshot(vec![entry("W", "www"), entry("A", "aaa")], false),
                &None,
                cx,
            );
            assert_eq!(vm.selected, None);
            assert!(vm.pending_focus_reveal.is_none());

            // Y streams in later: select and reveal it once.
            vm.apply_graph_snapshot(
                snapshot(
                    vec![entry("W", "www"), entry("A", "aaa"), entry("Y", "yyy")],
                    true,
                ),
                &None,
                cx,
            );
            assert!(vm.pending_focus_target.is_none());
            assert_eq!(vm.selected, Some(2));
            assert_eq!(vm.take_pending_focus_reveal().as_deref(), Some("Y"));
        });
    }

    #[gpui::test]
    fn reload_hold_reselects_the_target_without_revealing_it(cx: &mut TestAppContext) {
        let vm = cx.new(|_| RepoViewModel::empty("/tmp".into()));
        vm.update(cx, |vm, cx| {
            vm.focused_revision = Some("X".into());
            // A plain reload holds the current selection (Y), not the focus root, and must not scroll.
            vm.pending_focus_target = Some(PendingFocusTarget {
                revision: "Y".into(),
                commit_id: None,
                reveals_on_appear: false,
            });

            // The focus root X is always present under focus; the held selection Y sits among its lineage.
            vm.apply_graph_snapshot(
                snapshot(
                    vec![entry("D", "ddd"), entry("Y", "yyy"), entry("X", "xxx")],
                    true,
                ),
                &None,
                cx,
            );

            assert_eq!(vm.selected, Some(1));
            assert!(vm.pending_focus_target.is_none());
            assert!(vm.pending_focus_reveal.is_none());
        });
    }
}
