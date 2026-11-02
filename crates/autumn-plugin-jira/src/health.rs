//! The `jira` health indicator.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::runtime::Cell;

/// The `jira` health indicator.
pub struct JiraHealth {
    cell: Cell,
    failed: AtomicBool,
}

impl std::fmt::Debug for JiraHealth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JiraHealth")
            .field("started", &self.cell.get().is_some())
            .field("failed", &self.failed.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl JiraHealth {
    /// Makes the indicator of the runtime in `cell`.
    pub(crate) const fn new(cell: Cell) -> Self {
        Self {
            cell,
            failed: AtomicBool::new(false),
        }
    }

    /// Records that the start failed.
    pub(crate) fn mark_failed(&self) {
        self.failed.store(true, Ordering::SeqCst);
    }
}

impl autumn_web::actuator::HealthIndicator for JiraHealth {
    fn check(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = autumn_web::actuator::HealthCheckOutput> + Send + '_>,
    > {
        Box::pin(async { autumn_web::actuator::HealthCheckOutput::down() })
    }
}
