//! Configuration: environment variables and the `AGENTS_FILE`.
//!
//! Everything here is pure. The environment and the file system are read through closures
//! ([`Config::load`]), so the tests never touch the process environment. Every problem is a
//! [`ConfigError`] that names the variable, file or agent at fault, and none of them carries a
//! secret value: startup fails fast and fails closed.

use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use orch_app::AgentEntry;
use orch_core::{AgentId, UserId};
use orch_ports::AgentEndpoint;
use serde::Deserialize;
use url::Url;

const DEFAULT_LISTEN_ADDR: &str = "0.0.0.0:8080";
const DEFAULT_DATABASE_MAX_CONNECTIONS: u32 = 10;
const DEFAULT_DISPATCHER_CONCURRENCY: usize = 32;
const DEFAULT_OUTBOX_LEASE_SECS: u64 = 30;
const DEFAULT_SHUTDOWN_GRACE_SECS: u64 = 15;
const MAX_AGENT_ID_LEN: usize = 63;

/// A configuration problem. The message is what the operator sees.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A required environment variable is unset or empty.
    #[error("{0} is required")]
    Missing(&'static str),
    /// An environment variable has an unusable value.
    #[error("{var} is invalid: {reason}")]
    Invalid {
        /// The variable.
        var: &'static str,
        /// What is wrong with it.
        reason: String,
    },
    /// `AGENTS_FILE` could not be read.
    #[error("cannot read AGENTS_FILE {}", path.display())]
    AgentsFileRead {
        /// The path from `AGENTS_FILE`.
        path: PathBuf,
        /// The I/O error.
        source: io::Error,
    },
    /// `AGENTS_FILE` is not the expected YAML.
    #[error("AGENTS_FILE {} is not a valid agent list: {message}", path.display())]
    AgentsFileParse {
        /// The path from `AGENTS_FILE`.
        path: PathBuf,
        /// The parser's message.
        message: String,
    },
    /// `AGENTS_FILE` lists no agent, so no thread could ever be created.
    #[error("AGENTS_FILE {} lists no agents", path.display())]
    NoAgents {
        /// The path from `AGENTS_FILE`.
        path: PathBuf,
    },
    /// Two entries share an id.
    #[error("agent id {0:?} is listed more than once")]
    DuplicateAgent(String),
    /// An agent entry is unusable.
    #[error("agent {id:?}: {reason}")]
    InvalidAgent {
        /// The entry's id.
        id: String,
        /// What is wrong with it.
        reason: String,
    },
    /// The environment variable named by `tokenEnv` is unset or empty. Sending requests
    /// without the credential the operator asked for would be the unsafe reading.
    #[error(
        "agent {agent:?}: tokenEnv names {var}, but that environment variable is unset or empty"
    )]
    TokenEnvMissing {
        /// The agent's id.
        agent: String,
        /// The environment variable that should hold the token.
        var: String,
    },
}

/// One entry of `AGENTS_FILE`, as written.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentSpec {
    id: String,
    name: String,
    card_url: String,
    token_env: Option<String>,
}

/// Log output format (`LOG_FORMAT`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// One JSON object per line (the default; what a log collector wants).
    Json,
    /// Human-readable lines for local development.
    Text,
}

impl LogFormat {
    /// Parses `LOG_FORMAT`; anything but `text` is JSON.
    pub fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some(v) if v.eq_ignore_ascii_case("text") => LogFormat::Text,
            _ => LogFormat::Json,
        }
    }
}

/// The complete, validated configuration.
pub struct Config {
    /// `DATABASE_URL`. May contain a password: never logged.
    pub database_url: String,
    /// `LISTEN_ADDR`.
    pub listen_addr: SocketAddr,
    /// `AGENTS_FILE`, resolved: bearer tokens already read from their environment variables.
    pub agents: Vec<AgentEntry>,
    /// `AUTH_DEV_USER`: an identity for requests without `X-Auth-Request-Email`. Dev only.
    pub auth_dev_user: Option<UserId>,
    /// `DATABASE_MAX_CONNECTIONS` (at least 2: the wakeup listener holds one).
    pub database_max_connections: u32,
    /// `DISPATCHER_CONCURRENCY`: outbox rows processed at once.
    pub dispatcher_concurrency: usize,
    /// `OUTBOX_LEASE_SECS`: how long a crashed replica's claim blocks others.
    pub outbox_lease: Duration,
    /// `ORCH_INSTANCE_ID`: names this replica in outbox leases.
    pub instance_id: String,
    /// `SHUTDOWN_GRACE_SECS`: how long a graceful shutdown may take.
    pub shutdown_grace: Duration,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("database_url", &"<redacted>")
            .field("listen_addr", &self.listen_addr)
            .field("agents", &self.agents)
            .field("auth_dev_user", &self.auth_dev_user)
            .field("database_max_connections", &self.database_max_connections)
            .field("dispatcher_concurrency", &self.dispatcher_concurrency)
            .field("outbox_lease", &self.outbox_lease)
            .field("instance_id", &self.instance_id)
            .field("shutdown_grace", &self.shutdown_grace)
            .finish()
    }
}

impl Config {
    /// Loads the configuration from the process environment and the real file system.
    pub fn from_process_env() -> Result<Self, ConfigError> {
        Self::load(
            |name| std::env::var(name).ok(),
            |path| std::fs::read_to_string(path),
        )
    }

    /// Loads the configuration through `env` (variable lookup) and `read` (file contents).
    /// An empty (or all-whitespace) variable counts as unset.
    pub fn load(
        env: impl Fn(&str) -> Option<String>,
        read: impl Fn(&Path) -> io::Result<String>,
    ) -> Result<Self, ConfigError> {
        let get = |name: &str| {
            env(name)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };

        let database_url = get("DATABASE_URL").ok_or(ConfigError::Missing("DATABASE_URL"))?;
        let listen_addr = get("LISTEN_ADDR").unwrap_or_else(|| DEFAULT_LISTEN_ADDR.to_owned());
        let listen_addr = listen_addr
            .parse::<SocketAddr>()
            .map_err(|e| ConfigError::Invalid {
                var: "LISTEN_ADDR",
                reason: format!("{listen_addr:?} is not a socket address like 0.0.0.0:8080 ({e})"),
            })?;

        let agents_file =
            PathBuf::from(get("AGENTS_FILE").ok_or(ConfigError::Missing("AGENTS_FILE"))?);
        let text = read(&agents_file).map_err(|source| ConfigError::AgentsFileRead {
            path: agents_file.clone(),
            source,
        })?;
        let agents = parse_agents(&text, &agents_file, |name| get(name))?;

        let auth_dev_user = match get("AUTH_DEV_USER") {
            None => None,
            Some(email) if email.contains('@') => Some(UserId::new(&email)),
            Some(_) => {
                return Err(ConfigError::Invalid {
                    var: "AUTH_DEV_USER",
                    reason: "expected an e-mail address".to_owned(),
                });
            }
        };

        let database_max_connections = number(
            get("DATABASE_MAX_CONNECTIONS"),
            "DATABASE_MAX_CONNECTIONS",
            DEFAULT_DATABASE_MAX_CONNECTIONS,
            2,
        )?;
        let dispatcher_concurrency = number(
            get("DISPATCHER_CONCURRENCY"),
            "DISPATCHER_CONCURRENCY",
            DEFAULT_DISPATCHER_CONCURRENCY,
            1,
        )?;
        let outbox_lease_secs = number(
            get("OUTBOX_LEASE_SECS"),
            "OUTBOX_LEASE_SECS",
            DEFAULT_OUTBOX_LEASE_SECS,
            3,
        )?;
        let shutdown_grace_secs = number(
            get("SHUTDOWN_GRACE_SECS"),
            "SHUTDOWN_GRACE_SECS",
            DEFAULT_SHUTDOWN_GRACE_SECS,
            1,
        )?;
        let instance_id = get("ORCH_INSTANCE_ID").unwrap_or_else(|| {
            format!(
                "{}-{}",
                get("HOSTNAME").unwrap_or_else(|| "orchestrator".to_owned()),
                uuid::Uuid::now_v7().simple()
            )
        });

        Ok(Config {
            database_url,
            listen_addr,
            agents,
            auth_dev_user,
            database_max_connections,
            dispatcher_concurrency,
            outbox_lease: Duration::from_secs(outbox_lease_secs),
            instance_id,
            shutdown_grace: Duration::from_secs(shutdown_grace_secs),
        })
    }
}

/// Parses an optional numeric variable with a default and a lower bound.
fn number<T>(raw: Option<String>, var: &'static str, default: T, min: T) -> Result<T, ConfigError>
where
    T: std::str::FromStr + PartialOrd + fmt::Display + Copy,
    T::Err: fmt::Display,
{
    let Some(raw) = raw else { return Ok(default) };
    let value = raw.parse::<T>().map_err(|e| ConfigError::Invalid {
        var,
        reason: format!("{raw:?} is not a number ({e})"),
    })?;
    if value < min {
        return Err(ConfigError::Invalid {
            var,
            reason: format!("must be at least {min}"),
        });
    }
    Ok(value)
}

/// `^[a-z0-9][a-z0-9-]{0,62}$`: safe in URLs, file names and labels.
fn valid_agent_id(id: &str) -> bool {
    let mut chars = id.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    first_ok
        && id.len() <= MAX_AGENT_ID_LEN
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Parses the YAML list and resolves each `tokenEnv` through `env`.
fn parse_agents(
    yaml: &str,
    path: &Path,
    env: impl Fn(&str) -> Option<String>,
) -> Result<Vec<AgentEntry>, ConfigError> {
    let specs: Option<Vec<AgentSpec>> =
        serde_norway::from_str(yaml).map_err(|e| ConfigError::AgentsFileParse {
            path: path.to_owned(),
            message: e.to_string(),
        })?;
    let specs = specs.unwrap_or_default();
    if specs.is_empty() {
        return Err(ConfigError::NoAgents {
            path: path.to_owned(),
        });
    }

    let mut entries: Vec<AgentEntry> = Vec::with_capacity(specs.len());
    for spec in specs {
        let invalid = |reason: &str| ConfigError::InvalidAgent {
            id: spec.id.clone(),
            reason: reason.to_owned(),
        };
        if !valid_agent_id(&spec.id) {
            return Err(invalid(
                "id must match ^[a-z0-9][a-z0-9-]{0,62}$ (lower-case letters, digits, dashes)",
            ));
        }
        if entries.iter().any(|e| e.endpoint.id.as_str() == spec.id) {
            return Err(ConfigError::DuplicateAgent(spec.id));
        }
        if spec.name.trim().is_empty() {
            return Err(invalid("name must not be empty"));
        }
        match Url::parse(&spec.card_url) {
            Ok(url) if matches!(url.scheme(), "http" | "https") && url.has_host() => {}
            Ok(_) => return Err(invalid("cardUrl must be an http(s) URL with a host")),
            Err(e) => return Err(invalid(&format!("cardUrl is not a URL ({e})"))),
        }
        let bearer = match spec.token_env.as_deref().map(str::trim) {
            None => None,
            Some("") => return Err(invalid("tokenEnv must not be empty when present")),
            Some(var) => match env(var) {
                Some(token) => Some(token),
                None => {
                    return Err(ConfigError::TokenEnvMissing {
                        agent: spec.id,
                        var: var.to_owned(),
                    });
                }
            },
        };
        entries.push(AgentEntry {
            endpoint: AgentEndpoint {
                id: AgentId::new(spec.id),
                card_url: spec.card_url,
                bearer,
            },
            name: spec.name.trim().to_owned(),
        });
    }
    Ok(entries)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    const AGENTS: &str = "\
- id: coder
  name: Coder
  cardUrl: https://coder.example.com/.well-known/agent-card.json
  tokenEnv: CODER_A2A_TOKEN
- id: plain
  name: Plain
  cardUrl: http://plain.internal:9000/.well-known/agent-card.json
";

    fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |name| map.get(name).cloned()
    }

    fn load(pairs: &[(&str, &str)], agents_yaml: &str) -> Result<Config, ConfigError> {
        let yaml = agents_yaml.to_owned();
        Config::load(env_of(pairs), move |_| Ok(yaml.clone()))
    }

    fn base<'a>() -> Vec<(&'a str, &'a str)> {
        vec![
            ("DATABASE_URL", "postgres://u:secret@db/orch"),
            ("AGENTS_FILE", "/etc/orch/agents.yaml"),
            ("CODER_A2A_TOKEN", "tok-123"),
        ]
    }

    fn agents_err(yaml: &str, env: &[(&str, &str)]) -> ConfigError {
        match parse_agents(yaml, Path::new("agents.yaml"), env_of(env)) {
            Ok(_) => panic!("expected an error"),
            Err(e) => e,
        }
    }

    #[test]
    fn defaults_apply_when_only_the_required_variables_are_set() {
        let cfg = load(&base(), AGENTS).unwrap();
        assert_eq!(cfg.listen_addr, "0.0.0.0:8080".parse().unwrap());
        assert_eq!(cfg.database_max_connections, 10);
        assert_eq!(cfg.dispatcher_concurrency, 32);
        assert_eq!(cfg.outbox_lease, Duration::from_secs(30));
        assert_eq!(cfg.shutdown_grace, Duration::from_secs(15));
        assert!(cfg.auth_dev_user.is_none());
        assert!(cfg.instance_id.starts_with("orchestrator-"));
    }

    #[test]
    fn the_agent_list_is_parsed_and_tokens_are_read_from_their_variables() {
        let cfg = load(&base(), AGENTS).unwrap();
        assert_eq!(cfg.agents.len(), 2);
        let coder = &cfg.agents[0];
        assert_eq!(coder.endpoint.id.as_str(), "coder");
        assert_eq!(coder.name, "Coder");
        assert_eq!(
            coder.endpoint.card_url,
            "https://coder.example.com/.well-known/agent-card.json"
        );
        assert_eq!(coder.endpoint.bearer.as_deref(), Some("tok-123"));
        assert_eq!(cfg.agents[1].endpoint.bearer, None);
    }

    #[test]
    fn a_token_is_trimmed_because_secret_files_end_with_a_newline() {
        let mut env = base();
        env.retain(|(k, _)| *k != "CODER_A2A_TOKEN");
        env.push(("CODER_A2A_TOKEN", " tok-123\n"));
        // The load helper trims through the same `get` used for every variable.
        let cfg = load(&env, AGENTS).unwrap();
        assert_eq!(cfg.agents[0].endpoint.bearer.as_deref(), Some("tok-123"));
    }

    #[test]
    fn a_missing_token_variable_fails_startup_and_names_it() {
        let mut env = base();
        env.retain(|(k, _)| *k != "CODER_A2A_TOKEN");
        let err = load(&env, AGENTS).unwrap_err();
        assert!(matches!(&err, ConfigError::TokenEnvMissing { agent, var }
            if agent == "coder" && var == "CODER_A2A_TOKEN"));
        assert!(err.to_string().contains("CODER_A2A_TOKEN"));
    }

    #[test]
    fn an_empty_token_variable_fails_startup() {
        let mut env = base();
        env.retain(|(k, _)| *k != "CODER_A2A_TOKEN");
        env.push(("CODER_A2A_TOKEN", "  "));
        assert!(matches!(
            load(&env, AGENTS).unwrap_err(),
            ConfigError::TokenEnvMissing { .. }
        ));
    }

    #[test]
    fn duplicate_ids_are_refused() {
        let yaml = "\
- {id: a, name: A, cardUrl: 'https://a.example.com/card'}
- {id: a, name: B, cardUrl: 'https://b.example.com/card'}
";
        assert!(matches!(
            agents_err(yaml, &[]),
            ConfigError::DuplicateAgent(id) if id == "a"
        ));
    }

    #[test]
    fn ids_must_be_safe_slugs() {
        for bad in ["", "Coder", "-x", "has space", "a/b", "é", &"a".repeat(64)] {
            let yaml = format!(
                "- id: {}\n  name: X\n  cardUrl: https://x.example.com/card\n",
                serde_norway::to_string(bad).unwrap().trim()
            );
            assert!(
                matches!(agents_err(&yaml, &[]), ConfigError::InvalidAgent { .. }),
                "{bad:?} must be refused"
            );
        }
        assert!(valid_agent_id("a"));
        assert!(valid_agent_id("coder-2"));
        assert!(valid_agent_id(&"a".repeat(63)));
    }

    #[test]
    fn card_urls_must_be_http_urls() {
        for bad in [
            "not a url",
            "ftp://x.example.com/card",
            "file:///etc/passwd",
            "https://",
        ] {
            let yaml = format!("- id: a\n  name: A\n  cardUrl: '{bad}'\n");
            assert!(
                matches!(agents_err(&yaml, &[]), ConfigError::InvalidAgent { .. }),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn unknown_fields_and_wrong_shapes_are_parse_errors() {
        let typo = "- id: a\n  name: A\n  cardUrl: https://a.example.com/card\n  tokenenv: X\n";
        assert!(matches!(
            agents_err(typo, &[]),
            ConfigError::AgentsFileParse { .. }
        ));
        assert!(matches!(
            agents_err("id: a\nname: A\n", &[]),
            ConfigError::AgentsFileParse { .. }
        ));
        assert!(matches!(
            agents_err("- id: a\n", &[]),
            ConfigError::AgentsFileParse { .. }
        ));
    }

    #[test]
    fn an_empty_agent_list_is_refused() {
        for yaml in ["", "[]", "# nothing here\n"] {
            assert!(matches!(
                agents_err(yaml, &[]),
                ConfigError::NoAgents { .. }
            ));
        }
    }

    #[test]
    fn required_variables_are_reported_by_name() {
        let mut env = base();
        env.retain(|(k, _)| *k != "DATABASE_URL");
        assert!(matches!(
            load(&env, AGENTS).unwrap_err(),
            ConfigError::Missing("DATABASE_URL")
        ));
        let mut env = base();
        env.retain(|(k, _)| *k != "AGENTS_FILE");
        assert!(matches!(
            load(&env, AGENTS).unwrap_err(),
            ConfigError::Missing("AGENTS_FILE")
        ));
        // Set but empty is unset.
        let mut env = base();
        env.retain(|(k, _)| *k != "DATABASE_URL");
        env.push(("DATABASE_URL", ""));
        assert!(matches!(
            load(&env, AGENTS).unwrap_err(),
            ConfigError::Missing("DATABASE_URL")
        ));
    }

    #[test]
    fn an_unreadable_agents_file_is_an_error_naming_the_path() {
        let err = Config::load(env_of(&base()), |_| {
            Err(io::Error::new(io::ErrorKind::NotFound, "no such file"))
        })
        .unwrap_err();
        assert!(matches!(err, ConfigError::AgentsFileRead { .. }));
        assert!(err.to_string().contains("/etc/orch/agents.yaml"));
        // The cause is the source, printed once by the chain and not by the message.
        assert!(!err.to_string().contains("no such file"));
        let chain = orch_core::report(&err);
        assert_eq!(chain.matches("no such file").count(), 1, "{chain}");
    }

    #[test]
    fn numbers_and_addresses_are_validated() {
        let with = |k: &'static str, v: &'static str| {
            let mut env = base();
            env.push((k, v));
            load(&env, AGENTS)
        };
        let cfg = with("LISTEN_ADDR", "127.0.0.1:9090").unwrap();
        assert_eq!(cfg.listen_addr.port(), 9090);
        for (var, value) in [
            ("LISTEN_ADDR", "localhost"),
            ("DATABASE_MAX_CONNECTIONS", "1"),
            ("DATABASE_MAX_CONNECTIONS", "many"),
            ("DISPATCHER_CONCURRENCY", "0"),
            ("OUTBOX_LEASE_SECS", "2"),
            ("SHUTDOWN_GRACE_SECS", "0"),
        ] {
            let err = with(var, value).unwrap_err();
            assert!(
                matches!(&err, ConfigError::Invalid { var: v, .. } if *v == var),
                "{var}={value}: {err}"
            );
        }
        assert_eq!(
            with("DATABASE_MAX_CONNECTIONS", "2")
                .unwrap()
                .database_max_connections,
            2
        );
    }

    #[test]
    fn the_dev_user_is_optional_and_must_be_an_email() {
        let mut env = base();
        env.push(("AUTH_DEV_USER", " Me@Example.com "));
        let cfg = load(&env, AGENTS).unwrap();
        assert_eq!(cfg.auth_dev_user.unwrap().as_str(), "me@example.com");
        let mut env = base();
        env.push(("AUTH_DEV_USER", "me"));
        assert!(matches!(
            load(&env, AGENTS).unwrap_err(),
            ConfigError::Invalid {
                var: "AUTH_DEV_USER",
                ..
            }
        ));
    }

    #[test]
    fn the_instance_id_comes_from_the_environment_or_the_hostname() {
        let mut env = base();
        env.push(("ORCH_INSTANCE_ID", "pod-a"));
        assert_eq!(load(&env, AGENTS).unwrap().instance_id, "pod-a");
        let mut env = base();
        env.push(("HOSTNAME", "orch-0"));
        assert!(
            load(&env, AGENTS)
                .unwrap()
                .instance_id
                .starts_with("orch-0-")
        );
    }

    #[test]
    fn debug_output_never_contains_secrets() {
        let cfg = load(&base(), AGENTS).unwrap();
        let shown = format!("{cfg:?}");
        assert!(!shown.contains("secret"), "{shown}");
        assert!(!shown.contains("tok-123"), "{shown}");
    }

    #[test]
    fn the_log_format_defaults_to_json() {
        assert_eq!(LogFormat::parse(None), LogFormat::Json);
        assert_eq!(LogFormat::parse(Some("json")), LogFormat::Json);
        assert_eq!(LogFormat::parse(Some("weird")), LogFormat::Json);
        assert_eq!(LogFormat::parse(Some(" Text ")), LogFormat::Text);
    }
}
