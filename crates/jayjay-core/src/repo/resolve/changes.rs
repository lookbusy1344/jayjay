use std::collections::HashSet;
use std::sync::Arc;

use jj_lib::commit::Commit as JjCommit;
use jj_lib::hex_util::encode_reverse_hex;
use jj_lib::object_id::ObjectId;
use jj_lib::repo::ReadonlyRepo;

use super::super::Repo;
use super::super::log::ImmutableIds;
use super::super::support::{short_change_id, short_commit_id};
use super::CommitRefIndex;
use crate::types::*;

#[derive(Default)]
pub(crate) struct ChangeInfoContext<'a> {
    pub immutable_ids: Option<&'a ImmutableIds>,
    pub ref_index: Option<&'a CommitRefIndex>,
    pub divergent_change_ids: Option<&'a HashSet<String>>,
    pub is_empty: Option<bool>,
}

impl Repo {
    #[cfg_attr(feature = "hotpath", hotpath::measure(impl_type = "Repo"))]
    pub(crate) fn commit_to_change_info(
        &self,
        repo: &Arc<ReadonlyRepo>,
        commit: &JjCommit,
        context: ChangeInfoContext<'_>,
    ) -> ChangeInfo {
        let change_id = short_change_id(&**repo, commit);
        let commit_id = commit.id().hex();
        let author = commit.author();
        let refs = match context.ref_index {
            Some(index) => index.refs_for(commit.id()),
            None => self.commit_ref_index(repo).refs_for(commit.id()),
        };
        let has_conflict = commit.has_conflict();
        let is_empty = context
            .is_empty
            .unwrap_or_else(|| self.commit_emptiness.is_empty(repo, commit));
        // Keep display loading resilient to an invalid immutable() revset; mutation paths still enforce immutability.
        let (is_immutable, has_immutable_child) = match context.immutable_ids {
            Some(ids) => (
                ids.commits.contains(&commit_id),
                ids.parents.contains(&commit_id),
            ),
            None => {
                let is_immutable = self.is_commit_immutable(repo, commit).unwrap_or(false);
                let has_immutable_child =
                    is_immutable && self.has_immutable_child(repo, commit).unwrap_or(false);
                (is_immutable, has_immutable_child)
            }
        };
        let has_children = !repo.view().heads().contains(commit.id());
        let discardable_working_copy = refs.is_working_copy
            && is_empty
            && commit.description().is_empty()
            && refs.bookmarks.is_empty()
            && refs.tags.is_empty()
            && refs.workspaces.is_empty()
            && !has_children
            && !refs.has_remote_ref;
        let new_change = NewChangeEligibility {
            on_top: !discardable_working_copy,
            before: !is_immutable,
            after: has_children && !has_immutable_child,
        };
        let is_divergent = context
            .divergent_change_ids
            .map(|ids| ids.contains(&change_id.id))
            .unwrap_or(false);

        ChangeInfo {
            change_id,
            commit_id: short_commit_id(&**repo, commit),
            description: commit.description().to_owned(),
            author: CommitAuthor::new(
                author.name.clone(),
                author.email.clone(),
                author.timestamp.timestamp.0,
            ),
            parents: commit.parent_ids().iter().map(|id| id.hex()).collect(),
            bookmarks: refs.bookmarks,
            tags: refs.tags,
            workspaces: refs.workspaces,
            is_working_copy: refs.is_working_copy,
            has_conflict,
            is_empty,
            is_immutable,
            is_divergent,
            new_change,
        }
    }

    pub(crate) fn should_include_in_log(
        &self,
        repo: &Arc<ReadonlyRepo>,
        commit: &JjCommit,
    ) -> bool {
        let change_id = encode_reverse_hex(commit.change_id().as_bytes());
        let commit_id = commit.id().hex();
        let description = commit.description().trim();
        let has_bookmarks = self.commit_bookmarks(repo).contains(commit.id());
        let working_copy_commit_id = repo.view().get_wc_commit_id(self.workspace_name.as_ref());
        let is_working_copy = working_copy_commit_id.is_some_and(|id| id == commit.id());

        if !is_working_copy && description.is_empty() && !has_bookmarks {
            let all_zero_commit = commit_id.chars().all(|c| c == '0');
            let all_z_change = change_id.chars().all(|c| c == 'z');
            let no_parents = commit.parent_ids().is_empty();
            if all_zero_commit || all_z_change || no_parents {
                return false;
            }
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use jj_lib::backend::CommitId;
    use jj_lib::op_store::RefTarget;
    use jj_test::{LinearFixture, run_jj_in};

    use super::*;

    fn commit_id(fixture: &LinearFixture, description: &str) -> CommitId {
        let revset = format!("description(substring:{description:?})");
        let output = run_jj_in(
            &fixture.path,
            &["log", "--no-graph", "-r", &revset, "-T", "commit_id"],
        );
        let hex = String::from_utf8(output.stdout).expect("utf-8 commit id");
        CommitId::try_from_hex(hex.trim()).expect("hex commit id")
    }

    #[test]
    fn a_conflicted_bookmark_that_repeats_a_commit_across_sides_is_listed_once() {
        let fixture = LinearFixture::build();
        let [repeated, base, other] = ["add hello", "initial", "add feature"]
            .map(|description| commit_id(&fixture, description));
        let repo = Repo::open(&fixture.path).expect("open repo");
        let target = RefTarget::from_vec(vec![
            Some(repeated.clone()),
            Some(base.clone()),
            Some(other),
            Some(base),
            Some(repeated.clone()),
        ]);
        {
            let _write = repo.write_guard().expect("write guard");
            repo.update_local_bookmark("split", target, "conflict a bookmark")
                .expect("write conflicted bookmark");
        }
        let repeated = repeated.hex();

        let graph = repo.log_graph("all()").expect("log graph");
        let graph_row = graph
            .iter()
            .find(|entry| entry.change.commit_id.id == repeated)
            .expect("graph row for the repeated commit");
        assert_eq!(graph_row.change.bookmarks, ["split"]);

        let log = repo.log("all()").expect("log");
        let log_row = log
            .iter()
            .find(|change| change.commit_id.id == repeated)
            .expect("log row for the repeated commit");
        assert_eq!(log_row.bookmarks, ["split"]);
    }
}
