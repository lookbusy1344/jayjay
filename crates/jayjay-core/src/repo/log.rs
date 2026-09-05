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
    GraphLoadToken, LogGraphEvent, LogGraphProgress, LogGraphRequest, LogGraphSnapshot,
    RequestGuard, SystemClock,
};
use super::resolve::{ChangeInfoContext, CommitRefIndex};
use super::support::{block_on, on_worker_stack};
use crate::dag::{DagLayout, MAX_CONTINUOUS_CONNECTOR_ROWS};
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
                let commit =
                    repo.store()
                        .get_commit(&commit_id)
                        .map_err(|e| CoreError::Internal {
                            message: format!("get commit: {e}"),
                        })?;
                if !self.should_include_in_log(repo, &commit) {
                    continue;
                }
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
        let expression = self.parse_revset_str(&repo, &request.revset)?;
        let revset_result = self.evaluate_typed_revset(&repo, expression.clone())?;
        let prioritized_ids = self.log_graph_prioritized_ids(&repo, &expression)?;

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
        let mut consumed: u64 = 0;
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
            let commit = repo
                .store()
                .get_commit(&commit_id)
                .map_err(|e| CoreError::Internal {
                    message: format!("get commit: {e}"),
                })?;
            consumed += 1;
            if !self.should_include_in_log(&repo, &commit) {
                continue;
            }
            raw_rows.push((commit, collapse_graph_edges(edge_list, root_commit_id)));

            if consumed.is_multiple_of(background_batch_rows) {
                if guard.is_canceled() {
                    on_event(LogGraphEvent::Canceled);
                    return Ok(());
                }
                on_event(LogGraphEvent::Progress(LogGraphProgress {
                    consumed_rows: consumed,
                    materialized_rows: u64::from(published_rows),
                    elapsed: guard.elapsed(),
                    first_result_budget_expired: guard.first_result_budget_expired(),
                }));
            }

            let available_to_publish =
                (raw_rows.len() as u32).saturating_sub(MAX_CONTINUOUS_CONNECTOR_ROWS as u32);
            if available_to_publish >= next_threshold {
                self.publish_log_graph_prefix(
                    &repo,
                    &raw_rows,
                    next_threshold,
                    false,
                    guard,
                    on_event,
                )?;
                published_rows = next_threshold;
                next_threshold = next_threshold.saturating_mul(2);
            }
        }

        if guard.is_canceled() {
            on_event(LogGraphEvent::Canceled);
            return Ok(());
        }

        let total_rows = raw_rows.len() as u32;
        if total_rows > published_rows || published_rows == 0 {
            self.publish_log_graph_prefix(&repo, &raw_rows, total_rows, true, guard, on_event)?;
        }

        on_event(LogGraphEvent::Finished);
        Ok(())
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
    ) -> CoreResult<()> {
        if guard.is_canceled() {
            return Ok(());
        }
        let window_len = if is_final {
            raw_rows.len()
        } else {
            (threshold as usize + MAX_CONTINUOUS_CONNECTOR_ROWS).min(raw_rows.len())
        };
        let window = &raw_rows[..window_len];
        let entries_with_lookahead = self.materialize_graph_entries(repo, window)?;
        if guard.is_canceled() {
            return Ok(());
        }
        let layout = DagLayout::compute(&entries_with_lookahead);
        let publish_len = (threshold as usize).min(entries_with_lookahead.len());
        let entries = entries_with_lookahead[..publish_len].to_vec();
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
        Ok(())
    }

    pub(crate) fn materialize_graph_entries(
        &self,
        repo: &Arc<ReadonlyRepo>,
        rows: &[GraphRowData],
    ) -> CoreResult<Vec<GraphEntry>> {
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
            let displayed_commit_ids = rows
                .iter()
                .map(|(commit, _)| commit.id().hex())
                .collect::<HashSet<_>>();
            let divergent_change_ids =
                Self::repository_divergent_change_ids(repo, rows.iter().map(|(commit, _)| commit))?;
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
            let empty_checks_started = Instant::now();
            let (empty_states, empty_check_count) = {
                let span = tracing::debug_span!("log_graph.empty_checks");
                let _entered = span.enter();
                let displayed_tree_ids = rows
                    .iter()
                    .map(|(commit, _)| (commit.id().clone(), commit.tree_ids()))
                    .collect::<HashMap<_, _>>();
                let cache = self.empty_commit_cache.read().unwrap();
                let mut computed = Vec::new();
                let states = rows
                    .iter()
                    .map(|(commit, _)| {
                        cache.get(commit.id()).copied().unwrap_or_else(|| {
                            let result = match commit.parent_ids() {
                                [parent_id] => displayed_tree_ids
                                    .get(parent_id)
                                    .map(|tree_ids| commit.tree_ids() == *tree_ids)
                                    .or_else(|| block_on(commit.is_empty(repo.as_ref())).ok()),
                                _ => block_on(commit.is_empty(repo.as_ref())).ok(),
                            };
                            if let Some(is_empty) = result {
                                computed.push((commit.id().clone(), is_empty));
                            }
                            result.unwrap_or(false)
                        })
                    })
                    .collect::<Vec<_>>();
                drop(cache);
                let empty_check_count = computed.len();
                bounded_extend(
                    &mut self.empty_commit_cache.write().unwrap(),
                    computed,
                    EMPTY_COMMIT_CACHE_MAX_ENTRIES,
                );
                (states, empty_check_count)
            };
            tracing::debug!(
                elapsed_us = empty_checks_started.elapsed().as_micros() as u64,
                "empty checks timing"
            );
            let materialization_started = Instant::now();
            let entries: Vec<GraphEntry> = {
                let span = tracing::debug_span!("log_graph.commit_materialization");
                let _entered = span.enter();
                rows.iter()
                    .zip(empty_states)
                    .map(|((commit, edges), is_empty)| GraphEntry {
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
                    })
                    .collect()
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

            Ok(entries)
        })
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
        repo: &Arc<ReadonlyRepo>,
        commits: impl Iterator<Item = &'a jj_lib::commit::Commit>,
    ) -> CoreResult<HashSet<String>> {
        commits
            .map(|commit| commit.change_id().clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .filter_map(
                |change_id| match block_on(repo.resolve_change_id(&change_id)) {
                    Ok(Some(targets)) if targets.is_divergent() => {
                        Some(Ok(encode_reverse_hex(change_id.as_bytes())))
                    }
                    Ok(_) => None,
                    Err(error) => Some(Err(error)),
                },
            )
            .collect::<Result<_, _>>()
            .map_err(|error| CoreError::Internal {
                message: format!("resolve change id: {error}"),
            })
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
