use super::loaders::snapshot_reporting_stale;
use super::{DiffCache, GraphData, LoadingState, NotesState, RepoViewModel, ShownDiff, StatsState};
use crate::app::config;
use crate::diff::{DetailMode, DiffViewMode};
use gpui::{AppContext, Context, SharedString};
use jayjay_core::dag::{DagLayout, OrderedSelection};
use jayjay_core::{
    BookmarkInfo, ChangeInfo, DEFAULT_REVSET_DEPTH, GraphEntry, Repo, RevsetFilterState,
    RevsetVocabulary, TagInfo, WorkspaceInfo, build_default_revset, default_revset_depth,
};
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

struct OpenedRepo {
    repo: Arc<Repo>,
    working_copy_stale: bool,
    repo_root_path: String,
    entries: Vec<GraphEntry>,
    bookmarks: Vec<BookmarkInfo>,
    tags: Vec<TagInfo>,
    vocabulary: RevsetVocabulary,
    workspaces: Vec<WorkspaceInfo>,
    pr_host_name: Option<String>,
    fix_unavailable_reason: Option<String>,
}

impl RepoViewModel {
    pub fn new(path: PathBuf) -> Self {
        let repo_path: SharedString = path.display().to_string().into();
        let revset = build_default_revset(DEFAULT_REVSET_DEPTH);
        match Self::open_blocking_with_graph(path, &revset) {
            Ok(loaded) => Self::ready(repo_path, RevsetFilterState::new(&revset), loaded),
            Err(e) => Self::error(repo_path, format!("{e}")),
        }
    }

    /// Pair with [`RepoViewModel::open_async`], which opens the repository off the main thread and then starts the progressive graph session.
    pub fn opening(path: PathBuf) -> Self {
        Self::empty(path.display().to_string().into())
    }

    /// Keeps window-open off the UI thread, since open/revset eval is slow on large checkouts.
    pub fn open_async(&mut self, cx: &mut Context<Self>) {
        let path = PathBuf::from(self.repo_path.as_ref());
        let ready_revset = self.revset_filter.clone();
        self.begin_refreshing(cx);
        Self::background_update(
            cx,
            async move { Self::open_blocking(path) },
            move |vm, opened, cx| {
                vm.finish_repo_task(cx);
                match opened {
                    Ok(loaded) => {
                        *vm = Self::ready(vm.repo_path.clone(), ready_revset, loaded);
                        vm.finish_open(cx);
                    }
                    Err(e) => vm.present_error(e),
                }
                cx.notify();
            },
        );
    }

    pub(crate) async fn open_detached(
        path: PathBuf,
        cx: &mut gpui::AsyncApp,
    ) -> jayjay_core::JayResult<gpui::Entity<Self>> {
        let repo_path: SharedString = path.display().to_string().into();
        let revset = build_default_revset(DEFAULT_REVSET_DEPTH);
        let loaded = {
            let revset = revset.clone();
            cx.background_spawn(async move { Self::open_blocking_with_graph(path, &revset) })
                .await?
        };
        Ok(cx.new(|_| Self::ready(repo_path, RevsetFilterState::new(&revset), loaded)))
    }

    pub(crate) fn finish_open(&mut self, cx: &mut Context<Self>) {
        config::update(cx, |config| {
            config.record_opened_repo(Path::new(self.repo_path.as_ref()));
        });
        self.boot(cx);
    }

    fn open_blocking(path: PathBuf) -> jayjay_core::JayResult<OpenedRepo> {
        let repo_root_path = jayjay_core::workspace_primary_root(&path.to_string_lossy())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let repo = Repo::open(&path)?;
        Ok(OpenedRepo {
            repo: Arc::new(repo),
            working_copy_stale: false,
            repo_root_path,
            entries: Vec::new(),
            bookmarks: Vec::new(),
            tags: Vec::new(),
            vocabulary: RevsetVocabulary::default(),
            workspaces: Vec::new(),
            pr_host_name: None,
            fix_unavailable_reason: None,
        })
    }

    /// Loads the default-depth graph eagerly for callers that need a ready view model: component-test fixtures and detached workspace switches. Window open uses `open_async`.
    fn open_blocking_with_graph(path: PathBuf, revset: &str) -> jayjay_core::JayResult<OpenedRepo> {
        let mut opened = Self::open_blocking(path)?;
        opened.working_copy_stale = snapshot_reporting_stale(&opened.repo)?;
        opened.entries = opened.repo.log_graph(revset)?;
        opened.bookmarks = opened.repo.list_bookmarks().unwrap_or_default();
        opened.tags = opened.repo.list_tags().unwrap_or_default();
        opened.vocabulary = opened.repo.revset_vocabulary(&opened.bookmarks);
        opened.workspaces = opened.repo.workspace_list().unwrap_or_default();
        opened.pr_host_name = opened.repo.pr_host_name();
        opened.fix_unavailable_reason = opened.repo.fix_unavailable_reason();
        Ok(opened)
    }

    fn ready(
        repo_path: SharedString,
        revset_filter: RevsetFilterState,
        loaded: OpenedRepo,
    ) -> Self {
        let OpenedRepo {
            repo,
            working_copy_stale,
            repo_root_path,
            entries,
            bookmarks,
            tags,
            vocabulary,
            workspaces,
            pr_host_name,
            fix_unavailable_reason,
        } = loaded;
        let selected = entries
            .iter()
            .position(|e| e.change.is_working_copy)
            .or(if entries.is_empty() { None } else { Some(0) });
        let synthetic_elided_nodes = repo.log_graph_synthetic_elided_nodes().unwrap_or(true);
        let dag_layout = Arc::new(DagLayout::compute(&entries, synthetic_elided_nodes));
        let changes: Vec<ChangeInfo> = entries.iter().map(|e| e.change.clone()).collect();
        let mut selected_changes = OrderedSelection::default();
        if let Some(change) = selected.and_then(|selected| changes.get(selected)) {
            selected_changes.replace(change.selection_revision().to_owned());
        }
        Self {
            repo: Some(repo),
            fix_unavailable_reason: fix_unavailable_reason.map(SharedString::from),
            repo_path,
            repo_root_path: repo_root_path.into(),
            error: None,
            working_copy_stale,
            selected,
            selected_changes,
            files: None,
            conflicted_paths: Arc::default(),
            selected_file_ix: None,
            shown: ShownDiff::default(),
            diff_cache: DiffCache::default(),
            stats: StatsState::default(),
            working_copy_stats: None,
            current_operation_description: String::new(),
            view_mode: DiffViewMode::Unified,
            ignore_whitespace: false,
            can_load_more: default_revset_depth(&revset_filter.revset)
                .is_some_and(|depth| changes.len() >= depth as usize),
            focused_revision: None,
            expanded_elisions: Vec::new(),
            focused_commit_id: None,
            pending_focus_target: None,
            pending_focus_reveal: None,
            graph_awaiting_replacement: false,
            graph_replacement_backup: None,
            revset_filter,
            vocabulary,
            detail_mode: DetailMode::Diff,
            annotate_lines: None,
            avatar_in_flight: HashSet::new(),
            pr_info: None,
            pr_host_name: pr_host_name.map(SharedString::from),
            compare: None,
            graph: GraphData {
                changes: Arc::new(changes),
                entries: Arc::new(entries),
                dag_layout,
                bookmarks: Arc::new(bookmarks),
                tags: Arc::new(tags),
                workspaces: Arc::new(workspaces),
            },
            loading: LoadingState::default(),
            last_internal_mutation_at: None,
            refresh_suspended: false,
            notes: NotesState::default(),
            pending_file_selection: None,
            pending_error: None,
            selection_cache: RefCell::new(None),
        }
    }

    pub(super) fn empty(repo_path: SharedString) -> Self {
        Self {
            repo: None,
            fix_unavailable_reason: None,
            repo_root_path: repo_path.clone(),
            repo_path,
            error: None,
            working_copy_stale: false,
            selected: None,
            selected_changes: OrderedSelection::default(),
            files: None,
            conflicted_paths: Arc::default(),
            selected_file_ix: None,
            shown: ShownDiff::default(),
            diff_cache: DiffCache::default(),
            stats: StatsState::default(),
            working_copy_stats: None,
            current_operation_description: String::new(),
            view_mode: DiffViewMode::Unified,
            ignore_whitespace: false,
            revset_filter: RevsetFilterState::new(&build_default_revset(DEFAULT_REVSET_DEPTH)),
            can_load_more: false,
            focused_revision: None,
            expanded_elisions: Vec::new(),
            focused_commit_id: None,
            pending_focus_target: None,
            pending_focus_reveal: None,
            graph_awaiting_replacement: false,
            graph_replacement_backup: None,
            vocabulary: RevsetVocabulary::default(),
            detail_mode: DetailMode::Diff,
            annotate_lines: None,
            avatar_in_flight: HashSet::new(),
            pr_info: None,
            pr_host_name: None,
            compare: None,
            graph: GraphData::default(),
            loading: LoadingState::default(),
            last_internal_mutation_at: None,
            refresh_suspended: false,
            notes: NotesState::default(),
            pending_file_selection: None,
            pending_error: None,
            selection_cache: RefCell::new(None),
        }
    }

    fn error(repo_path: SharedString, msg: String) -> Self {
        let mut vm = Self::empty(repo_path);
        vm.present_error(msg);
        vm
    }

    pub fn boot(&mut self, cx: &mut Context<Self>) {
        self.sync_ignore_whitespace(cx);
        cx.observe_global::<config::AppConfigStore>(|vm, cx| vm.sync_ignore_whitespace(cx))
            .detach();
        cx.on_app_quit(|vm, _| {
            if let Some(repo) = &vm.repo {
                repo.cancel_running_jj_processes();
            }
            async {}
        })
        .detach();
        // Snapshot small repos on open so the WC is current; huge checkouts still load their graph progressively but defer the expensive snapshot.
        let snapshot_working_copy = self
            .repo
            .as_ref()
            .is_some_and(|repo| !repo.working_copy_is_large());
        self.refresh_with_working_copy_snapshot(false, snapshot_working_copy, cx);
    }
}
