use std::future::Future;
use std::sync::Arc;
use std::sync::mpsc::{Sender, TryRecvError};
use std::time::Duration;

use gpui::{AppContext, Context, Task};
use jayjay_core::{CoreResult, Error, Repo};

use super::RepoViewModel;

const BACKGROUND_STREAM_POLL_INTERVAL: Duration = Duration::from_millis(10);

impl RepoViewModel {
    pub(in crate::repo) fn background_update<T>(
        cx: &mut Context<Self>,
        future: impl Future<Output = T> + Send + 'static,
        update: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
    ) where
        T: Send + 'static,
    {
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(future).await;
            let _ = this.update(cx, move |vm, cx| update(vm, result, cx));
        })
        .detach();
    }

    /// Runs `produce` on a background thread, streaming each item it sends back to `on_item` on the
    /// main thread as it arrives, in send order. `produce` owns pacing and termination: once it
    /// returns (or its sender is dropped), the stream ends and `on_item` stops being called.
    ///
    /// Unlike `background_update`, this supports incremental progress instead of one result at the
    /// end. The background thread keeps running to completion even if this view model is dropped or
    /// superseded; `produce` must carry its own cancellation (e.g. a `GraphLoadToken`) if it should
    /// stop early.
    pub(in crate::repo) fn background_stream<T>(
        cx: &mut Context<Self>,
        produce: impl FnOnce(Sender<T>) + Send + 'static,
        mut on_item: impl FnMut(&mut Self, T, &mut Context<Self>) + 'static,
    ) where
        T: Send + 'static,
    {
        let (tx, rx) = std::sync::mpsc::channel();
        drop(std::thread::spawn(move || produce(tx)));
        cx.spawn(async move |this, cx| {
            loop {
                loop {
                    match rx.try_recv() {
                        Ok(item) => {
                            if this.update(cx, |vm, cx| on_item(vm, item, cx)).is_err() {
                                return;
                            }
                        }
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => return,
                    }
                }
                cx.background_executor()
                    .timer(BACKGROUND_STREAM_POLL_INTERVAL)
                    .await;
            }
        })
        .detach();
    }

    pub(in crate::repo) fn delayed_update(
        cx: &mut Context<Self>,
        delay: Duration,
        update: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
    ) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, update);
        })
        .detach();
    }

    pub(in crate::repo) fn repo_write_task(
        &mut self,
        cx: &mut Context<Self>,
        write: impl FnOnce(Arc<Repo>) -> CoreResult<()> + Send + 'static,
        on_success: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
    ) -> Task<CoreResult<()>> {
        // A graph session owns a pinned read-only snapshot. Latch it before starting any jj
        // write, and reject already-queued snapshots before the operation can change the repo.
        self.cancel_graph_session_for_mutation(cx);
        self.repo_result_task(cx, write, move |vm, _, cx| on_success(vm, cx))
    }

    pub(in crate::repo) fn repo_result_task<T>(
        &mut self,
        cx: &mut Context<Self>,
        read_or_write: impl FnOnce(Arc<Repo>) -> CoreResult<T> + Send + 'static,
        on_success: impl FnOnce(&mut Self, &T, &mut Context<Self>) + 'static,
    ) -> Task<CoreResult<T>>
    where
        T: Send + 'static,
    {
        self.repo_result_task_with_indicator(cx, true, read_or_write, on_success)
    }

    pub(in crate::repo) fn repo_result_task_without_indicator<T>(
        &mut self,
        cx: &mut Context<Self>,
        read_or_write: impl FnOnce(Arc<Repo>) -> CoreResult<T> + Send + 'static,
        on_success: impl FnOnce(&mut Self, &T, &mut Context<Self>) + 'static,
    ) -> Task<CoreResult<T>>
    where
        T: Send + 'static,
    {
        self.repo_result_task_with_indicator(cx, false, read_or_write, on_success)
    }

    fn repo_result_task_with_indicator<T>(
        &mut self,
        cx: &mut Context<Self>,
        show_refresh_indicator: bool,
        read_or_write: impl FnOnce(Arc<Repo>) -> CoreResult<T> + Send + 'static,
        on_success: impl FnOnce(&mut Self, &T, &mut Context<Self>) + 'static,
    ) -> Task<CoreResult<T>>
    where
        T: Send + 'static,
    {
        let Some(repo) = self.repo.clone() else {
            self.present_error("repository is not open");
            cx.notify();
            return cx.spawn(async move |_, _| Err(Error::internal("repository is not open")));
        };

        self.clear_error();
        // Stamp before the write so the FS echo from our own jj mutation is ignored.
        self.last_internal_mutation_at = Some(std::time::Instant::now());
        if show_refresh_indicator {
            self.begin_refreshing(cx);
        } else {
            self.begin_repo_task(cx);
        }

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { read_or_write(repo) })
                .await;
            this.update(cx, move |vm, cx| {
                vm.finish_repo_task(cx);
                match result {
                    Ok(value) => {
                        on_success(vm, &value, cx);
                        Ok(value)
                    }
                    Err(error) => {
                        vm.present_error(&error);
                        cx.notify();
                        Err(error)
                    }
                }
            })
            .unwrap_or_else(|error| Err(Error::internal(error)))
        })
    }

    pub(in crate::repo) fn core_result_task(
        cx: &mut Context<Self>,
        future: impl Future<Output = CoreResult<()>> + Send + 'static,
        update: impl FnOnce(&mut Self, CoreResult<()>, &mut Context<Self>) -> CoreResult<()> + 'static,
    ) -> Task<CoreResult<()>> {
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(future).await;
            this.update(cx, move |vm, cx| update(vm, result, cx))
                .unwrap_or_else(|error| Err(Error::internal(error)))
        })
    }
}
