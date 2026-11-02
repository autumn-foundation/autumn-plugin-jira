//! Verus spec and proof for `crates/autumn-jira/src/policy.rs`.
//!
//! Keep this file in step with `src/policy.rs`. The test
//! `policy::verus_bodies_match` compares the function bodies and the
//! constants. The test `policy::every_const_fn_is_proved_or_listed` makes
//! sure that each public function of `src/policy.rs` is in this file.
//! Run: `verus verus/policy.rs`.
//!
//! The proofs use no `assume` and no `admit`.

use vstd::prelude::*;

verus! {

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

pub open spec fn spec_min(a: int, b: int) -> int {
    if a <= b {
        a
    } else {
        b
    }
}

pub open spec fn spec_max(a: int, b: int) -> int {
    if a >= b {
        a
    } else {
        b
    }
}

pub open spec fn spec_clamp(x: int, low: int, high: int) -> int {
    if x < low {
        low
    } else if x > high {
        high
    } else {
        x
    }
}

// ------------------------------------------------------------ classify

/// The class of one attempt.
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
pub fn classify_status(status: u16) -> (r: Outcome)
    ensures
        (200 <= status <= 299) ==> r is Success,
        status == 429 ==> r is RateLimited,
        (300 <= status <= 399) ==> r is Redirect,
        (status == 408 || status == 425 || status >= 500) ==> r is ServerError,
        !(200 <= status <= 399 || status == 408 || status == 425 || status == 429 || status
            >= 500) ==> r is ClientError,
        !(r is NotSent),
        !(r is MaybeSent),
{
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

// ------------------------------------------------------------ decide

/// The retry class of one operation.
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

pub open spec fn spec_cap_attempts(max_attempts: u32) -> int {
    spec_min(max_attempts as int, MAX_ATTEMPTS as int)
}

/// Spec: the request can have reached Jira, and the reply says nothing safe.
pub open spec fn spec_maybe_sent(outcome: Outcome) -> bool {
    outcome is ServerError || outcome is MaybeSent
}

/// Spec: an outcome that a retry can fix.
pub open spec fn spec_transient(outcome: Outcome) -> bool {
    outcome is RateLimited || outcome is ServerError || outcome is NotSent || outcome is MaybeSent
}

/// Spec: a class that is safe to send again after any transient outcome.
pub open spec fn spec_resendable(class: OpClass) -> bool {
    class is Read || class is IdempotentWrite || class is CheckedWrite
}

/// Decides the next step after attempt number `attempt` (1-based).
pub fn decide(class: OpClass, outcome: Outcome, attempt: u32, max_attempts: u32) -> (r: Decision)
    ensures
        // (a) A success is done, and only a success is done.
        outcome is Success ==> r is Done,
        r is Done ==> outcome is Success,
        // (b) A client error or a redirect stops.
        (outcome is ClientError || outcome is Redirect) ==> r is Stop,
        // (c) no_resend_after_maybe_sent: an unsafe or marked write is
        // never sent again after a 5xx or a "maybe sent" network error.
        ((class is UnsafeWrite || class is MarkedWrite) && spec_maybe_sent(outcome)) ==> (r is Unknown
            || r is Confirm),
        (class is UnsafeWrite && spec_maybe_sent(outcome)) ==> r is Unknown,
        (class is MarkedWrite && spec_maybe_sent(outcome)) ==> r is Confirm,
        // (d) An unsafe write does not retry a 429.
        (class is UnsafeWrite && outcome is RateLimited) ==> r is Stop,
        // (e) Bounded attempts, also for a large `max_attempts`.
        r is Retry ==> attempt < spec_cap_attempts(max_attempts),
        // (f) The safe classes retry each transient outcome while attempts are left.
        (spec_resendable(class) && spec_transient(outcome)) ==> (r is Retry <==> attempt
            < spec_cap_attempts(max_attempts)),
        (spec_resendable(class) && spec_transient(outcome)) ==> (r is Retry || r is Stop),
        // (g) A marked write retries a 429 and a "not sent" error while attempts are left.
        (class is MarkedWrite && (outcome is RateLimited || outcome is NotSent)) ==> (r is Retry
            <==> attempt < spec_cap_attempts(max_attempts)),
        (class is MarkedWrite && (outcome is RateLimited || outcome is NotSent)) ==> (r is Retry
            || r is Stop),
        // A request that did not leave the process is safe to send again.
        (class is UnsafeWrite && outcome is NotSent) ==> (r is Retry <==> attempt
            < spec_cap_attempts(max_attempts)),
        // `Confirm` and `Unknown` come only from (c).
        r is Confirm ==> (class is MarkedWrite && spec_maybe_sent(outcome)),
        r is Unknown ==> (class is UnsafeWrite && spec_maybe_sent(outcome)),
{
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

/// One step of the retry loop: a retry at `attempt < max` gives a next
/// attempt that is still `<= MAX_ATTEMPTS`.
proof fn lemma_attempts_bounded(attempt: u32, max_attempts: u32)
    requires
        attempt < spec_cap_attempts(max_attempts),
    ensures
        attempt + 1 <= MAX_ATTEMPTS,
{
}

// ------------------------------------------------------------ backoff
// Copied from the proved san-mateo `verus/policy.rs`.

/// Spec: `initial * 2^(attempt - 1)`, capped at `cap`.
pub open spec fn spec_backoff(initial: int, attempt: int, cap: int) -> int
    decreases attempt,
{
    if attempt <= 1 {
        spec_min(initial, cap)
    } else {
        spec_min(spec_backoff(initial, attempt - 1, cap) * 2, cap)
    }
}

proof fn lemma_backoff_bounds(initial: int, attempt: int, cap: int)
    requires
        initial >= 0,
        cap >= 0,
    ensures
        0 <= spec_backoff(initial, attempt, cap) <= cap,
    decreases attempt,
{
    if attempt > 1 {
        lemma_backoff_bounds(initial, attempt - 1, cap);
    }
}

/// The backoff does not decrease from one attempt to the next.
proof fn lemma_backoff_monotonic(initial: int, attempt: int, cap: int)
    requires
        initial >= 0,
        cap >= 0,
        attempt >= 1,
    ensures
        spec_backoff(initial, attempt, cap) <= spec_backoff(initial, attempt + 1, cap),
{
    lemma_backoff_bounds(initial, attempt, cap);
}

/// Once the backoff is at the cap, it stays at the cap.
proof fn lemma_backoff_stays_at_cap(initial: int, i: int, j: int, cap: int)
    requires
        cap >= 0,
        1 <= i <= j,
        spec_backoff(initial, i, cap) == cap,
    ensures
        spec_backoff(initial, j, cap) == cap,
    decreases j - i,
{
    if j > i {
        lemma_backoff_stays_at_cap(initial, i, j - 1, cap);
    }
}

/// Once the backoff is zero, it stays zero.
proof fn lemma_backoff_stays_at_zero(initial: int, i: int, j: int, cap: int)
    requires
        cap >= 0,
        1 <= i <= j,
        spec_backoff(initial, i, cap) == 0,
    ensures
        spec_backoff(initial, j, cap) == 0,
    decreases j - i,
{
    if j > i {
        lemma_backoff_stays_at_zero(initial, i, j - 1, cap);
    }
}

/// Returns the wait in ms before retry number `attempt` (1-based).
pub fn backoff_ms(initial_ms: u64, attempt: u32, cap_ms: u64) -> (r: u64)
    ensures
        r <= cap_ms,
        r == spec_backoff(initial_ms as int, attempt as int, cap_ms as int),
{
    let mut b: u64 = if initial_ms <= cap_ms {
        initial_ms
    } else {
        cap_ms
    };
    let mut i: u32 = 1;
    while i < attempt
        invariant
            1 <= i,
            attempt == 0 ==> i == 1,
            attempt >= 1 ==> i <= attempt,
            b <= cap_ms,
            b == spec_backoff(initial_ms as int, i as int, cap_ms as int),
        decreases attempt - i,
    {
        if b == cap_ms {
            proof {
                lemma_backoff_stays_at_cap(initial_ms as int, i as int, attempt as int, cap_ms as int);
            }
            return b;
        }
        if b == 0 {
            proof {
                lemma_backoff_stays_at_zero(initial_ms as int, i as int, attempt as int, cap_ms as int);
            }
            return b;
        }
        b = if cap_ms - b <= b {
            cap_ms
        } else {
            b * 2
        };
        i = i + 1;
    }
    assert(attempt == 0 ==> spec_backoff(initial_ms as int, attempt as int, cap_ms as int)
        == spec_backoff(initial_ms as int, 1, cap_ms as int));
    b
}

// ------------------------------------------------------------ waits

/// Spec: the wait that a `Retry-After` header asks for, or zero.
pub open spec fn spec_floor(retry_after_ms: Option<u64>) -> int {
    match retry_after_ms {
        Some(w) => w as int,
        None => 0,
    }
}

/// Returns the wait before retry number `attempt`, or `None` when a
/// `Retry-After` asks for more than the cap.
pub fn retry_wait_ms(
    attempt: u32,
    base_ms: u64,
    cap_ms: u64,
    retry_after_ms: Option<u64>,
) -> (r: Option<u64>)
    ensures
        r is None <==> (retry_after_ms is Some && retry_after_ms->Some_0 > cap_ms),
        r is Some ==> r->Some_0 <= cap_ms,
        r is Some ==> r->Some_0 >= spec_floor(retry_after_ms),
        r is Some ==> r->Some_0 >= spec_backoff(base_ms as int, attempt as int, cap_ms as int),
        (retry_after_ms is None) ==> (r is Some && r->Some_0 == spec_backoff(
            base_ms as int,
            attempt as int,
            cap_ms as int,
        )),
{
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

/// The wait `a * b` fits in a `u64` for `a <= u64::MAX / 1300` and `b <= 1300`.
proof fn lemma_scale_fits(a: int, b: int)
    requires
        0 <= a <= u64::MAX / 1_300,
        0 <= b <= 1_300,
    ensures
        0 <= a * b <= u64::MAX,
{
    assert(a * b <= a * 1_300) by (nonlinear_arith)
        requires
            0 <= a,
            b <= 1_300,
    {
    }
    assert(0 <= a * b) by (nonlinear_arith)
        requires
            0 <= a,
            0 <= b,
    {
    }
}

/// Spec: `value * permille / 1000`. A value that can overflow stays as it is.
pub open spec fn spec_scale(value: int, permille: int) -> int {
    if value <= u64::MAX / 1_300 && permille <= 1_300 {
        value * permille / 1_000
    } else {
        value
    }
}

/// Returns `value * permille / 1000`, with no overflow.
pub fn scale_permille(value: u64, permille: u32) -> (r: u64)
    ensures
        r == spec_scale(value as int, permille as int),
{
    if value <= u64::MAX / 1_300 && permille <= 1_300 {
        proof {
            lemma_scale_fits(value as int, permille as int);
        }
        value * (permille as u64) / 1_000
    } else {
        value
    }
}

/// Spec: the jitter factor, clamped to the jitter range.
pub open spec fn spec_permille(permille: int) -> int {
    spec_clamp(permille, JITTER_MIN_PERMILLE as int, JITTER_MAX_PERMILLE as int)
}

/// Applies a jitter factor to a wait, then clamps it to the floor and the cap.
pub fn apply_jitter(wait_ms: u64, permille: u32, floor_ms: u64, cap_ms: u64) -> (r: u64)
    ensures
        floor_ms <= r,
        r <= spec_max(cap_ms as int, floor_ms as int),
        r == spec_clamp(
            spec_scale(wait_ms as int, spec_permille(permille as int)),
            floor_ms as int,
            spec_max(cap_ms as int, floor_ms as int),
        ),
{
    let p = if permille < JITTER_MIN_PERMILLE {
        JITTER_MIN_PERMILLE
    } else if permille > JITTER_MAX_PERMILLE {
        JITTER_MAX_PERMILLE
    } else {
        permille
    };
    let scaled = scale_permille(wait_ms, p);
    let high = if cap_ms > floor_ms {
        cap_ms
    } else {
        floor_ms
    };
    if scaled < floor_ms {
        floor_ms
    } else if scaled > high {
        high
    } else {
        scaled
    }
}

// ------------------------------------------------------------ durable hosts

/// What a durable host knows about the previous attempt.
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
pub fn host_may_send(previous: Previous, resend_opt_in: bool) -> (r: bool)
    ensures
        r == (previous is First || previous is NotSent || resend_opt_in),
        // A "maybe sent" or unknown previous attempt never leads to a send
        // without the opt-in.
        ((previous is MaybeSent || previous is Unknown) && !resend_opt_in) ==> !r,
{
    match previous {
        Previous::First | Previous::NotSent => true,
        Previous::MaybeSent | Previous::Unknown => resend_opt_in,
    }
}

// ------------------------------------------------------------ pagers

/// What a token pager does after one page.
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

/// Spec: the pager fetches one more page.
pub open spec fn spec_fetches(
    pages_done: int,
    max_pages: int,
    is_last: bool,
    has_token: bool,
    repeated: bool,
) -> bool {
    pages_done < max_pages && !is_last && has_token && !repeated
}

/// Decides the next step of a token pager after `pages_done` pages.
pub fn token_page_step(
    pages_done: u32,
    max_pages: u32,
    is_last: bool,
    has_token: bool,
    repeated: bool,
) -> (r: PageStep)
    ensures
        r is Fetch <==> spec_fetches(pages_done as int, max_pages as int, is_last, has_token, repeated),
        r is Fetch ==> (pages_done < max_pages && !is_last && has_token && !repeated),
        r is Done <==> (is_last || !has_token),
        r is Stalled <==> (!is_last && has_token && repeated),
        r is Limit <==> (!is_last && has_token && !repeated && pages_done >= max_pages),
{
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

/// Spec: step `i` of a pager run that starts at `start` fetches.
pub open spec fn spec_fetch_at(
    start: int,
    max_pages: int,
    inputs: Seq<(bool, bool, bool)>,
    i: int,
) -> bool {
    spec_fetches(start + i, max_pages, inputs[i].0, inputs[i].1, inputs[i].2)
}

/// token_pager_terminates: a loop that adds 1 to `pages_done` on each
/// `Fetch` ends within `max_pages` steps. `inputs[i]` holds `is_last`,
/// `has_token` and `repeated` of step `i`.
proof fn lemma_token_pager_terminates(start: int, max_pages: int, inputs: Seq<(bool, bool, bool)>)
    requires
        0 <= start,
        forall|i: int| 0 <= i < inputs.len() ==> #[trigger] spec_fetch_at(start, max_pages, inputs, i),
    ensures
        inputs.len() == 0 || start + inputs.len() <= max_pages,
{
    if inputs.len() > 0 {
        assert(spec_fetch_at(start, max_pages, inputs, inputs.len() - 1));
    }
}

/// Returns the next `startAt` of an offset pager, or `None` at the end.
pub fn offset_next(start_at: u64, returned: u64, total: u64) -> (r: Option<u64>)
    ensures
        r is Some ==> (r->Some_0 > start_at && r->Some_0 < total),
        r is Some ==> r->Some_0 == start_at + returned,
        r is None <==> (returned == 0 || start_at as int + returned as int >= total as int),
{
    if returned == 0 || start_at > u64::MAX - returned || start_at + returned >= total {
        None
    } else {
        Some(start_at + returned)
    }
}

// ------------------------------------------------------------ webhooks

/// The age class of a webhook delivery.
pub enum Freshness {
    /// Inside the age and skew bounds.
    Fresh,
    /// Older than the largest age.
    Stale,
    /// Too far in the future.
    Future,
}

/// Classifies the age of a webhook timestamp.
pub fn freshness(event_ms: u64, now_ms: u64, max_age_ms: u64, max_skew_ms: u64) -> (r: Freshness)
    ensures
        r is Stale <==> (now_ms > event_ms && now_ms - event_ms > max_age_ms),
        r is Future <==> (event_ms > now_ms && event_ms - now_ms > max_skew_ms),
        r is Fresh <==> (!(now_ms > event_ms && now_ms - event_ms > max_age_ms) && !(event_ms
            > now_ms && event_ms - now_ms > max_skew_ms)),
{
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
pub fn webhook_status(verdict: Verdict) -> (r: u16)
    ensures
        r == 200 || r == 503,
        (verdict is HandlerFailed || verdict is TimedOut) ==> r == 503,
        (verdict is Accepted || verdict is Ignored) ==> r == 200,
{
    match verdict {
        Verdict::Accepted | Verdict::Ignored => 200,
        Verdict::HandlerFailed | Verdict::TimedOut => 503,
    }
}

/// Returns `true` for a lower-case hex digit: `0-9` (48..=57) or `a-f` (97..=102).
pub fn is_lower_hex(b: u8) -> (r: bool)
    ensures
        r == ((48 <= b <= 57) || (97 <= b <= 102)),
{
    (b >= 48 && b <= 57) || (b >= 97 && b <= 102)
}

fn main() {
}

} // verus!
