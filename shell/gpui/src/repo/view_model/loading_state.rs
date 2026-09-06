use std::collections::HashSet;

use jayjay_core::GraphLoadToken;

/// An op-heads event only owes a check: jj may have written the operation we are already loaded at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingRefresh {
    CheckOperation,
    Reload,
}

#[derive(Default)]
pub struct LoadingState {
    pub(crate) files: bool,
    pub diff: bool,
    pub(crate) annotate: bool,
    pub more: bool,
    pub(super) pr: bool,
    pub refresh_indicator: bool,
    pub(super) change_gen: u64,
    pub diff_gen: u64,
    pub(super) annotate_gen: u64,
    pub pr_gen: u64,
    pub(super) review_notes_gen: u64,
    /// True while any refresh/mutation runs; FS-triggered refreshes bail to avoid the snapshot-echo loop.
    pub refreshing: bool,
    /// `refreshing == (in_flight > 0)` keeps the gate set until all finish.
    pub in_flight: u32,
    pub operations: u32,
    pub(crate) refresh_gen: u64,
    /// An owed auto-refresh: set when an FS event arrives mid-refresh or while refreshes are suspended.
    pub pending_auto_refresh: Option<PendingRefresh>,
    pub(super) refresh_indicator_gen: u64,
    pub(super) refresh_minimum_elapsed: bool,
    /// Set while a `start_log_graph` session is running for the current `refresh_gen`; the toolbar refresh button becomes a cancel action for it. Cleared once the session's terminal event lands.
    pub(crate) graph_session: Option<GraphLoadToken>,
    /// Generation that owns `graph_session`. A mutation invalidates `refresh_gen` immediately so stale snapshots cannot apply, but the terminal event for this generation still owns cleanup.
    pub(crate) graph_session_gen: Option<u64>,
    /// Graph generations currently represented in the shared repository-task count. Pausing temporarily removes a generation; resuming adds it back without starting a new worker.
    pub(super) graph_in_flight_generations: HashSet<u64>,
    /// True once `graph_session`'s token has been latched but its terminal event has not arrived yet.
    pub graph_session_canceling: bool,
    /// True once the active session's first snapshot has been applied; a later snapshot in the same session only appends rows instead of re-selecting.
    pub(super) graph_first_snapshot_applied: bool,
    /// The first-result budget elapsed before any usable graph prefix arrived.
    pub graph_load_slow: bool,
    /// True while a session has paused at the row ceiling with more history available; drives the Continue Loading affordance.
    pub graph_paused: bool,
    /// Row ceiling for the next session; `0` means the core default (`MAX_AUTO_LOADED_ROWS`). Continue Loading raises it geometrically; a new revset resets it to `0`.
    pub graph_row_ceiling: u32,
}
