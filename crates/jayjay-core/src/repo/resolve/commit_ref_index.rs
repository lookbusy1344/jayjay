use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use jj_lib::backend::CommitId;
use jj_lib::repo::ReadonlyRepo;

use super::super::Repo;
use super::CommitRefNames;

/// Refs per commit for one render; the bookmark, tag, and remote-ref parts are shared per-operation caches.
pub(crate) struct CommitRefIndex {
    bookmarks: Arc<CommitRefNames>,
    tags: Arc<CommitRefNames>,
    remote_ref_commits: Arc<HashSet<CommitId>>,
    workspaces: HashMap<CommitId, Vec<String>>,
    working_copy_commit_id: Option<CommitId>,
}

pub(super) struct CommitRefs {
    pub bookmarks: Vec<String>,
    pub tags: Vec<String>,
    pub workspaces: Vec<String>,
    pub is_working_copy: bool,
    pub has_remote_ref: bool,
}

impl CommitRefIndex {
    pub(super) fn refs_for(&self, commit_id: &CommitId) -> CommitRefs {
        CommitRefs {
            bookmarks: self.bookmarks.names(commit_id),
            tags: self.tags.names(commit_id),
            workspaces: self.workspaces.get(commit_id).cloned().unwrap_or_default(),
            is_working_copy: self.is_working_copy(commit_id),
            has_remote_ref: self.remote_ref_commits.contains(commit_id),
        }
    }

    pub(crate) fn is_working_copy(&self, commit_id: &CommitId) -> bool {
        self.working_copy_commit_id.as_ref() == Some(commit_id)
    }
}

impl Repo {
    pub(crate) fn commit_ref_index(&self, repo: &Arc<ReadonlyRepo>) -> CommitRefIndex {
        let view = repo.view();
        let mut workspaces: HashMap<CommitId, Vec<String>> = HashMap::new();
        for (name, id) in view.wc_commit_ids() {
            if *name != *self.workspace_name {
                workspaces
                    .entry(id.clone())
                    .or_default()
                    .push(name.as_str().to_owned());
            }
        }
        CommitRefIndex {
            bookmarks: self.commit_bookmarks(repo),
            tags: self
                .commit_tags_cache
                .get_or_init(repo, || CommitRefNames::from_refs(view.local_tags())),
            remote_ref_commits: self.remote_ref_commits_cache.get_or_init(repo, || {
                view.all_remote_bookmarks()
                    .flat_map(|(_, remote_ref)| remote_ref.target.present_adds().cloned())
                    .collect()
            }),
            workspaces,
            working_copy_commit_id: view.get_wc_commit_id(&self.workspace_name).cloned(),
        }
    }

    pub(crate) fn commit_bookmarks(&self, repo: &Arc<ReadonlyRepo>) -> Arc<CommitRefNames> {
        self.commit_bookmarks_cache.get_or_init(repo, || {
            CommitRefNames::from_refs(repo.view().local_bookmarks())
        })
    }
}
