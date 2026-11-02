//! `SearchPager`: the token pager of `POST /search/jql`.

use std::collections::BTreeSet;

use super::JiraClient;
use crate::error::Result;
use crate::ids::PageToken;
use crate::model::Issue;
use crate::request::SearchRequest;

/// Reads the pages of one search. It stops on `isLast`, on no token, on a
/// repeated token (`PaginationStalled`) and at `max_pages` (`PageLimit`).
#[derive(Debug)]
pub struct SearchPager {
    client: JiraClient,
    request: SearchRequest,
    token: Option<PageToken>,
    seen: BTreeSet<String>,
    pages: u32,
    done: bool,
}

impl SearchPager {
    /// Makes a pager. It makes no call.
    pub(crate) const fn new(client: JiraClient, request: SearchRequest) -> Self {
        Self {
            client,
            request,
            token: None,
            seen: BTreeSet::new(),
            pages: 0,
            done: false,
        }
    }

    /// Reads the next page. `Ok(None)` at the end.
    ///
    /// # Errors
    ///
    /// `PaginationStalled` on a repeated token. `PageLimit` at
    /// `max_pages`. The typed errors of the call. After an error, the next
    /// call gives `Ok(None)`.
    pub async fn next_page(&mut self) -> Result<Option<Vec<Issue>>> {
        let _ = (&self.client, &self.request, &self.token, &self.seen, self.done);
        Err(super::stub())
    }

    /// Returns the number of pages read.
    #[must_use]
    pub const fn pages(&self) -> u32 {
        self.pages
    }
}
