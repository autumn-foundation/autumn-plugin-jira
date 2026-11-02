//! The settle wait of a lookup-only `createIssue`.
//!
//! The wait is a tokio timer. A camel route runs on tokio, so the wait
//! needs no thread of its own. A paused tokio clock in a test moves it.

use std::time::Duration;

/// Waits for `wait`. A zero wait ends at once.
pub(crate) async fn settle(wait: Duration) {
    if !wait.is_zero() {
        tokio::time::sleep(wait).await;
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

    /// The wait is a tokio timer. A paused tokio clock moves it, and no
    /// thread sleeps for the wait.
    #[tokio::test(start_paused = true)]
    async fn the_wait_follows_the_tokio_clock() {
        let start = tokio::time::Instant::now();
        let wait = Duration::from_secs(3_600);
        let ended = tokio::time::timeout(wait + Duration::from_secs(1), settle(wait)).await;
        assert!(ended.is_ok(), "the wait did not end on the tokio clock");
        assert_eq!(start.elapsed(), wait);
    }
}
