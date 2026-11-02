//! HMAC-SHA256 signatures for webhook deliveries.
//!
//! This is the own HMAC code of the fake (RFC 2104). It shares no code with
//! the client, so the two readings of the Atlassian rule stay independent.

use sha2::{Digest, Sha256};

/// The block size of SHA-256 in bytes.
const BLOCK: usize = 64;

/// Signs a webhook body: `"sha256=<lower hex>"`.
///
/// The value is the `X-Hub-Signature` header that Jira sends with a
/// webhook that has a secret.
#[must_use]
pub fn sign(secret: &[u8], body: &[u8]) -> String {
    format!("sha256={}", hex(&hmac_sha256(secret, body)))
}

/// Returns HMAC-SHA256 of `message` with `key`.
fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0_u8; BLOCK];
    if key.len() > BLOCK {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let inner_pad = block.map(|b| b ^ 0x36);
    let outer_pad = block.map(|b| b ^ 0x5c);
    let inner = Sha256::new()
        .chain_update(inner_pad)
        .chain_update(message)
        .finalize();
    Sha256::new()
        .chain_update(outer_pad)
        .chain_update(inner)
        .finalize()
        .into()
}

/// Returns the lower-case hex text of `bytes`.
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from(DIGITS[usize::from(b >> 4)]));
        out.push(char::from(DIGITS[usize::from(b & 0x0f)]));
    }
    out
}
