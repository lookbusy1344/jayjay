mod diff;
mod diff_compute;
mod review_notes;

use std::sync::Arc;
use std::time::Duration;

use gpui::{Context, SharedString};
use jayjay_core::{
    BookmarkInfo, ChangeInfo, CoreResult, DEFAULT_REVSET_DEPTH, DiffStats, GraphLoadToken,
    LogGraphEvent, LogGraphRequest, LogGraphSnapshot, MAX_AUTO_LOADED_ROWS, Repo, WorkspaceInfo,
    build_default_revset,
};

use super::RepoViewModel;
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

    pub fn handle_fs_event(&mut self, cx: &mut Context<Self>) {
        // Gate before the echo check: an event remembered here must survive even if a mutation stamps the echo window before the overlay closes.
        if self.refresh_suspended {
            self.loading.pending_auto_refresh = true;
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
        if !suspended && self.loading.pending_auto_refresh {
            self.loading.pending_auto_refresh = false;
            self.refresh(true, cx);
        }
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
        let selection = self
            .selected
            .and_then(|ix| self.graph.changes.get(ix))
            .map(|c| (c.change_id.id.clone(), c.commit_id.id.clone()));
        self.refresh_preferring(is_auto_triggered, selection, cx);
    }

    /// `selection` is (change id, commit id): the commit wins, the change id is the fallback once a rewrite retired that commit.
    pub(super) fn refresh_preferring(
        &mut self,
        is_auto_triggered: bool,
        selection: Option<(String, String)>,
        cx: &mut Context<Self>,
    ) {
        // FS event mid-refresh: defer it and re-run from the completion so the user's latest write isn't lost.
        if is_auto_triggered && self.loading.refreshing {
            self.loading.pending_auto_refresh = true;
            // An external change makes the streaming snapshot stale; cancel the active session so the
            // deferred refresh starts against fresh state instead of waiting for the whole stale
            // stream to drain. The Canceled terminal event runs the pending refresh.
            self.cancel_graph_session(cx);
            return;
        }
        let Some(repo) = self.repo.clone() else {
            return;
        };
        self.loading.pending_auto_refresh = false;
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
        let revset = self.revset.to_string();
        let previous_selection = selection;
        let token = GraphLoadToken::new();
        self.loading.graph_session = Some(token.clone());
        self.loading.graph_session_gen = Some(generation);
        self.loading.graph_session_canceling = false;
        self.loading.graph_first_snapshot_applied = false;
        self.loading.graph_paused = false;
        let row_ceiling = self.effective_row_ceiling();

        Self::background_stream(
            cx,
            move |tx| {
                let ancillary = refresh_ancillary_blocking(&repo);
                let is_err = ancillary.is_err();
                let _ = tx.unbounded_send(RefreshUpdate::Ancillary(ancillary));
                if is_err {
                    return;
                }
                let request = LogGraphRequest {
                    row_ceiling,
                    ..LogGraphRequest::new(revset)
                };
                repo.start_log_graph(request, token, |event| {
                    let _ = tx.unbounded_send(RefreshUpdate::Graph(event));
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

    /// Resume a session that paused at the row ceiling, doubling the ceiling so the next prefix
    /// loads more history. Preserves the current selection; a no-op when not paused.
    pub fn continue_loading(&mut self, cx: &mut Context<Self>) {
        if !self.loading.graph_paused {
            return;
        }
        self.loading.graph_row_ceiling = self.effective_row_ceiling().saturating_mul(2);
        self.refresh(false, cx);
    }

    /// Run an owed auto-refresh if one is pending and refreshes aren't suspended. Returns whether it
    /// ran; while suspended the pending flag stays set for `set_refresh_suspended` to honor.
    fn resume_pending_auto_refresh(&mut self, cx: &mut Context<Self>) -> bool {
        if self.loading.pending_auto_refresh && !self.refresh_suspended {
            self.loading.pending_auto_refresh = false;
            self.refresh(true, cx);
            return true;
        }
        false
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
            RefreshUpdate::Graph(LogGraphEvent::Progress(_)) => {}
            RefreshUpdate::Graph(LogGraphEvent::Paused) => {
                self.finish_graph_session(generation, cx);
                if self.loading.refresh_gen != generation {
                    return;
                }
                // An owed refresh supersedes the paused prefix; otherwise expose Continue Loading.
                if self.resume_pending_auto_refresh(cx) {
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
                if is_auto_triggered && self.refresh_suspended {
                    self.loading.pending_auto_refresh = true;
                    return;
                }
                if self.loading.pending_auto_refresh {
                    self.loading.pending_auto_refresh = false;
                    self.refresh(true, cx);
                }
            }
            RefreshUpdate::Graph(LogGraphEvent::Canceled) => {
                self.finish_graph_session(generation, cx);
                // A stale-session cancel from an FS event leaves a deferred refresh owed; run it now
                // against fresh state. A user-initiated cancel leaves no pending refresh, so this is
                // inert. While suspended, keep it owed for `set_refresh_suspended` to run later.
                if self.loading.refresh_gen == generation {
                    self.resume_pending_auto_refresh(cx);
                }
            }
            RefreshUpdate::Graph(LogGraphEvent::Failed(error)) => {
                self.finish_graph_session(generation, cx);
                if self.loading.refresh_gen == generation {
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
        self.finish_repo_task(cx);
        if self.loading.graph_session_gen == Some(generation) {
            self.loading.graph_session = None;
            self.loading.graph_session_gen = None;
            self.loading.graph_session_canceling = false;
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
        let is_first = !self.loading.graph_first_snapshot_applied;
        self.loading.graph_first_snapshot_applied = true;

        if snapshot.is_complete {
            self.can_load_more =
                self.revset_is_default() && snapshot.entries.len() >= self.revset_depth as usize;
        }
        self.graph.dag_layout = Arc::new(snapshot.layout);
        let changes: Vec<ChangeInfo> = snapshot.entries.iter().map(|e| e.change.clone()).collect();

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

    pub fn apply_revset(&mut self, revset: &str, cx: &mut Context<Self>) {
        let trimmed = revset.trim();
        let default_revset = build_default_revset(DEFAULT_REVSET_DEPTH);
        if trimmed.is_empty() || trimmed == default_revset {
            self.revset_depth = DEFAULT_REVSET_DEPTH;
            self.revset = default_revset.into();
        } else {
            self.revset = trimmed.to_owned().into();
        }
        self.can_load_more = false;
        // A new revset is a fresh query; drop any raised Continue Loading ceiling.
        self.loading.graph_row_ceiling = 0;
        self.refresh(false, cx);
    }

    pub(crate) fn revset_is_default(&self) -> bool {
        self.revset.as_ref() == build_default_revset(self.revset_depth)
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

fn refresh_ancillary_blocking(repo: &Repo) -> CoreResult<AncillaryRefreshData> {
    repo.refresh_working_copy()?;
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
