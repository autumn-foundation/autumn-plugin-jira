//! The values of the env vars that the config names.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use autumn_jira::san_mateo::Secret;

use crate::error::PluginError;

/// Reads secret values: the builder overrides, then the process env, then
/// `.env`. `Debug` shows the names of the overrides, never a value.
pub(crate) struct Secrets {
    overrides: BTreeMap<String, String>,
    dotenv: OnceLock<Vec<(String, String)>>,
}

impl std::fmt::Debug for Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Names only. The values are secrets.
        f.debug_struct("Secrets")
            .field("overrides", &self.overrides.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl Secrets {
    /// Makes the reader with the builder overrides.
    pub(crate) const fn new(overrides: BTreeMap<String, String>) -> Self {
        Self {
            overrides,
            dotenv: OnceLock::new(),
        }
    }

    /// The value of the env var `name`, if it is set and not empty.
    pub(crate) fn get(&self, name: &str) -> Option<String> {
        self.overrides
            .get(name)
            .cloned()
            .or_else(|| std::env::var(name).ok())
            .or_else(|| self.dotenv_var(name))
            .filter(|v| !v.trim().is_empty())
    }

    /// The value of `name` as a [`Secret`].
    ///
    /// # Errors
    /// [`PluginError::MissingSecret`] with the name when the value is not
    /// set or is empty.
    pub(crate) fn require(&self, name: &str) -> Result<Secret, PluginError> {
        self.get(name)
            .map(Secret::new)
            .ok_or_else(|| PluginError::MissingSecret {
                env: name.to_owned(),
            })
    }

    /// The value of `name` in `.env`. The plugin reads the file one time.
    fn dotenv_var(&self, name: &str) -> Option<String> {
        self.dotenv
            .get_or_init(|| autumn_web::dotenv::resolve_process_dotenv().unwrap_or_default())
            .iter()
            .find_map(|(k, v)| (k == name).then(|| v.clone()))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;

    #[test]
    fn overrides_win_and_empty_values_count_as_missing() {
        let secrets = Secrets::new(BTreeMap::from([
            ("JIRA_T_SET".to_owned(), "value-1".to_owned()),
            ("JIRA_T_EMPTY".to_owned(), "  ".to_owned()),
        ]));
        assert_eq!(secrets.get("JIRA_T_SET").as_deref(), Some("value-1"));
        assert_eq!(secrets.require("JIRA_T_SET").unwrap().expose(), "value-1");
        assert!(secrets.get("JIRA_T_EMPTY").is_none());
        let e = secrets.require("AUTUMN_PLUGIN_JIRA_T_UNSET").unwrap_err();
        assert_eq!(
            e.to_string(),
            "set the environment variable AUTUMN_PLUGIN_JIRA_T_UNSET"
        );
        let text = format!("{secrets:?}");
        assert!(
            text.contains("JIRA_T_SET") && !text.contains("value-1"),
            "{text}"
        );
    }
}
