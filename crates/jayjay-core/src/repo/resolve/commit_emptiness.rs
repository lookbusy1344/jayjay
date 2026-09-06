use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use jj_lib::backend::CommitId;
use jj_lib::commit::Commit;
use jj_lib::repo::ReadonlyRepo;

use super::super::support::block_on;

/// Repeated rewrites mint fresh commit ids, so an uncapped cache would grow for the whole session.
const CAPACITY: usize = 100_000;

/// Emptiness is fixed by a commit's own tree and parents, so answers stay valid across operations.
pub(crate) struct CommitEmptiness {
    known: RwLock<HashMap<CommitId, bool>>,
    capacity: usize,
}

impl Default for CommitEmptiness {
    fn default() -> Self {
        Self::with_capacity(CAPACITY)
    }
}

impl CommitEmptiness {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            known: RwLock::default(),
            capacity,
        }
    }

    /// Display-only: a backend error reads as non-empty and is not cached.
    #[cfg_attr(feature = "hotpath", hotpath::measure(impl_type = "CommitEmptiness"))]
    pub(crate) fn is_empty(&self, repo: &Arc<ReadonlyRepo>, commit: &Commit) -> bool {
        if let Some(empty) = self.get(commit.id()) {
            return empty;
        }
        let Ok(empty) = block_on(commit.is_empty(repo.as_ref())) else {
            return false;
        };
        self.extend([(commit.id().clone(), empty)]);
        empty
    }

    pub(crate) fn get(&self, commit_id: &CommitId) -> Option<bool> {
        self.known.read().unwrap().get(commit_id).copied()
    }

    /// Clears wholesale rather than evicting by recency: an evicted answer only costs a recomputation.
    pub(crate) fn extend(&self, computed: impl IntoIterator<Item = (CommitId, bool)>) {
        let computed: Vec<_> = computed.into_iter().collect();
        let mut known = self.known.write().unwrap();
        if known.len() + computed.len() > self.capacity {
            known.clear();
        }
        known.extend(computed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(byte: u8) -> CommitId {
        CommitId::new(vec![byte; 20])
    }

    #[test]
    fn extending_past_the_capacity_clears_before_inserting() {
        let cache = CommitEmptiness::with_capacity(2);
        cache.extend([(id(1), true), (id(2), false)]);

        cache.extend([(id(3), true)]);

        assert_eq!(cache.get(&id(1)), None);
        assert_eq!(cache.get(&id(2)), None);
        assert_eq!(cache.get(&id(3)), Some(true));
    }

    #[test]
    fn extending_up_to_the_capacity_keeps_earlier_answers() {
        let cache = CommitEmptiness::with_capacity(2);
        cache.extend([(id(1), true)]);

        cache.extend([(id(2), false)]);

        assert_eq!(cache.get(&id(1)), Some(true));
        assert_eq!(cache.get(&id(2)), Some(false));
    }
}
