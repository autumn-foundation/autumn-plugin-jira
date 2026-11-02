//! The fixed data of the fake site: statuses, the workflow graph,
//! resolutions, priorities and users.

use crate::{ANN, BOT, DEFAULT_EMAIL};

/// A status category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Category {
    pub(crate) id: u64,
    pub(crate) key: &'static str,
    pub(crate) name: &'static str,
    pub(crate) color: &'static str,
}

/// A workflow status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Status {
    pub(crate) id: &'static str,
    pub(crate) name: &'static str,
    pub(crate) category: Category,
    /// The place of the status in the workflow. Search sorts by it.
    pub(crate) rank: u8,
}

/// The `new` category.
const NEW: Category = Category {
    id: 2,
    key: "new",
    name: "To Do",
    color: "blue-gray",
};
/// The `indeterminate` category.
const INDETERMINATE: Category = Category {
    id: 4,
    key: "indeterminate",
    name: "In Progress",
    color: "yellow",
};
/// The `done` category.
const DONE_CATEGORY: Category = Category {
    id: 3,
    key: "done",
    name: "Done",
    color: "green",
};

/// The status of a new issue.
pub(crate) const TODO: Status = Status {
    id: "10000",
    name: "To Do",
    category: NEW,
    rank: 0,
};
/// The work status.
pub(crate) const IN_PROGRESS: Status = Status {
    id: "10001",
    name: "In Progress",
    category: INDETERMINATE,
    rank: 1,
};
/// The end status. Its transition needs a resolution.
pub(crate) const DONE: Status = Status {
    id: "10002",
    name: "Done",
    category: DONE_CATEGORY,
    rank: 2,
};

/// All statuses.
pub(crate) const STATUSES: [Status; 3] = [TODO, IN_PROGRESS, DONE];

/// Returns the status with this id. An unknown id gives To Do.
pub(crate) fn status(id: &str) -> Status {
    STATUSES.into_iter().find(|s| s.id == id).unwrap_or(TODO)
}

/// One edge of the workflow graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Transition {
    pub(crate) id: &'static str,
    /// The name differs from the target status name on purpose. A client
    /// must match on `to`, not on the name.
    pub(crate) name: &'static str,
    pub(crate) from: &'static str,
    pub(crate) to: &'static str,
    pub(crate) needs_resolution: bool,
}

/// The workflow graph: To Do to In Progress to Done, and Done to To Do.
pub(crate) const TRANSITIONS: [Transition; 3] = [
    Transition {
        id: "21",
        name: "Start work",
        from: TODO.id,
        to: IN_PROGRESS.id,
        needs_resolution: false,
    },
    Transition {
        id: "31",
        name: "Resolve",
        from: IN_PROGRESS.id,
        to: DONE.id,
        needs_resolution: true,
    },
    Transition {
        id: "11",
        name: "Reopen",
        from: DONE.id,
        to: TODO.id,
        needs_resolution: false,
    },
];

/// Returns the transitions out of a status.
pub(crate) fn transitions_from(status_id: &str) -> impl Iterator<Item = Transition> + '_ {
    TRANSITIONS.into_iter().filter(move |t| t.from == status_id)
}

/// A resolution: id, name and description.
pub(crate) type Resolution = (&'static str, &'static str, &'static str);

/// The resolutions of the site.
pub(crate) const RESOLUTIONS: [Resolution; 4] = [
    ("10000", "Done", "Work on this issue is complete."),
    ("10001", "Won't Do", "This issue will not be done."),
    (
        "10002",
        "Duplicate",
        "The problem is a duplicate of an issue.",
    ),
    (
        "10003",
        "Cannot Reproduce",
        "The problem cannot be reproduced.",
    ),
];

/// Returns the resolution with this id.
pub(crate) fn resolution(id: &str) -> Option<Resolution> {
    RESOLUTIONS.into_iter().find(|r| r.0 == id)
}

/// A priority: id and name.
pub(crate) type Priority = (&'static str, &'static str);

/// The priorities of the site, from high to low.
pub(crate) const PRIORITIES: [Priority; 5] = [
    ("1", "Highest"),
    ("2", "High"),
    ("3", "Medium"),
    ("4", "Low"),
    ("5", "Lowest"),
];

/// The default priority.
pub(crate) const MEDIUM: &str = "3";

/// Returns the priority with this id. An unknown id gives Medium.
pub(crate) fn priority(id: &str) -> Priority {
    PRIORITIES
        .into_iter()
        .find(|p| p.0 == id)
        .unwrap_or(("3", "Medium"))
}

/// A user of the site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct User {
    pub(crate) account_id: &'static str,
    pub(crate) alias: &'static str,
    pub(crate) display_name: &'static str,
    pub(crate) email: &'static str,
}

/// The users of the site: ann, a person, and the bot, the API account.
pub(crate) const USERS: [User; 2] = [
    User {
        account_id: ANN,
        alias: "ann",
        display_name: "Ann Example",
        email: "ann@acme.example",
    },
    User {
        account_id: BOT,
        alias: "bot",
        display_name: "Ops Bot",
        email: DEFAULT_EMAIL,
    },
];

/// Returns the account id for an actor: an alias (`ann`) or an account id.
pub(crate) fn account_for(actor: &str) -> String {
    USERS
        .iter()
        .find(|u| u.alias == actor || u.account_id == actor)
        .map_or_else(|| actor.to_owned(), |u| u.account_id.to_owned())
}

/// Returns the known user with this account id.
pub(crate) fn user(account_id: &str) -> Option<User> {
    USERS.into_iter().find(|u| u.account_id == account_id)
}
