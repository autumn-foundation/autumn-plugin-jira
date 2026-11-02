//! `JiraService`: the started client, for handlers and jobs.

use std::ops::Deref;

use autumn_jira::{JiraClient, JiraError};
use autumn_web::reexports::axum::extract::FromRequestParts;
use autumn_web::reexports::http::request::Parts;
use autumn_web::{AppState, AutumnError, AutumnResult};

/// The message of a 503 before the plugin starts.
pub(crate) const NOT_STARTED: &str = "Jira has not started";

/// The started Jira client. The plugin puts it in the app state.
#[derive(Clone, Debug)]
pub struct JiraService {
    client: JiraClient,
}

impl JiraService {
    /// The service, if the plugin started.
    #[must_use]
    pub fn from_state(state: &AppState) -> Option<Self> {
        state.extension::<Self>().map(|s| (*s).clone())
    }

    /// The service.
    ///
    /// # Errors
    /// Returns a 503 [`AutumnError`] when the plugin did not start.
    pub fn require(state: &AppState) -> AutumnResult<Self> {
        Self::from_state(state).ok_or_else(|| AutumnError::service_unavailable_msg(NOT_STARTED))
    }

    /// The client.
    #[must_use]
    pub const fn client(&self) -> &JiraClient {
        &self.client
    }

    /// San Mateo mode: a client for another connection of the same
    /// integration.
    ///
    /// # Errors
    /// Returns a [`JiraError`].
    pub fn for_connection(&self, connection_id: &str) -> Result<JiraClient, JiraError> {
        let _ = connection_id;
        Err(JiraError::Config { code: "todo" })
    }
}

impl Deref for JiraService {
    type Target = JiraClient;

    fn deref(&self) -> &JiraClient {
        &self.client
    }
}

impl FromRequestParts<AppState> for JiraService {
    type Rejection = AutumnError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let _ = (parts, state);
        Err(AutumnError::internal_server_error_msg("stub"))
    }
}

/// Converts a Jira error to an [`AutumnError`].
#[must_use]
pub fn autumn_error(err: &JiraError) -> AutumnError {
    let _ = err;
    AutumnError::internal_server_error_msg("stub")
}

/// `.http()` on a Jira result.
pub trait JiraResultExt<T> {
    /// Maps the error with [`autumn_error`].
    ///
    /// # Errors
    /// The mapped error.
    fn http(self) -> AutumnResult<T>;
}

impl<T> JiraResultExt<T> for Result<T, JiraError> {
    fn http(self) -> AutumnResult<T> {
        self.map_err(|e| autumn_error(&e))
    }
}
