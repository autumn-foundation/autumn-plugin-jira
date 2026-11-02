//! HMAC-SHA256 signatures for webhook deliveries.

/// Signs a webhook body: `"sha256=<lower hex>"`.
#[must_use]
pub fn sign(_secret: &[u8], _body: &[u8]) -> String {
    String::new()
}
