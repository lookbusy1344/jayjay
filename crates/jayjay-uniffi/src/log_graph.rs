use std::sync::Arc;
use std::time::Duration;

use jayjay_core as core;

use crate::error::JayJayError;

/// Cancellation handle for a running graph-load session, shared with the core worker. The shell
/// holds it while a session streams and calls `cancel` to stop it at the next cooperative check.
#[derive(uniffi::Object)]
pub struct JayJayGraphLoadToken {
    pub(crate) inner: core::GraphLoadToken,
}

#[uniffi::export]
impl JayJayGraphLoadToken {
    #[uniffi::constructor]
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: core::GraphLoadToken::new(),
        })
    }

    fn cancel(&self) {
        self.inner.cancel();
    }

    fn is_canceled(&self) -> bool {
        self.inner.is_canceled()
    }

    fn continue_loading(&self, row_ceiling: u32) {
        self.inner.continue_loading(row_ceiling);
    }
}

/// Progressive graph-load request. `first_result_budget_ms` is the millisecond form of the core
/// `Duration`; `row_ceiling` of 0 requests the core default.
#[derive(uniffi::Record)]
pub struct LogGraphRequest {
    pub revset: String,
    pub initial_rows: u32,
    pub background_batch_rows: u32,
    pub first_result_budget_ms: u64,
    pub row_ceiling: u32,
}

/// The request the shell should start from: the plan's policy defaults for `revset`.
#[uniffi::export]
fn default_log_graph_request(revset: String) -> LogGraphRequest {
    core::LogGraphRequest::new(revset).into()
}

impl From<core::LogGraphRequest> for LogGraphRequest {
    fn from(request: core::LogGraphRequest) -> Self {
        Self {
            revset: request.revset,
            initial_rows: request.initial_rows,
            background_batch_rows: request.background_batch_rows,
            first_result_budget_ms: request.first_result_budget.as_millis() as u64,
            row_ceiling: request.row_ceiling,
        }
    }
}

impl From<LogGraphRequest> for core::LogGraphRequest {
    fn from(request: LogGraphRequest) -> Self {
        let row_ceiling = if request.row_ceiling == 0 {
            core::MAX_AUTO_LOADED_ROWS
        } else {
            request.row_ceiling
        };
        Self {
            revset: request.revset,
            initial_rows: request.initial_rows,
            background_batch_rows: request.background_batch_rows,
            first_result_budget: Duration::from_millis(request.first_result_budget_ms),
            row_ceiling,
        }
    }
}

/// One published prefix: entries and the layout computed from the same rows, so the shell renders
/// directly without a second layout round trip across the boundary.
#[derive(uniffi::Record)]
pub struct LogGraphSnapshot {
    pub entries: Vec<core::GraphEntry>,
    pub layout: core::dag::DagLayout,
    pub loaded_rows: u32,
    pub is_complete: bool,
}

impl From<core::LogGraphSnapshot> for LogGraphSnapshot {
    fn from(snapshot: core::LogGraphSnapshot) -> Self {
        Self {
            entries: snapshot.entries,
            layout: snapshot.layout,
            loaded_rows: snapshot.loaded_rows,
            is_complete: snapshot.is_complete,
        }
    }
}

/// A late correction to a published row's `is_empty` flag; merge and off-page rows are published
/// as non-empty and refined once their parent-tree merge completes off the first-paint path.
#[derive(uniffi::Record)]
pub struct EmptyStateUpdate {
    pub commit_id: String,
    pub is_empty: bool,
}

impl From<core::EmptyStateUpdate> for EmptyStateUpdate {
    fn from(update: core::EmptyStateUpdate) -> Self {
        Self {
            commit_id: update.commit_id,
            is_empty: update.is_empty,
        }
    }
}

/// One update from a running session. A session emits zero or more
/// `Snapshot`/`Progress`/`EmptyStates`/`Paused` events, then exactly one terminal event
/// (`Finished`, `Canceled`, or `Failed`).
#[derive(uniffi::Enum)]
pub enum LogGraphEvent {
    Snapshot {
        snapshot: LogGraphSnapshot,
    },
    Progress {
        consumed_rows: u64,
        materialized_rows: u64,
        elapsed_ms: u64,
        first_result_budget_expired: bool,
    },
    EmptyStates {
        updates: Vec<EmptyStateUpdate>,
    },
    Finished,
    Paused,
    Canceled,
    Failed {
        message: String,
    },
}

impl From<core::LogGraphEvent> for LogGraphEvent {
    fn from(event: core::LogGraphEvent) -> Self {
        match event {
            core::LogGraphEvent::Snapshot(snapshot) => Self::Snapshot {
                snapshot: snapshot.into(),
            },
            core::LogGraphEvent::Progress(progress) => Self::Progress {
                consumed_rows: progress.consumed_rows,
                materialized_rows: progress.materialized_rows,
                elapsed_ms: progress.elapsed.as_millis() as u64,
                first_result_budget_expired: progress.first_result_budget_expired,
            },
            core::LogGraphEvent::EmptyStates(updates) => Self::EmptyStates {
                updates: updates.into_iter().map(EmptyStateUpdate::from).collect(),
            },
            core::LogGraphEvent::Finished => Self::Finished,
            core::LogGraphEvent::Paused => Self::Paused,
            core::LogGraphEvent::Canceled => Self::Canceled,
            core::LogGraphEvent::Failed(error) => Self::Failed {
                message: JayJayError::from(error).to_string(),
            },
        }
    }
}

/// Foreign observer that receives session events on the worker thread as they are published; the
/// shell marshals each to its UI thread. The core never invokes it while holding a repository lock.
#[uniffi::export(rust, foreign)]
pub trait LogGraphObserver: Send + Sync {
    fn on_event(&self, event: LogGraphEvent);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips_through_the_core_type() {
        let defaults = default_log_graph_request("all()".to_owned());
        assert_eq!(defaults.revset, "all()");
        assert_eq!(defaults.row_ceiling, core::MAX_AUTO_LOADED_ROWS);

        let core_request: core::LogGraphRequest = LogGraphRequest {
            revset: "@".to_owned(),
            initial_rows: 10,
            background_batch_rows: 100,
            first_result_budget_ms: 2500,
            row_ceiling: 0,
        }
        .into();
        assert_eq!(
            core_request.first_result_budget,
            Duration::from_millis(2500)
        );
        assert_eq!(core_request.initial_rows, 10);
        assert_eq!(core_request.row_ceiling, core::MAX_AUTO_LOADED_ROWS);
    }

    #[test]
    fn terminal_events_convert_to_their_ffi_variants() {
        assert!(matches!(
            LogGraphEvent::from(core::LogGraphEvent::Finished),
            LogGraphEvent::Finished
        ));
        assert!(matches!(
            LogGraphEvent::from(core::LogGraphEvent::Paused),
            LogGraphEvent::Paused
        ));
        assert!(matches!(
            LogGraphEvent::from(core::LogGraphEvent::Canceled),
            LogGraphEvent::Canceled
        ));
        let failed =
            LogGraphEvent::from(core::LogGraphEvent::Failed(core::CoreError::RevNotFound {
                rev: "zzz".to_owned(),
            }));
        match failed {
            LogGraphEvent::Failed { message } => assert!(message.contains("zzz")),
            _ => panic!("expected Failed"),
        }
    }

    #[test]
    fn snapshot_carries_both_entries_and_layout() {
        let snapshot = core::LogGraphSnapshot {
            entries: Vec::new(),
            layout: core::dag::DagLayout::compute(&[]),
            loaded_rows: 0,
            is_complete: true,
        };
        match LogGraphEvent::from(core::LogGraphEvent::Snapshot(snapshot)) {
            LogGraphEvent::Snapshot { snapshot } => {
                assert!(snapshot.is_complete);
                assert_eq!(snapshot.loaded_rows, 0);
                assert!(snapshot.entries.is_empty());
            }
            _ => panic!("expected Snapshot"),
        }
    }
}
