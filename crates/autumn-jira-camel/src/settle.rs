//! The settle wait of a lookup-only `createIssue`.
//!
//! The crate has no async runtime dependency, so a short-lived thread
//! sleeps and then wakes the task. The wait runs only on a retry attempt
//! whose first marker lookup finds nothing, so this thread is rare.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

/// The state that the timer thread and the task share.
#[derive(Debug, Default)]
struct Shared {
    done: bool,
    waker: Option<Waker>,
}

/// A future that is ready after `wait`.
#[derive(Debug)]
pub(crate) struct Settle {
    wait: Duration,
    shared: Option<Arc<Mutex<Shared>>>,
}

/// Returns a future that is ready after `wait`.
pub(crate) const fn settle(wait: Duration) -> Settle {
    Settle { wait, shared: None }
}

impl Future for Settle {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.wait.is_zero() {
            return Poll::Ready(());
        }
        if let Some(shared) = &self.shared {
            let mut state = shared.lock().unwrap_or_else(PoisonError::into_inner);
            let poll = if state.done {
                Poll::Ready(())
            } else {
                state.waker = Some(cx.waker().clone());
                Poll::Pending
            };
            drop(state);
            return poll;
        }
        let shared = Arc::new(Mutex::new(Shared {
            done: false,
            waker: Some(cx.waker().clone()),
        }));
        let wait = self.wait;
        let timer = Arc::clone(&shared);
        let spawned = std::thread::Builder::new()
            .name("jira-settle".to_owned())
            .spawn(move || {
                std::thread::sleep(wait);
                let mut state = timer.lock().unwrap_or_else(PoisonError::into_inner);
                state.done = true;
                if let Some(waker) = state.waker.take() {
                    waker.wake();
                }
            });
        if spawned.is_err() {
            // No thread, so no wait. The lookup then comes early and can
            // miss the issue. That fails closed: the producer does not send the write.
            tracing::warn!("jira: no thread for the settle wait; the lookup does not wait");
            return Poll::Ready(());
        }
        self.shared = Some(shared);
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    #[tokio::test]
    async fn the_wait_ends_after_the_time() {
        let start = Instant::now();
        settle(Duration::ZERO).await;
        assert!(start.elapsed() < Duration::from_millis(50));
        settle(Duration::from_millis(60)).await;
        assert!(start.elapsed() >= Duration::from_millis(60));
    }
}
