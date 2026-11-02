//! Guard tests (plan section 16). Each guard reads files of the repository
//! as text and checks one rule of `CLAUDE.md` or `docs/plan.md`.
//!
//! The guards use `std` only. Each guard is a pure function over text, so a
//! self-test can feed it a bad sample and see it fail. A guard that cannot
//! fail does not guard (the `the_drift_check_sees_changes` pattern).
//!
//! Most guards read code, not comments: a small lexer marks comments and
//! string literals. Items under `#[cfg(test)]` are test code. The rules for
//! `src/` do not apply to them.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "guard tests: a failure is a test failure"
)]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use autumn_jira::writes::{WRITES, WriteKind};

mod check {
    //! The checks. Each returns one line for each violation.

    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    // --- A small Rust lexer ----------------------------------------------

    /// The kind of one char of a source.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Kind {
        Code,
        Comment,
        Str,
    }

    /// A Rust source with a kind for each char and a mark for test items.
    struct Source {
        chars: Vec<char>,
        kinds: Vec<Kind>,
        test: Vec<bool>,
    }

    const fn is_ident(c: char) -> bool {
        c.is_ascii_alphanumeric() || c == '_'
    }

    impl Source {
        fn new(text: &str) -> Self {
            let chars: Vec<char> = text.chars().collect();
            let kinds = lex(&chars);
            let marks = test_marks(&chars, &kinds);
            Self {
                chars,
                kinds,
                test: marks,
            }
        }

        /// The text with the chars that `keep` refuses, and test items when
        /// `keep_tests` is false, as spaces. New lines stay, so the line
        /// numbers stay.
        fn view(&self, keep: fn(Kind) -> bool, keep_tests: bool) -> String {
            self.chars
                .iter()
                .enumerate()
                .map(|(i, &c)| {
                    let kept = keep(self.kinds[i]) && (keep_tests || !self.test[i]);
                    if c == '\n' || kept { c } else { ' ' }
                })
                .collect()
        }

        /// Code and strings, no comments, no test items.
        fn code(&self) -> String {
            self.view(|k| k != Kind::Comment, false)
        }

        /// Code only: no comments, no string contents, no test items.
        fn bare(&self) -> String {
            self.view(|k| k == Kind::Code, false)
        }

        fn line_of(&self, index: usize) -> usize {
            self.chars[..index.min(self.chars.len())]
                .iter()
                .filter(|c| **c == '\n')
                .count()
                + 1
        }

        /// The string literals outside test items: (line, contents).
        fn literals(&self) -> Vec<(usize, String)> {
            let n = self.chars.len();
            let mut out = Vec::new();
            let mut i = 0;
            while i < n {
                if self.kinds[i] != Kind::Str || self.chars[i] == '\'' {
                    i += 1;
                    continue;
                }
                let start = i;
                while i < n && self.kinds[i] == Kind::Str {
                    i += 1;
                }
                if self.test[start] {
                    continue;
                }
                let raw: String = self.chars[start..i].iter().collect();
                if let (Some(a), Some(b)) = (raw.find('"'), raw.rfind('"'))
                    && b > a
                {
                    out.push((self.line_of(start), raw[a + 1..b].to_owned()));
                }
            }
            out
        }

        /// Each attribute `#[..]` or `#![..]` in code: (line, the text
        /// inside the brackets, with no comments). It reads attributes that
        /// rustfmt splits over many lines.
        fn attributes(&self) -> Vec<(usize, String)> {
            let n = self.chars.len();
            let code = |i: usize| self.kinds[i] == Kind::Code;
            let mut out = Vec::new();
            let mut i = 0;
            while i < n {
                if !(code(i) && self.chars[i] == '#') {
                    i += 1;
                    continue;
                }
                let mut open = i + 1;
                if self.chars.get(open) == Some(&'!') {
                    open += 1;
                }
                if self.chars.get(open) != Some(&'[') || !code(open) {
                    i += 1;
                    continue;
                }
                let mut depth = 0;
                let mut close = open;
                while close < n {
                    if code(close) {
                        match self.chars[close] {
                            '[' => depth += 1,
                            ']' => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    close += 1;
                }
                let text: String = (open + 1..close.min(n))
                    .map(|k| {
                        if self.kinds[k] == Kind::Comment {
                            ' '
                        } else {
                            self.chars[k]
                        }
                    })
                    .collect();
                out.push((self.line_of(i), text));
                i = close;
            }
            out
        }
    }

    /// Marks each char as code, comment or string literal. It knows line
    /// and nested block comments, escapes, raw strings, and char literals
    /// as against lifetimes.
    fn lex(chars: &[char]) -> Vec<Kind> {
        let n = chars.len();
        let at = |i: usize| chars.get(i).copied().unwrap_or('\0');
        let mut kinds = vec![Kind::Code; n];
        let mut i = 0;
        while i < n {
            let c = chars[i];
            let prev = if i == 0 { ' ' } else { chars[i - 1] };
            let end = if c == '/' && at(i + 1) == '/' {
                let end = (i..n).find(|&j| chars[j] == '\n').unwrap_or(n);
                kinds[i..end].fill(Kind::Comment);
                end
            } else if c == '/' && at(i + 1) == '*' {
                let mut depth = 0;
                let mut j = i;
                while j < n {
                    if chars[j] == '/' && at(j + 1) == '*' {
                        depth += 1;
                        j += 2;
                    } else if chars[j] == '*' && at(j + 1) == '/' {
                        depth -= 1;
                        j += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        j += 1;
                    }
                }
                let end = j.min(n);
                kinds[i..end].fill(Kind::Comment);
                end
            } else if (c == 'r' || (c == 'b' && at(i + 1) == 'r')) && !is_ident(prev) {
                let open = if c == 'r' { i + 1 } else { i + 2 };
                let hashes = (open..n).take_while(|&j| chars[j] == '#').count();
                if at(open + hashes) == '"' {
                    let mut j = open + hashes + 1;
                    while j < n && !(chars[j] == '"' && (1..=hashes).all(|k| at(j + k) == '#')) {
                        j += 1;
                    }
                    let end = (j + 1 + hashes).min(n);
                    kinds[i..end].fill(Kind::Str);
                    end
                } else {
                    i + 1
                }
            } else if c == '"' {
                let mut j = i + 1;
                while j < n && chars[j] != '"' {
                    j += if chars[j] == '\\' { 2 } else { 1 };
                }
                let end = (j + 1).min(n);
                kinds[i..end].fill(Kind::Str);
                end
            } else if c == '\'' && at(i + 1) == '\\' {
                let mut j = i + 3;
                while j < n && chars[j] != '\'' {
                    j += 1;
                }
                let end = (j + 1).min(n);
                kinds[i..end].fill(Kind::Str);
                end
            } else if c == '\'' && at(i + 2) == '\'' {
                kinds[i..i + 3].fill(Kind::Str);
                i + 3
            } else {
                i + 1
            };
            i = end;
        }
        kinds
    }

    /// Marks the items under `#[cfg(test)]`: from the attribute to the
    /// `;` or the matching `}` of the item.
    fn test_marks(chars: &[char], kinds: &[Kind]) -> Vec<bool> {
        let n = chars.len();
        let pattern: Vec<char> = "#[cfg(test)]".chars().collect();
        let mut test = vec![false; n];
        let mut i = 0;
        while i + pattern.len() <= n {
            let hit = pattern
                .iter()
                .enumerate()
                .all(|(k, p)| kinds[i + k] == Kind::Code && chars[i + k] == *p);
            if hit {
                let end = item_end(chars, kinds, i + pattern.len());
                test[i..end].fill(true);
                i = end;
            } else {
                i += 1;
            }
        }
        test
    }

    fn item_end(chars: &[char], kinds: &[Kind], from: usize) -> usize {
        let n = chars.len();
        let mut depth = 0_i32;
        for j in from..n {
            if kinds[j] != Kind::Code {
                continue;
            }
            match chars[j] {
                '(' | '[' => depth += 1,
                ')' | ']' => depth -= 1,
                ';' if depth == 0 => return j + 1,
                '{' if depth == 0 => {
                    let mut braces = 0_i32;
                    for k in j..n {
                        if kinds[k] != Kind::Code {
                            continue;
                        }
                        if chars[k] == '{' {
                            braces += 1;
                        } else if chars[k] == '}' {
                            braces -= 1;
                            if braces == 0 {
                                return k + 1;
                            }
                        }
                    }
                    return n;
                }
                _ => {}
            }
        }
        n
    }

    /// The text with no comments and no test items. Strings stay.
    pub fn code_only(text: &str) -> String {
        Source::new(text).code()
    }

    /// The identifiers in a text.
    fn idents(text: &str) -> impl Iterator<Item = &str> {
        text.split(|c: char| !is_ident(c))
            .filter(|w| w.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_'))
    }

    // --- Rust rules ------------------------------------------------------

    /// AC2: only `JiraSite::base_url` makes a base URL. A literal with an
    /// Atlassian host, the gateway path or a host from a variable is a
    /// second builder. The san-mateo backend may add the gateway path, as
    /// the san-mateo connection gives the host.
    pub fn base_url_violations(path: &str, text: &str) -> Vec<String> {
        if path.ends_with("autumn-jira/src/site.rs") {
            return Vec::new();
        }
        let prefix_ok = path.ends_with("src/backend/san_mateo.rs");
        Source::new(text)
            .literals()
            .into_iter()
            .filter(|(_, lit)| {
                let lit = lit.to_ascii_lowercase();
                let host = lit.contains("atlassian.net")
                    || lit.contains("atlassian.com")
                    || lit.contains("://{");
                let gateway = lit.contains("/ex/jira");
                host || (gateway && !(prefix_ok && lit.starts_with("/ex/jira/{")))
            })
            .map(|(line, lit)| {
                format!("{path}:{line}: a base URL outside `JiraSite::base_url`: {lit:?}")
            })
            .collect()
    }

    /// AC7: the client searches with `POST /search/jql` only. A path that
    /// ends at `/search`, or goes on with an other segment, is the legacy
    /// search. `/project/search` is a different endpoint.
    pub fn legacy_search_violations(path: &str, text: &str) -> Vec<String> {
        let mut out = Vec::new();
        for (line, lit) in Source::new(text).literals() {
            let mut from = 0;
            while let Some(found) = lit[from..].find("/search") {
                let at = from + found;
                let before = &lit[..at];
                let after = &lit[at + "/search".len()..];
                let api_root = before.is_empty()
                    || ["{API}", "/rest/api/3", "/rest/api/2", "/rest/api/latest"]
                        .iter()
                        .any(|r| before.ends_with(r));
                let new_api = after.starts_with("/jql") || after.starts_with("/approximate-count");
                if api_root && !new_api {
                    out.push(format!("{path}:{line}: the legacy search path: {lit:?}"));
                }
                from = at + "/search".len();
            }
        }
        out
    }

    /// AC21: `#[instrument]` records every argument unless it has
    /// `skip_all`. An argument can be a token or a body.
    pub fn instrument_violations(path: &str, text: &str) -> Vec<String> {
        Source::new(text)
            .attributes()
            .into_iter()
            .filter(|(_, attr)| {
                let name = attr.split('(').next().unwrap_or_default().trim();
                (name == "instrument" || name.ends_with("::instrument"))
                    && !attr.contains("skip_all")
            })
            .map(|(line, attr)| format!("{path}:{line}: #[{}] needs skip_all", attr.trim()))
            .collect()
    }

    /// The identifiers that name MAC bytes.
    fn is_mac_word(ident: &str) -> bool {
        let ident = ident.to_ascii_lowercase();
        let whole = ["tag", "expected", "computed", "finalize", "into_bytes"];
        let parts = ["mac", "hmac", "digest", "signature", "sig"];
        whole.contains(&ident.as_str()) || ident.split('_').any(|p| parts.contains(&p))
    }

    /// The start of the left operand of the operator at `at`. A separator
    /// or a lower `&&` or `||` at depth 0 ends the operand.
    const fn operand_start(chars: &[char], at: usize) -> usize {
        let mut depth = 0_i32;
        let mut j = at;
        while j > 0 {
            let c = chars[j - 1];
            let pair = j >= 2 && chars[j - 2] == c;
            let stop =
                matches!(c, '(' | '[' | ';' | '{' | '}' | ',') || (matches!(c, '&' | '|') && pair);
            if depth == 0 && stop {
                break;
            }
            match c {
                ')' | ']' => depth += 1,
                '(' | '[' => depth -= 1,
                _ => {}
            }
            j -= 1;
        }
        j
    }

    /// The end of the right operand of the operator that ends at `from`.
    const fn operand_end(chars: &[char], from: usize) -> usize {
        let mut depth = 0_i32;
        let mut j = from;
        while j < chars.len() {
            let c = chars[j];
            let pair = j + 1 < chars.len() && chars[j + 1] == c;
            let stop =
                matches!(c, ')' | ']' | ';' | '{' | '}' | ',') || (matches!(c, '&' | '|') && pair);
            if depth == 0 && stop {
                break;
            }
            match c {
                '(' | '[' => depth += 1,
                ')' | ']' => depth -= 1,
                _ => {}
            }
            j += 1;
        }
        j
    }

    /// AC19: a file that computes a MAC compares it in constant time. An
    /// `==` or `!=` with MAC bytes as an operand leaks the match length.
    pub fn mac_eq_violations(path: &str, text: &str) -> Vec<String> {
        let src = Source::new(text);
        let bare = src.bare();
        if !bare.to_ascii_lowercase().contains("hmac") {
            return Vec::new();
        }
        let chars: Vec<char> = bare.chars().collect();
        let mut out = Vec::new();
        for i in 1..chars.len().saturating_sub(1) {
            let is_op = (chars[i] == '=' || chars[i] == '!')
                && chars[i + 1] == '='
                && !matches!(chars[i - 1], '=' | '<' | '>' | '!')
                && chars.get(i + 2) != Some(&'=');
            if !is_op {
                continue;
            }
            let left: String = chars[operand_start(&chars, i)..i].iter().collect();
            let right: String = chars[i + 2..operand_end(&chars, i + 2)].iter().collect();
            if idents(&left).chain(idents(&right)).any(is_mac_word) {
                out.push(format!(
                    "{path}:{}: `{}` compares MAC bytes; use `verify_slice`",
                    src.line_of(i),
                    format!(
                        "{} {}{} {}",
                        left.trim(),
                        chars[i],
                        chars[i + 1],
                        right.trim()
                    )
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                ));
            }
        }
        out
    }

    /// AC33: no `ctx.now()` in harvest code. The workflow start-time clock
    /// does not move, so a deadline from it is wrong after a long wait.
    pub fn ctx_now_violations(path: &str, text: &str) -> Vec<String> {
        let src = Source::new(text);
        // Test items count too: a test that uses `ctx.now()` teaches it.
        let bare = src.view(|k| k == Kind::Code, true);
        let mut compact = String::new();
        let mut index = Vec::new();
        for (i, c) in bare.chars().enumerate() {
            if !c.is_whitespace() {
                compact.push(c);
                index.push(i);
            }
        }
        let mut out = Vec::new();
        for pattern in [
            "ctx.now(",
            "context.now(",
            "WorkflowContext::now(",
            "ActivityContext::now(",
        ] {
            for (at, _) in compact.match_indices(pattern) {
                let char_at = compact[..at].chars().count();
                out.push(format!(
                    "{path}:{}: `{pattern}..)`: use `ctx.system_now()`",
                    src.line_of(index[char_at])
                ));
            }
        }
        out
    }

    /// AC38: no `unwrap`, `expect`, `todo!`, `unimplemented!` in code, and
    /// no `TODO` or `FIXME` in comments, outside test items.
    pub fn unwrap_todo_violations(path: &str, text: &str) -> Vec<String> {
        let src = Source::new(text);
        let mut out = Vec::new();
        for (n, line) in src.bare().lines().enumerate() {
            let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
            for pattern in [".unwrap()", ".expect(", "todo!", "unimplemented!"] {
                if compact.contains(pattern) {
                    out.push(format!("{path}:{}: `{pattern}` outside tests", n + 1));
                }
            }
        }
        let comments = src.view(|k| k == Kind::Comment, false);
        for (n, line) in comments.lines().enumerate() {
            for word in line.split(|c: char| !c.is_ascii_alphanumeric()) {
                if word == "TODO" || word == "FIXME" {
                    out.push(format!("{path}:{}: {word} outside tests", n + 1));
                }
            }
        }
        out
    }

    /// AC38: each `allow` or `expect` attribute has a `reason`. A comment
    /// next to it is not a reason: clippy does not show it.
    pub fn allow_reason_violations(path: &str, text: &str) -> Vec<String> {
        Source::new(text)
            .attributes()
            .into_iter()
            .filter(|(_, attr)| {
                let compact: String = attr.chars().filter(|c| !c.is_whitespace()).collect();
                let name = compact.split('(').next().unwrap_or_default();
                let lint = name == "allow"
                    || name == "expect"
                    || (name == "cfg_attr"
                        && (compact.contains("allow(") || compact.contains("expect(")));
                lint && !compact.contains("reason=")
            })
            .map(|(line, attr)| {
                let attr = attr.split_whitespace().collect::<Vec<_>>().join(" ");
                format!("{path}:{line}: #[{attr}] has no reason")
            })
            .collect()
    }

    // --- Text rules ------------------------------------------------------

    /// AC30: no text claims exactly-once delivery. A negation in the same
    /// sentence, or a quoted mention, is allowed.
    pub fn exactly_once_violations(path: &str, text: &str) -> Vec<String> {
        let markers = ["//", "///", "//!", "*", "/*", "*/", ">", "#"];
        let words: Vec<&str> = text
            .split_whitespace()
            .filter(|w| !markers.contains(w))
            .collect();
        let lower = words.join(" ").to_lowercase();
        let negations = [
            "not", "no", "never", "cannot", "nor", "without", "don't", "doesn't", "isn't",
            "aren't", "won't",
        ];
        let mut out = Vec::new();
        for needle in ["exactly once", "exactly-once"] {
            for (at, _) in lower.match_indices(needle) {
                let before = &lower[..at];
                let quoted = before.ends_with(['"', '`', '\'', '\u{201c}']);
                let sentence = before
                    .rfind(['.', '!', '?', ';'])
                    .map_or(before, |i| &before[i + 1..]);
                let negated = sentence
                    .split(|c: char| !c.is_alphanumeric() && c != '\'')
                    .any(|w| negations.contains(&w));
                if !quoted && !negated {
                    let from = before.len().saturating_sub(60);
                    let from = (from..=before.len())
                        .find(|i| lower.is_char_boundary(*i))
                        .unwrap_or(0);
                    out.push(format!(
                        "{path}: claims \"{needle}\": ..{}{needle}",
                        &lower[from..at]
                    ));
                }
            }
        }
        out
    }

    /// Removes a TOML comment: the text after a `#` outside quotes.
    fn strip_toml_comment(line: &str) -> &str {
        let mut quoted = false;
        for (i, c) in line.char_indices() {
            match c {
                '"' => quoted = !quoted,
                '#' if !quoted => return &line[..i],
                _ => {}
            }
        }
        line
    }

    fn is_dependency_table(section: &str) -> bool {
        ["dependencies", "dev-dependencies", "build-dependencies"]
            .iter()
            .any(|d| section == *d || section.ends_with(&format!(".{d}")))
    }

    /// The dependency name of a `[dependencies.<name>]` header.
    fn dependency_header(section: &str) -> Option<&str> {
        ["dependencies.", "dev-dependencies.", "build-dependencies."]
            .iter()
            .find_map(|d| {
                section
                    .find(d)
                    .filter(|i| *i == 0 || section[..*i].ends_with('.'))
                    .map(|i| section[i + d.len()..].trim_matches('"'))
            })
    }

    /// AC22: the fake has its own JSON, JQL and HMAC code. A dependency on
    /// `autumn-jira`, also under another name, makes it test the client
    /// with the client.
    pub fn fake_dependency_violations(manifest: &str) -> Vec<String> {
        const CLIENT: &str = "autumn-jira";
        let mut out = Vec::new();
        let mut section = String::new();
        for (n, line) in manifest.lines().enumerate() {
            let t = strip_toml_comment(line).trim();
            if t.starts_with('[') {
                t.trim_matches(['[', ']']).trim().clone_into(&mut section);
                if dependency_header(&section) == Some(CLIENT) {
                    out.push(format!("Cargo.toml:{}: [{section}]", n + 1));
                }
                continue;
            }
            let Some((key, value)) = t.split_once('=') else {
                continue;
            };
            let key = key
                .trim()
                .split('.')
                .next()
                .unwrap_or_default()
                .trim_matches('"');
            let value: String = value.chars().filter(|c| !c.is_whitespace()).collect();
            let renamed = value.contains(&format!("package=\"{CLIENT}\""));
            let in_table = is_dependency_table(&section) && (key == CLIENT || renamed);
            let in_header = dependency_header(&section).is_some()
                && key == "package"
                && value.trim_matches('"') == CLIENT;
            if in_table || in_header {
                out.push(format!("Cargo.toml:{}: {t}", n + 1));
            }
        }
        out
    }

    /// The status capture `name=$?`: the script checks the code later.
    fn is_status_capture(text: &str) -> bool {
        let name = text.chars().take_while(|c| is_ident(*c)).count();
        name > 0 && text[name..].starts_with("=$?")
    }

    /// AC38: CI hides no failure. No `continue-on-error`, and no `||`
    /// after a command in a `run` script, except a status capture.
    pub fn ci_violations(ci: &str) -> Vec<String> {
        let lines: Vec<&str> = ci.lines().collect();
        let indent = |l: &str| l.len() - l.trim_start().len();
        let mut out = Vec::new();
        let mut check = |n: usize, line: &str| {
            let t = line.trim();
            if t.starts_with('#') {
                return;
            }
            let mut rest = t;
            while let Some(at) = rest.find("||") {
                rest = &rest[at + 2..];
                if !is_status_capture(rest.trim_start()) {
                    out.push(format!("ci.yml:{}: `||` hides a failure: {t}", n + 1));
                }
            }
        };
        let mut i = 0;
        let mut hidden = Vec::new();
        while i < lines.len() {
            let line = lines[i];
            let t = line.trim_start();
            if t.starts_with('#') {
                i += 1;
                continue;
            }
            if t.contains("continue-on-error") {
                hidden.push(format!("ci.yml:{}: {}", i + 1, t.trim()));
            }
            let item = t.strip_prefix("- ").unwrap_or(t);
            if let Some(value) = item.strip_prefix("run:") {
                let key_indent = line.len() - item.len();
                let value = value.trim();
                if value.starts_with('|') || value.starts_with('>') {
                    let mut j = i + 1;
                    while j < lines.len()
                        && (lines[j].trim().is_empty() || indent(lines[j]) > key_indent)
                    {
                        check(j, lines[j]);
                        j += 1;
                    }
                    i = j;
                    continue;
                }
                check(i, value);
            }
            i += 1;
        }
        hidden.extend(out);
        hidden
    }

    /// AC39: the root lock has no crate that only the ecosystem example
    /// needs. These bring C builds or the full harvest-camel graph.
    pub fn c_build_violations(lock: &str) -> Vec<String> {
        let banned = [
            "harvest-camel",
            "autumn-harvest-plugin",
            "pq-src",
            "libsqlite3-sys",
        ];
        lock.lines()
            .filter(|l| banned.iter().any(|b| l.trim() == format!("name = \"{b}\"")))
            .map(|l| format!("Cargo.lock: {}", l.trim()))
            .collect()
    }

    /// AC39: the lint tables of a manifest: `[lints.*]` and
    /// `[workspace.lints.*]` both give `lints.*`. Values lose white space,
    /// and comments go, so only the lints count.
    pub fn lint_tables(manifest: &str) -> BTreeMap<String, BTreeMap<String, String>> {
        let mut out: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        let mut current: Option<String> = None;
        for line in manifest.lines() {
            let t = strip_toml_comment(line).trim();
            if t.is_empty() {
                continue;
            }
            if t.starts_with('[') {
                let name = t.trim_matches(['[', ']']).trim();
                let name = name.strip_prefix("workspace.").unwrap_or(name);
                current = name.starts_with("lints.").then(|| name.to_owned());
                if let Some(table) = &current {
                    out.entry(table.clone()).or_default();
                }
                continue;
            }
            if let (Some(table), Some((key, value))) = (&current, t.split_once('=')) {
                let value: String = value.chars().filter(|c| !c.is_whitespace()).collect();
                out.entry(table.clone())
                    .or_default()
                    .insert(key.trim().to_owned(), value);
            }
        }
        out
    }

    /// Splits a Markdown table row into cells. A `|` in a code span, or
    /// after `\`, is text.
    fn table_cells(row: &str) -> Vec<String> {
        let row = row.trim().trim_start_matches('|');
        let mut cells = Vec::new();
        let mut cell = String::new();
        let mut code = false;
        let mut prev = ' ';
        for c in row.chars() {
            if c == '`' {
                code = !code;
            }
            if c == '|' && !code && prev != '\\' {
                cells.push(cell.trim().to_owned());
                cell.clear();
            } else {
                cell.push(c);
            }
            prev = c;
        }
        if !cell.trim().is_empty() {
            cells.push(cell.trim().to_owned());
        }
        cells
    }

    fn is_separator(row: &str) -> bool {
        let t = row.trim();
        t.starts_with('|') && t.contains("---") && t.chars().all(|c| "|-: ".contains(c))
    }

    /// AC40: the rows (what, kind, when) of the first table in the README
    /// section "What the plugin writes to your Jira". `None` when the
    /// section is missing. More columns are allowed.
    pub fn writes_rows(readme: &str) -> Option<Vec<(String, String, String)>> {
        let lines: Vec<&str> = readme.lines().collect();
        let heading = |l: &str| {
            let t = l.trim_start();
            t.starts_with('#')
                .then(|| t.chars().take_while(|c| *c == '#').count())
        };
        let start = lines.iter().position(|l| {
            heading(l).is_some()
                && l.trim_start().trim_start_matches('#').trim()
                    == "What the plugin writes to your Jira"
        })?;
        let level = heading(lines[start]).unwrap_or(0);
        let mut rows = Vec::new();
        let mut header = false;
        for line in &lines[start + 1..] {
            let t = line.trim();
            if heading(line).is_some_and(|l| l <= level) {
                break;
            }
            if !t.starts_with('|') {
                if header && !t.is_empty() {
                    break;
                }
                continue;
            }
            if !header {
                header = true;
                continue;
            }
            if is_separator(t) {
                continue;
            }
            let cells: Vec<String> = table_cells(t).iter().map(|c| c.replace('`', "")).collect();
            if let [what, kind, when, ..] = cells.as_slice() {
                rows.push((what.clone(), kind.to_lowercase(), when.clone()));
            }
        }
        Some(rows)
    }

    // --- The evidence file -------------------------------------------------

    /// How a test name in the evidence is found.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Mode {
        Plain,
        Unit,
        Example,
        Ecosystem,
        Suite,
    }

    /// Expands the short paths of plan section 16: `core/t/x.rs` is
    /// `crates/autumn-jira/tests/x.rs`.
    fn expand(path: &str) -> String {
        let aliases = [
            ("core/", "crates/autumn-jira/"),
            ("fake/", "crates/autumn-jira-fake/"),
            ("plugin/", "crates/autumn-plugin-jira/"),
            ("harvest/", "crates/autumn-jira-harvest/"),
            ("camel/", "crates/autumn-jira-camel/"),
            ("eco/", "examples/ecosystem/"),
        ];
        let mut out = path.to_owned();
        for (short, long) in aliases {
            if let Some(rest) = path.strip_prefix(short) {
                out = format!("{long}{rest}");
            }
        }
        out.replacen("/t/", "/tests/", 1)
    }

    fn rust_files(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if path.is_dir() {
                if name != "target" {
                    out.extend(rust_files(&path));
                }
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
        out
    }

    /// The files that can hold the tests of a module path such as
    /// `backend::san_mateo`, `embedded` or `tests`.
    fn module_files(root: &Path, mode: Mode, module: &str) -> Vec<PathBuf> {
        let parts: Vec<&str> = module.split("::").collect();
        let first = parts.first().copied().unwrap_or_default();
        let mut out = Vec::new();
        let crates = root.join("crates");
        let mut krates: Vec<PathBuf> = std::fs::read_dir(&crates)
            .map(|d| d.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        krates.sort();
        if mode == Mode::Ecosystem {
            return rust_files(&root.join("examples/ecosystem/src"));
        }
        if mode != Mode::Example {
            let rel = parts.join("/");
            for krate in &krates {
                let src = krate.join("src");
                for file in [src.join(format!("{rel}.rs")), src.join(&rel).join("mod.rs")] {
                    if file.is_file() {
                        out.push(file);
                    }
                }
                out.extend(rust_files(&src.join(&rel)));
            }
        }
        if out.is_empty() {
            for krate in &krates {
                let examples = krate.join("examples");
                let file = examples.join(format!("{first}.rs"));
                if file.is_file() {
                    out.push(file);
                }
                out.extend(rust_files(&examples.join(first)));
            }
            out.extend(rust_files(&root.join("examples").join(first).join("src")));
        }
        out
    }

    fn is_snake(text: &str) -> bool {
        text.starts_with(|c: char| c.is_ascii_lowercase())
            && text
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            && text.contains('_')
    }

    /// The names in `{a, b}` or `a`.
    fn names(text: &str) -> Vec<String> {
        text.trim()
            .trim_start_matches('{')
            .trim_end_matches('}')
            .split(',')
            .map(|n| n.trim().to_owned())
            .filter(|n| !n.is_empty())
            .collect()
    }

    fn has_fn(files: &[PathBuf], name: &str) -> bool {
        let call = format!("fn {name}(");
        let generic = format!("fn {name}<");
        files.iter().any(|f| {
            std::fs::read_to_string(f).is_ok_and(|t| t.contains(&call) || t.contains(&generic))
        })
    }

    /// The code spans of a text, each with the text before it.
    fn code_spans(text: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut rest = text;
        while let Some(open) = rest.find('`') {
            let Some(len) = rest[open + 1..].find('`') else {
                break;
            };
            out.push((
                rest[..open].to_owned(),
                rest[open + 1..open + 1 + len].to_owned(),
            ));
            rest = &rest[open + 2 + len..];
        }
        out
    }

    /// Splits a cell at `;` outside code spans.
    fn segments(cell: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut seg = String::new();
        let mut code = false;
        for c in cell.chars() {
            if c == '`' {
                code = !code;
            }
            if c == ';' && !code {
                out.push(std::mem::take(&mut seg));
            } else {
                seg.push(c);
            }
        }
        out.push(seg);
        out
    }

    fn mode_of(prefix: &str) -> Mode {
        let p = prefix.to_ascii_lowercase();
        if p.contains("eco") {
            Mode::Ecosystem
        } else if p.contains("ex:") {
            Mode::Example
        } else if p.contains("cases") {
            Mode::Suite
        } else if p.contains("u:") {
            Mode::Unit
        } else {
            Mode::Plain
        }
    }

    fn check_names(row: &str, names: &[String], files: &[PathBuf], root: &Path) -> Vec<String> {
        let shown: Vec<String> = files
            .iter()
            .map(|f| {
                f.strip_prefix(root)
                    .unwrap_or(f)
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names
            .iter()
            .filter(|n| !has_fn(files, n))
            .map(|n| format!("{row}: `{n}` is not a fn in {}", shown.join(", ")))
            .collect()
    }

    fn check_segment(row: &str, segment: &str, root: &Path) -> Vec<String> {
        let spans = code_spans(segment);
        let mode = spans
            .first()
            .map_or(Mode::Plain, |(before, _)| mode_of(before));
        let suite = root.join("crates/autumn-jira/tests/common/suite.rs");
        let mut file: Option<PathBuf> = (mode == Mode::Suite).then_some(suite);
        let mut out = Vec::new();
        for (k, (before, span)) in spans.iter().enumerate() {
            let span = span.trim();
            if let Some(end) = span.find(".rs") {
                let path = root.join(expand(&span[..end + 3]));
                if !path.is_file() {
                    out.push(format!("{row}: `{span}`: no file {}", &span[..end + 3]));
                    continue;
                }
                let rest = span[end + 3..].trim_start_matches("::");
                if !rest.is_empty() {
                    out.extend(check_names(
                        row,
                        &names(rest),
                        std::slice::from_ref(&path),
                        root,
                    ));
                }
                file = Some(path);
            } else if span.starts_with('{') {
                match &file {
                    Some(f) => out.extend(check_names(
                        row,
                        &names(span),
                        std::slice::from_ref(f),
                        root,
                    )),
                    None => out.push(format!("{row}: `{span}`: no file for these names")),
                }
            } else if span.contains("::") {
                let (module, list) = span.split_once("::{").map_or_else(
                    || {
                        let (m, l) = span.rsplit_once("::").unwrap_or_default();
                        (m, l.to_owned())
                    },
                    |(m, l)| (m, format!("{{{l}")),
                );
                let module = module.trim();
                let valid = module
                    .split("::")
                    .all(|p| !p.is_empty() && p.chars().all(is_ident));
                if !valid || module.starts_with(|c: char| c.is_ascii_uppercase()) {
                    continue;
                }
                let files = module_files(root, mode, module);
                if files.is_empty() {
                    out.push(format!("{row}: `{span}`: no module {module}"));
                } else {
                    out.extend(check_names(row, &names(&list), &files, root));
                }
            } else if span.contains('/')
                && [".sh", ".py", ".yml", ".md", ".toml"]
                    .iter()
                    .any(|e| span.ends_with(e))
            {
                if !root.join(expand(span)).is_file() {
                    out.push(format!("{row}: `{span}`: no file"));
                }
            } else if k == 0 && is_snake(span) {
                let words = before
                    .replace("eco", "")
                    .replace("u:", "")
                    .replace("ex:", "");
                if words.trim().is_empty() {
                    let mut files = rust_files(&root.join("crates"));
                    files.extend(rust_files(&root.join("examples")));
                    out.extend(check_names(row, &[span.to_owned()], &files, root));
                }
            }
        }
        out
    }

    /// AC40: each test name in a "Tests" column of `ac-evidence.md` is a
    /// `fn` in the file or module that the row names. Forms:
    /// `core/t/x.rs::{a, b}`, `core/t/x.rs` then `{a}`, suite cases `{a}`,
    /// u: `module::{a}`, ex: `example::{a}`, eco u: `tests::{a}`, and a bare
    /// `name` at the start of a part.
    pub fn evidence_violations(md: &str, root: &Path) -> Vec<String> {
        let lines: Vec<&str> = md.lines().collect();
        let mut columns: Vec<usize> = Vec::new();
        let mut out = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            let t = line.trim();
            if !t.starts_with('|') {
                columns.clear();
                continue;
            }
            if is_separator(t) {
                continue;
            }
            let cells = table_cells(t);
            if lines.get(i + 1).is_some_and(|l| is_separator(l)) {
                columns = cells
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.to_ascii_lowercase().contains("test"))
                    .map(|(k, _)| k)
                    .collect();
                continue;
            }
            let row = cells.first().map_or("", String::as_str);
            for &k in &columns {
                for segment in segments(cells.get(k).map_or("", String::as_str)) {
                    out.extend(check_segment(row, &segment, root));
                }
            }
        }
        out
    }

    // --- ADRs --------------------------------------------------------------

    /// The ADR numbers that a text cites: `ADR 0003` and `adr/0003-..`.
    pub fn adr_references(text: &str) -> Vec<u32> {
        let mut out = Vec::new();
        for marker in ["ADR ", "adr/"] {
            for (at, _) in text.match_indices(marker) {
                let digits: String = text[at + marker.len()..].chars().take(4).collect();
                if digits.len() == 4
                    && digits.chars().all(|c| c.is_ascii_digit())
                    && let Ok(n) = digits.parse()
                {
                    out.push(n);
                }
            }
        }
        out
    }

    /// AC40: ADRs 0001 to 0008 exist, one file each, and each cited ADR
    /// exists.
    pub fn adr_violations(names: &[String], referenced: &[u32]) -> Vec<String> {
        let mut count: BTreeMap<u32, usize> = BTreeMap::new();
        for name in names {
            let digits: String = name.chars().take_while(char::is_ascii_digit).collect();
            if let Ok(n) = digits.parse::<u32>() {
                *count.entry(n).or_default() += 1;
            }
        }
        let mut out = Vec::new();
        for n in 1..=8 {
            if !count.contains_key(&n) {
                out.push(format!("docs/adr: no ADR {n:04}"));
            }
        }
        for (n, c) in &count {
            if *c > 1 {
                out.push(format!("docs/adr: {c} files for ADR {n:04}"));
            }
        }
        let mut cited: Vec<u32> = referenced.to_vec();
        cited.sort_unstable();
        cited.dedup();
        for n in cited {
            if !count.contains_key(&n) {
                out.push(format!("a text cites ADR {n:04}, which does not exist"));
            }
        }
        out
    }
}

/// The five library crates of the workspace.
const CRATES: [&str; 5] = [
    "autumn-jira",
    "autumn-jira-fake",
    "autumn-plugin-jira",
    "autumn-jira-harvest",
    "autumn-jira-camel",
];

/// The crates that call Jira. The fake is the server, so it is not one.
const CLIENT_CRATES: [&str; 4] = [
    "autumn-jira",
    "autumn-plugin-jira",
    "autumn-jira-harvest",
    "autumn-jira-camel",
];

/// No violations. `assert_eq!` with it shows the violations on a failure.
const NONE: [String; 0] = [];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &str) -> String {
    std::fs::read_to_string(root().join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The path relative to the repository root, with `/`.
fn rel(path: &Path) -> String {
    let root = root().canonicalize().unwrap();
    let path = path.canonicalize().unwrap();
    path.strip_prefix(&root)
        .unwrap()
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// The files under `dir` with one of the extensions, in a stable order.
/// Build and VCS folders are skipped.
fn files_under(dir: &Path, exts: &[&str]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut entries: Vec<PathBuf> = entries.map(|e| e.unwrap().path()).collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            if !matches!(name.as_str(), "target" | ".git" | "node_modules") {
                out.extend(files_under(&path, exts));
            }
        } else if path
            .extension()
            .is_some_and(|e| exts.iter().any(|x| e == *x))
        {
            out.push(path);
        }
    }
    out
}

fn src_files(krates: &[&str]) -> Vec<PathBuf> {
    krates
        .iter()
        .flat_map(|k| files_under(&root().join("crates").join(k).join("src"), &["rs"]))
        .collect()
}

/// Runs `check` on each file and fails with all violations.
fn assert_files(name: &str, files: &[PathBuf], check: fn(&str, &str) -> Vec<String>) {
    assert!(!files.is_empty(), "{name}: no files to check");
    let found: Vec<String> = files
        .iter()
        .flat_map(|f| check(&rel(f), &std::fs::read_to_string(f).unwrap()))
        .collect();
    assert!(found.is_empty(), "{name}:\n{}", found.join("\n"));
}

// --- The lexer -----------------------------------------------------------

#[test]
fn the_lexer_drops_comments_and_test_items() {
    let text = "let a = \"// not a comment\"; // a comment\n\
                let b = r#\"/* raw */\"#; /* block /* nested */ */ let c = 'x';\n\
                fn f<'a>(x: &'a str) {}\n\
                #[cfg(test)]\nmod tests { fn t() { let s = \"}\"; x.unwrap(); } }\n\
                fn g() {}\n";
    let code = check::code_only(text);
    assert!(code.contains("\"// not a comment\""), "{code}");
    assert!(!code.contains("a comment\n"), "{code}");
    assert!(code.contains("r#\"/* raw */\"#"), "{code}");
    assert!(!code.contains("nested"), "{code}");
    assert!(code.contains("let c = 'x';"), "{code}");
    assert!(code.contains("fn f<'a>(x: &'a str)"), "{code}");
    assert!(!code.contains("unwrap"), "{code}");
    assert!(code.contains("fn g() {}"), "{code}");
    // The line count stays the same, so a violation names the right line.
    assert_eq!(code.lines().count(), text.lines().count());
}

// --- AC2: one base URL builder -------------------------------------------

#[test]
fn one_base_url_builder() {
    assert_files(
        "one_base_url_builder",
        &src_files(&CLIENT_CRATES),
        check::base_url_violations,
    );
}

#[test]
fn one_base_url_builder_sees_a_second_builder() {
    let bad = "fn url(site: &str) -> String { format!(\"https://{site}.atlassian.net/rest\") }";
    assert_eq!(
        check::base_url_violations("crates/autumn-plugin-jira/src/x.rs", bad).len(),
        1
    );
    let gateway = "const G: &str = \"https://api.atlassian.com/ex/jira/\";";
    assert_eq!(
        check::base_url_violations("crates/autumn-jira/src/client.rs", gateway).len(),
        1
    );
    // The builder itself, the san-mateo path prefix and comments pass.
    assert_eq!(
        check::base_url_violations("crates/autumn-jira/src/site.rs", bad),
        NONE
    );
    let prefix = "fn p(id: &str) -> String { format!(\"/ex/jira/{id}\") }";
    assert_eq!(
        check::base_url_violations("crates/autumn-jira/src/backend/san_mateo.rs", prefix),
        NONE
    );
    assert_eq!(
        check::base_url_violations("crates/autumn-jira/src/client.rs", prefix).len(),
        1
    );
    let comment = "/// Sends to `https://acme.atlassian.net`.\nfn f() {}";
    assert_eq!(
        check::base_url_violations("crates/autumn-jira/src/x.rs", comment),
        NONE
    );
}

// --- AC7: no legacy search path ------------------------------------------

#[test]
fn no_legacy_search_path() {
    assert_files(
        "no_legacy_search_path",
        &src_files(&CLIENT_CRATES),
        check::legacy_search_violations,
    );
}

#[test]
fn no_legacy_search_path_sees_the_old_path() {
    let p = "crates/autumn-jira/src/client.rs";
    for bad in [
        "let p = format!(\"{API}/search\");",
        "let p = \"/rest/api/3/search?jql=x\";",
        "let p = format!(\"{API}/search/\");",
        "let p = \"/search\";",
    ] {
        assert_eq!(check::legacy_search_violations(p, bad).len(), 1, "{bad}");
    }
    for good in [
        "let p = format!(\"{API}/search/jql\");",
        "let p = format!(\"{API}/search/approximate-count\");",
        "let p = format!(\"{API}/project/search\");",
        "/// The old `GET /rest/api/3/search` is gone.\nfn f() {}",
    ] {
        assert!(
            check::legacy_search_violations(p, good).is_empty(),
            "{good}"
        );
    }
}

// --- AC21: no #[instrument] without skip_all -----------------------------

#[test]
fn instrument_skips_all() {
    assert_files(
        "instrument_skips_all",
        &src_files(&CRATES),
        check::instrument_violations,
    );
}

#[test]
fn instrument_skips_all_sees_a_skip_list() {
    let p = "crates/autumn-jira/src/x.rs";
    let bad = "#[tracing::instrument(skip(self))]\nasync fn f(&self, body: &str) {}";
    assert_eq!(check::instrument_violations(p, bad).len(), 1);
    let bare = "#[instrument]\nfn f(token: &str) {}";
    assert_eq!(check::instrument_violations(p, bare).len(), 1);
    let good = "#[instrument(\n    skip_all,\n    fields(op = %op)\n)]\nfn f(op: u8) {}";
    assert_eq!(check::instrument_violations(p, good), NONE);
    let comment = "// #[instrument] is not used here.\nfn f() {}";
    assert_eq!(check::instrument_violations(p, comment), NONE);
}

// --- AC19: no == on MAC bytes --------------------------------------------

#[test]
fn no_eq_on_mac_bytes() {
    assert_files(
        "no_eq_on_mac_bytes",
        &src_files(&CRATES),
        check::mac_eq_violations,
    );
}

#[test]
fn no_eq_on_mac_bytes_sees_an_eq() {
    let p = "crates/autumn-jira/src/webhook.rs";
    let bad = "use hmac::Mac;\n\
               fn v(mac: HmacSha256, expected: &[u8]) -> bool {\n\
               \x20   mac.finalize().into_bytes().as_slice() == expected\n}";
    assert_eq!(check::mac_eq_violations(p, bad).len(), 1);
    let tag = "use hmac::Mac;\nfn v(tag: &str, want: &str) -> bool { tag != want }";
    assert_eq!(check::mac_eq_violations(p, tag).len(), 1);
    let good = "use hmac::Mac;\n\
                // Never write `mac == expected`.\n\
                fn v(mac: HmacSha256, expected: &[u8]) -> bool {\n\
                \x20   bytes.len() != TAG_LEN || mac.verify_slice(expected).is_ok()\n}";
    assert_eq!(check::mac_eq_violations(p, good), NONE);
}

// --- AC22: the fake does not depend on the client ------------------------

#[test]
fn fake_does_not_depend_on_the_client() {
    let found = check::fake_dependency_violations(&read("crates/autumn-jira-fake/Cargo.toml"));
    assert!(found.is_empty(), "{}", found.join("\n"));
    // The lock agrees: the fake package lists no `autumn-jira` dependency.
    let lock = read("Cargo.lock");
    let block = lock
        .split("[[package]]")
        .find(|b| b.lines().any(|l| l.trim() == "name = \"autumn-jira-fake\""))
        .expect("the lock has autumn-jira-fake");
    assert!(
        !block
            .lines()
            .any(|l| l.trim().trim_end_matches(',') == "\"autumn-jira\""),
        "Cargo.lock: autumn-jira-fake depends on autumn-jira"
    );
}

#[test]
fn fake_does_not_depend_on_the_client_sees_a_dependency() {
    let good = "[package]\nname = \"autumn-jira-fake\"\n\n[dependencies]\n\
                san-mateo.workspace = true\n\n[dev-dependencies]\n\
                autumn-jira-fake = { path = \".\" }\n";
    assert_eq!(check::fake_dependency_violations(good), NONE);
    for bad in [
        "[dependencies]\nautumn-jira.workspace = true\n",
        "[dev-dependencies]\nautumn-jira = { path = \"../autumn-jira\" }\n",
        "[dependencies]\nclient = { package = \"autumn-jira\", version = \"0.1\" }\n",
        "[dependencies.autumn-jira]\nworkspace = true\n",
        "[target.'cfg(unix)'.dependencies]\n\"autumn-jira\" = \"0.1\"\n",
    ] {
        assert_eq!(check::fake_dependency_violations(bad).len(), 1, "{bad}");
    }
}

// --- AC33: no ctx.now() in harvest code ----------------------------------

#[test]
fn no_ctx_now_in_harvest() {
    let mut files = files_under(&root().join("crates/autumn-jira-harvest"), &["rs"]);
    files.extend(files_under(&root().join("examples/ecosystem/src"), &["rs"]));
    assert_files("no_ctx_now_in_harvest", &files, check::ctx_now_violations);
}

#[test]
fn no_ctx_now_in_harvest_sees_ctx_now() {
    let p = "crates/autumn-jira-harvest/src/wait.rs";
    let bad = "let deadline = ctx\n    .now()\n    + timeout;";
    assert_eq!(check::ctx_now_violations(p, bad).len(), 1);
    let typed = "let t = WorkflowContext::now(ctx);";
    assert_eq!(check::ctx_now_violations(p, typed).len(), 1);
    let good = "// Never call ctx.now() here.\nlet t = ctx.system_now();\nlet s = \"ctx.now()\";";
    assert_eq!(check::ctx_now_violations(p, good), NONE);
}

// --- AC30: no exactly-once claim -----------------------------------------

#[test]
fn no_exactly_once_claim() {
    let mut files = files_under(&root(), &["md", "rs"]);
    files.retain(|f| !rel(f).starts_with("target/"));
    assert_files(
        "no_exactly_once_claim",
        &files,
        check::exactly_once_violations,
    );
}

#[test]
fn no_exactly_once_claim_sees_a_claim() {
    // The phrase is built at run time, so this file does not hold it.
    let phrase = ["exactly", "once"].join(" ");
    let p = "README.md";
    for bad in [
        format!("The route delivers each event {phrase}."),
        format!("It gives {}-processing.", phrase.replace(' ', "-")),
        format!("/// Each sink runs\n/// {phrase}.\nfn f() {{}}"),
        format!("Exactly once for each delivery. Not a typo, {phrase} again."),
    ] {
        assert!(!check::exactly_once_violations(p, &bad).is_empty(), "{bad}");
    }
    for good in [
        format!("Webhooks are at least once. Do not claim {phrase}."),
        format!("/// Delivery is not\n/// {phrase}: a sink must be idempotent."),
        format!("No \"{phrase}\" here."),
        format!("The docs say \"best effort; no {phrase}\"."),
        format!("- \"{}\". Webhooks are at least once.", capitalize(&phrase)),
    ] {
        assert!(
            check::exactly_once_violations(p, &good).is_empty(),
            "{good}"
        );
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

// --- AC38: no unwrap, expect, todo!, TODO in src/ ------------------------

#[test]
fn no_unwrap_or_todo_in_src() {
    assert_files(
        "no_unwrap_or_todo_in_src",
        &src_files(&CRATES),
        check::unwrap_todo_violations,
    );
}

#[test]
fn no_unwrap_or_todo_in_src_sees_each_kind() {
    let p = "crates/autumn-jira/src/x.rs";
    for bad in [
        "fn f(x: Option<u8>) -> u8 { x.unwrap() }",
        "fn f(x: Option<u8>) -> u8 {\n    x\n        .expect(\"a value\")\n}",
        "fn f() { todo!() }",
        "fn f() { unimplemented!(\"later\") }",
        "// TODO: make this fast.\nfn f() {}",
        "/* FIXME */ fn f() {}",
    ] {
        assert_eq!(check::unwrap_todo_violations(p, bad).len(), 1, "{bad}");
    }
    for good in [
        "fn f(x: Option<u8>) -> u8 { x.unwrap_or(1) }",
        "fn f(x: Result<u8, u8>) { let _ = x.expect_err; }",
        "/// ```\n/// let x = Some(1).unwrap();\n/// ```\nfn f() {}",
        "let s = \"do not unwrap() here, TODO\";",
        "pub(crate) const TODO: u8 = 1;",
        "#[cfg(test)]\nmod tests {\n    // TODO in a test is a test matter.\n    fn t() { Some(1).unwrap(); }\n}",
    ] {
        assert!(check::unwrap_todo_violations(p, good).is_empty(), "{good}");
    }
}

// --- AC38: each allow has a reason ---------------------------------------

#[test]
fn every_allow_has_a_reason() {
    let mut files = files_under(&root().join("crates"), &["rs"]);
    files.extend(files_under(&root().join("examples"), &["rs"]));
    assert_files(
        "every_allow_has_a_reason",
        &files,
        check::allow_reason_violations,
    );
}

#[test]
fn every_allow_has_a_reason_sees_a_bare_allow() {
    let p = "crates/autumn-jira/src/x.rs";
    for bad in [
        "#[allow(dead_code)] // reason: a comment is not a reason\nfn f() {}",
        "#![allow(\n    clippy::unwrap_used,\n    clippy::expect_used\n)]",
        "#[expect(clippy::cast_possible_truncation)]\nfn f() {}",
        "#[cfg_attr(test, allow(dead_code))]\nfn f() {}",
    ] {
        assert_eq!(check::allow_reason_violations(p, bad).len(), 1, "{bad}");
    }
    for good in [
        "#[allow(\n    clippy::too_many_lines,\n    reason = \"one table\"\n)]\nfn f() {}",
        "#![allow(clippy::print_stdout, reason = \"the example prints its summary\")]",
        "// #[allow(dead_code)] in a comment is not an attribute.\nfn f() {}",
        "let s = \"#[allow(dead_code)]\";",
    ] {
        assert!(check::allow_reason_violations(p, good).is_empty(), "{good}");
    }
}

// --- AC38: no continue-on-error, no || after a checked command -----------

#[test]
fn no_continue_on_error() {
    let found = check::ci_violations(&read(".github/workflows/ci.yml"));
    assert!(found.is_empty(), "{}", found.join("\n"));
}

#[test]
fn no_continue_on_error_sees_hidden_failures() {
    for bad in [
        "    steps:\n      - run: cargo test\n        continue-on-error: true\n",
        "    steps:\n      - run: cargo test || true\n",
        "    steps:\n      - name: x\n        run: |\n          cargo build\n          cargo test || echo failed\n",
        "    steps:\n      - name: x\n        run: >-\n          cargo llvm-cov\n          --json || :\n",
    ] {
        assert_eq!(check::ci_violations(bad).len(), 1, "{bad}");
    }
    let good = "    steps:\n\
                \x20     - name: x\n\
                \x20       if: github.event_name == 'schedule' || github.event_name == 'push'\n\
                \x20       run: |\n\
                \x20         # A comment with || in it.\n\
                \x20         ra=0; wait \"$a\" || ra=$?\n\
                \x20         test \"$ra\" -eq 0\n\
                \x20     - run: cargo test\n";
    let found = check::ci_violations(good);
    assert!(found.is_empty(), "{found:?}");
}

// --- AC39: the root lock is free of C builds -----------------------------

#[test]
fn root_lock_is_free_of_c_builds() {
    let found = check::c_build_violations(&read("Cargo.lock"));
    assert!(found.is_empty(), "{}", found.join("\n"));
}

#[test]
fn root_lock_is_free_of_c_builds_sees_one() {
    let lock = "[[package]]\nname = \"libsqlite3-sys\"\nversion = \"0.30.1\"\n\n\
                [[package]]\nname = \"harvest-camel\"\nversion = \"0.2.0\"\n\n\
                [[package]]\nname = \"camel-core\"\nversion = \"0.57.0\"\n";
    assert_eq!(check::c_build_violations(lock).len(), 2);
    assert_eq!(
        check::c_build_violations("[[package]]\nname = \"pq-sys\"\n"),
        NONE
    );
}

// --- AC39: the ecosystem lints equal the root lints ----------------------

#[test]
fn ecosystem_lints_equal_the_root() {
    let path = "examples/ecosystem/Cargo.toml";
    assert!(
        root().join(path).exists(),
        "{path} is missing: the ecosystem example copies the root lints"
    );
    let root_lints = check::lint_tables(&read("Cargo.toml"));
    assert!(
        root_lints.contains_key("lints.rust") && root_lints.contains_key("lints.clippy"),
        "the root has no [workspace.lints.rust] and [workspace.lints.clippy]"
    );
    assert_eq!(
        check::lint_tables(&read(path)),
        root_lints,
        "{path}: copy [workspace.lints] of the root Cargo.toml"
    );
}

#[test]
fn the_lint_compare_sees_a_change() {
    let root = "[workspace.lints.rust]\nunsafe_code = \"forbid\"\n\n\
                [workspace.lints.clippy]\n# A comment.\npedantic = { level = \"warn\", priority = -1 }\n\
                unwrap_used = \"deny\"\n\n[profile.dev]\ndebug = 1\n";
    let same = "[package]\nname = \"ecosystem\"\n\n[lints.clippy]\nunwrap_used = \"deny\"\n\
                pedantic = {level = \"warn\",   priority = -1}\n\n[lints.rust]\n\
                unsafe_code = \"forbid\" # forbid\n";
    let tables = check::lint_tables(root);
    assert_eq!(tables.len(), 2, "{tables:?}");
    assert_eq!(check::lint_tables(same), tables);
    let missing = same.replace("unwrap_used = \"deny\"\n", "");
    assert_ne!(check::lint_tables(&missing), tables);
    let weaker = same.replace("\"forbid\"", "\"deny\"");
    assert_ne!(check::lint_tables(&weaker), tables);
}

// --- AC40: the README writes table equals WRITES -------------------------

/// The README text of a [`WriteKind`]. A new kind breaks this match.
const fn kind_text(kind: WriteKind) -> &'static str {
    match kind {
        WriteKind::Label => "label",
        WriteKind::IssueProperty => "issue property",
        WriteKind::CommentText => "comment text",
    }
}

fn expected_writes() -> Vec<(String, String, String)> {
    WRITES
        .iter()
        .map(|w| {
            (
                w.what.to_owned(),
                kind_text(w.kind).to_owned(),
                w.when.to_owned(),
            )
        })
        .collect()
}

#[test]
fn readme_writes_table_equals_writes() {
    let rows = check::writes_rows(&read("README.md"))
        .expect("README.md has no section \"What the plugin writes to your Jira\"");
    assert_eq!(
        rows,
        expected_writes(),
        "README.md: one row for each WRITES entry"
    );
}

#[test]
fn the_writes_compare_sees_a_change() {
    let mut table = String::new();
    for (what, kind, when) in expected_writes() {
        writeln!(table, "| `{what}` | {kind} | `{when}` | why |").unwrap();
    }
    let readme = format!(
        "# x\n\n## What the plugin writes to your Jira\n\nText.\n\n\
         | What | Kind | When | Why |\n|---|---|---|---|\n{table}\n## Next\n\n| a | b | c |\n"
    );
    assert_eq!(check::writes_rows(&readme), Some(expected_writes()));
    let fewer = readme.replacen("| `autumn-tracked`", "| `autumn-other`", 1);
    assert_ne!(check::writes_rows(&fewer), Some(expected_writes()));
    assert_eq!(check::writes_rows("# x\n\n## Other\n"), None);
}

// --- AC40: the evidence names real tests ---------------------------------

#[test]
fn evidence_names_real_tests() {
    let path = "docs/ac-evidence.md";
    assert!(root().join(path).exists(), "{path} is missing");
    let found = check::evidence_violations(&read(path), &root());
    assert!(found.is_empty(), "{path}:\n{}", found.join("\n"));
}

#[test]
fn the_evidence_check_sees_a_missing_test() {
    let md = "| AC | Tests | Status |\n|---|---|---|\n\
              | AC1 | `core/t/guards.rs::{one_base_url_builder, no_such_guard}`; u: `site::{base_urls}` | Done |\n\
              | AC2 | u: `ids::{strict_rules_table, no_such_unit_test}`; CI job `verus` | Done |\n\
              | AC3 | suite cases `{create_get_edit_delete, no_such_case}` | Done |\n\
              | AC4 | ex: `embedded::{the_example_runs_and_checks, no_such_example_test}` | Done |\n\
              | AC5 | `core/t/no_such_file.rs::x`; `idempotency_key_derive_is_stable`; `no_such_fn` | Done |\n";
    let found = check::evidence_violations(md, &root());
    let missing: Vec<&str> = [
        "no_such_guard",
        "no_such_unit_test",
        "no_such_case",
        "no_such_example_test",
        "no_such_file.rs",
        "no_such_fn",
    ]
    .into_iter()
    .filter(|name| !found.iter().any(|f| f.contains(name)))
    .collect();
    assert!(
        missing.is_empty(),
        "not seen: {missing:?}; found: {found:?}"
    );
    assert_eq!(found.len(), 6, "{found:?}");
}

// --- AC40: the ADR files exist -------------------------------------------

fn adr_names() -> Vec<String> {
    files_under(&root().join("docs/adr"), &["md"])
        .iter()
        .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn adr_files_exist() {
    let mut referenced = Vec::new();
    for file in files_under(&root(), &["md", "rs"]) {
        if !rel(&file).starts_with("target/") {
            referenced.extend(check::adr_references(
                &std::fs::read_to_string(file).unwrap(),
            ));
        }
    }
    let found = check::adr_violations(&adr_names(), &referenced);
    assert!(found.is_empty(), "{}", found.join("\n"));
    for name in adr_names() {
        let text = read(&format!("docs/adr/{name}"));
        assert!(
            text.starts_with("# "),
            "docs/adr/{name}: the first line is a title"
        );
    }
}

#[test]
fn adr_files_exist_sees_a_gap() {
    let names: Vec<String> = (1..=8)
        .filter(|n| *n != 5)
        .map(|n| format!("{n:04}-x.md"))
        .collect();
    assert_eq!(check::adr_violations(&names, &[]).len(), 1);
    let mut all: Vec<String> = (1..=8).map(|n| format!("{n:04}-x.md")).collect();
    assert_eq!(check::adr_violations(&all, &[3, 8]), NONE);
    assert_eq!(check::adr_violations(&all, &[9]).len(), 1);
    all.push("0002-y.md".to_owned());
    assert_eq!(check::adr_violations(&all, &[]).len(), 1);
    assert_eq!(
        // The number is built at run time: a cited 0010 in this file fails
        // `adr_files_exist`.
        check::adr_references(&format!("See ADR 0003 and ADR {}. adr/0004-x.md", "0010")),
        [3, 10, 4]
    );
}
