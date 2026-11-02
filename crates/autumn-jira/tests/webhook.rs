//! Webhook flow over the public API: sign, verify, parse, deliver.

use std::sync::Arc;

use autumn_jira::ids::WebhookDeliveryId;
use autumn_jira::webhook::{
    DELIVERY_HEADER, DeliveryMeta, FLOW_HEADER, Flow, RETRY_HEADER, SIGNATURE_HEADER, parse_event,
    sign, verify_signature,
};
use autumn_jira::{Delivery, EventSink, HandlerError, JiraEvent};

const BODY: &[u8] = br#"{
  "timestamp": 1606480436302,
  "webhookEvent": "jira:issue_updated",
  "user": {"accountId": "5b10a2844c20165700ede21g"},
  "issue": {"id": "99291", "key": "OPS-7", "fields": {
    "status": {"id": "10001", "name": "Done", "statusCategory": {"id": 3, "key": "done"}},
    "labels": ["autumn-tracked"]
  }},
  "changelog": {"id": 10124, "items": [
    {"field": "status", "from": "3", "fromString": "In Progress", "to": "10001", "toString": "Done"}
  ]}
}"#;

struct DoneWatcher;

#[async_trait::async_trait]
impl EventSink for DoneWatcher {
    async fn deliver(&self, delivery: Arc<Delivery>) -> Result<(), HandlerError> {
        let change = delivery
            .event
            .status_change()
            .ok_or_else(|| HandlerError::permanent("no status change"))?;
        if change.to.name.as_deref() == Some("Done") {
            Ok(())
        } else {
            Err(HandlerError::transient("not done yet"))
        }
    }
}

#[test]
fn header_names_are_pinned() {
    assert_eq!(SIGNATURE_HEADER, "X-Hub-Signature");
    assert_eq!(DELIVERY_HEADER, "X-Atlassian-Webhook-Identifier");
    assert_eq!(RETRY_HEADER, "X-Atlassian-Webhook-Retry");
    assert_eq!(FLOW_HEADER, "X-Atlassian-Webhook-Flow");
}

#[tokio::test]
async fn signed_delivery_reaches_a_sink() {
    let secret = b"webhook-secret";
    let header = sign(secret, BODY);
    assert!(header.starts_with("sha256="));
    assert_eq!(header.len(), 71);
    assert!(verify_signature(secret, BODY, &header));
    let mut tampered = BODY.to_vec();
    tampered[20] ^= 1;
    assert!(!verify_signature(secret, &tampered, &header));

    let parsed = parse_event(BODY).unwrap();
    assert!(matches!(parsed.event, JiraEvent::IssueUpdated(_)));
    let meta = DeliveryMeta::new(
        WebhookDeliveryId::parse("c9d0d0d6-8c5a-4b37").unwrap(),
        0,
        Flow::parse("Primary"),
    );
    let delivery = Arc::new(Delivery::new(meta, parsed));
    let issue = delivery.event.issue().unwrap();
    assert_eq!(
        issue.fields.status.as_ref().unwrap().category.as_str(),
        "done"
    );
    let sink: Arc<dyn EventSink> = Arc::new(DoneWatcher);
    assert_eq!(sink.deliver(Arc::clone(&delivery)).await, Ok(()));
    // At least once: the same delivery can come again, and the sink takes it again.
    assert_eq!(sink.deliver(delivery).await, Ok(()));
}
