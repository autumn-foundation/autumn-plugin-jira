//! `SearchPager`: the token pager of `POST /search/jql`.

use std::collections::BTreeSet;

use super::JiraClient;
use crate::error::{JiraError, Op, Result};
use crate::ids::PageToken;
use crate::model::Issue;
use crate::policy::{self, PageStep};
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
        if self.done {
            return Ok(None);
        }
        let page = match self
            .client
            .search_page(&self.request, self.token.as_ref())
            .await
        {
            Ok(page) => page,
            Err(e) => {
                self.done = true;
                return Err(e);
            }
        };
        self.pages += 1;
        let next = page.next_page_token;
        let repeated = next
            .as_ref()
            .is_some_and(|t| self.seen.contains(t.as_str()));
        let max = self.client.settings.max_pages;
        match policy::token_page_step(self.pages, max, page.is_last, next.is_some(), repeated) {
            PageStep::Fetch => {
                if let Some(token) = next {
                    self.seen.insert(token.as_str().to_owned());
                    self.token = Some(token);
                }
                Ok(Some(page.issues))
            }
            PageStep::Done => {
                self.done = true;
                Ok(Some(page.issues))
            }
            PageStep::Stalled => {
                self.done = true;
                Err(JiraError::PaginationStalled {
                    op: Op::Search,
                    pages: self.pages,
                })
            }
            PageStep::Limit => {
                self.done = true;
                Err(JiraError::PageLimit { op: Op::Search, max })
            }
        }
    }

    /// Returns the number of pages read.
    #[must_use]
    pub const fn pages(&self) -> u32 {
        self.pages
    }
}
