//! The JQL subset of the fake, with its own parser.
//!
//! Grammar: `clause (AND clause)* [ORDER BY field [ASC|DESC] (, ...)*]`.
//! A clause is `field = v`, `field != v`, `field [NOT] IN (v, ...)` or
//! `field IS [NOT] EMPTY`. A value is a word, `"text"` or `'text'` (with
//! the escapes `\"` and `\\`), or `currentUser()`. Keywords ignore case.

use std::cmp::Ordering;

use crate::state::Issue;
use crate::types::FakeProject;

/// A field that a clause can test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Field {
    Project,
    Key,
    Labels,
    Status,
    StatusCategory,
    IssueType,
    Assignee,
}

impl Field {
    /// Reads a field name. Names ignore case.
    fn parse(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "project" => Self::Project,
            "key" | "issuekey" | "issue" | "id" => Self::Key,
            "labels" | "label" => Self::Labels,
            "status" => Self::Status,
            "statuscategory" => Self::StatusCategory,
            "issuetype" | "type" => Self::IssueType,
            "assignee" => Self::Assignee,
            _ => return None,
        })
    }
}

/// A field that `ORDER BY` can use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SortField {
    Field(Field),
    Created,
    Updated,
    Summary,
    Priority,
}

impl SortField {
    /// Reads a sort field name.
    fn parse(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "created" => Self::Created,
            "updated" => Self::Updated,
            "summary" => Self::Summary,
            "priority" => Self::Priority,
            _ => Self::Field(Field::parse(name)?),
        })
    }
}

/// A value in a clause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Val {
    Text(String),
    /// `currentUser()`.
    Me,
}

/// The test of a clause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Cond {
    Eq(Val),
    Ne(Val),
    In(Vec<Val>),
    NotIn(Vec<Val>),
    Empty,
    NotEmpty,
}

/// One clause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Clause {
    pub(crate) field: Field,
    pub(crate) cond: Cond,
}

/// A parsed query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Query {
    pub(crate) clauses: Vec<Clause>,
    pub(crate) order: Vec<(SortField, bool)>,
}

/// A query that the fake does not accept. Jira answers 400.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum JqlError {
    /// The query has no clause.
    Unbounded,
    /// The query names an unknown field.
    UnknownField(String),
    /// `ORDER BY` names a field that cannot sort.
    BadSort(String),
    /// The text does not parse.
    Syntax(String),
    /// A project value names no project.
    UnknownProject(String),
}

impl JqlError {
    /// Returns the Jira error message.
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Unbounded => "Unbounded JQL queries are not allowed here. Please add a search restriction to your query.".to_owned(),
            Self::UnknownField(name) => {
                format!("Field '{name}' does not exist or you do not have permission to view it.")
            }
            Self::BadSort(name) => format!("Not able to sort using field '{name}'."),
            Self::Syntax(what) => format!("Error in the JQL Query: {what}"),
            Self::UnknownProject(value) => {
                format!("The value '{value}' does not exist for the field 'project'.")
            }
        }
    }
}

/// One token of the JQL text.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Word(String),
    Str(String),
    LParen,
    RParen,
    Comma,
    Eq,
    Ne,
    Other(char),
}

/// Splits the JQL text into tokens.
fn lex(text: &str) -> Result<Vec<Tok>, JqlError> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {}
            '(' => out.push(Tok::LParen),
            ')' => out.push(Tok::RParen),
            ',' => out.push(Tok::Comma),
            '=' => out.push(Tok::Eq),
            '!' if chars.peek() == Some(&'=') => {
                chars.next();
                out.push(Tok::Ne);
            }
            '"' | '\'' => {
                let mut value = String::new();
                loop {
                    match chars.next() {
                        None => {
                            return Err(JqlError::Syntax(
                                "The quoted string is not closed.".to_owned(),
                            ));
                        }
                        Some('\\') => match chars.next() {
                            Some(escaped) => value.push(escaped),
                            None => {
                                return Err(JqlError::Syntax(
                                    "The escape at the end is not complete.".to_owned(),
                                ));
                            }
                        },
                        Some(end) if end == c => break,
                        Some(other) => value.push(other),
                    }
                }
                out.push(Tok::Str(value));
            }
            c if is_word_char(c) => {
                let mut word = String::from(c);
                while let Some(&next) = chars.peek() {
                    if !is_word_char(next) {
                        break;
                    }
                    word.push(next);
                    chars.next();
                }
                out.push(Tok::Word(word));
            }
            other => out.push(Tok::Other(other)),
        }
    }
    Ok(out)
}

/// Returns `true` for a character of an unquoted word.
fn is_word_char(c: char) -> bool {
    !c.is_whitespace() && !"()\",'=!~<>".contains(c)
}

/// A cursor over the tokens.
struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let tok = self.toks.get(self.pos).cloned();
        self.pos += 1;
        tok
    }

    /// Returns `true` and moves on when the next token is this keyword.
    fn keyword(&mut self, word: &str) -> bool {
        let found = matches!(self.peek(), Some(Tok::Word(w)) if w.eq_ignore_ascii_case(word));
        if found {
            self.pos += 1;
        }
        found
    }

    fn expect_keyword(&mut self, word: &str) -> Result<(), JqlError> {
        if self.keyword(word) {
            Ok(())
        } else {
            Err(JqlError::Syntax(format!("Expecting '{word}'.")))
        }
    }

    fn clause(&mut self) -> Result<Clause, JqlError> {
        let field = match self.next() {
            Some(Tok::Word(name)) => Field::parse(&name).ok_or(JqlError::UnknownField(name))?,
            _ => return Err(JqlError::Syntax("Expecting a field name.".to_owned())),
        };
        let cond = match self.next() {
            Some(Tok::Eq) => match self.value()? {
                Val::Text(t) if is_empty_word(&t) => Cond::Empty,
                v => Cond::Eq(v),
            },
            Some(Tok::Ne) => match self.value()? {
                Val::Text(t) if is_empty_word(&t) => Cond::NotEmpty,
                v => Cond::Ne(v),
            },
            Some(Tok::Word(w)) if w.eq_ignore_ascii_case("in") => Cond::In(self.list()?),
            Some(Tok::Word(w)) if w.eq_ignore_ascii_case("not") => {
                self.expect_keyword("in")?;
                Cond::NotIn(self.list()?)
            }
            Some(Tok::Word(w)) if w.eq_ignore_ascii_case("is") => {
                let negated = self.keyword("not");
                match self.next() {
                    Some(Tok::Word(w)) if is_empty_word(&w) => {
                        if negated {
                            Cond::NotEmpty
                        } else {
                            Cond::Empty
                        }
                    }
                    _ => return Err(JqlError::Syntax("Expecting EMPTY.".to_owned())),
                }
            }
            _ => {
                return Err(JqlError::Syntax(
                    "Expecting an operator: =, !=, IN, NOT IN or IS.".to_owned(),
                ));
            }
        };
        Ok(Clause { field, cond })
    }

    fn value(&mut self) -> Result<Val, JqlError> {
        match self.next() {
            Some(Tok::Str(s)) => Ok(Val::Text(s)),
            Some(Tok::Word(w)) if w.eq_ignore_ascii_case("currentUser") => {
                if self.next() == Some(Tok::LParen) && self.next() == Some(Tok::RParen) {
                    Ok(Val::Me)
                } else {
                    Err(JqlError::Syntax("Expecting currentUser().".to_owned()))
                }
            }
            Some(Tok::Word(w)) => Ok(Val::Text(w)),
            _ => Err(JqlError::Syntax("Expecting a value.".to_owned())),
        }
    }

    fn list(&mut self) -> Result<Vec<Val>, JqlError> {
        if self.next() != Some(Tok::LParen) {
            return Err(JqlError::Syntax("Expecting '('.".to_owned()));
        }
        let mut values = vec![self.value()?];
        loop {
            match self.next() {
                Some(Tok::Comma) => values.push(self.value()?),
                Some(Tok::RParen) => return Ok(values),
                _ => return Err(JqlError::Syntax("Expecting ',' or ')'.".to_owned())),
            }
        }
    }

    fn order(&mut self) -> Result<Vec<(SortField, bool)>, JqlError> {
        let mut order = Vec::new();
        loop {
            let field = match self.next() {
                Some(Tok::Word(name)) => SortField::parse(&name).ok_or(JqlError::BadSort(name))?,
                _ => return Err(JqlError::Syntax("Expecting a sort field.".to_owned())),
            };
            let descending = if self.keyword("desc") {
                true
            } else {
                self.keyword("asc");
                false
            };
            order.push((field, descending));
            if self.peek() == Some(&Tok::Comma) {
                self.pos += 1;
            } else {
                return Ok(order);
            }
        }
    }
}

/// Returns `true` for `EMPTY` or `NULL`.
const fn is_empty_word(word: &str) -> bool {
    word.eq_ignore_ascii_case("empty") || word.eq_ignore_ascii_case("null")
}

/// Parses a query of the subset.
pub(crate) fn parse(text: &str) -> Result<Query, JqlError> {
    let mut p = Parser {
        toks: lex(text)?,
        pos: 0,
    };
    let mut clauses = Vec::new();
    let starts_with_order =
        matches!(p.peek(), Some(Tok::Word(w)) if w.eq_ignore_ascii_case("order"));
    if p.peek().is_some() && !starts_with_order {
        clauses.push(p.clause()?);
        while p.keyword("and") {
            clauses.push(p.clause()?);
        }
    }
    let order = if p.keyword("order") {
        p.expect_keyword("by")?;
        p.order()?
    } else {
        Vec::new()
    };
    match p.next() {
        None => {}
        Some(Tok::Word(w)) if w.eq_ignore_ascii_case("or") => {
            return Err(JqlError::Syntax(
                "The fake supports AND only, not OR.".to_owned(),
            ));
        }
        Some(_) => {
            return Err(JqlError::Syntax(
                "Expecting either 'AND' or 'ORDER BY'.".to_owned(),
            ));
        }
    }
    if clauses.is_empty() {
        return Err(JqlError::Unbounded);
    }
    Ok(Query { clauses, order })
}

/// What a match needs besides the issue.
pub(crate) struct Ctx<'a> {
    pub(crate) projects: &'a [FakeProject],
    pub(crate) me: &'a str,
}

impl Query {
    /// Checks that each project value names a project.
    pub(crate) fn check_projects(&self, projects: &[FakeProject]) -> Result<(), JqlError> {
        for clause in self.clauses.iter().filter(|c| c.field == Field::Project) {
            let values: &[Val] = match &clause.cond {
                Cond::Eq(v) | Cond::Ne(v) => std::slice::from_ref(v),
                Cond::In(vs) | Cond::NotIn(vs) => vs,
                Cond::Empty | Cond::NotEmpty => &[],
            };
            for value in values {
                if let Val::Text(text) = value
                    && !projects.iter().any(|p| project_matches(p, text))
                {
                    return Err(JqlError::UnknownProject(text.clone()));
                }
            }
        }
        Ok(())
    }

    /// Returns `true` when the issue matches every clause.
    pub(crate) fn matches(&self, issue: &Issue, ctx: &Ctx<'_>) -> bool {
        self.clauses.iter().all(|c| clause_matches(c, issue, ctx))
    }

    /// Sorts issues by `ORDER BY`. Equal issues keep id order. With no
    /// `ORDER BY`, the newest issue comes first.
    pub(crate) fn sort(&self, issues: &mut [&Issue], ctx: &Ctx<'_>) {
        if self.order.is_empty() {
            issues.sort_by_key(|i| std::cmp::Reverse(i.id));
            return;
        }
        issues.sort_by(|a, b| {
            self.order
                .iter()
                .map(|(field, descending)| {
                    let ord = compare(*field, a, b, ctx);
                    if *descending { ord.reverse() } else { ord }
                })
                .find(|o| o.is_ne())
                .unwrap_or_else(|| a.id.cmp(&b.id))
        });
    }
}

/// Returns `true` when a project matches a value: key, id or name.
fn project_matches(project: &FakeProject, value: &str) -> bool {
    project.key.eq_ignore_ascii_case(value)
        || project.id == value
        || project.name.eq_ignore_ascii_case(value)
}

/// Returns `true` when the field of an issue has a value.
fn has_value(field: Field, issue: &Issue) -> bool {
    match field {
        Field::Labels => !issue.labels.is_empty(),
        Field::Assignee => issue.assignee.is_some(),
        _ => true,
    }
}

/// Returns `true` when the field of an issue holds the value.
fn holds(field: Field, issue: &Issue, value: &Val, ctx: &Ctx<'_>) -> bool {
    let text = match value {
        Val::Me => ctx.me,
        Val::Text(t) => t.as_str(),
    };
    let status = issue.status();
    match field {
        Field::Project => ctx
            .projects
            .iter()
            .find(|p| p.key == issue.project)
            .is_some_and(|p| project_matches(p, text)),
        Field::Key => issue.key.eq_ignore_ascii_case(text) || issue.id.to_string() == text,
        Field::Labels => issue.labels.contains(text),
        Field::Status => status.name.eq_ignore_ascii_case(text) || status.id == text,
        Field::StatusCategory => {
            let category = status.category;
            category.key.eq_ignore_ascii_case(text)
                || category.name.eq_ignore_ascii_case(text)
                || category.id.to_string() == text
        }
        Field::IssueType => ctx
            .projects
            .iter()
            .find(|p| p.key == issue.project)
            .and_then(|p| p.issue_types.iter().find(|t| t.id == issue.type_id))
            .is_some_and(|t| t.name.eq_ignore_ascii_case(text) || t.id == text),
        Field::Assignee => issue.assignee.as_deref() == Some(text),
    }
}

/// Returns `true` when an issue matches one clause. As in Jira, `!=` and
/// `NOT IN` do not match an issue with no value.
fn clause_matches(clause: &Clause, issue: &Issue, ctx: &Ctx<'_>) -> bool {
    let field = clause.field;
    let present = has_value(field, issue);
    match &clause.cond {
        Cond::Eq(v) => holds(field, issue, v, ctx),
        Cond::Ne(v) => present && !holds(field, issue, v, ctx),
        Cond::In(vs) => vs.iter().any(|v| holds(field, issue, v, ctx)),
        Cond::NotIn(vs) => present && !vs.iter().any(|v| holds(field, issue, v, ctx)),
        Cond::Empty => !present,
        Cond::NotEmpty => present,
    }
}

/// Compares two issues on one sort field.
fn compare(field: SortField, a: &Issue, b: &Issue, ctx: &Ctx<'_>) -> Ordering {
    match field {
        SortField::Created => a.created.cmp(&b.created),
        SortField::Updated => a.updated.cmp(&b.updated),
        SortField::Summary => a.summary.cmp(&b.summary),
        SortField::Priority => a.priority.cmp(b.priority),
        SortField::Field(Field::Project | Field::Key) => {
            (&a.project, a.number).cmp(&(&b.project, b.number))
        }
        SortField::Field(Field::Labels) => a.labels.cmp(&b.labels),
        SortField::Field(Field::Status | Field::StatusCategory) => {
            a.status().rank.cmp(&b.status().rank)
        }
        SortField::Field(Field::IssueType) => {
            let name = |i: &Issue| {
                ctx.projects
                    .iter()
                    .find(|p| p.key == i.project)
                    .and_then(|p| p.issue_types.iter().find(|t| t.id == i.type_id))
                    .map(|t| t.name.clone())
            };
            name(a).cmp(&name(b))
        }
        SortField::Field(Field::Assignee) => a.assignee.cmp(&b.assignee),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexer_and_parser_edges() {
        assert_eq!(
            parse("project = 'OPS' AND labels = 'it\\'s'")
                .unwrap()
                .clauses
                .len(),
            2
        );
        assert!(matches!(parse("labels = \"a\\"), Err(JqlError::Syntax(_))));
        assert!(matches!(
            parse("assignee = currentUser"),
            Err(JqlError::Syntax(_))
        ));
        assert!(matches!(parse("project is OPS"), Err(JqlError::Syntax(_))));
        assert!(matches!(
            parse("project not = OPS"),
            Err(JqlError::Syntax(_))
        ));
        assert!(matches!(
            parse("project = OPS ORDER key"),
            Err(JqlError::Syntax(_))
        ));
        assert!(matches!(
            parse("project = OPS ORDER BY ,"),
            Err(JqlError::Syntax(_))
        ));
        assert!(matches!(parse("= OPS"), Err(JqlError::Syntax(_))));
        assert!(matches!(parse("project = ,"), Err(JqlError::Syntax(_))));
        assert!(matches!(parse("project in OPS"), Err(JqlError::Syntax(_))));
        assert_eq!(
            parse("assignee = EMPTY AND labels != null")
                .unwrap()
                .clauses,
            [
                Clause {
                    field: Field::Assignee,
                    cond: Cond::Empty
                },
                Clause {
                    field: Field::Labels,
                    cond: Cond::NotEmpty
                },
            ]
        );
        let order = parse("project = OPS ORDER BY priority, labels ASC, type DESC, assignee")
            .unwrap()
            .order;
        assert_eq!(order.len(), 4);
    }
}
