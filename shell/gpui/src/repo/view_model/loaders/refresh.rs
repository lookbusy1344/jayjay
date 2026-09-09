use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use gpui::{Context, SharedString};
use jayjay_core::{
    BookmarkInfo, ChangeInfo, DiffStats, FIRST_RESULT_BUDGET, GraphLoadToken, JayError, JayResult,
    LogGraphEvent, LogGraphRequest, LogGraphSnapshot, MAX_AUTO_LOADED_ROWS, Repo, RevsetVocabulary,
    TagInfo, WorkspaceInfo,
};

use super::super::{PendingFocusTarget, PendingRefresh, RepoViewModel};

const MUTATION_ECHO_WINDOW: Duration = Duration::from_secs(5);

impl RepoViewModel {
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

    pub fn refresh(&mut self, is_auto_triggered: bool, cx: &mut Context<Self>) {
        self.refresh_with_working_copy_snapshot(is_auto_triggered, true, cx);
    }

    pub(in crate::repo::view_model) fn refresh_with_working_copy_snapshot(
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
    pub(in crate::repo::view_model) fn refresh_preferring(
        &mut self,
        is_auto_triggered: bool,
        snapshot_working_copy: bool,
        selection: Option<(String, String)>,
        cx: &mut Context<Self>,
    ) {
        // FS event mid-refresh: defer it and re-run from the completion so the user's latest write isn't lost.
        if is_auto_triggered && self.loading.refreshing {
            self.loading.pending_auto_refresh = Some(PendingRefresh::Reload);
            // An external change makes the streaming snapshot stale; cancel the active session so the deferred refresh starts against fresh state. The Canceled terminal event runs the pending refresh.
            self.cancel_graph_session(cx);
            return;
        }
        let Some(repo) = self.repo.clone() else {
            return;
        };
        // A reload while focused (auto or manual) is not a revset replacement: it neither dims the graph nor scrolls, but the composed revset can emit descendants above the selected row, so hold the current selection across snapshots rather than letting the fallback snap to `@`. Focus/clear transitions set their own pinned target first; leave it untouched.
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
                    revision: change.selection_revision().into(),
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
        // A manual refresh (e.g. a revset change) can reach here while an older session is still streaming; cancel it so it stops consuming CPU instead of running to completion unseen.
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

    /// Cancel any in-flight graph load session, or start one if none is running. Wired to the toolbar refresh/cancel control so it never enqueues a second overlapping refresh.
    pub fn refresh_or_cancel(&mut self, cx: &mut Context<Self>) {
        if self.cancel_graph_session(cx) {
            return;
        }
        self.run_refresh(cx);
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

    /// Latch cancellation of the active graph session, if one is running. Returns whether a session was present, so callers can distinguish "canceled the running load" from "nothing to cancel".
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

    /// A write must never race a pinned graph snapshot. Invalidate its generation immediately so any event already queued for the UI cannot overwrite mutation-era state; retain the token until that worker's terminal event performs its own task bookkeeping.
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
                if let Some(stale) = data.working_copy_stale {
                    self.working_copy_stale = stale;
                }
                self.graph.bookmarks = Arc::new(data.bookmarks);
                self.graph.tags = Arc::new(data.tags);
                self.vocabulary = data.vocabulary;
                if let Some(workspaces) = data.workspaces {
                    self.graph.workspaces = Arc::new(workspaces);
                }
                self.pr_host_name = data.pr_host_name.map(SharedString::from);
                self.working_copy_stats = data.working_copy_stats;
                self.current_operation_description = data.current_operation_description;
                self.fix_unavailable_reason = data.fix_unavailable_reason.map(SharedString::from);
                cx.notify();
            }
            RefreshUpdate::Ancillary(Err(error)) => {
                self.finish_graph_session(generation, cx);
                if self.loading.refresh_gen == generation {
                    self.pending_error = None;
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
                // The core omits the terminal is_complete snapshot when every row was already streamed, so the focus-root recovery that apply_graph_snapshot runs on completion must also run here.
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
                // A stale-session cancel from an FS event leaves a deferred refresh owed; run it now against fresh state. A user-initiated cancel leaves no pending refresh, so this is inert. While suspended, keep it owed for `set_refresh_suspended` to run later.
                if self.loading.refresh_gen == generation {
                    self.restore_graph_replacement();
                    self.resume_pending_refresh(cx);
                }
            }
            RefreshUpdate::Graph(LogGraphEvent::Failed(error)) => {
                self.finish_graph_session(generation, cx);
                if self.loading.refresh_gen == generation {
                    self.pending_error = None;
                    self.restore_graph_replacement();
                    self.present_error(error);
                    cx.notify();
                }
            }
        }
    }

    /// Clears the session token and repo-task bookkeeping for `generation`'s terminal event, regardless of whether `generation` is still current — every `begin_refreshing()` needs exactly one matching `finish_repo_task()`, even for a superseded run.
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

    /// Applies one published graph prefix. The first snapshot of a session restores selection from `previous_selection`; later snapshots only append rows, since a session's prefixes share a stable ordering and never renumber an already-published row.
    pub(super) fn apply_graph_snapshot(
        &mut self,
        snapshot: LogGraphSnapshot,
        previous_selection: &Option<(String, String)>,
        cx: &mut Context<Self>,
    ) {
        // A focused refresh that completes without surfacing the focus root means the root left the graph. Snapshot entries are cumulative, so a complete snapshot missing the root is authoritative. The core omits this terminal snapshot when every row was already streamed, so the Finished handler repeats the check for that path.
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

        // A pinned focus target may surface in any snapshot, not just the first. Select and reveal it once it appears; until then hold selection rather than falling back to `@` or the first row.
        if let Some(target) = &self.pending_focus_target {
            let appeared = changes.iter().position(|c| target.matches(c));
            let reveals_on_appear = target.reveals_on_appear;
            self.graph.changes = Arc::new(changes);
            self.graph.entries = Arc::new(snapshot.entries);
            if let Some(ix) = appeared {
                self.pending_focus_target = None;
                if reveals_on_appear {
                    let revision = self.graph.changes[ix].selection_revision().to_owned();
                    self.pending_focus_reveal = Some(revision.into());
                }
                self.select_change(ix, cx);
            } else {
                // Selection is index-based, so a snapshot without the target has no valid row to keep; clear until it appears. Matching by id then re-selects the correct row, not a stale index.
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

        let pending_error = self.pending_error.take();
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
            // Keep the user's place in the file column across a background reload.
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
        if pending_error.is_some() {
            self.error = pending_error;
        }
        cx.notify();
    }

    /// Apply a batch of deferred `is_empty` corrections to the already-published rows. Merge and off-page rows are published as non-empty; these refine them once their parent-tree merge completes off the first-paint path.
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
}

/// One item flowing back from a refresh's background thread: the ancillary read (once, first), then the graph session's events, in that order.
enum RefreshUpdate {
    Ancillary(JayResult<AncillaryRefreshData>),
    Graph(LogGraphEvent),
}

struct AncillaryRefreshData {
    /// `None` when this refresh skipped the snapshot and so learned nothing about staleness.
    working_copy_stale: Option<bool>,
    bookmarks: Vec<BookmarkInfo>,
    tags: Vec<TagInfo>,
    vocabulary: RevsetVocabulary,
    workspaces: Option<Vec<WorkspaceInfo>>,
    pr_host_name: Option<String>,
    working_copy_stats: Option<DiffStats>,
    current_operation_description: String,
    fix_unavailable_reason: Option<String>,
}

/// Snapshots the working copy; `Ok(true)` means it is stale and was left unsnapshotted.
pub(in crate::repo::view_model) fn snapshot_reporting_stale(repo: &Repo) -> JayResult<bool> {
    match repo.refresh_working_copy() {
        Ok(()) => Ok(false),
        Err(JayError::WorkingCopyStale) => Ok(true),
        Err(error) => Err(error),
    }
}

fn refresh_ancillary_blocking(
    repo: &Repo,
    snapshot_working_copy: bool,
) -> JayResult<AncillaryRefreshData> {
    let working_copy_stale = snapshot_working_copy
        .then(|| snapshot_reporting_stale(repo))
        .transpose()?;
    let bookmarks = repo.list_bookmarks().unwrap_or_default();
    let tags = repo.list_tags().unwrap_or_default();
    let vocabulary = repo.revset_vocabulary(&bookmarks);
    let workspaces = repo.workspace_list().ok();
    let pr_host_name = repo.pr_host_name();
    let working_copy_stats = repo.diff_stats("@").ok();
    let current_operation_description = repo.current_operation_description();
    let fix_unavailable_reason = repo.fix_unavailable_reason();
    Ok(AncillaryRefreshData {
        working_copy_stale,
        bookmarks,
        tags,
        vocabulary,
        workspaces,
        pr_host_name,
        working_copy_stats,
        current_operation_description,
        fix_unavailable_reason,
    })
}
