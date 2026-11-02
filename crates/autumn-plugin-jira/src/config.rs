//! `[jira]` configuration.
//!
//! Sources, in order (later wins):
//! 1. `[jira]` in `autumn.toml`.
//! 2. `[jira]` in the profile file, for example `autumn-prod.toml`.
//! 3. `.env` values, then the process environment: `AUTUMN_JIRA__<PATH>`,
//!    for example `AUTUMN_JIRA__HTTP__MAX_PAGES=200`. Put a comma between
//!    list items.
//!
//! The config holds no secrets. It holds the names of the env vars that hold
//! them. The plugin reads those env vars at startup.
//!
//! The table that exists sets the mode: `[jira.api_token]` or
//! `[jira.san_mateo]`. Do not set both.
//!
//! ```toml
//! [jira.api_token]
//! site = "acme"
//! email_env = "JIRA_EMAIL"
//! token_env = "JIRA_API_TOKEN"
//!
//! [jira.webhook]
//! max_age_secs = 7200
//! ```

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::PluginError;
use crate::{ENDPOINT_NAME, ENV_PREFIX, SECTION};

/// The plugin configuration: the `[jira]` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct JiraConfig {
    /// API token mode (Basic auth). Its presence selects the mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_token: Option<ApiTokenConfig>,
    /// San Mateo mode. Its presence selects the mode. It needs
    /// `SanMateoPlugin`, registered before `JiraPlugin`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub san_mateo: Option<SanMateoTarget>,
    /// Retry, page and timeout settings of the client.
    pub http: JiraHttpConfig,
    /// Settings of the webhook route.
    pub webhook: JiraWebhookConfig,
    /// Settings of the health indicator.
    pub health: JiraHealthConfig,
}

/// The mode of the plugin. The table that exists in `[jira]` selects it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JiraMode {
    /// `[jira.api_token]`: Basic auth with an email and an API token.
    ApiToken,
    /// `[jira.san_mateo]`: San Mateo keeps the credentials.
    SanMateo,
}

impl JiraMode {
    /// The name of the mode, the same as the name of its table.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ApiToken => "api_token",
            Self::SanMateo => "san_mateo",
        }
    }
}

/// `[jira.api_token]`: Basic auth with an email and an API token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct ApiTokenConfig {
    /// The site name, `acme` for `https://acme.atlassian.net`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
    /// The cloud id, for a scoped token. Set `site` or `cloud_id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cloud_id: Option<String>,
    /// The env var with the email of the Atlassian account.
    pub email_env: String,
    /// The env var with the API token.
    pub token_env: String,
}

impl Default for ApiTokenConfig {
    fn default() -> Self {
        Self {
            site: None,
            cloud_id: None,
            email_env: "JIRA_EMAIL".to_owned(),
            token_env: "JIRA_API_TOKEN".to_owned(),
        }
    }
}

/// `[jira.san_mateo]`: the San Mateo connection that gives the auth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct SanMateoTarget {
    /// The San Mateo environment id.
    pub environment: String,
    /// The integration id. Its provider is `jira-basic` or `atlassian`.
    pub integration: String,
    /// The connection id. Optional: `for_connection(id)` serves other
    /// tenants.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,
    /// The cloud id of the `connection_id` connection. Optional,
    /// `atlassian` only. It needs `connection_id`.
    ///
    /// The clients of `for_connection(id)` for other ids do not use it:
    /// each tenant gets its site from the `cloudId` in its own connection
    /// metadata, else from a discovery with its own token.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cloud_id: Option<String>,
    /// The site URL of the `connection_id` connection. Optional. It
    /// selects the site at discovery. It needs `connection_id`, and other
    /// connections do not use it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site_url: Option<String>,
}

impl Default for SanMateoTarget {
    fn default() -> Self {
        Self {
            environment: "default".to_owned(),
            integration: "jira".to_owned(),
            connection_id: None,
            cloud_id: None,
            site_url: None,
        }
    }
}

/// `[jira.http]`: retry, page and timeout settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct JiraHttpConfig {
    /// The most attempts for one safe call. 1 or more.
    pub max_attempts: u32,
    /// The first backoff in milliseconds.
    pub base_backoff_ms: u64,
    /// The longest backoff in milliseconds.
    pub max_backoff_ms: u64,
    /// The most pages that a pager reads. 1 or more.
    pub max_pages: u32,
    /// The most marker lookups after a failed create. 1 or more.
    pub lookup_attempts: u32,
    /// The wait between two marker lookups in milliseconds.
    pub lookup_delay_ms: u64,
    /// The request timeout in API token mode, in seconds. 1 or more.
    pub timeout_secs: u64,
}

impl Default for JiraHttpConfig {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            base_backoff_ms: 2000,
            max_backoff_ms: 30_000,
            max_pages: 1000,
            lookup_attempts: 3,
            lookup_delay_ms: 2000,
            timeout_secs: 30,
        }
    }
}

/// `[jira.webhook]`: settings of the webhook route.
///
/// `Debug` shows the number of ignored account ids, not the ids. An
/// account id identifies a person.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct JiraWebhookConfig {
    /// The name of the `[[security.webhooks.endpoints]]` entry.
    pub endpoint_name: String,
    /// The oldest event that the route accepts, in seconds. It must not be
    /// over the `replay_window_secs` of the endpoint.
    pub max_age_secs: u64,
    /// The most that an event time can be in the future, in seconds.
    pub max_future_skew_secs: u64,
    /// The longest wait for each sink, in milliseconds. 1 or more.
    ///
    /// After this time the route answers 503, and the task of the sink
    /// continues until `abandon_after_ms`.
    pub handler_timeout_ms: u64,
    /// The most handler tasks that run at the same time. 1 or more.
    ///
    /// A task that times out continues, and it holds its slot until it
    /// ends. When each slot holds a task, the route answers 503 at once,
    /// starts no task, and adds one to `timed_out`. So a sink that hangs
    /// cannot make tasks without a limit.
    pub max_handler_tasks: u32,
    /// The longest time of one handler task, in milliseconds. Then the
    /// plugin stops the task. It must be `handler_timeout_ms` or more.
    pub abandon_after_ms: u64,
    /// Events by these account ids are ignored. Use a dedicated service
    /// account for the writes of the app, and put its id here.
    pub ignore_actor_account_ids: Vec<String>,
}

impl Default for JiraWebhookConfig {
    fn default() -> Self {
        Self {
            endpoint_name: ENDPOINT_NAME.to_owned(),
            max_age_secs: 7200,
            max_future_skew_secs: 300,
            handler_timeout_ms: 8000,
            max_handler_tasks: 64,
            abandon_after_ms: 300_000,
            ignore_actor_account_ids: Vec::new(),
        }
    }
}

impl std::fmt::Debug for JiraWebhookConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JiraWebhookConfig")
            .field("endpoint_name", &self.endpoint_name)
            .field("max_age_secs", &self.max_age_secs)
            .field("max_future_skew_secs", &self.max_future_skew_secs)
            .field("handler_timeout_ms", &self.handler_timeout_ms)
            .field("max_handler_tasks", &self.max_handler_tasks)
            .field("abandon_after_ms", &self.abandon_after_ms)
            // A count only: an account id identifies a person.
            .field(
                "ignore_actor_account_ids",
                &self.ignore_actor_account_ids.len(),
            )
            .finish()
    }
}

/// `[jira.health]`: settings of the health indicator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[non_exhaustive]
pub struct JiraHealthConfig {
    /// The health check calls Jira at most one time in this period.
    pub cache_secs: u64,
    /// The timeout of one health check, in milliseconds. 1 or more.
    pub timeout_ms: u64,
}

impl Default for JiraHealthConfig {
    fn default() -> Self {
        Self {
            cache_secs: 60,
            timeout_ms: 5000,
        }
    }
}

impl JiraConfig {
    /// Reads `[jira]` from the text of an `autumn.toml` file.
    ///
    /// # Errors
    /// Returns [`PluginError::Config`] for bad TOML, unknown keys, or failed
    /// validation.
    pub fn from_toml_str(autumn_toml: &str) -> Result<Self, PluginError> {
        Self::from_layers(Some(autumn_toml), None, std::iter::empty())
    }

    /// Merges, in order: `[jira]` in `base`, `[jira]` in `profile_file`, and
    /// env vars. Then validates.
    fn from_layers(
        base: Option<&str>,
        profile_file: Option<&str>,
        env: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, PluginError> {
        let mut table = toml::Table::new();
        for text in [base, profile_file].into_iter().flatten() {
            let parsed: toml::Table =
                toml::from_str(text).map_err(|e| PluginError::config(format!("toml: {e}")))?;
            if let Some(toml::Value::Table(section)) = parsed.get(SECTION) {
                merge(&mut table, section.clone());
            }
        }
        let schema = schema()?;
        for (key, value) in env {
            let Some(path) = key.strip_prefix(ENV_PREFIX) else {
                continue;
            };
            let path: Vec<String> = path.split("__").map(str::to_ascii_lowercase).collect();
            // The error names the variable, never the value.
            let typed = typed_env_value(&schema, &path, &value)
                .ok_or_else(|| PluginError::config(format!("{key}: the value is not valid")))?;
            insert_path(&mut table, &path, typed);
        }
        reject_secret_keys(&table)?;
        // A serde type error can quote a file value. The secret keys fail
        // above, so such an error never quotes a secret.
        let cfg: Self = toml::Value::Table(table)
            .try_into()
            .map_err(|e: toml::de::Error| PluginError::config(e.to_string()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Loads config from files in `manifest_dir` (else the current
    /// directory) and from `env`.
    ///
    /// `profile_names` are the profile file names to try, in order. The
    /// first file that exists counts.
    ///
    /// # Errors
    /// Returns [`PluginError::Config`] for a file that does not read or
    /// parse, or for failed validation.
    pub fn load_from_dir(
        manifest_dir: &Path,
        profile_names: &[String],
        env: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, PluginError> {
        let find = |file: &str| {
            let candidate = manifest_dir.join(file);
            if candidate.exists() {
                candidate
            } else {
                PathBuf::from(file)
            }
        };
        let base = read_optional(&find("autumn.toml"))?;
        let mut profile = None;
        for name in profile_names {
            if let Some(text) = read_optional(&find(&format!("autumn-{name}.toml")))? {
                profile = Some(text);
                break;
            }
        }
        Self::from_layers(base.as_deref(), profile.as_deref(), env)
    }

    /// Loads config like autumn-web does: `$AUTUMN_MANIFEST_DIR` (else `.`),
    /// the app profile, `.env`, and the process environment.
    ///
    /// # Errors
    /// Returns [`PluginError::Config`] when loading or validation fails.
    pub fn load(profile: Option<&str>) -> Result<Self, PluginError> {
        use autumn_web::config::Env as _;
        let os = autumn_web::config::OsEnv;
        let names = profile
            .map(|p| {
                // Same selector order as autumn-web: env vars, then `--profile`.
                let selector = ["AUTUMN_ENV", "AUTUMN_PROFILE"]
                    .iter()
                    .find_map(|k| os.var(k).ok().filter(|v| !v.trim().is_empty()))
                    .or_else(profile_flag)
                    .map_or_else(|| p.to_owned(), |v| v.trim().to_owned());
                autumn_web::config::profile_override_file_lookup_names(p, &selector)
            })
            .unwrap_or_default();
        let dir = os
            .var("AUTUMN_MANIFEST_DIR")
            .map_or_else(|_| PathBuf::from("."), PathBuf::from);
        Self::load_from_dir(&dir, &names, process_env()?)
    }

    /// Checks the values.
    ///
    /// # Errors
    /// Returns [`PluginError::Config`] with the first problem found.
    pub fn validate(&self) -> Result<(), PluginError> {
        self.mode()?;
        if let Some(api) = &self.api_token {
            let site = api.site.as_deref().filter(|s| !s.trim().is_empty());
            let cloud = api.cloud_id.as_deref().filter(|s| !s.trim().is_empty());
            match (site, cloud) {
                (None, None) => {
                    return Err(PluginError::config(
                        "set site or cloud_id in [jira.api_token]",
                    ));
                }
                (Some(_), Some(_)) => {
                    return Err(PluginError::config(
                        "set site or cloud_id in [jira.api_token], not both",
                    ));
                }
                _ => {}
            }
            require_text(&api.email_env, "[jira.api_token] email_env")?;
            require_text(&api.token_env, "[jira.api_token] token_env")?;
        }
        if let Some(target) = &self.san_mateo {
            require_text(&target.environment, "[jira.san_mateo] environment")?;
            require_text(&target.integration, "[jira.san_mateo] integration")?;
            let set = |v: &Option<String>| v.as_deref().is_some_and(|s| !s.trim().is_empty());
            if !set(&target.connection_id) && (set(&target.cloud_id) || set(&target.site_url)) {
                return Err(PluginError::config(
                    "[jira.san_mateo] cloud_id and site_url apply to connection_id only; set connection_id, or remove them and store cloudId in the metadata of each connection",
                ));
            }
        }
        let http = &self.http;
        require_positive(u64::from(http.max_attempts), "[jira.http] max_attempts")?;
        require_positive(u64::from(http.max_pages), "[jira.http] max_pages")?;
        require_positive(
            u64::from(http.lookup_attempts),
            "[jira.http] lookup_attempts",
        )?;
        require_positive(http.timeout_secs, "[jira.http] timeout_secs")?;
        if http.max_backoff_ms < http.base_backoff_ms {
            return Err(PluginError::config(
                "[jira.http] max_backoff_ms must be base_backoff_ms or more",
            ));
        }
        require_text(&self.webhook.endpoint_name, "[jira.webhook] endpoint_name")?;
        require_positive(self.webhook.max_age_secs, "[jira.webhook] max_age_secs")?;
        require_positive(
            self.webhook.handler_timeout_ms,
            "[jira.webhook] handler_timeout_ms",
        )?;
        require_positive(
            u64::from(self.webhook.max_handler_tasks),
            "[jira.webhook] max_handler_tasks",
        )?;
        if self.webhook.abandon_after_ms < self.webhook.handler_timeout_ms {
            return Err(PluginError::config(
                "[jira.webhook] abandon_after_ms must be handler_timeout_ms or more",
            ));
        }
        require_positive(self.health.timeout_ms, "[jira.health] timeout_ms")?;
        Ok(())
    }

    /// The mode that the tables select. `None` when neither table exists.
    ///
    /// # Errors
    /// Returns [`PluginError::Config`] when both tables exist.
    pub fn mode(&self) -> Result<Option<JiraMode>, PluginError> {
        match (&self.api_token, &self.san_mateo) {
            (Some(_), Some(_)) => Err(PluginError::config(
                "set [jira.api_token] or [jira.san_mateo], not both",
            )),
            (Some(_), None) => Ok(Some(JiraMode::ApiToken)),
            (None, Some(_)) => Ok(Some(JiraMode::SanMateo)),
            (None, None) => Ok(None),
        }
    }
}

/// The keys that would hold a secret value. The config holds env var names.
const SECRET_KEYS: [&str; 3] = ["token", "api_token", "password"];

/// Fails when a table holds a secret value: a key `token`, `password`, or an
/// `api_token` that is not the mode table. The text names `token_env`.
fn reject_secret_keys(section: &toml::Table) -> Result<(), PluginError> {
    let fail = |at: &str, key: &str| {
        Err(PluginError::config(format!(
            "{at} {key}: do not put a secret in the config; set [jira.api_token] token_env to the name of the env var that holds the API token"
        )))
    };
    for (key, value) in section {
        let mode_table = key == "api_token" && value.is_table();
        if SECRET_KEYS.contains(&key.as_str()) && !mode_table {
            return fail("[jira]", key);
        }
    }
    for table in ["api_token", "san_mateo"] {
        if let Some(toml::Value::Table(inner)) = section.get(table)
            && let Some(key) = inner.keys().find(|k| SECRET_KEYS.contains(&k.as_str()))
        {
            return fail(&format!("[jira.{table}]"), key);
        }
    }
    Ok(())
}

fn require_text(value: &str, what: &str) -> Result<(), PluginError> {
    if value.trim().is_empty() {
        return Err(PluginError::config(format!("{what} must not be empty")));
    }
    Ok(())
}

fn require_positive(value: u64, what: &str) -> Result<(), PluginError> {
    if value == 0 {
        return Err(PluginError::config(format!("{what} must be 1 or more")));
    }
    Ok(())
}

/// The schema for env values: the defaults, with both mode tables.
fn schema() -> Result<toml::Table, PluginError> {
    let sample = JiraConfig {
        api_token: Some(ApiTokenConfig::default()),
        san_mateo: Some(SanMateoTarget::default()),
        ..JiraConfig::default()
    };
    toml::Table::try_from(sample).map_err(|e| PluginError::config(e.to_string()))
}

/// Returns `.env` values, then the process environment (later wins).
///
/// It skips a variable whose name or value is not UTF-8.
fn process_env() -> Result<Vec<(String, String)>, PluginError> {
    let mut vars = autumn_web::dotenv::resolve_process_dotenv()
        .map_err(|e| PluginError::config(format!(".env: {e}")))?;
    vars.extend(
        std::env::vars_os()
            .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?))),
    );
    Ok(vars)
}

/// The value of `--profile <name>` or `--profile=<name>` in the process args.
fn profile_flag() -> Option<String> {
    let args: Vec<String> = std::env::args_os()
        .filter_map(|a| a.into_string().ok())
        .collect();
    args.iter()
        .enumerate()
        .find_map(|(i, a)| {
            a.strip_prefix("--profile=").map(str::to_owned).or_else(|| {
                (a == "--profile")
                    .then(|| args.get(i + 1).cloned())
                    .flatten()
            })
        })
        // Like autumn-web: an empty value selects nothing.
        .filter(|p| !p.trim().is_empty())
}

fn read_optional(path: &Path) -> Result<Option<String>, PluginError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(PluginError::config(format!("{}: {e}", path.display()))),
    }
}

/// Deep merge: tables merge by key; other values replace.
fn merge(into: &mut toml::Table, from: toml::Table) {
    for (key, value) in from {
        match (into.get_mut(&key), value) {
            (Some(toml::Value::Table(dst)), toml::Value::Table(src)) => merge(dst, src),
            (_, value) => {
                into.insert(key, value);
            }
        }
    }
}

fn insert_path(table: &mut toml::Table, path: &[String], value: toml::Value) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let mut cur = table;
    for key in parents {
        let entry = cur
            .entry(key.clone())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if !entry.is_table() {
            *entry = toml::Value::Table(toml::Table::new());
        }
        let toml::Value::Table(next) = entry else {
            return;
        };
        cur = next;
    }
    cur.insert(last.clone(), value);
}

/// Converts an env value to the type of the schema value at the same path.
/// A list is a comma-separated string.
fn typed_env_value(schema: &toml::Table, path: &[String], raw: &str) -> Option<toml::Value> {
    let mut node: Option<&toml::Value> = None;
    let mut table = Some(schema);
    for key in path {
        node = table.and_then(|t| t.get(key));
        table = node.and_then(toml::Value::as_table);
    }
    match node {
        Some(toml::Value::Integer(_)) => raw.trim().parse().ok().map(toml::Value::Integer),
        Some(toml::Value::Boolean(_)) => raw.trim().parse().ok().map(toml::Value::Boolean),
        Some(toml::Value::Array(_)) => Some(toml::Value::Array(
            raw.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| toml::Value::String(s.to_owned()))
                .collect(),
        )),
        _ => Some(toml::Value::String(raw.to_owned())),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;

    const SECRET_VALUE: &str = "s3cr3t-value-0123456789";

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn scratch_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("jira-plugin-config-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn layers() {
        let dir = scratch_dir("layers");
        std::fs::write(
            dir.join("autumn.toml"),
            "[jira.api_token]\nsite = \"base\"\n[jira.http]\nmax_attempts = 4\nmax_pages = 9\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("autumn-prod.toml"),
            "[jira.http]\nmax_attempts = 3\n",
        )
        .unwrap();
        let vars = env(&[
            ("AUTUMN_JIRA__HTTP__MAX_PAGES", "7"),
            ("AUTUMN_JIRA__API_TOKEN__SITE", "envsite"),
            ("AUTUMN_JIRA__WEBHOOK__IGNORE_ACTOR_ACCOUNT_IDS", "a, b,"),
            ("AUTUMN_JIRA__WEBHOOK__ENDPOINT_NAME", "jira-in"),
            ("OTHER_VAR", "ignored"),
        ]);
        let c = JiraConfig::load_from_dir(&dir, &["prod".to_owned()], vars).unwrap();
        assert_eq!(
            c.http.max_attempts, 3,
            "the profile file wins over the base"
        );
        assert_eq!(c.http.max_pages, 7, "the env wins over the files");
        assert_eq!(
            c.http.base_backoff_ms, 2000,
            "a key not set keeps its default"
        );
        let api = c.api_token.as_ref().unwrap();
        assert_eq!(api.site.as_deref(), Some("envsite"));
        assert_eq!(api.token_env, "JIRA_API_TOKEN");
        assert_eq!(c.webhook.ignore_actor_account_ids, ["a", "b"]);
        assert_eq!(c.webhook.endpoint_name, "jira-in");

        // No profile file for the name: only the base and the env.
        let c = JiraConfig::load_from_dir(&dir, &["staging".to_owned()], Vec::new()).unwrap();
        assert_eq!(c.http.max_attempts, 4);
        assert_eq!(c.api_token.unwrap().site.as_deref(), Some("base"));

        // No files: the defaults.
        let c = JiraConfig::load_from_dir(&scratch_dir("empty"), &[], Vec::new()).unwrap();
        assert_eq!(c, JiraConfig::default());

        // A bad env value: the error names the variable, never the value.
        let e = JiraConfig::load_from_dir(
            &dir,
            &[],
            env(&[("AUTUMN_JIRA__HTTP__MAX_PAGES", SECRET_VALUE)]),
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("AUTUMN_JIRA__HTTP__MAX_PAGES"), "{e}");
        assert!(!e.contains(SECRET_VALUE), "{e}");

        // An inline profile table is not a layer. Only the profile file is.
        let c = JiraConfig::from_toml_str(
            "[jira.http]\nmax_pages = 1\n[profile.prod.jira.http]\nmax_pages = 3\n",
        )
        .unwrap();
        assert_eq!(c.http.max_pages, 1);

        // Bad TOML and an unknown key fail.
        assert!(JiraConfig::from_toml_str("[jira").is_err());
        let e = JiraConfig::from_toml_str("[jira.http]\nretries = 2\n")
            .unwrap_err()
            .to_string();
        assert!(e.contains("retries"), "{e}");
    }

    #[test]
    fn mode_from_table() {
        let c = JiraConfig::from_toml_str("[jira.api_token]\nsite = \"acme\"\n").unwrap();
        assert_eq!(c.mode().unwrap(), Some(JiraMode::ApiToken));
        assert_eq!(JiraMode::ApiToken.as_str(), "api_token");

        let c = JiraConfig::from_toml_str("[jira.san_mateo]\n").unwrap();
        assert_eq!(c.mode().unwrap(), Some(JiraMode::SanMateo));
        assert_eq!(JiraMode::SanMateo.as_str(), "san_mateo");
        let target = c.san_mateo.unwrap();
        assert_eq!(target.environment, "default");
        assert_eq!(target.integration, "jira");
        assert_eq!(target.connection_id, None);

        let c = JiraConfig::from_toml_str("[jira.api_token]\ncloud_id = \"0b5c-uuid\"\n").unwrap();
        assert_eq!(c.mode().unwrap(), Some(JiraMode::ApiToken));

        // No table: no mode. The plugin decides if that is an error.
        let c = JiraConfig::from_toml_str("[jira.http]\nmax_pages = 2\n").unwrap();
        assert_eq!(c.mode().unwrap(), None);
        assert_eq!(JiraConfig::default().mode().unwrap(), None);
    }

    #[test]
    fn both_tables_fail() {
        let text = "[jira.api_token]\nsite = \"acme\"\n[jira.san_mateo]\n";
        let e = JiraConfig::from_toml_str(text).unwrap_err().to_string();
        assert!(
            e.contains("set [jira.api_token] or [jira.san_mateo], not both"),
            "{e}"
        );
        let c = JiraConfig {
            api_token: Some(ApiTokenConfig::default()),
            san_mateo: Some(SanMateoTarget::default()),
            ..JiraConfig::default()
        };
        let e = c.mode().unwrap_err().to_string();
        assert!(e.contains("not both"), "{e}");
    }

    #[test]
    fn token_key_names_token_env() {
        let texts = [
            format!("[jira.api_token]\nsite = \"a\"\ntoken = \"{SECRET_VALUE}\"\n"),
            format!("[jira.api_token]\nsite = \"a\"\napi_token = \"{SECRET_VALUE}\"\n"),
            format!("[jira.api_token]\nsite = \"a\"\npassword = \"{SECRET_VALUE}\"\n"),
            format!("[jira]\napi_token = \"{SECRET_VALUE}\"\n"),
            format!("[jira]\ntoken = \"{SECRET_VALUE}\"\n"),
            format!("[jira]\npassword = \"{SECRET_VALUE}\"\n"),
            format!("[jira.san_mateo]\ntoken = \"{SECRET_VALUE}\"\n"),
        ];
        for text in &texts {
            let e = JiraConfig::from_toml_str(text).unwrap_err().to_string();
            assert!(e.contains("token_env"), "{text}: {e}");
            assert!(!e.contains(SECRET_VALUE), "{text}: {e}");
        }
        // The same from the env.
        let e = JiraConfig::from_layers(
            Some("[jira.api_token]\nsite = \"a\"\n"),
            None,
            env(&[("AUTUMN_JIRA__API_TOKEN__TOKEN", SECRET_VALUE)]),
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("token_env"), "{e}");
        assert!(!e.contains(SECRET_VALUE), "{e}");
    }

    #[test]
    fn debug_hides_values() {
        let text = "[jira.api_token]\nsite = \"acme\"\n[jira.webhook]\nignore_actor_account_ids = [\"557058:f00d-person\"]\n";
        let c = JiraConfig::from_toml_str(text).unwrap();
        let debug = format!("{c:?}");
        assert!(!debug.contains("557058:f00d-person"), "{debug}");
        assert!(debug.contains("ignore_actor_account_ids: 1"), "{debug}");
        // The config holds env var names. The names show, so an operator
        // can see which var to set.
        assert!(debug.contains("JIRA_API_TOKEN"), "{debug}");
    }

    #[test]
    fn validation_rules() {
        let bad = [
            ("[jira.api_token]\n", "site or cloud_id"),
            (
                "[jira.api_token]\nsite = \"a\"\ncloud_id = \"b\"\n",
                "not both",
            ),
            ("[jira.api_token]\nsite = \" \"\n", "site or cloud_id"),
            (
                "[jira.api_token]\nsite = \"a\"\ntoken_env = \"\"\n",
                "token_env",
            ),
            (
                "[jira.api_token]\nsite = \"a\"\nemail_env = \"\"\n",
                "email_env",
            ),
            ("[jira.san_mateo]\nenvironment = \"\"\n", "environment"),
            ("[jira.san_mateo]\nintegration = \"\"\n", "integration"),
            (
                "[jira.san_mateo]\ncloud_id = \"11111111-2222-4333-8444-555555555555\"\n",
                "apply to connection_id only",
            ),
            (
                "[jira.san_mateo]\nsite_url = \"https://acme.atlassian.net\"\n",
                "apply to connection_id only",
            ),
            ("[jira.http]\nmax_attempts = 0\n", "max_attempts"),
            ("[jira.http]\nmax_pages = 0\n", "max_pages"),
            ("[jira.http]\nlookup_attempts = 0\n", "lookup_attempts"),
            ("[jira.http]\ntimeout_secs = 0\n", "timeout_secs"),
            (
                "[jira.http]\nbase_backoff_ms = 5\nmax_backoff_ms = 4\n",
                "max_backoff_ms",
            ),
            ("[jira.webhook]\nendpoint_name = \"\"\n", "endpoint_name"),
            ("[jira.webhook]\nmax_age_secs = 0\n", "max_age_secs"),
            (
                "[jira.webhook]\nhandler_timeout_ms = 0\n",
                "handler_timeout_ms",
            ),
            ("[jira.health]\ntimeout_ms = 0\n", "timeout_ms"),
            (
                "[jira.webhook]\nmax_handler_tasks = 0\n",
                "max_handler_tasks",
            ),
            (
                "[jira.webhook]\nhandler_timeout_ms = 10\nabandon_after_ms = 9\n",
                "abandon_after_ms",
            ),
        ];
        for (text, word) in bad {
            let e = JiraConfig::from_toml_str(text).unwrap_err().to_string();
            assert!(e.starts_with("jira config: "), "{text}: {e}");
            assert!(e.contains(word), "{text}: {e}");
        }
        assert!(JiraConfig::default().validate().is_ok());
    }
}
