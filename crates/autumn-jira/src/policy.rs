//! Verified pure core: retry decision, waits, pagers, webhook freshness
//! and status.
//!
//! `verus/policy.rs` holds the spec and the proofs. Keep the bodies the
//! same. The test `verus_bodies_match` compares them.

/// Largest number of attempts for one client call.
pub const MAX_ATTEMPTS: u32 = 5;
/// Largest wait before a retry: 30 seconds.
pub const MAX_BACKOFF_MS: u64 = 30_000;
/// Largest number of marker lookups after a lost reply.
pub const MAX_LOOKUPS: u32 = 3;
/// Default largest number of pages for one list call.
pub const DEFAULT_MAX_PAGES: u32 = 1_000;
/// Smallest jitter factor, in permille.
pub const JITTER_MIN_PERMILLE: u32 = 700;
/// Largest jitter factor, in permille.
pub const JITTER_MAX_PERMILLE: u32 = 1_300;
/// Largest clock skew for a webhook timestamp in the future: 5 minutes.
pub const MAX_FUTURE_SKEW_MS: u64 = 300_000;

/// The class of one attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// HTTP 2xx.
    Success,
    /// HTTP 429.
    RateLimited,
    /// HTTP 408, 425, 5xx, or a status of 600 or more.
    ServerError,
    /// HTTP 3xx.
    Redirect,
    /// Other HTTP status.
    ClientError,
    /// No HTTP reply. The request did not leave the process.
    NotSent,
    /// No HTTP reply. The request can have reached Jira.
    MaybeSent,
}

/// Classifies an HTTP status.
#[must_use]
// The body is the same text as in the Verus spec. Verus has no `contains`.
#[allow(
    clippy::manual_range_contains,
    reason = "the body matches the Verus spec"
)]
pub const fn classify_status(status: u16) -> Outcome {
    if status >= 200 && status <= 299 {
        Outcome::Success
    } else if status == 429 {
        Outcome::RateLimited
    } else if status >= 300 && status <= 399 {
        Outcome::Redirect
    } else if status == 408 || status == 425 || status >= 500 {
        Outcome::ServerError
    } else {
        Outcome::ClientError
    }
}

/// The retry class of one operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OpClass {
    /// A read. It is safe to send again.
    Read,
    /// A PUT or a DELETE. It is safe to send again.
    IdempotentWrite,
    /// A write with a state check before each attempt.
    CheckedWrite,
    /// A write with a marker. A lookup finds it after a lost reply.
    MarkedWrite,
    /// A write that is not safe to send again.
    UnsafeWrite,
}

/// What the client does after one attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Decode the reply.
    Done,
    /// Wait, then send again.
    Retry,
    /// Look up the marker. Never send again.
    Confirm,
    /// Stop with "outcome unknown". Never send again.
    Unknown,
    /// Stop with a typed error.
    Stop,
}

/// Decides the next step after attempt number `attempt` (1-based).
#[must_use]
pub const fn decide(class: OpClass, outcome: Outcome, attempt: u32, max_attempts: u32) -> Decision {
    let max = if max_attempts < MAX_ATTEMPTS {
        max_attempts
    } else {
        MAX_ATTEMPTS
    };
    let left = attempt < max;
    match outcome {
        Outcome::Success => Decision::Done,
        Outcome::ClientError | Outcome::Redirect => Decision::Stop,
        Outcome::ServerError | Outcome::MaybeSent => match class {
            OpClass::UnsafeWrite => Decision::Unknown,
            OpClass::MarkedWrite => Decision::Confirm,
            OpClass::Read | OpClass::IdempotentWrite | OpClass::CheckedWrite => {
                if left {
                    Decision::Retry
                } else {
                    Decision::Stop
                }
            }
        },
        Outcome::RateLimited => match class {
            OpClass::UnsafeWrite => Decision::Stop,
            OpClass::Read
            | OpClass::IdempotentWrite
            | OpClass::CheckedWrite
            | OpClass::MarkedWrite => {
                if left {
                    Decision::Retry
                } else {
                    Decision::Stop
                }
            }
        },
        Outcome::NotSent => {
            if left {
                Decision::Retry
            } else {
                Decision::Stop
            }
        }
    }
}

/// Returns the wait in ms before retry number `attempt` (1-based).
///
/// The wait is `initial_ms * 2^(attempt - 1)`, capped at `cap_ms`. This is
/// the proved san-mateo function. `backoff_matches_san_mateo` compares them.
#[must_use]
pub const fn backoff_ms(initial_ms: u64, attempt: u32, cap_ms: u64) -> u64 {
    let mut b: u64 = if initial_ms <= cap_ms {
        initial_ms
    } else {
        cap_ms
    };
    let mut i: u32 = 1;
    while i < attempt {
        // At the cap or at zero, the value stays. A large `attempt` costs no time.
        if b == cap_ms || b == 0 {
            return b;
        }
        // Compare with no overflow: `cap - b <= b` is `2b >= cap`.
        b = if cap_ms - b <= b { cap_ms } else { b * 2 };
        i += 1;
    }
    b
}

/// Returns the wait before retry number `attempt`, or `None` when a
/// `Retry-After` asks for more than the cap.
#[must_use]
pub const fn retry_wait_ms(
    attempt: u32,
    base_ms: u64,
    cap_ms: u64,
    retry_after_ms: Option<u64>,
) -> Option<u64> {
    let backoff = backoff_ms(base_ms, attempt, cap_ms);
    match retry_after_ms {
        Some(after) => {
            if after > cap_ms {
                None
            } else if after > backoff {
                Some(after)
            } else {
                Some(backoff)
            }
        }
        None => Some(backoff),
    }
}

/// Returns `value * permille / 1000`, with no overflow.
///
/// A `value` above `u64::MAX / 1300` or a `permille` above 1300 gives
/// `value` with no change. The Verus file has proof code in this body, so
/// a proptest checks it.
#[must_use]
// `u64::from` is not `const`.
#[allow(clippy::cast_lossless, reason = "the body matches the Verus spec")]
pub const fn scale_permille(value: u64, permille: u32) -> u64 {
    if value <= u64::MAX / 1_300 && permille <= 1_300 {
        value * (permille as u64) / 1_000
    } else {
        value
    }
}

/// Applies a jitter factor to a wait, then clamps it to the floor and the cap.
#[must_use]
pub const fn apply_jitter(wait_ms: u64, permille: u32, floor_ms: u64, cap_ms: u64) -> u64 {
    let p = if permille < JITTER_MIN_PERMILLE {
        JITTER_MIN_PERMILLE
    } else if permille > JITTER_MAX_PERMILLE {
        JITTER_MAX_PERMILLE
    } else {
        permille
    };
    let scaled = scale_permille(wait_ms, p);
    let high = if cap_ms > floor_ms { cap_ms } else { floor_ms };
    if scaled < floor_ms {
        floor_ms
    } else if scaled > high {
        high
    } else {
        scaled
    }
}

/// What a durable host knows about the previous attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Previous {
    /// This is the first attempt.
    First,
    /// The previous attempt did not send the write.
    NotSent,
    /// The previous attempt can have sent the write.
    MaybeSent,
    /// The previous attempt ended with no typed failure (a crash or a timeout).
    Unknown,
}

/// Returns `true` when a durable host can send a marked write now.
#[must_use]
pub const fn host_may_send(previous: Previous, resend_opt_in: bool) -> bool {
    match previous {
        Previous::First | Previous::NotSent => true,
        Previous::MaybeSent | Previous::Unknown => resend_opt_in,
    }
}

/// What a token pager does after one page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageStep {
    /// Fetch the next page.
    Fetch,
    /// The list is complete.
    Done,
    /// Jira sent a page token again. Stop with an error.
    Stalled,
    /// The page limit is reached. Stop with an error.
    Limit,
}

/// Decides the next step of a token pager after `pages_done` pages.
#[must_use]
pub const fn token_page_step(
    pages_done: u32,
    max_pages: u32,
    is_last: bool,
    has_token: bool,
    repeated: bool,
) -> PageStep {
    if is_last || !has_token {
        PageStep::Done
    } else if repeated {
        PageStep::Stalled
    } else if pages_done >= max_pages {
        PageStep::Limit
    } else {
        PageStep::Fetch
    }
}

/// Returns the next `startAt` of an offset pager, or `None` at the end.
#[must_use]
pub const fn offset_next(start_at: u64, returned: u64, total: u64) -> Option<u64> {
    if returned == 0 || start_at > u64::MAX - returned || start_at + returned >= total {
        None
    } else {
        Some(start_at + returned)
    }
}

/// The age class of a webhook delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// Inside the age and skew bounds.
    Fresh,
    /// Older than the largest age.
    Stale,
    /// Too far in the future.
    Future,
}

/// Classifies the age of a webhook timestamp.
#[must_use]
pub const fn freshness(event_ms: u64, now_ms: u64, max_age_ms: u64, max_skew_ms: u64) -> Freshness {
    if now_ms > event_ms {
        if now_ms - event_ms > max_age_ms {
            Freshness::Stale
        } else {
            Freshness::Fresh
        }
    } else if event_ms - now_ms > max_skew_ms {
        Freshness::Future
    } else {
        Freshness::Fresh
    }
}

/// The result of one verified webhook delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The handlers took the delivery.
    Accepted,
    /// The plugin skips the delivery on purpose.
    Ignored,
    /// A handler failed with a transient error.
    HandlerFailed,
    /// A handler did not finish in time.
    TimedOut,
}

/// Returns the HTTP status for a verified delivery: 200 or 503, never a 4xx.
#[must_use]
pub const fn webhook_status(verdict: Verdict) -> u16 {
    match verdict {
        Verdict::Accepted | Verdict::Ignored => 200,
        Verdict::HandlerFailed | Verdict::TimedOut => 503,
    }
}

/// Returns `true` for a lower-case hex digit: `0-9` (48..=57) or `a-f` (97..=102).
#[must_use]
// The body is the same text as in the Verus spec. Verus has no `contains`.
#[allow(
    clippy::manual_range_contains,
    reason = "the body matches the Verus spec"
)]
pub const fn is_lower_hex(b: u8) -> bool {
    (b >= 48 && b <= 57) || (b >= 97 && b <= 102)
}

/// Returns `true` when `header` has the shape `sha256=<64 lower hex>`.
///
/// Not in Verus: the proptest `spec_as_test_signature_shape` checks it.
#[must_use]
pub fn signature_shape_ok(header: &[u8]) -> bool {
    header.len() == 71
        && header
            .strip_prefix(b"sha256=")
            .is_some_and(|hex| hex.iter().all(|b| is_lower_hex(*b)))
}

/// Quotes a JQL string value. It puts `\` before each `"` and `\`.
///
/// The result is one JQL string literal for each input, so a value such as
/// `x" OR project = SECRET` cannot change the query. Not in Verus: the
/// proptest `spec_as_test_jql_quote_round_trip` checks it.
#[must_use]
pub fn jql_quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::collections::BTreeMap;

    /// The functions that are not in `verus/policy.rs`. Proptests check
    /// them.
    const PROPTEST_ONLY: [&str; 2] = ["signature_shape_ok", "jql_quote"];

    /// The functions that Verus proves, but with a body that differs from
    /// the exec body: the Verus loop has proof steps in its branches. Each
    /// has a named proptest against a closed form.
    const PROOF_PROPTESTED: [(&str, &str); 1] = [("backoff_ms", "spec_as_test_backoff")];

    /// The functions that `verus_bodies_match` compares. The compare drops
    /// each `proof { ... }` block of the Verus body.
    const BODY_MATCHED: [&str; 11] = [
        "classify_status",
        "decide",
        "retry_wait_ms",
        "scale_permille",
        "apply_jitter",
        "host_may_send",
        "token_page_step",
        "offset_next",
        "freshness",
        "webhook_status",
        "is_lower_hex",
    ];

    /// The text that makes Verus skip a proof. Verus still says
    /// "verified" with one of them, so the file must not hold any.
    const PROOF_ESCAPES: [&str; 7] = [
        "assume(",
        "admit(",
        "external_body",
        "verifier::external",
        "#[verifier(",
        "assume_specification",
        "exec_allows_no_decreases_clause",
    ];

    /// Returns each proof escape in `spec`, outside `//` comments.
    fn proof_escapes(spec: &str) -> Vec<String> {
        let code = strip_block_comments(spec);
        let mut out = Vec::new();
        for (n, line) in code.lines().enumerate() {
            let line = line.split("//").next().unwrap_or("");
            for escape in PROOF_ESCAPES {
                if line.contains(escape) {
                    out.push(format!("line {}: {escape}", n + 1));
                }
            }
        }
        out
    }

    /// Returns the name of each `fn` of any visibility in `text`.
    fn all_fns(text: &str) -> Vec<String> {
        text.lines()
            .filter_map(|l| {
                let mut rest = l.trim_start();
                for prefix in ["pub(crate) ", "pub(super) ", "pub ", "const "] {
                    rest = rest.strip_prefix(prefix).unwrap_or(rest);
                }
                rest.strip_prefix("fn ")
            })
            .filter_map(|rest| rest.split(['(', '<']).next())
            .map(str::to_owned)
            .collect()
    }

    /// Returns the name of each `pub fn` line of the spec. A name in a
    /// comment does not count.
    fn spec_fns(spec: &str) -> Vec<String> {
        spec.lines()
            .filter_map(|l| l.trim_start().strip_prefix("pub fn "))
            .filter_map(|rest| rest.split(['(', '<']).next())
            .map(str::to_owned)
            .collect()
    }

    /// Returns each coverage problem: a function of `exec` that is not in
    /// exactly one list, a list entry that is not a function, or a proved
    /// function with no `pub fn` line in `spec`.
    fn coverage_violations(exec: &str, spec: &str) -> Vec<String> {
        let fns = all_fns(exec);
        let in_spec = spec_fns(spec);
        let proved: Vec<&str> = BODY_MATCHED
            .iter()
            .copied()
            .chain(PROOF_PROPTESTED.iter().map(|(name, _)| *name))
            .collect();
        let listed: Vec<&str> = proved.iter().copied().chain(PROPTEST_ONLY).collect();
        let mut out = Vec::new();
        for name in &fns {
            let n = listed.iter().filter(|l| **l == name.as_str()).count();
            if n != 1 {
                out.push(format!("{name} is in {n} lists, not in exactly one"));
            }
        }
        for name in &listed {
            if !fns.iter().any(|f| f == name) {
                out.push(format!("{name} is listed but is not a function"));
            }
        }
        for name in &proved {
            if !in_spec.iter().any(|f| f == name) {
                out.push(format!("{name} has no `pub fn` line in verus/policy.rs"));
            }
        }
        for name in PROPTEST_ONLY {
            if in_spec.iter().any(|f| f == name) {
                out.push(format!("{name} is in verus/policy.rs: match its body"));
            }
        }
        let tests = include_str!("policy.rs");
        for (name, test) in PROOF_PROPTESTED {
            if !tests.contains(&format!("fn {test}(")) {
                out.push(format!("{name} has no proptest {test}"));
            }
        }
        out
    }

    /// Reads the Verus file. Returns `None` when the file is absent (a
    /// packaged crate). The caller fails when `CI` is set.
    fn verus_file() -> Option<String> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../verus/policy.rs");
        let text = std::fs::read_to_string(path).ok();
        assert!(
            text.is_some() || std::env::var_os("CI").is_none(),
            "verus/policy.rs is missing in CI"
        );
        text
    }

    /// The part of this file before the test module.
    fn exec_code() -> &'static str {
        let code = include_str!("policy.rs");
        code.split("#[cfg(test)]").next().unwrap_or(code)
    }

    /// Removes `/* ... */` comments.
    fn strip_block_comments(text: &str) -> String {
        let mut out = String::new();
        let mut rest = text;
        while let Some(at) = rest.find("/*") {
            out.push_str(&rest[..at]);
            rest = rest[at..]
                .find("*/")
                .map_or("", |end| &rest[at + end + 2..]);
        }
        out.push_str(rest);
        out
    }

    /// Returns each `pub const NAME: T = V;` line, with no spaces.
    fn constants(text: &str) -> BTreeMap<String, String> {
        text.lines()
            .filter_map(|l| l.trim().strip_prefix("pub const "))
            .filter(|l| !l.contains("fn "))
            .filter_map(|l| {
                let (name, rest) = l.split_once(':')?;
                Some((
                    name.trim().to_owned(),
                    rest.chars().filter(|c| !c.is_whitespace()).collect(),
                ))
            })
            .collect()
    }

    /// Removes each `proof { ... }` block: proof code of the Verus body.
    fn strip_proof_blocks(text: &str) -> String {
        let mut out = String::new();
        let mut rest = text;
        while let Some(at) = rest.find("proof {") {
            out.push_str(&rest[..at]);
            let block = &rest[at + "proof ".len()..];
            let mut depth = 0_i32;
            let mut end = block.len();
            for (i, c) in block.char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = i + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            rest = &block[end..];
        }
        out.push_str(rest);
        out
    }

    /// Returns the body of `pub fn name` with no comments, no proof blocks
    /// and no white space. In the spec, the body starts at a line with only
    /// `{` (after `ensures`).
    fn body(text: &str, name: &str, spec: bool) -> Option<String> {
        let at = text
            .find(&format!("pub fn {name}("))
            .or_else(|| text.find(&format!("pub const fn {name}(")))?;
        let open = if spec { "\n{\n" } else { " {\n" };
        let start = at + text[at..].find(open)? + open.len();
        let mut depth = 1_i32;
        let mut end = text.len();
        for (i, c) in text[start..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = start + i;
                        break;
                    }
                }
                _ => {}
            }
        }
        Some(
            strip_proof_blocks(&strip_block_comments(&text[start..end]))
                .lines()
                .map(|l| l.split("//").next().unwrap_or(""))
                .collect::<String>()
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect(),
        )
    }

    #[test]
    fn verus_bodies_match() {
        let Some(spec) = verus_file() else { return };
        let code = exec_code();
        for name in BODY_MATCHED {
            let spec_body = body(&spec, name, true);
            assert!(spec_body.is_some(), "no spec body for {name}");
            assert_eq!(spec_body, body(code, name, false), "{name}");
        }
        // The constants that the proofs use must have the same values.
        let spec_consts = constants(&spec);
        let code_consts = constants(code);
        assert_eq!(spec_consts.len(), 7, "{spec_consts:?}");
        assert_eq!(spec_consts, code_consts);
    }

    #[test]
    fn every_const_fn_is_proved_or_listed() {
        // Each function is in exactly one of BODY_MATCHED, PROOF_PROPTESTED
        // and PROPTEST_ONLY. A proved function has a `pub fn` line in the
        // spec. `each_fn_is_in_exactly_one_list` tests the check itself.
        let Some(spec) = verus_file() else { return };
        assert_eq!(
            coverage_violations(exec_code(), &spec),
            Vec::<String>::new()
        );
    }

    #[test]
    fn the_spec_has_no_proof_escape() {
        let Some(spec) = verus_file() else { return };
        assert_eq!(proof_escapes(&spec), Vec::<String>::new());
        // The check sees an escape.
        let with =
            |escape: &str| spec.replacen("pub fn decide(", &format!("{escape}\npub fn decide("), 1);
        for escape in [
            "#[verifier::external_body]",
            "#[verifier::external]",
            "#[verifier(external_body)]",
            "#[verifier::exec_allows_no_decreases_clause]",
        ] {
            assert_ne!(
                proof_escapes(&with(escape)),
                Vec::<String>::new(),
                "{escape}"
            );
        }
        let body = spec.replacen("{\n", "{\n    assume(false);\n", 1);
        assert_ne!(proof_escapes(&body), Vec::<String>::new());
        let admit = spec.replacen("{\n", "{\n    proof { admit(); }\n", 1);
        assert_ne!(proof_escapes(&admit), Vec::<String>::new());
        // A word in a comment is no escape.
        assert_eq!(
            proof_escapes("// we do not assume(x) here\n"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn each_fn_is_in_exactly_one_list() {
        let Some(spec) = verus_file() else { return };
        assert_eq!(
            coverage_violations(exec_code(), &spec),
            Vec::<String>::new()
        );
        assert_eq!(all_fns(exec_code()).len(), 14, "{:?}", all_fns(exec_code()));
        // A new function in both files, in no list: a violation.
        let exec = format!(
            "{}\npub const fn h(x: u32) -> u32 {{\n    x\n}}\n",
            exec_code()
        );
        let spec_h = format!("{spec}\npub fn h(x: u32) -> (r: u32)\n{{\n    x\n}}\n");
        assert_ne!(coverage_violations(&exec, &spec_h), Vec::<String>::new());
        // A private helper is seen too.
        let private = format!("{}\nconst fn helper() -> u32 {{\n    1\n}}\n", exec_code());
        assert_ne!(coverage_violations(&private, &spec), Vec::<String>::new());
        assert_eq!(all_fns("pub(crate) fn a() {}\nfn b() {}\n"), ["a", "b"]);
        // A name in a doc comment of the spec is not a proof.
        let gone = spec.replace("pub fn decide(", "pub fn decided(");
        let noted = format!("{gone}\n/// pub fn decide(\n");
        assert_ne!(
            coverage_violations(exec_code(), &noted),
            Vec::<String>::new()
        );
    }

    #[test]
    fn the_drift_check_sees_changes() {
        let code =
            "pub const A: u64 = 1;\npub fn f(x: u64) -> u64 {\n    x /* note */ + 1 // tail\n}\n";
        assert_eq!(body(code, "f", false).as_deref(), Some("x+1"));
        assert_eq!(constants(code).get("A").map(String::as_str), Some("u64=1;"));
        assert_eq!(strip_block_comments("a/*b*/c/*d"), "ac");
        // An edited copy of a real body fails the match.
        let spec = "pub fn g(x: u64) -> (r: u64)\n    ensures r == x,\n{\n    x\n}\n";
        let edited = "pub const fn g(x: u64) -> u64 {\n    x + 0\n}\n";
        let same = "pub const fn g(x: u64) -> u64 {\n    x\n}\n";
        assert_ne!(body(spec, "g", true), body(edited, "g", false));
        assert_eq!(body(spec, "g", true), body(same, "g", false));
        // An edited constant fails the match.
        assert_ne!(
            constants("pub const A: u64 = 1;"),
            constants("pub const A: u64 = 2;")
        );
        assert_eq!(all_fns(same), vec!["g".to_owned()]);
        // A proof block of the Verus body is not part of the compare.
        let proof = "pub fn k(x: u64) -> (r: u64)\n    ensures r == x,\n{\n    if x > 0 {\n        proof { lemma(x); }\n        x\n    } else {\n        x\n    }\n}\n";
        let exec = "pub const fn k(x: u64) -> u64 {\n    if x > 0 {\n        x\n    } else {\n        x\n    }\n}\n";
        assert_eq!(body(proof, "k", true), body(exec, "k", false));
        assert_eq!(strip_proof_blocks("a proof { b { c } } d"), "a  d");
    }

    #[test]
    fn constants_match_the_plan() {
        assert_eq!(MAX_ATTEMPTS, 5);
        assert_eq!(MAX_BACKOFF_MS, 30_000);
        assert_eq!(MAX_LOOKUPS, 3);
        assert_eq!(DEFAULT_MAX_PAGES, 1_000);
        assert_eq!(JITTER_MIN_PERMILLE, 700);
        assert_eq!(JITTER_MAX_PERMILLE, 1_300);
        assert_eq!(MAX_FUTURE_SKEW_MS, 5 * 60 * 1_000);
    }

    #[test]
    fn classify_examples() {
        assert_eq!(classify_status(200), Outcome::Success);
        assert_eq!(classify_status(204), Outcome::Success);
        assert_eq!(classify_status(429), Outcome::RateLimited);
        assert_eq!(classify_status(302), Outcome::Redirect);
        assert_eq!(classify_status(408), Outcome::ServerError);
        assert_eq!(classify_status(425), Outcome::ServerError);
        assert_eq!(classify_status(503), Outcome::ServerError);
        assert_eq!(classify_status(600), Outcome::ServerError);
        assert_eq!(classify_status(404), Outcome::ClientError);
        assert_eq!(classify_status(100), Outcome::ClientError);
        assert_eq!(classify_status(0), Outcome::ClientError);
    }

    #[test]
    fn decide_examples() {
        use Decision::{Confirm, Done, Retry, Stop, Unknown};
        use OpClass::{CheckedWrite, IdempotentWrite, MarkedWrite, Read, UnsafeWrite};
        assert_eq!(decide(Read, Outcome::Success, 1, 5), Done);
        assert_eq!(decide(Read, Outcome::ServerError, 1, 5), Retry);
        assert_eq!(decide(Read, Outcome::ServerError, 5, 5), Stop);
        assert_eq!(decide(Read, Outcome::ServerError, 5, 50), Stop);
        assert_eq!(decide(IdempotentWrite, Outcome::MaybeSent, 1, 5), Retry);
        assert_eq!(decide(CheckedWrite, Outcome::ServerError, 1, 5), Retry);
        assert_eq!(decide(UnsafeWrite, Outcome::ServerError, 1, 5), Unknown);
        assert_eq!(decide(UnsafeWrite, Outcome::MaybeSent, 1, 5), Unknown);
        assert_eq!(decide(UnsafeWrite, Outcome::RateLimited, 1, 5), Stop);
        assert_eq!(decide(UnsafeWrite, Outcome::NotSent, 1, 5), Retry);
        assert_eq!(decide(MarkedWrite, Outcome::ServerError, 1, 5), Confirm);
        assert_eq!(decide(MarkedWrite, Outcome::MaybeSent, 5, 5), Confirm);
        assert_eq!(decide(MarkedWrite, Outcome::RateLimited, 1, 5), Retry);
        assert_eq!(decide(MarkedWrite, Outcome::NotSent, 2, 2), Stop);
        assert_eq!(decide(Read, Outcome::ClientError, 1, 5), Stop);
        assert_eq!(decide(Read, Outcome::Redirect, 1, 5), Stop);
        assert_eq!(decide(Read, Outcome::NotSent, 0, 0), Stop);
        // Each resendable class retries a 429 and a "not sent" (f).
        for class in [Read, IdempotentWrite, CheckedWrite] {
            assert_eq!(
                decide(class, Outcome::RateLimited, 1, 5),
                Retry,
                "{class:?}"
            );
            assert_eq!(decide(class, Outcome::NotSent, 1, 5), Retry, "{class:?}");
            assert_eq!(decide(class, Outcome::MaybeSent, 4, 5), Retry, "{class:?}");
            assert_eq!(decide(class, Outcome::RateLimited, 5, 5), Stop, "{class:?}");
        }
    }

    #[test]
    fn backoff_doubles_and_caps() {
        assert_eq!(backoff_ms(100, 0, 1_000), 100);
        assert_eq!(backoff_ms(100, 1, 1_000), 100);
        assert_eq!(backoff_ms(100, 2, 1_000), 200);
        assert_eq!(backoff_ms(100, 4, 1_000), 800);
        assert_eq!(backoff_ms(100, 5, 1_000), 1_000);
        assert_eq!(backoff_ms(5_000, 1, 1_000), 1_000);
        assert_eq!(backoff_ms(0, 9, 1_000), 0);
        assert_eq!(backoff_ms(u64::MAX, 60, u64::MAX), u64::MAX);
        assert_eq!(backoff_ms(3, u32::MAX, u64::MAX), u64::MAX);
    }

    #[test]
    fn wait_and_jitter_examples() {
        assert_eq!(retry_wait_ms(1, 2_000, 30_000, None), Some(2_000));
        assert_eq!(retry_wait_ms(3, 2_000, 30_000, None), Some(8_000));
        assert_eq!(retry_wait_ms(1, 2_000, 30_000, Some(5_000)), Some(5_000));
        assert_eq!(retry_wait_ms(3, 2_000, 30_000, Some(5_000)), Some(8_000));
        assert_eq!(retry_wait_ms(1, 2_000, 30_000, Some(30_001)), None);
        assert_eq!(apply_jitter(1_000, 1_000, 0, 30_000), 1_000);
        assert_eq!(apply_jitter(1_000, 0, 0, 30_000), 700);
        assert_eq!(apply_jitter(1_000, 9_999, 0, 30_000), 1_300);
        assert_eq!(apply_jitter(1_000, 700, 900, 30_000), 900);
        assert_eq!(apply_jitter(30_000, 1_300, 0, 30_000), 30_000);
        assert_eq!(apply_jitter(10, 1_300, 50, 20), 50);
        assert_eq!(apply_jitter(u64::MAX, 1_300, 0, u64::MAX), u64::MAX);
        assert_eq!(scale_permille(2_000, 1_150), 2_300);
        assert_eq!(scale_permille(u64::MAX, 700), u64::MAX);
        assert_eq!(scale_permille(10, 5_000), 10);
        // The edge of the multiply branch.
        let edge = u64::MAX / 1_300;
        assert_eq!(scale_permille(edge, 1_000), edge);
        let exact = u128::from(edge) * 1_300 / 1_000;
        assert_eq!(u128::from(scale_permille(edge, 1_300)), exact);
        assert_eq!(scale_permille(edge + 1, 1_300), edge + 1);
        assert_eq!(scale_permille(edge + 1, 700), edge + 1);
        assert_eq!(scale_permille(1_000, 1_301), 1_000);
        assert_eq!(scale_permille(1_000, 1_300), 1_300);
    }

    #[test]
    fn pager_and_webhook_examples() {
        assert_eq!(token_page_step(1, 10, false, true, false), PageStep::Fetch);
        assert_eq!(token_page_step(1, 10, true, true, false), PageStep::Done);
        assert_eq!(token_page_step(1, 10, false, false, false), PageStep::Done);
        assert_eq!(token_page_step(1, 10, false, true, true), PageStep::Stalled);
        assert_eq!(token_page_step(10, 10, false, true, false), PageStep::Limit);
        assert_eq!(offset_next(0, 50, 120), Some(50));
        assert_eq!(offset_next(100, 20, 120), None);
        assert_eq!(offset_next(0, 0, 120), None);
        assert_eq!(offset_next(u64::MAX, 1, u64::MAX), None);
        assert_eq!(freshness(1_000, 1_000, 0, 0), Freshness::Fresh);
        assert_eq!(freshness(1_000, 2_001, 1_000, 0), Freshness::Stale);
        assert_eq!(freshness(1_000, 2_000, 1_000, 0), Freshness::Fresh);
        assert_eq!(freshness(2_001, 1_000, 0, 1_000), Freshness::Future);
        assert_eq!(freshness(2_000, 1_000, 0, 1_000), Freshness::Fresh);
        assert_eq!(webhook_status(Verdict::Accepted), 200);
        assert_eq!(webhook_status(Verdict::Ignored), 200);
        assert_eq!(webhook_status(Verdict::HandlerFailed), 503);
        assert_eq!(webhook_status(Verdict::TimedOut), 503);
        assert!(host_may_send(Previous::First, false));
        assert!(host_may_send(Previous::NotSent, false));
        assert!(!host_may_send(Previous::MaybeSent, false));
        assert!(!host_may_send(Previous::Unknown, false));
        assert!(host_may_send(Previous::Unknown, true));
    }

    #[test]
    fn signature_and_quote_examples() {
        let good = "sha256=a4771c39fbe90f317c7824e83ddef3caae9cb3d976c214ace1f2937e133263c9";
        assert!(signature_shape_ok(good.as_bytes()));
        assert!(!signature_shape_ok(good.to_uppercase().as_bytes()));
        assert!(!signature_shape_ok(
            good.replace("sha256", "sha1").as_bytes()
        ));
        assert!(!signature_shape_ok(&good.as_bytes()[..70]));
        assert!(!signature_shape_ok(b""));
        // The right prefix and length, with a hex digit that is not lower case.
        let upper = format!("sha256={}", "A".repeat(64));
        assert!(!signature_shape_ok(upper.as_bytes()));
        let one = format!("sha256={}F", &good[7..70]);
        assert!(!signature_shape_ok(one.as_bytes()));
        assert_eq!(jql_quote("abc"), "\"abc\"");
        assert_eq!(jql_quote("a\"b"), "\"a\\\"b\"");
        assert_eq!(jql_quote("a\\b"), "\"a\\\\b\"");
        assert!(is_lower_hex(b'0') && is_lower_hex(b'9') && is_lower_hex(b'a'));
        assert!(is_lower_hex(b'f') && !is_lower_hex(b'g') && !is_lower_hex(b'A'));
        assert!(!is_lower_hex(b'/') && !is_lower_hex(b':') && !is_lower_hex(b'`'));
    }

    fn any_class() -> impl Strategy<Value = OpClass> {
        prop_oneof![
            Just(OpClass::Read),
            Just(OpClass::IdempotentWrite),
            Just(OpClass::CheckedWrite),
            Just(OpClass::MarkedWrite),
            Just(OpClass::UnsafeWrite),
        ]
    }

    fn any_outcome() -> impl Strategy<Value = Outcome> {
        prop_oneof![
            Just(Outcome::Success),
            Just(Outcome::RateLimited),
            Just(Outcome::ServerError),
            Just(Outcome::Redirect),
            Just(Outcome::ClientError),
            Just(Outcome::NotSent),
            Just(Outcome::MaybeSent),
        ]
    }

    fn any_previous() -> impl Strategy<Value = Previous> {
        prop_oneof![
            Just(Previous::First),
            Just(Previous::NotSent),
            Just(Previous::MaybeSent),
            Just(Previous::Unknown),
        ]
    }

    fn any_verdict() -> impl Strategy<Value = Verdict> {
        prop_oneof![
            Just(Verdict::Accepted),
            Just(Verdict::Ignored),
            Just(Verdict::HandlerFailed),
            Just(Verdict::TimedOut),
        ]
    }

    const fn maybe_sent(o: Outcome) -> bool {
        matches!(o, Outcome::ServerError | Outcome::MaybeSent)
    }

    const fn transient(o: Outcome) -> bool {
        matches!(
            o,
            Outcome::RateLimited | Outcome::ServerError | Outcome::NotSent | Outcome::MaybeSent
        )
    }

    const fn resendable(c: OpClass) -> bool {
        matches!(
            c,
            OpClass::Read | OpClass::IdempotentWrite | OpClass::CheckedWrite
        )
    }

    fn cap(max_attempts: u32) -> u32 {
        max_attempts.min(MAX_ATTEMPTS)
    }

    /// Closed form of the backoff, with 128-bit math (san-mateo copy).
    fn closed_form(initial: u64, attempt: u32, cap: u64) -> u64 {
        let exp = attempt.max(1) - 1;
        if exp >= 64 {
            return if initial == 0 { 0 } else { cap };
        }
        let raw = u128::from(initial) << exp;
        u64::try_from(raw.min(u128::from(cap))).unwrap_or(cap)
    }

    /// Reads back a `jql_quote` result. `None` when the text is not one literal.
    fn unquote(quoted: &str) -> Option<String> {
        let inner = quoted.strip_prefix('"')?.strip_suffix('"')?;
        let mut out = String::new();
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => out.push(chars.next()?),
                '"' => return None,
                other => out.push(other),
            }
        }
        Some(out)
    }

    /// The rules (e) to (h) of `decide` for one input.
    fn decide_attempt_rules(
        class: OpClass,
        outcome: Outcome,
        attempt: u32,
        max: u32,
    ) -> Result<(), TestCaseError> {
        let r = decide(class, outcome, attempt, max);
        // (e).
        if r == Decision::Retry {
            prop_assert!(attempt < cap(max));
        }
        // (f).
        if resendable(class) && transient(outcome) {
            prop_assert_eq!(r == Decision::Retry, attempt < cap(max));
            prop_assert!(matches!(r, Decision::Retry | Decision::Stop));
        }
        // (g).
        if class == OpClass::MarkedWrite
            && matches!(outcome, Outcome::RateLimited | Outcome::NotSent)
        {
            prop_assert_eq!(r == Decision::Retry, attempt < cap(max));
            prop_assert!(matches!(r, Decision::Retry | Decision::Stop));
        }
        if class == OpClass::UnsafeWrite && outcome == Outcome::NotSent {
            prop_assert_eq!(r == Decision::Retry, attempt < cap(max));
        }
        Ok(())
    }

    proptest! {
        #[test]
        fn spec_as_test_classify_status(status: u16) {
            let r = classify_status(status);
            let expected = match status {
                200..=299 => Outcome::Success,
                429 => Outcome::RateLimited,
                300..=399 => Outcome::Redirect,
                408 | 425 | 500.. => Outcome::ServerError,
                _ => Outcome::ClientError,
            };
            prop_assert_eq!(r, expected);
        }

        #[test]
        fn spec_as_test_decide_done_and_stop(class in any_class(), outcome in any_outcome(), attempt in 0_u32..10, max in 0_u32..10) {
            let r = decide(class, outcome, attempt, max);
            // (a) and (b).
            prop_assert_eq!(r == Decision::Done, outcome == Outcome::Success);
            if matches!(outcome, Outcome::ClientError | Outcome::Redirect) {
                prop_assert_eq!(r, Decision::Stop);
            }
            // (d).
            if class == OpClass::UnsafeWrite && outcome == Outcome::RateLimited {
                prop_assert_eq!(r, Decision::Stop);
            }
        }

        #[test]
        fn spec_as_test_decide_no_resend_after_maybe_sent(class in any_class(), outcome in any_outcome(), attempt: u32, max: u32) {
            let r = decide(class, outcome, attempt, max);
            if matches!(class, OpClass::UnsafeWrite | OpClass::MarkedWrite) && maybe_sent(outcome) {
                prop_assert!(matches!(r, Decision::Unknown | Decision::Confirm));
                prop_assert_ne!(r, Decision::Retry);
            }
            if class == OpClass::UnsafeWrite && maybe_sent(outcome) {
                prop_assert_eq!(r, Decision::Unknown);
            }
            if class == OpClass::MarkedWrite && maybe_sent(outcome) {
                prop_assert_eq!(r, Decision::Confirm);
            }
            if r == Decision::Confirm {
                prop_assert!(class == OpClass::MarkedWrite && maybe_sent(outcome));
            }
            if r == Decision::Unknown {
                prop_assert!(class == OpClass::UnsafeWrite && maybe_sent(outcome));
            }
        }

        #[test]
        fn spec_as_test_decide_attempt_bound_small(class in any_class(), outcome in any_outcome(), attempt in 0_u32..8, max in 0_u32..8) {
            // Small values reach both sides of `attempt < cap(max)`.
            decide_attempt_rules(class, outcome, attempt, max)?;
        }

        #[test]
        fn spec_as_test_decide_no_resend_after_maybe_sent_small(class in any_class(), outcome in any_outcome(), attempt in 0_u32..8, max in 0_u32..8) {
            let r = decide(class, outcome, attempt, max);
            if matches!(class, OpClass::UnsafeWrite | OpClass::MarkedWrite) && maybe_sent(outcome) {
                prop_assert!(matches!(r, Decision::Unknown | Decision::Confirm));
            }
            if r == Decision::Retry {
                prop_assert!(!(matches!(class, OpClass::UnsafeWrite | OpClass::MarkedWrite) && maybe_sent(outcome)));
            }
        }

        #[test]
        fn spec_as_test_decide_attempt_bound(class in any_class(), outcome in any_outcome(), attempt: u32, max: u32) {
            decide_attempt_rules(class, outcome, attempt, max)?;
        }

        #[test]
        fn spec_as_test_attempt_loop_is_bounded(class in any_class(), outcome in any_outcome(), max: u32) {
            // A loop that adds 1 to `attempt` on each `Retry` sends at most
            // `MAX_ATTEMPTS` times.
            let mut attempt = 1_u32;
            while decide(class, outcome, attempt, max) == Decision::Retry {
                attempt += 1;
                prop_assert!(attempt <= MAX_ATTEMPTS);
            }
        }

        #[test]
        fn spec_as_test_backoff(initial in 0_u64..1_000_000, attempt in 0_u32..80, cap in 0_u64..10_000_000) {
            let b = backoff_ms(initial, attempt, cap);
            prop_assert!(b <= cap);
            prop_assert_eq!(b, closed_form(initial, attempt, cap));
            prop_assert!(b <= backoff_ms(initial, attempt.saturating_add(1), cap));
        }

        #[test]
        fn backoff_matches_san_mateo(initial: u64, attempt in 0_u32..200, cap: u64) {
            prop_assert_eq!(
                backoff_ms(initial, attempt, cap),
                san_mateo::policy::backoff_ms(initial, attempt, cap)
            );
        }

        #[test]
        fn spec_as_test_retry_wait(attempt in 0_u32..40, base in 0_u64..100_000, cap in 0_u64..100_000, after in proptest::option::of(0_u64..200_000)) {
            let r = retry_wait_ms(attempt, base, cap, after);
            prop_assert_eq!(r.is_none(), after.is_some_and(|a| a > cap));
            if let Some(w) = r {
                prop_assert!(w <= cap);
                prop_assert!(w >= after.unwrap_or(0));
                prop_assert!(w >= backoff_ms(base, attempt, cap));
            }
            if after.is_none() {
                prop_assert_eq!(r, Some(backoff_ms(base, attempt, cap)));
            }
        }

        #[test]
        fn spec_as_test_scale_permille_in_range(value in 0..=u64::MAX / 1_300, permille in 0_u32..=1_300) {
            // The multiply branch: the uniform `u64` strategy almost never
            // reaches it.
            let exact = u128::from(value) * u128::from(permille) / 1_000;
            prop_assert_eq!(u128::from(scale_permille(value, permille)), exact);
        }

        #[test]
        fn spec_as_test_scale_permille_out_of_range(value in u64::MAX / 1_300 + 1.., permille: u32) {
            prop_assert_eq!(scale_permille(value, permille), value);
        }

        #[test]
        fn spec_as_test_jitter_bounds_small(wait in 0_u64..100_000, permille in 0_u32..2_000, floor in 0_u64..50_000, cap in 0_u64..100_000) {
            // Real waits: the scaled branch of `scale_permille`.
            let r = apply_jitter(wait, permille, floor, cap);
            let p = u128::from(permille.clamp(JITTER_MIN_PERMILLE, JITTER_MAX_PERMILLE));
            let scaled = u128::from(wait) * p / 1_000;
            let high = u128::from(cap.max(floor));
            prop_assert_eq!(u128::from(r), scaled.clamp(u128::from(floor), high));
        }

        #[test]
        fn spec_as_test_scale_permille(value: u64, permille: u32) {
            let r = scale_permille(value, permille);
            if value <= u64::MAX / 1_300 && permille <= 1_300 {
                let exact = u128::from(value) * u128::from(permille) / 1_000;
                prop_assert_eq!(u128::from(r), exact);
            } else {
                prop_assert_eq!(r, value);
            }
        }

        #[test]
        fn spec_as_test_jitter_bounds(wait: u64, permille: u32, floor: u64, cap: u64) {
            let r = apply_jitter(wait, permille, floor, cap);
            prop_assert!(floor <= r);
            prop_assert!(r <= cap.max(floor));
            let p = permille.clamp(JITTER_MIN_PERMILLE, JITTER_MAX_PERMILLE);
            prop_assert_eq!(r, scale_permille(wait, p).clamp(floor, cap.max(floor)));
        }

        #[test]
        fn spec_as_test_host_may_send(previous in any_previous(), opt_in: bool) {
            let r = host_may_send(previous, opt_in);
            prop_assert_eq!(r, matches!(previous, Previous::First | Previous::NotSent) || opt_in);
            if matches!(previous, Previous::MaybeSent | Previous::Unknown) && !opt_in {
                prop_assert!(!r);
            }
        }

        #[test]
        fn spec_as_test_token_page_step(pages: u32, max: u32, is_last: bool, has_token: bool, repeated: bool) {
            let r = token_page_step(pages, max, is_last, has_token, repeated);
            let fetch = pages < max && !is_last && has_token && !repeated;
            prop_assert_eq!(r == PageStep::Fetch, fetch);
            prop_assert_eq!(r == PageStep::Done, is_last || !has_token);
            prop_assert_eq!(r == PageStep::Stalled, !is_last && has_token && repeated);
            prop_assert_eq!(r == PageStep::Limit, !is_last && has_token && !repeated && pages >= max);
        }

        #[test]
        fn spec_as_test_token_pager_terminates(start in 0_u32..50, max in 0_u32..100, inputs in proptest::collection::vec(any::<(bool, bool, bool)>(), 0..200)) {
            // Each step reads the next input. A step that is not `Fetch` ends the loop.
            let mut steps = 0_u32;
            for (is_last, has_token, repeated) in inputs {
                if token_page_step(start + steps, max, is_last, has_token, repeated) != PageStep::Fetch {
                    break;
                }
                steps += 1;
            }
            prop_assert!(steps == 0 || start + steps <= max);
            // Always `Fetch` (the worst case) stops at `max_pages`.
            let mut pages = start;
            while token_page_step(pages, max, false, true, false) == PageStep::Fetch {
                pages += 1;
            }
            prop_assert_eq!(pages, start.max(max));
        }

        #[test]
        fn spec_as_test_offset_next(start: u64, returned: u64, total: u64) {
            let r = offset_next(start, returned, total);
            if let Some(n) = r {
                prop_assert!(n > start && n < total);
                prop_assert_eq!(n, start + returned);
            }
            let end = returned == 0 || u128::from(start) + u128::from(returned) >= u128::from(total);
            prop_assert_eq!(r.is_none(), end);
        }

        #[test]
        fn spec_as_test_freshness(event: u64, now: u64, age: u64, skew: u64) {
            let r = freshness(event, now, age, skew);
            let stale = now > event && now - event > age;
            let future = event > now && event - now > skew;
            prop_assert_eq!(r == Freshness::Stale, stale);
            prop_assert_eq!(r == Freshness::Future, future);
            prop_assert_eq!(r == Freshness::Fresh, !stale && !future);
        }

        #[test]
        fn spec_as_test_webhook_status(verdict in any_verdict()) {
            let r = webhook_status(verdict);
            prop_assert!(r == 200 || r == 503);
            prop_assert_eq!(r == 503, matches!(verdict, Verdict::HandlerFailed | Verdict::TimedOut));
        }

        #[test]
        fn spec_as_test_is_lower_hex(b: u8) {
            prop_assert_eq!(is_lower_hex(b), b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        }

        #[test]
        fn spec_as_test_signature_shape_mixed_case(hex in "[0-9a-zA-Z]{64}") {
            // The right prefix and length: only the hex rule decides.
            let header = format!("sha256={hex}");
            let expected = hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
            prop_assert_eq!(signature_shape_ok(header.as_bytes()), expected);
        }

        #[test]
        fn spec_as_test_signature_shape_one_bad_byte(hex in "[0-9a-f]{64}", at in 0_usize..64, bad in "[g-zA-Z:/`]") {
            let mut text = hex.into_bytes();
            text[at] = bad.as_bytes()[0];
            let header = [b"sha256=".as_slice(), &text].concat();
            prop_assert!(!signature_shape_ok(&header));
        }

        #[test]
        fn spec_as_test_signature_shape(header in proptest::collection::vec(any::<u8>(), 0..80), hex in "[0-9a-f]{64}") {
            let expected = header.len() == 71
                && header.starts_with(b"sha256=")
                && header[7..].iter().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b));
            prop_assert_eq!(signature_shape_ok(&header), expected);
            let good = format!("sha256={hex}");
            prop_assert!(signature_shape_ok(good.as_bytes()));
        }

        #[test]
        fn spec_as_test_jql_quote_round_trip(value in any::<String>()) {
            let quoted = jql_quote(&value);
            prop_assert_eq!(unquote(&quoted), Some(value));
            // Every `"` inside has an odd run of `\` before it.
            let inner = &quoted[1..quoted.len() - 1];
            let bytes = inner.as_bytes();
            for (i, b) in bytes.iter().enumerate() {
                if *b == b'"' {
                    let run = bytes[..i].iter().rev().take_while(|c| **c == b'\\').count();
                    prop_assert!(run % 2 == 1, "{quoted}");
                }
            }
        }
    }
}
