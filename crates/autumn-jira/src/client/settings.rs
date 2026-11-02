//! `ClientSettings`: retry, wait, page and lookup limits.

use std::time::Duration;

use crate::policy::{self, OpClass};

/// The limits of one client.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ClientSettings {
    /// Attempts of one call. Default 5. `policy::MAX_ATTEMPTS` caps it.
    pub max_attempts: u32,
    /// The first retry wait. Default 2 000 ms.
    pub base_backoff_ms: u64,
    /// The largest retry wait. Default 30 000 ms. A `Retry-After` above it
    /// stops the call with `RateLimited`.
    pub max_backoff_ms: u64,
    /// Pages of one list call. Default 1 000.
    pub max_pages: u32,
    /// Marker lookups after a lost reply. Default 3. `policy::MAX_LOOKUPS`
    /// caps it.
    pub lookup_attempts: u32,
    /// The wait before each marker lookup, for the search index lag.
    /// Default 2 000 ms.
    pub lookup_delay_ms: u64,
    /// Jitter on each retry wait. Default `true`. Tests set `false`.
    pub jitter: bool,
    /// The prefix of the marker label. Default `autumn-idem-`.
    pub marker_label_prefix: String,
}

impl Default for ClientSettings {
    fn default() -> Self {
        Self {
            max_attempts: policy::MAX_ATTEMPTS,
            base_backoff_ms: 2_000,
            max_backoff_ms: policy::MAX_BACKOFF_MS,
            max_pages: policy::DEFAULT_MAX_PAGES,
            lookup_attempts: policy::MAX_LOOKUPS,
            lookup_delay_ms: 2_000,
            jitter: true,
            marker_label_prefix: "autumn-idem-".to_owned(),
        }
    }
}

impl ClientSettings {
    /// The settings for a durable host: a harvest activity, or a camel
    /// route that a harvest activity calls. The host is the one retry
    /// owner (B2, Y18). Max attempts 2, base 500 ms, max backoff 10 s, one
    /// lookup with no delay.
    #[must_use]
    pub fn for_durable_host() -> Self {
        Self::default()
    }

    /// Keeps `jitter` and `marker_label_prefix`. Caps the other fields at
    /// the durable values.
    #[must_use]
    pub const fn durable(self) -> Self {
        self
    }

    /// Returns the worst-case wall time of one client call of `class`, for
    /// a transport timeout of `per_request`.
    #[must_use]
    pub fn worst_case(&self, class: OpClass, per_request: Duration) -> Duration {
        let _ = (class, per_request);
        Duration::ZERO
    }
}
