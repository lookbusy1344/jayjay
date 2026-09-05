use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use futures::StreamExt as _;
use jj_lib::backend::CommitId;
use jj_lib::config::ConfigGetResultExt as _;
use jj_lib::graph::TopoGroupedGraph;
use jj_lib::hex_util::encode_reverse_hex;
use jj_lib::object_id::ObjectId;
use jj_lib::repo::ReadonlyRepo;
use jj_lib::repo::Repo as _;
use jj_lib::revset::{self, SymbolResolver, UserRevsetExpression};

use super::Repo;
use super::graph_load::{
    BACKGROUND_LOG_BATCH_ROWS, EmptyStateUpdate, GraphLoadToken, LogGraphEvent, LogGraphProgress,
    LogGraphRequest, LogGraphSnapshot, RequestGuard, SystemClock,
};
use super::resolve::{ChangeInfoContext, CommitRefIndex};
use super::support::{block_on, on_worker_stack};
use crate::dag::{DagLayout, DagLayoutInput, MAX_CONTINUOUS_CONNECTOR_ROWS};
use crate::types::*;

pub(crate) struct ImmutableIds {
    pub(crate) commits: HashSet<String>,
    pub(crate) parents: HashSet<String>,
}

pub(crate) type GraphRowData = (jj_lib::commit::Commit, Vec<GraphEdge>);

/// Ceiling on retained empty-commit results. Repeated rewrites mint fresh commit ids, so the cache
/// would otherwise grow without bound across a long session.
const EMPTY_COMMIT_CACHE_MAX_ENTRIES: usize = 100_000;

/// Insert `computed` into `cache`, clearing it wholesale if it would exceed `max_entries`.
/// Emptiness is keyed by content-addressed commit id and never changes, so eviction only forces
/// later recomputation; wholesale clearing bounds growth without tracking per-entry recency.
fn bounded_extend(
    cache: &mut HashMap<CommitId, bool>,
    computed: Vec<(CommitId, bool)>,
    max_entries: usize,
) {
    if cache.len() + computed.len() > max_entries {
        cache.clear();
    }
    cache.extend(computed);
}

/// Computes `is_empty` for each `(index, commit)`, returning `(index, commit id, is_empty)` for those
/// that resolve. Emptiness of a merge commit needs a tree merge; those checks are independent and
/// CPU-bound and dominate a large `all()`, so the batch is split across the available cores. A commit
/// whose check errors is dropped (the caller defaults it to non-empty).
fn compute_empty_states(
    repo: &Arc<ReadonlyRepo>,
    to_compute: &[(usize, &jj_lib::commit::Commit)],
) -> Vec<(usize, CommitId, bool)> {
    if to_compute.is_empty() {
        return Vec::new();
    }
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(to_compute.len());
    let chunk_size = to_compute.len().div_ceil(threads);
    std::thread::scope(|scope| {
        let handles: Vec<_> = to_compute
            .chunks(chunk_size)
            .map(|chunk| {
                scope.spawn(move || {
                    chunk
                        .iter()
                        .filter_map(|(ix, commit)| {
                            block_on(commit.is_empty(repo.as_ref()))
                                .ok()
                                .map(|is_empty| (*ix, commit.id().clone(), is_empty))
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("empty-state worker panicked"))
            .collect()
    })
}

/// Collapse jj-lib's per-boundary `Missing` edges into a single one, matching `jj log`.
///
/// For a revset whose selected commits are disconnected from their parents, jj-lib
/// enumerates one missing edge per external boundary edge — hundreds for a deep history.
/// They all mean the same thing ("ancestry continues off-page"), so a node with one parent
/// keeps one termination stub instead of fanning into one lane per boundary commit.
fn collapse_graph_edges(
    edge_list: Vec<jj_lib::graph::GraphEdge<CommitId>>,
    root_commit_id: &CommitId,
) -> Vec<GraphEdge> {
    use jj_lib::graph::GraphEdgeType;
    let mut edges = Vec::with_capacity(edge_list.len());
    let mut missing_target = None;
    for edge in edge_list {
        let edge_type = match edge.edge_type {
            GraphEdgeType::Direct if &edge.target != root_commit_id => EdgeType::Direct,
            GraphEdgeType::Indirect if &edge.target != root_commit_id => EdgeType::Indirect,
            _ => {
                missing_target = Some(edge.target);
                continue;
            }
        };
        edges.push(GraphEdge {
            target: edge.target.hex(),
            edge_type,
        });
    }
    if let Some(target) = missing_target {
        edges.push(GraphEdge {
            target: target.hex(),
            edge_type: EdgeType::Missing,
        });
    }
    edges
}

impl Repo {
    pub fn log(&self, revset_str: &str) -> CoreResult<Vec<ChangeInfo>> {
        let repo = self.get_repo();
        let revset = self.evaluate_revset(&repo, revset_str)?;
        self.collect_changes(&repo, revset)
    }

    /// Same as `log`, but takes a pre-built typed revset expression (avoids string formatting).
    pub(crate) fn log_typed(
        &self,
        expression: Arc<UserRevsetExpression>,
    ) -> CoreResult<Vec<ChangeInfo>> {
        let repo = self.get_repo();
        let revset = self.evaluate_typed_revset(&repo, expression)?;
        self.collect_changes(&repo, revset)
    }

    fn collect_changes<'a>(
        &self,
        repo: &Arc<ReadonlyRepo>,
        revset: Box<dyn jj_lib::revset::Revset + 'a>,
    ) -> CoreResult<Vec<ChangeInfo>> {
        on_worker_stack(|| {
            let immutable_ids = self.immutable_ids(repo);
            let mut changes = Vec::new();
            let mut stream = revset.stream();
            while let Some(result) = block_on(stream.next()) {
                let commit_id = result.map_err(|e| CoreError::Internal {
                    message: format!("revset stream: {e}"),
                })?;
                let commit =
                    repo.store()
                        .get_commit(&commit_id)
                        .map_err(|e| CoreError::Internal {
                            message: format!("get commit: {e}"),
                        })?;
                if self.should_include_in_log(repo, &commit) {
                    changes.push(self.commit_to_change_info(
                        repo,
                        &commit,
                        ChangeInfoContext {
                            immutable_ids: Some(&immutable_ids),
                            ..ChangeInfoContext::default()
                        },
                    ));
                }
            }
            Self::mark_divergent(&mut changes);
            Ok(changes)
        })
    }

    /// Fully materializes `revset_str`. UI code must not call this directly on a large revset;
    /// use `start_log_graph` for a progressive session instead. Kept for tests and non-UI callers
    /// that genuinely need the complete result.
    pub fn log_graph(&self, revset_str: &str) -> CoreResult<Vec<GraphEntry>> {
        let repo = self.get_repo();
        let expression = self.parse_revset_str(&repo, revset_str)?;
        let rows = self.collect_graph_rows(&repo, &expression)?;
        self.materialize_graph_entries(&repo, &rows)
    }

    /// Streams `expression` through the same prioritized `TopoGroupedGraph` order `jj log` uses.
    pub(crate) fn collect_graph_rows(
        &self,
        repo: &Arc<ReadonlyRepo>,
        expression: &Arc<UserRevsetExpression>,
    ) -> CoreResult<Vec<GraphRowData>> {
        on_worker_stack(|| {
            let evaluation_started = Instant::now();
            let (revset_result, prioritized_ids) = {
                let span = tracing::debug_span!("log_graph.revset_evaluation");
                let _entered = span.enter();
                (
                    self.evaluate_typed_revset(repo, expression.clone())?,
                    self.log_graph_prioritized_ids(repo, expression)?,
                )
            };
            tracing::debug!(
                elapsed_us = evaluation_started.elapsed().as_micros() as u64,
                "revset evaluation timing"
            );

            let mut topo_order =
                TopoGroupedGraph::new(revset_result.stream_graph(), |id: &CommitId| id);
            for id in prioritized_ids {
                topo_order.prioritize_branch(id);
            }

            let mut rows = Vec::new();
            let root_commit_id = repo.store().root_commit_id();
            let mut stream = std::pin::pin!(topo_order.stream());
            let grouping_started = Instant::now();
            let grouping_span = tracing::debug_span!("log_graph.group_and_limit");
            let grouping_entered = grouping_span.enter();
            while let Some(result) = block_on(stream.next()) {
                let (commit_id, edge_list) = result.map_err(|e| CoreError::Internal {
                    message: format!("graph stream: {e}"),
                })?;
                // The graph stream includes jj's synthetic root. It is the only row this path
                // excludes, so avoid the broader display predicate and its ref lookups for every
                // ordinary commit.
                if &commit_id == root_commit_id {
                    continue;
                }
                let commit =
                    repo.store()
                        .get_commit(&commit_id)
                        .map_err(|e| CoreError::Internal {
                            message: format!("get commit: {e}"),
                        })?;
                rows.push((commit, collapse_graph_edges(edge_list, root_commit_id)));
            }
            drop(grouping_entered);
            tracing::debug!(
                elapsed_us = grouping_started.elapsed().as_micros() as u64,
                "grouping timing"
            );

            Ok(rows)
        })
    }

    /// Runs a progressive graph-load session for `request`, invoking `on_event` once per published
    /// snapshot and exactly once with a terminal event (`Finished`, `Canceled`, or `Failed`).
    ///
    /// Runs to completion on the calling thread; shells must call this from a background worker
    /// and marshal `on_event` invocations back to their UI thread themselves. Never call this while
    /// holding a lock `on_event` might need.
    pub fn start_log_graph(
        &self,
        request: LogGraphRequest,
        token: GraphLoadToken,
        mut on_event: impl FnMut(LogGraphEvent),
    ) {
        on_worker_stack(|| {
            let clock = SystemClock;
            token.initialize_row_ceiling(request.row_ceiling.max(1));
            let guard = RequestGuard::new(token, &clock, request.first_result_budget);
            if let Err(error) = self.run_log_graph_session(&request, &guard, &mut on_event) {
                on_event(LogGraphEvent::Failed(error));
            }
        })
    }

    fn run_log_graph_session(
        &self,
        request: &LogGraphRequest,
        guard: &RequestGuard<'_>,
        on_event: &mut impl FnMut(LogGraphEvent),
    ) -> CoreResult<()> {
        let repo = self.get_repo();
        let (revset_result, prioritized_ids) = {
            let span = tracing::debug_span!("log_graph.revset_evaluation");
            let _entered = span.enter();
            let expression = self.parse_revset_str(&repo, &request.revset)?;
            (
                self.evaluate_typed_revset(&repo, expression.clone())?,
                self.log_graph_prioritized_ids(&repo, &expression)?,
            )
        };

        let mut topo_order =
            TopoGroupedGraph::new(revset_result.stream_graph(), |id: &CommitId| id);
        for id in prioritized_ids {
            topo_order.prioritize_branch(id);
        }

        let root_commit_id = repo.store().root_commit_id();
        let mut stream = std::pin::pin!(topo_order.stream());

        let mut raw_rows: Vec<GraphRowData> = Vec::new();
        let mut published_rows: u32 = 0;
        let mut next_threshold: u32 = request.initial_rows.max(1);
        let mut row_ceiling = request.row_ceiling.max(1);
        let mut consumed: u64 = 0;
        let mut last_reported_consumed: u64 = 0;
        let background_batch_rows = u64::from(request.background_batch_rows.max(1));

        loop {
            if guard.is_canceled() {
                on_event(LogGraphEvent::Canceled);
                return Ok(());
            }
            let Some(result) = block_on(stream.next()) else {
                break;
            };
            let (commit_id, edge_list) = result.map_err(|e| CoreError::Internal {
                message: format!("graph stream: {e}"),
            })?;
            // `TopoGroupedGraph` yields the synthetic root too. Filter it before loading a
            // commit; `should_include_in_log()` is intentionally broader and would perform
            // working-copy/bookmark queries once for every real graph row.
            if &commit_id == root_commit_id {
                continue;
            }
            let commit = repo
                .store()
                .get_commit(&commit_id)
                .map_err(|e| CoreError::Internal {
                    message: format!("get commit: {e}"),
                })?;
            consumed += 1;
            raw_rows.push((commit, collapse_graph_edges(edge_list, root_commit_id)));

            if consumed.is_multiple_of(background_batch_rows) {
                if guard.is_canceled() {
                    on_event(LogGraphEvent::Canceled);
                    return Ok(());
                }
                report_graph_progress(consumed, published_rows, guard, on_event);
                last_reported_consumed = consumed;
            }

            let available_to_publish =
                (raw_rows.len() as u32).saturating_sub(MAX_CONTINUOUS_CONNECTOR_ROWS as u32);
            let publish_target = next_threshold.min(row_ceiling);
            let budget_target = raw_rows.len() as u32;
            let budget_requires_publish =
                guard.should_publish_first_result(published_rows, budget_target);
            if available_to_publish >= publish_target || budget_requires_publish {
                if last_reported_consumed != consumed {
                    report_graph_progress(consumed, published_rows, guard, on_event);
                    last_reported_consumed = consumed;
                }
                let publish_target = if budget_requires_publish {
                    budget_target.min(row_ceiling)
                } else {
                    publish_target
                };
                if !self.publish_log_graph_prefix(
                    &repo,
                    &raw_rows,
                    publish_target,
                    false,
                    guard,
                    on_event,
                )? {
                    on_event(LogGraphEvent::Canceled);
                    return Ok(());
                }
                published_rows = publish_target;
                // The ceiling is reached while the stream still has rows; pause for Continue Loading
                // instead of retaining an unbounded revset in memory.
                if published_rows >= row_ceiling {
                    match self.pause_at_row_ceiling(
                        &repo,
                        &raw_rows[..published_rows as usize],
                        row_ceiling,
                        guard,
                        on_event,
                    )? {
                        Some(higher_ceiling) => row_ceiling = higher_ceiling,
                        None => {
                            on_event(LogGraphEvent::Canceled);
                            return Ok(());
                        }
                    }
                }
                next_threshold = next_threshold.max(published_rows).saturating_mul(2);
            }
        }

        self.finish_log_graph_session(
            &repo,
            &raw_rows,
            published_rows,
            consumed,
            last_reported_consumed,
            guard,
            on_event,
        )
    }

    /// Publish any rows the loop consumed past the last published prefix, refine the whole result's
    /// empty flags, and emit the terminal event. Emits `Canceled` instead of `Finished` if the
    /// session was canceled at any of the cooperative checks.
    #[allow(clippy::too_many_arguments)]
    fn finish_log_graph_session(
        &self,
        repo: &Arc<ReadonlyRepo>,
        raw_rows: &[GraphRowData],
        published_rows: u32,
        consumed: u64,
        last_reported_consumed: u64,
        guard: &RequestGuard<'_>,
        on_event: &mut impl FnMut(LogGraphEvent),
    ) -> CoreResult<()> {
        if guard.is_canceled() {
            on_event(LogGraphEvent::Canceled);
            return Ok(());
        }
        let total_rows = raw_rows.len() as u32;
        if last_reported_consumed != consumed {
            report_graph_progress(consumed, published_rows, guard, on_event);
        }
        if (total_rows > published_rows || published_rows == 0)
            && !self.publish_log_graph_prefix(repo, raw_rows, total_rows, true, guard, on_event)?
        {
            on_event(LogGraphEvent::Canceled);
            return Ok(());
        }
        if guard.is_canceled() {
            on_event(LogGraphEvent::Canceled);
            return Ok(());
        }
        if !self.refine_empty_states(repo, &raw_rows[..total_rows as usize], guard, on_event)? {
            on_event(LogGraphEvent::Canceled);
            return Ok(());
        }
        on_event(LogGraphEvent::Finished);
        Ok(())
    }

    /// Announce the pause at the retained-row ceiling, refine the visible prefix's empty flags while
    /// the worker would otherwise idle, then block until Continue Loading raises the ceiling. Returns
    /// the raised ceiling, or `None` if the session was canceled while paused or refining.
    fn pause_at_row_ceiling(
        &self,
        repo: &Arc<ReadonlyRepo>,
        prefix: &[GraphRowData],
        current_ceiling: u32,
        guard: &RequestGuard<'_>,
        on_event: &mut impl FnMut(LogGraphEvent),
    ) -> CoreResult<Option<u32>> {
        if guard.is_canceled() {
            return Ok(None);
        }
        on_event(LogGraphEvent::Paused);
        if !self.refine_empty_states(repo, prefix, guard, on_event)? {
            return Ok(None);
        }
        Ok(guard.wait_for_higher_row_ceiling(current_ceiling))
    }

    /// Materializes `raw_rows[..threshold]` (plus look-ahead for layout) and publishes it as one
    /// snapshot. Look-ahead rows are used only to stabilize connector projection at the prefix
    /// boundary; only the first `threshold` rows are published.
    fn publish_log_graph_prefix(
        &self,
        repo: &Arc<ReadonlyRepo>,
        raw_rows: &[GraphRowData],
        threshold: u32,
        is_final: bool,
        guard: &RequestGuard<'_>,
        on_event: &mut impl FnMut(LogGraphEvent),
    ) -> CoreResult<bool> {
        if guard.is_canceled() {
            return Ok(false);
        }
        let window_len = if is_final {
            raw_rows.len()
        } else {
            (threshold as usize + MAX_CONTINUOUS_CONNECTOR_ROWS).min(raw_rows.len())
        };
        let publish_len = (threshold as usize).min(window_len);
        let Some(entries) = self.materialize_graph_entries_guarded(
            repo,
            &raw_rows[..publish_len],
            Some(guard),
            true,
        )?
        else {
            return Ok(false);
        };
        if guard.is_canceled() {
            return Ok(false);
        }
        let mut layout_inputs = entries.iter().map(DagLayoutInput::from).collect::<Vec<_>>();
        let lookahead = &raw_rows[publish_len..window_len];
        if !lookahead.is_empty() {
            let lookahead_ids = lookahead
                .iter()
                .map(|(commit, _)| commit.id().hex())
                .collect::<HashSet<_>>();
            let refs = CommitRefIndex::build(repo, self.workspace_name.as_ref(), &lookahead_ids);
            layout_inputs.extend(lookahead.iter().map(|(commit, edges)| {
                let commit_id = commit.id().hex();
                DagLayoutInput {
                    parents: commit.parent_ids().iter().map(|id| id.hex()).collect(),
                    is_working_copy: refs.is_working_copy(&commit_id),
                    has_ref: refs.has_layout_ref(&commit_id),
                    commit_id,
                    edges: edges.clone(),
                }
            }));
        }
        let layout = DagLayout::compute_inputs(&layout_inputs);
        if guard.is_canceled() {
            return Ok(false);
        }
        let rows = layout.rows[..publish_len].to_vec();
        on_event(LogGraphEvent::Snapshot(LogGraphSnapshot {
            entries,
            layout: DagLayout {
                rows,
                logical_column_count: layout.logical_column_count,
            },
            loaded_rows: publish_len as u32,
            is_complete: is_final,
        }));
        Ok(true)
    }

    pub(crate) fn materialize_graph_entries(
        &self,
        repo: &Arc<ReadonlyRepo>,
        rows: &[GraphRowData],
    ) -> CoreResult<Vec<GraphEntry>> {
        self.materialize_graph_entries_guarded(repo, rows, None, false)
            .map(|entries| entries.expect("an unguarded materialization cannot be canceled"))
    }

    /// `defer_empty` resolves only the cheap `is_empty` states (cache hits and single-in-page-parent
    /// rows) and leaves merge/off-page rows as non-empty for a later `refine_empty_states` pass; the
    /// non-progressive path passes `false` to compute every state eagerly.
    fn materialize_graph_entries_guarded(
        &self,
        repo: &Arc<ReadonlyRepo>,
        rows: &[GraphRowData],
        guard: Option<&RequestGuard<'_>>,
        defer_empty: bool,
    ) -> CoreResult<Option<Vec<GraphEntry>>> {
        on_worker_stack(|| {
            let metadata_started = Instant::now();
            let metadata_span = tracing::debug_span!("log_graph.metadata", rows = rows.len());
            let metadata_entered = metadata_span.enter();
            let immutability_started = Instant::now();
            let immutable_ids = {
                let span = tracing::debug_span!("log_graph.immutability_membership");
                let _entered = span.enter();
                self.bounded_immutable_ids(repo, rows.iter().map(|(commit, _)| commit))?
            };
            tracing::debug!(
                elapsed_us = immutability_started.elapsed().as_micros() as u64,
                "immutability timing"
            );
            if guard.is_some_and(RequestGuard::is_canceled) {
                return Ok(None);
            }
            let displayed_commit_ids = rows
                .iter()
                .map(|(commit, _)| commit.id().hex())
                .collect::<HashSet<_>>();
            let divergent_change_ids = {
                let span = tracing::debug_span!("log_graph.divergence");
                let _entered = span.enter();
                self.repository_divergent_change_ids(repo, rows.iter().map(|(commit, _)| commit))?
            };
            if guard.is_some_and(RequestGuard::is_canceled) {
                return Ok(None);
            }
            let ref_index_started = Instant::now();
            let ref_index = {
                let span = tracing::debug_span!("log_graph.ref_index");
                let _entered = span.enter();
                CommitRefIndex::build(repo, self.workspace_name.as_ref(), &displayed_commit_ids)
            };
            tracing::debug!(
                elapsed_us = ref_index_started.elapsed().as_micros() as u64,
                "ref index timing"
            );
            if guard.is_some_and(RequestGuard::is_canceled) {
                return Ok(None);
            }
            let empty_checks_started = Instant::now();
            let (empty_states, empty_check_count) = if defer_empty {
                let (states, pending) = self.classify_empty_states(rows);
                (states, pending.len())
            } else {
                let Some(resolved) = self.empty_states_guarded(repo, rows, guard)? else {
                    return Ok(None);
                };
                resolved
            };
            tracing::debug!(
                elapsed_us = empty_checks_started.elapsed().as_micros() as u64,
                "empty checks timing"
            );
            let materialization_started = Instant::now();
            let entries: Vec<GraphEntry> = {
                let span = tracing::debug_span!("log_graph.commit_materialization");
                let _entered = span.enter();
                let mut entries = Vec::with_capacity(rows.len());
                for start in (0..rows.len()).step_by(BACKGROUND_LOG_BATCH_ROWS as usize) {
                    if guard.is_some_and(RequestGuard::is_canceled) {
                        return Ok(None);
                    }
                    let end = (start + BACKGROUND_LOG_BATCH_ROWS as usize).min(rows.len());
                    entries.extend(rows[start..end].iter().zip(&empty_states[start..end]).map(
                        |((commit, edges), &is_empty)| GraphEntry {
                            change: self.commit_to_change_info(
                                repo,
                                commit,
                                ChangeInfoContext {
                                    immutable_ids: Some(&immutable_ids),
                                    ref_index: Some(&ref_index),
                                    divergent_change_ids: Some(&divergent_change_ids),
                                    is_empty: Some(is_empty),
                                },
                            ),
                            edges: edges.clone(),
                        },
                    ));
                }
                entries
            };
            tracing::debug!(
                elapsed_us = materialization_started.elapsed().as_micros() as u64,
                "commit materialization timing"
            );
            drop(metadata_entered);
            tracing::debug!(
                elapsed_us = metadata_started.elapsed().as_micros() as u64,
                "metadata timing"
            );
            tracing::debug!(
                rows_materialized = entries.len(),
                immutable_ids_enumerated = immutable_ids.commits.len(),
                immutable_parent_ids_enumerated = immutable_ids.parents.len(),
                empty_checks = empty_check_count,
                "log graph work counters"
            );

            Ok(Some(entries))
        })
    }

    /// Resolve `is_empty` for the rows decidable without a parent-tree merge: cache hits and
    /// single-parent rows whose parent is on the page. Returns the states (non-empty where still
    /// undecided) and the indices of merge/off-page rows that need the expensive check. Caches the
    /// cheaply resolved results.
    fn classify_empty_states(&self, rows: &[GraphRowData]) -> (Vec<bool>, Vec<usize>) {
        let displayed_tree_ids = rows
            .iter()
            .map(|(commit, _)| (commit.id().clone(), commit.tree_ids()))
            .collect::<HashMap<_, _>>();
        let mut states = vec![false; rows.len()];
        let mut newly = Vec::new();
        let mut pending = Vec::new();
        {
            let cache = self.empty_commit_cache.read().unwrap();
            for (ix, (commit, _)) in rows.iter().enumerate() {
                if let Some(&cached) = cache.get(commit.id()) {
                    states[ix] = cached;
                } else if let [parent_id] = commit.parent_ids()
                    && let Some(parent_tree_ids) = displayed_tree_ids.get(parent_id)
                {
                    let is_empty = commit.tree_ids() == *parent_tree_ids;
                    states[ix] = is_empty;
                    newly.push((commit.id().clone(), is_empty));
                } else {
                    pending.push(ix);
                }
            }
        }
        bounded_extend(
            &mut self.empty_commit_cache.write().unwrap(),
            newly,
            EMPTY_COMMIT_CACHE_MAX_ENTRIES,
        );
        (states, pending)
    }

    /// Run the parent-tree-merge `is_empty` check over `pending` rows in cancelable batches, caching
    /// every result and handing each batch's resolved `(index, commit id, is_empty)` triples to
    /// `sink`. Returns `false` if `is_canceled` fired before a batch. Callers decide what to do with
    /// each batch: fill a states vector, or emit corrections.
    fn resolve_pending_empty_states(
        &self,
        repo: &Arc<ReadonlyRepo>,
        rows: &[GraphRowData],
        pending: &[usize],
        is_canceled: impl Fn() -> bool,
        mut sink: impl FnMut(&[(usize, CommitId, bool)]),
    ) -> bool {
        for batch_ixs in pending.chunks(BACKGROUND_LOG_BATCH_ROWS as usize) {
            if is_canceled() {
                return false;
            }
            let batch: Vec<(usize, &jj_lib::commit::Commit)> =
                batch_ixs.iter().map(|&ix| (ix, &rows[ix].0)).collect();
            let resolved = compute_empty_states(repo, &batch);
            bounded_extend(
                &mut self.empty_commit_cache.write().unwrap(),
                resolved
                    .iter()
                    .map(|(_, id, is_empty)| (id.clone(), *is_empty))
                    .collect(),
                EMPTY_COMMIT_CACHE_MAX_ENTRIES,
            );
            sink(&resolved);
        }
        true
    }

    fn empty_states_guarded(
        &self,
        repo: &Arc<ReadonlyRepo>,
        rows: &[GraphRowData],
        guard: Option<&RequestGuard<'_>>,
    ) -> CoreResult<Option<(Vec<bool>, usize)>> {
        let span = tracing::debug_span!("log_graph.empty_checks");
        let _entered = span.enter();
        let (mut states, pending) = self.classify_empty_states(rows);
        let empty_check_count = pending.len();
        let completed = self.resolve_pending_empty_states(
            repo,
            rows,
            &pending,
            || guard.is_some_and(RequestGuard::is_canceled),
            |resolved| {
                for &(ix, _, is_empty) in resolved {
                    states[ix] = is_empty;
                }
            },
        );
        if !completed {
            return Ok(None);
        }
        Ok(Some((states, empty_check_count)))
    }

    /// Compute the deferred merge/off-page `is_empty` states for an already-published prefix and
    /// emit each batch's empty rows as an `EmptyStates` correction. Cheap rows were resolved at
    /// publish time and cached rows (including any refined in an earlier prefix) are skipped, so this
    /// only pays for rows it has not seen. Returns `false` if canceled mid-pass.
    fn refine_empty_states(
        &self,
        repo: &Arc<ReadonlyRepo>,
        rows: &[GraphRowData],
        guard: &RequestGuard<'_>,
        on_event: &mut impl FnMut(LogGraphEvent),
    ) -> CoreResult<bool> {
        let span = tracing::debug_span!("log_graph.empty_refine");
        let _entered = span.enter();
        let (_states, pending) = self.classify_empty_states(rows);
        Ok(self.resolve_pending_empty_states(
            repo,
            rows,
            &pending,
            || guard.is_canceled(),
            |resolved| {
                let updates: Vec<EmptyStateUpdate> = resolved
                    .iter()
                    .filter(|(_, _, is_empty)| *is_empty)
                    .map(|(_, id, _)| EmptyStateUpdate {
                        commit_id: id.hex(),
                        is_empty: true,
                    })
                    .collect();
                if !updates.is_empty() {
                    on_event(LogGraphEvent::EmptyStates(updates));
                }
            },
        ))
    }

    /// Refuse to rewrite `commit` (resolved from `rev`) when it is immutable, using the same `immutable()` revset that drives `ChangeInfo::is_immutable`; rewrite paths that bypass the jj CLI get no immutability enforcement from jj-lib and must call this themselves.
    pub(crate) fn ensure_commit_mutable(
        &self,
        repo: &Arc<ReadonlyRepo>,
        commit: &jj_lib::commit::Commit,
        rev: &str,
    ) -> CoreResult<()> {
        if self.is_commit_immutable(repo, commit)? {
            return Err(CoreError::Internal {
                message: format!("{rev} is immutable and cannot be rewritten"),
            });
        }
        Ok(())
    }

    pub(crate) fn is_commit_immutable(
        &self,
        repo: &Arc<ReadonlyRepo>,
        commit: &jj_lib::commit::Commit,
    ) -> CoreResult<bool> {
        self.revset_contains(repo, "immutable()", commit)
    }

    pub(crate) fn has_immutable_child(
        &self,
        repo: &Arc<ReadonlyRepo>,
        commit: &jj_lib::commit::Commit,
    ) -> CoreResult<bool> {
        self.revset_contains(repo, "parents(immutable())", commit)
    }

    fn revset_contains(
        &self,
        repo: &Arc<ReadonlyRepo>,
        revset_str: &str,
        commit: &jj_lib::commit::Commit,
    ) -> CoreResult<bool> {
        let revset = self.evaluate_revset(repo, revset_str)?;
        block_on(revset.containing_fn()(commit.id())).map_err(|e| CoreError::Internal {
            message: format!("{revset_str} check: {e}"),
        })
    }

    fn immutable_ids(&self, repo: &Arc<ReadonlyRepo>) -> ImmutableIds {
        ImmutableIds {
            commits: self.revset_commit_ids(repo, "immutable()"),
            parents: self.revset_commit_ids(repo, "parents(immutable())"),
        }
    }

    /// Immutability membership for exactly `commits`, not the whole repository: intersects `immutable()`
    /// and `parents(immutable())` with an explicit set built from `commits` before evaluating, so the
    /// walk stays bounded by the page size instead of enumerating every immutable commit in the repo
    /// (which can be hundreds of thousands on a large checkout).
    fn bounded_immutable_ids<'a>(
        &self,
        repo: &Arc<ReadonlyRepo>,
        commits: impl Iterator<Item = &'a jj_lib::commit::Commit>,
    ) -> CoreResult<ImmutableIds> {
        let commit_ids: Vec<CommitId> = commits.map(|commit| commit.id().clone()).collect();
        if commit_ids.is_empty() {
            return Ok(ImmutableIds {
                commits: HashSet::new(),
                parents: HashSet::new(),
            });
        }
        let displayed = UserRevsetExpression::commits(commit_ids);
        let immutable = self.parse_revset_str(repo, "immutable()")?;
        let parents_of_immutable = self.parse_revset_str(repo, "parents(immutable())")?;
        Ok(ImmutableIds {
            commits: self.typed_revset_commit_ids(repo, immutable.intersection(&displayed)),
            parents: self
                .typed_revset_commit_ids(repo, parents_of_immutable.intersection(&displayed)),
        })
    }

    /// Evaluate `expression` once and return its commit ID hex strings; an invalid revset yields an empty set so display loading stays resilient.
    fn typed_revset_commit_ids(
        &self,
        repo: &Arc<ReadonlyRepo>,
        expression: Arc<UserRevsetExpression>,
    ) -> HashSet<String> {
        let Ok(result) = self.evaluate_typed_revset(repo, expression) else {
            return HashSet::new();
        };
        let mut stream = result.stream();
        let mut ids = HashSet::new();
        while let Some(result) = block_on(stream.next()) {
            if let Ok(id) = result {
                ids.insert(id.hex());
            }
        }
        ids
    }

    /// Evaluate `revset_str` once and return its commit ID hex strings; an invalid revset yields an empty set so display loading stays resilient.
    fn revset_commit_ids(&self, repo: &Arc<ReadonlyRepo>, revset_str: &str) -> HashSet<String> {
        let Ok(result) = self.evaluate_revset(repo, revset_str) else {
            return HashSet::new();
        };
        on_worker_stack(|| {
            let mut stream = result.stream();
            let mut ids = HashSet::new();
            while let Some(result) = block_on(stream.next()) {
                if let Ok(id) = result {
                    ids.insert(id.hex());
                }
            }
            ids
        })
    }

    /// Find change IDs that appear more than once in the given changes.
    fn find_divergent_ids<'a>(changes: impl Iterator<Item = &'a ChangeInfo>) -> HashSet<String> {
        let mut counts: HashMap<&str, u32> = HashMap::new();
        let mut all_ids: Vec<&str> = Vec::new();
        for change in changes {
            *counts.entry(&change.change_id.id).or_insert(0) += 1;
            all_ids.push(&change.change_id.id);
        }
        counts
            .into_iter()
            .filter(|(_, count)| *count > 1)
            .map(|(id, _)| id.to_owned())
            .collect()
    }

    fn repository_divergent_change_ids<'a>(
        &self,
        repo: &Arc<ReadonlyRepo>,
        commits: impl Iterator<Item = &'a jj_lib::commit::Commit>,
    ) -> CoreResult<HashSet<String>> {
        let displayed_commit_ids = commits
            .map(|commit| commit.id().clone())
            .collect::<Vec<_>>();
        if displayed_commit_ids.is_empty() {
            return Ok(HashSet::new());
        }

        let divergent = self.parse_revset_str(repo, "divergent()")?;
        let displayed = UserRevsetExpression::commits(displayed_commit_ids);
        let revset = self.evaluate_typed_revset(repo, divergent.intersection(&displayed))?;
        let mut stream = revset.stream();
        let mut change_ids = HashSet::new();
        while let Some(result) = block_on(stream.next()) {
            let commit_id = result.map_err(|error| CoreError::Internal {
                message: format!("divergent revset stream: {error}"),
            })?;
            let commit =
                repo.store()
                    .get_commit(&commit_id)
                    .map_err(|error| CoreError::Internal {
                        message: format!("get divergent commit: {error}"),
                    })?;
            change_ids.insert(encode_reverse_hex(commit.change_id().as_bytes()));
        }
        Ok(change_ids)
    }

    /// Mark changes with duplicate change IDs as divergent.
    fn mark_divergent(changes: &mut [ChangeInfo]) {
        let divergent = Self::find_divergent_ids(changes.iter());
        for change in changes {
            if divergent.contains(&change.change_id.id) {
                change.is_divergent = true;
            }
        }
    }

    pub(crate) fn is_change_id_divergent(
        &self,
        repo: &Arc<ReadonlyRepo>,
        change_id: &str,
    ) -> CoreResult<bool> {
        let revset = self.evaluate_revset(repo, &format!("change_id({change_id})"))?;
        on_worker_stack(|| {
            let mut count = 0;
            let mut stream = revset.stream();
            while let Some(result) = block_on(stream.next()) {
                result.map_err(|e| CoreError::Internal {
                    message: format!("revset stream: {e}"),
                })?;
                count += 1;
                if count > 1 {
                    return Ok(true);
                }
            }
            Ok(false)
        })
    }

    /// Number of commits matching `expr`. Returns 0 if the revset can't evaluate.
    pub(crate) fn count_revset(&self, repo: &Arc<ReadonlyRepo>, expr: &str) -> u32 {
        let Ok(revset) = self.evaluate_revset(repo, expr) else {
            return 0;
        };
        on_worker_stack(|| {
            let mut count = 0u32;
            let mut stream = revset.stream();
            while let Some(result) = block_on(stream.next()) {
                if result.is_err() {
                    break;
                }
                count = count.saturating_add(1);
            }
            count
        })
    }

    pub(crate) fn evaluate_typed_revset<'a>(
        &self,
        repo: &'a Arc<ReadonlyRepo>,
        expression: Arc<UserRevsetExpression>,
    ) -> CoreResult<Box<dyn jj_lib::revset::Revset + 'a>> {
        #[allow(clippy::borrowed_box)]
        let empty_extensions: &[&Box<dyn revset::SymbolResolverExtension>] = &[];
        let symbol_resolver = SymbolResolver::new(repo.as_ref(), empty_extensions);
        let resolved = expression
            .resolve_user_expression(repo.as_ref(), &symbol_resolver)
            .map_err(|e| CoreError::Internal {
                message: format!("resolve revset: {e}"),
            })?;
        resolved
            .evaluate(repo.as_ref())
            .map_err(|e| CoreError::Internal {
                message: format!("eval revset: {e}"),
            })
    }

    fn evaluate_revset<'a>(
        &self,
        repo: &'a Arc<ReadonlyRepo>,
        revset_str: &str,
    ) -> CoreResult<Box<dyn jj_lib::revset::Revset + 'a>> {
        let expression = self.parse_revset_str(repo, revset_str)?;
        self.evaluate_typed_revset(repo, expression)
    }

    fn parse_revset_str(
        &self,
        repo: &Arc<ReadonlyRepo>,
        revset_str: &str,
    ) -> CoreResult<Arc<UserRevsetExpression>> {
        let settings = repo.settings();
        let aliases_map = self.revset_aliases_map(settings)?;
        let fileset_aliases_map = self.fileset_aliases_map(settings)?;
        self.parse_revset(
            &aliases_map,
            &fileset_aliases_map,
            settings.user_email(),
            revset_str,
        )
        .map_err(|e| CoreError::Internal {
            message: format!("parse revset: {e}"),
        })
    }

    /// Commit IDs matching `revsets.log-graph-prioritize`, intersected with `expression`, in the order the config revset yields them. Empty when the config key is unset.
    fn log_graph_prioritized_ids(
        &self,
        repo: &Arc<ReadonlyRepo>,
        expression: &Arc<UserRevsetExpression>,
    ) -> CoreResult<Vec<CommitId>> {
        let prioritize_revset_str = repo
            .settings()
            .get_string(["revsets", "log-graph-prioritize"])
            .optional()
            .map_err(|e| CoreError::Internal {
                message: format!("read revsets.log-graph-prioritize: {e}"),
            })?
            .unwrap_or_default();
        if prioritize_revset_str.trim().is_empty() {
            return Ok(Vec::new());
        }

        let prioritize_expression = self.parse_revset_str(repo, &prioritize_revset_str)?;
        let intersected = prioritize_expression.intersection(expression);
        let revset = self.evaluate_typed_revset(repo, intersected)?;

        let mut ids = Vec::new();
        let mut stream = revset.stream();
        while let Some(result) = block_on(stream.next()) {
            ids.push(result.map_err(|e| CoreError::Internal {
                message: format!("revsets.log-graph-prioritize stream: {e}"),
            })?);
        }
        Ok(ids)
    }
}

fn report_graph_progress(
    consumed_rows: u64,
    materialized_rows: u32,
    guard: &RequestGuard<'_>,
    on_event: &mut impl FnMut(LogGraphEvent),
) {
    on_event(LogGraphEvent::Progress(LogGraphProgress {
        consumed_rows,
        materialized_rows: u64::from(materialized_rows),
        elapsed: guard.elapsed(),
        first_result_budget_expired: guard.first_result_budget_expired(),
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use jj_lib::graph::GraphEdge as JjEdge;

    const ROOT: &str = "0000000000000000000000000000000000000000";

    fn cid(hex: &'static str) -> CommitId {
        CommitId::from_hex(hex)
    }

    #[test]
    fn bounded_extend_clears_before_exceeding_the_cap() {
        let mut cache = HashMap::new();
        cache.insert(CommitId::new(vec![1; 20]), true);
        cache.insert(CommitId::new(vec![2; 20]), false);

        // Adding a third entry with cap 2 clears first, then inserts only the new batch.
        bounded_extend(&mut cache, vec![(CommitId::new(vec![3; 20]), true)], 2);

        assert_eq!(cache.len(), 1);
        assert_eq!(cache.get(&CommitId::new(vec![3; 20])), Some(&true));
    }

    #[test]
    fn bounded_extend_keeps_entries_while_under_the_cap() {
        let mut cache = HashMap::new();
        cache.insert(CommitId::new(vec![1; 20]), true);

        bounded_extend(&mut cache, vec![(CommitId::new(vec![2; 20]), false)], 100);

        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn many_missing_edges_collapse_to_one_keeping_direct_and_indirect() {
        let root = cid(ROOT);
        let a = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let b = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let c = "cccccccccccccccccccccccccccccccccccccccc";
        let d = "dddddddddddddddddddddddddddddddddddddddd";
        let edges = vec![
            JjEdge::indirect(cid(a)),
            JjEdge::missing(cid(b)),
            JjEdge::missing(cid(c)),
            JjEdge::direct(cid(d)),
        ];

        let collapsed = collapse_graph_edges(edges, &root);

        assert_eq!(
            collapsed,
            vec![
                GraphEdge {
                    target: a.to_owned(),
                    edge_type: EdgeType::Indirect
                },
                GraphEdge {
                    target: d.to_owned(),
                    edge_type: EdgeType::Direct
                },
                GraphEdge {
                    target: c.to_owned(),
                    edge_type: EdgeType::Missing
                },
            ]
        );
    }

    #[test]
    fn a_single_parent_off_page_node_keeps_one_termination() {
        let root = cid(ROOT);
        let edges = (0u8..50)
            .map(|i| JjEdge::missing(CommitId::new(vec![i; 20])))
            .collect();

        let collapsed = collapse_graph_edges(edges, &root);

        assert_eq!(collapsed.len(), 1);
        assert_eq!(collapsed[0].edge_type, EdgeType::Missing);
    }

    #[test]
    fn root_targeted_edges_are_treated_as_missing() {
        let root = cid(ROOT);
        let collapsed = collapse_graph_edges(vec![JjEdge::direct(root.clone())], &root);

        assert_eq!(collapsed.len(), 1);
        assert_eq!(collapsed[0].edge_type, EdgeType::Missing);
    }
}
