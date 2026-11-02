//! The timeline of one run, and the check of it.
//!
//! Each line comes from a fact that the run saw: the fakes, the harvest
//! database, the workflow output or an HTTP reply. No line is a constant
//! that the run prints when a step passes.

use std::fmt;

/// The timeline of the happy path (plan 11.3). `check` compares each line
/// exactly.
pub const EXPECTED: [&str; 9] = [
    "[1] camel    alert A-1001 sent twice -> one run incident_flow/A-1001",
    "[2] sanmateo token for acme expired -> refreshed (1 token call); cloudId stored in metadata",
    "[3] escape   san_mateo_proxy GET project OPS expand=lead -> lead acct-lead",
    "[4] jira     create: 503 after commit -> outcome_pending -> attempt 2 found OPS-1 (1 POST)",
    "[5] jira     OPS-1 -> In Progress (moved)",
    "[6] harvest  worker A stopped while waiting; worker B resumed the run",
    "[7] autumn   POST /jira/webhook -> 200; bridge signalled 1 run",
    "[8] harvest  OPS-1 Done (source signal) -> comment (created) -> chat message 201",
    "[9] autumn   GET /incidents/OPS-1 -> OPS-1 Done",
];

/// What one run saw.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    /// The timeline, one line for each step.
    pub lines: Vec<String>,
    /// The chat API got exactly one message, with an `Idempotency-Key`.
    pub one_chat_message_with_a_key: bool,
}

impl Summary {
    /// Adds the line `[n] <tag> <text>`. The tag column is 8 wide.
    pub fn line(&mut self, tag: &str, text: impl fmt::Display) {
        let n = self.lines.len() + 1;
        self.lines.push(format!("[{n}] {tag:<8} {text}"));
    }
}

impl fmt::Display for Summary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for line in &self.lines {
            writeln!(f, "{line}")?;
        }
        Ok(())
    }
}

/// Compares the summary with the expected timeline. The error starts with
/// `check failed:` and names the first line that differs.
///
/// # Errors
///
/// The text of the first difference.
pub fn check(summary: &Summary) -> Result<(), String> {
    for (i, want) in EXPECTED.iter().enumerate() {
        let got = summary.lines.get(i).map_or("(no line)", String::as_str);
        if got != *want {
            return Err(format!(
                "check failed: line {} is {got:?}, not {want:?}",
                i + 1
            ));
        }
    }
    if summary.lines.len() != EXPECTED.len() {
        return Err(format!(
            "check failed: {} lines, not {}",
            summary.lines.len(),
            EXPECTED.len()
        ));
    }
    if !summary.one_chat_message_with_a_key {
        return Err(
            "check failed: the chat API did not get one message with an Idempotency-Key".to_owned(),
        );
    }
    Ok(())
}
