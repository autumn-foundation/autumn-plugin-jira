//! The event seam: the plugin calls each `EventSink` with a verified
//! delivery. The harvest bridge is a sink.

use std::fmt::Display;
use std::sync::Arc;

use async_trait::async_trait;

use crate::error::JiraError;
use crate::webhook::Delivery;

/// The error of a sink.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HandlerError {
    /// Jira sends the delivery again (503).
    #[error("transient: {0}")]
    Transient(String),
    /// A retry cannot fix it. The plugin answers 200 and logs an error.
    #[error("permanent: {0}")]
    Permanent(String),
}

impl HandlerError {
    /// A transient error. Jira sends the delivery again.
    pub fn transient(error: impl Display) -> Self {
        Self::Transient(error.to_string())
    }

    /// A permanent error. Jira does not send the delivery again.
    pub fn permanent(error: impl Display) -> Self {
        Self::Permanent(error.to_string())
    }
}

impl From<JiraError> for HandlerError {
    /// `JiraError::is_retryable` picks the variant (R3).
    fn from(error: JiraError) -> Self {
        if error.is_retryable() {
            Self::Transient(error.to_string())
        } else {
            Self::Permanent(error.to_string())
        }
    }
}

/// A handler of verified webhook deliveries.
#[async_trait]
pub trait EventSink: Send + Sync + 'static {
    /// A name for logs and health details.
    fn name(&self) -> &str {
        std::any::type_name::<Self>()
    }

    /// Takes one delivery. Deliveries are at least once: the same delivery
    /// id can come again, so make the work idempotent.
    async fn deliver(&self, delivery: Arc<Delivery>) -> Result<(), HandlerError>;
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;
    use crate::error::{MarkerRef, Op, UnknownCause};
    use crate::ids::WebhookDeliveryId;
    use crate::webhook::{DeliveryMeta, Flow, parse_event};
    use std::sync::Mutex;

    #[test]
    fn handler_error_from_jira_error() {
        let rate = JiraError::RateLimited {
            op: Op::GetIssue,
            retry_after: None,
            reason: None,
        };
        let from_rate = HandlerError::from(rate.clone());
        assert_eq!(from_rate, HandlerError::Transient(rate.to_string()));
        let pending = JiraError::OutcomeUnknown {
            op: Op::CreateIssue,
            cause: UnknownCause::Network,
            marker: Some(MarkerRef::label("autumn-idem-x".to_owned())),
        };
        assert!(matches!(
            HandlerError::from(pending),
            HandlerError::Transient(_)
        ));
        let missing = JiraError::NotFoundOrForbidden { op: Op::GetIssue };
        assert_eq!(
            HandlerError::from(missing.clone()),
            HandlerError::Permanent(missing.to_string())
        );
        let unknown = JiraError::OutcomeUnknown {
            op: Op::CreateIssue,
            cause: UnknownCause::Timeout,
            marker: None,
        };
        assert!(matches!(
            HandlerError::from(unknown),
            HandlerError::Permanent(_)
        ));
        assert_eq!(
            HandlerError::transient("db down"),
            HandlerError::Transient("db down".to_owned())
        );
        assert_eq!(
            HandlerError::permanent(42),
            HandlerError::Permanent("42".to_owned())
        );
        assert_eq!(HandlerError::transient("x").to_string(), "transient: x");
        assert_eq!(HandlerError::permanent("y").to_string(), "permanent: y");
    }

    struct Recorder(Mutex<Vec<String>>);

    #[async_trait]
    impl EventSink for Recorder {
        async fn deliver(&self, delivery: Arc<Delivery>) -> Result<(), HandlerError> {
            self.0.lock().unwrap().push(delivery.meta.id.to_string());
            Ok(())
        }
    }

    struct Named;

    #[async_trait]
    impl EventSink for Named {
        #[allow(
            clippy::unnecessary_literal_bound,
            reason = "the trait gives the signature"
        )]
        fn name(&self) -> &str {
            "named"
        }

        async fn deliver(&self, _delivery: Arc<Delivery>) -> Result<(), HandlerError> {
            Err(HandlerError::permanent("no"))
        }
    }

    #[tokio::test]
    async fn default_sink_name() {
        let recorder = Recorder(Mutex::new(Vec::new()));
        assert!(
            recorder.name().ends_with("sink::tests::Recorder"),
            "{}",
            recorder.name()
        );
        assert_eq!(Named.name(), "named");
        let parsed = parse_event(br#"{"webhookEvent": "sprint_started"}"#).unwrap();
        let meta = DeliveryMeta::new(WebhookDeliveryId::parse("d-1").unwrap(), 0, Flow::Primary);
        let delivery = Arc::new(Delivery::new(meta, parsed));
        let sinks: Vec<Arc<dyn EventSink>> = vec![Arc::new(recorder), Arc::new(Named)];
        assert_eq!(sinks[0].deliver(Arc::clone(&delivery)).await, Ok(()));
        assert!(sinks[1].deliver(delivery).await.is_err());
    }
}
