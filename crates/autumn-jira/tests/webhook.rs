//! Webhook flow over the public API: sign, verify, parse, deliver.

use std::sync::Arc;

use autumn_jira::ids::WebhookDeliveryId;
use autumn_jira::webhook::{
    DELIVERY_HEADER, DeliveryMeta, FLOW_HEADER, Flow, RETRY_HEADER, SIGNATURE_HEADER, parse_event,
    sign, verify_signature,
};
use autumn_jira::{Delivery, EventSink, HandlerError, JiraEvent, JiraSite};

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

/// A delivery of site `host` for issue 10042 with a comment.
fn site_body(issue_self: &str, comment_self: &str) -> Vec<u8> {
    serde_json::json!({
        "webhookEvent": "comment_created",
        "issue": {"id": "10042", "self": issue_self, "key": "OPS-7"},
        "comment": {"id": "5", "self": comment_self, "body": "hi"}
    })
    .to_string()
    .into_bytes()
}

#[test]
fn a_delivery_keeps_the_host_of_its_site() {
    use autumn_jira::ids::SiteHost;
    let acme = "https://acme.atlassian.net/rest/api/2/issue/10042";
    let acme_comment = "https://ACME.atlassian.net:443/rest/api/2/issue/10042/comment/5";
    let parsed = parse_event(&site_body(acme, acme_comment)).unwrap();
    let issue = parsed.event.issue().unwrap();
    assert_eq!(
        issue.site.as_ref().map(SiteHost::as_str),
        Some("acme.atlassian.net")
    );
    assert_eq!(
        parsed.event.site().map(SiteHost::as_str),
        Some("acme.atlassian.net")
    );
    let meta = DeliveryMeta::new(WebhookDeliveryId::parse("d-9").unwrap(), 0, Flow::Primary);
    let delivery = Delivery::new(meta, parsed);
    assert_eq!(
        delivery.site().map(SiteHost::as_str),
        Some("acme.atlassian.net")
    );
    // Two sites in one delivery, or no site: no site is known.
    let other = "https://other.atlassian.net/rest/api/2/issue/10042/comment/5";
    let mixed = parse_event(&site_body(acme, other)).unwrap();
    assert_eq!(mixed.event.site(), None);
    let none =
        parse_event(br#"{"webhookEvent": "jira:issue_created", "issue": {"id": "1"}}"#).unwrap();
    assert_eq!(none.event.site(), None);
    // A bad `self` gives no site, and the event still parses.
    for bad in [
        "acme.atlassian.net/rest",
        "https://user@evil.example/rest",
        "https://evil..example/",
        "https://-x.example/",
        "https:///rest",
        "ftp://acme.atlassian.net/",
        "https://acme.atlassian.net:99999/",
    ] {
        let parsed = parse_event(&site_body(bad, bad)).unwrap();
        assert_eq!(parsed.event.site(), None, "{bad}");
    }
}

#[test]
fn a_site_host_has_a_strict_rule() {
    use autumn_jira::ids::{SiteHost, SiteName};
    use autumn_jira::{JiraSite, ids::CloudId};
    assert_eq!(
        SiteHost::parse("acme.atlassian.net").unwrap().as_str(),
        "acme.atlassian.net"
    );
    for bad in ["", "ACME.net", "a..b", "-a.b", "a-.b", "a b", "a/b", "a:1"] {
        assert!(SiteHost::parse(bad).is_err(), "{bad}");
    }
    assert_eq!(
        SiteHost::from_url("https://Acme.Atlassian.net/x?y#z").map(|h| h.to_string()),
        Some("acme.atlassian.net".to_owned())
    );
    let site = JiraSite::Subdomain(SiteName::parse("acme").unwrap());
    assert_eq!(
        site.host().map(|h| h.to_string()),
        Some("acme.atlassian.net".to_owned())
    );
    let cloud = JiraSite::CloudId(CloudId::parse("11111111-2222-4333-8444-555555555555").unwrap());
    assert_eq!(
        cloud.host(),
        None,
        "the host of a cloud id site is not known"
    );
}

#[tokio::test]
async fn each_fake_delivery_names_its_site() {
    use autumn_jira::{IssueKey, NewIssue, ProjectKey, request::IssueTypeRef};
    let fake = autumn_jira_fake::FakeJira::new().with_webhook_secret("s");
    let site = JiraSite::Subdomain(autumn_jira::ids::SiteName::parse("acme").unwrap());
    let auth = autumn_jira::BasicAuth::new(
        autumn_jira_fake::DEFAULT_EMAIL,
        autumn_jira::san_mateo::Secret::new(autumn_jira_fake::DEFAULT_API_TOKEN),
    )
    .unwrap();
    let client = autumn_jira::JiraClient::api_token(site.clone(), auth, Arc::new(fake.clone()));
    let project = ProjectKey::parse("OPS").unwrap();
    let new = NewIssue::new(project, IssueTypeRef::Name("Task".to_owned()), "s");
    let key: IssueKey = client.create_issue(&new).await.unwrap().key;
    fake.human_comment(key.as_str(), "hi", "ann").unwrap();
    let deliveries = fake.take_deliveries();
    assert!(deliveries.len() >= 2, "{}", deliveries.len());
    for d in deliveries {
        let parsed = parse_event(&d.body).unwrap();
        assert_eq!(
            parsed.event.site(),
            site.host().as_ref(),
            "{}",
            parsed.event
        );
    }
}
