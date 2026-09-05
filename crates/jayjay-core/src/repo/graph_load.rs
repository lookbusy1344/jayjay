use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::dag::DagLayout;
use crate::types::{CoreError, GraphEntry};

/// Named policy constants for progressive graph loading. See `dag-loading-performance-plan.md`.
pub const INITIAL_LOG_BATCH_ROWS: u32 = 50;
pub const BACKGROUND_LOG_BATCH_ROWS: u32 = 500;
pub const FIRST_RESULT_BUDGET: Duration = Duration::from_secs(10);
/// Retained-row ceiling before a session pauses for an explicit Continue Loading. Bounds resident
/// memory on an unbounded revset (e.g. `all()`) rather than loading every row automatically.
pub const MAX_AUTO_LOADED_ROWS: u32 = 10_000;

/// A cooperative cancellation flag for one graph-load session, shared between the core worker
/// and whichever shell owns the session's lifetime.
#[derive(Clone, Debug, Default)]
pub struct GraphLoadToken {
    canceled: Arc<AtomicBool>,
}

impl GraphLoadToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.canceled.store(true, Ordering::SeqCst);
    }

    pub fn is_canceled(&self) -> bool {
        self.canceled.load(Ordering::SeqCst)
    }
}

/// A source of monotonic time, injectable so budget expiry is testable without wall-clock sleeps.
pub(crate) trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

#[derive(Default)]
pub(crate) struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Tracks cancellation and the first-result budget for one graph-load session.
///
/// The budget only bounds how long the session may withhold its first published snapshot; it has
/// no bearing on cancellation or on how long background loading may continue after that.
pub(crate) struct RequestGuard<'a> {
    token: GraphLoadToken,
    clock: &'a dyn Clock,
    started_at: Instant,
    first_result_deadline: Instant,
}

impl<'a> RequestGuard<'a> {
    pub(crate) fn new(
        token: GraphLoadToken,
        clock: &'a dyn Clock,
        first_result_budget: Duration,
    ) -> Self {
        let started_at = clock.now();
        let first_result_deadline = started_at + first_result_budget;
        Self {
            token,
            clock,
            started_at,
            first_result_deadline,
        }
    }

    pub(crate) fn is_canceled(&self) -> bool {
        self.token.is_canceled()
    }

    pub(crate) fn first_result_budget_expired(&self) -> bool {
        self.clock.now() >= self.first_result_deadline
    }

    pub(crate) fn elapsed(&self) -> Duration {
        self.clock.now().saturating_duration_since(self.started_at)
    }
}

/// A request to progressively load a log graph. See `dag-loading-performance-plan.md`.
#[derive(Debug, Clone)]
pub struct LogGraphRequest {
    pub revset: String,
    pub initial_rows: u32,
    pub background_batch_rows: u32,
    pub first_result_budget: Duration,
    /// Retained-row ceiling: once this many rows are published the session pauses (emits `Paused`)
    /// so the shell can offer Continue Loading. `u32::MAX` disables the pause for non-UI callers.
    pub row_ceiling: u32,
}

impl LogGraphRequest {
    pub fn new(revset: impl Into<String>) -> Self {
        Self {
            revset: revset.into(),
            initial_rows: INITIAL_LOG_BATCH_ROWS,
            background_batch_rows: BACKGROUND_LOG_BATCH_ROWS,
            first_result_budget: FIRST_RESULT_BUDGET,
            row_ceiling: MAX_AUTO_LOADED_ROWS,
        }
    }
}

/// One complete, ordered prefix of the requested revset's graph, ready to display.
#[derive(Debug)]
pub struct LogGraphSnapshot {
    pub entries: Vec<GraphEntry>,
    pub layout: DagLayout,
    pub loaded_rows: u32,
    pub is_complete: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct LogGraphProgress {
    pub consumed_rows: u64,
    pub materialized_rows: u64,
    pub elapsed: Duration,
    /// True once the first-result budget has elapsed. Meaningless after the first `Snapshot` has
    /// been published; background loading continues regardless of this flag.
    pub first_result_budget_expired: bool,
}

/// One update from a running graph-load session. A session emits zero or more `Snapshot`/`Progress`
/// events, then exactly one terminal event (`Finished`, `Paused`, `Canceled`, or `Failed`).
#[derive(Debug)]
pub enum LogGraphEvent {
    Snapshot(LogGraphSnapshot),
    Progress(LogGraphProgress),
    Finished,
    /// The retained-row ceiling was reached with more history still available. The last published
    /// snapshot stands; a Continue Loading action resumes with a higher ceiling.
    Paused,
    Canceled,
    Failed(CoreError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakeClock {
        base: Instant,
        offset: Mutex<Duration>,
    }

    impl FakeClock {
        fn new() -> Self {
            Self {
                base: Instant::now(),
                offset: Mutex::new(Duration::ZERO),
            }
        }

        fn advance(&self, by: Duration) {
            let mut offset = self.offset.lock().unwrap();
            *offset += by;
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> Instant {
            self.base + *self.offset.lock().unwrap()
        }
    }

    #[test]
    fn first_result_budget_not_expired_before_deadline() {
        let clock = FakeClock::new();
        let guard = RequestGuard::new(GraphLoadToken::new(), &clock, Duration::from_secs(10));

        clock.advance(Duration::from_secs(9));

        assert!(!guard.first_result_budget_expired());
        assert!(!guard.is_canceled());
    }

    #[test]
    fn first_result_budget_expires_at_deadline() {
        let clock = FakeClock::new();
        let guard = RequestGuard::new(GraphLoadToken::new(), &clock, Duration::from_secs(10));

        clock.advance(Duration::from_secs(10));

        assert!(guard.first_result_budget_expired());
    }

    #[test]
    fn cancellation_is_observed_through_the_shared_token() {
        let clock = FakeClock::new();
        let token = GraphLoadToken::new();
        let guard = RequestGuard::new(token.clone(), &clock, Duration::from_secs(10));

        assert!(!guard.is_canceled());
        token.cancel();
        assert!(guard.is_canceled());
    }

    #[test]
    fn cancellation_is_independent_of_the_first_result_budget() {
        let clock = FakeClock::new();
        let token = GraphLoadToken::new();
        let guard = RequestGuard::new(token.clone(), &clock, Duration::from_secs(10));

        token.cancel();

        assert!(guard.is_canceled());
        assert!(!guard.first_result_budget_expired());
    }

    #[test]
    fn cloned_tokens_share_cancellation_state() {
        let token = GraphLoadToken::new();
        let clone = token.clone();

        clone.cancel();

        assert!(token.is_canceled());
    }
}
