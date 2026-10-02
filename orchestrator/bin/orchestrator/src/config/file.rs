//! The configuration file (`ORCH_CONFIG_FILE`, [ADR 0034], [`docs/api/config.md`]): the loader.
//!
//! `orch-config` is pure; this is the part that reads: the file, the environment and the secret
//! files. Loading goes in the three passes of the ADR, with the legacy variables laid over the
//! tree between the first and the second:
//!
//! 1. **Syntax**: `orch_config::parse_yaml`; an error is reported alone.
//! 2. **The variables over the file.** Each legacy variable (or flag) that is set is converted
//!    with *today's parser for it* (`ORCH_STEPS_RECORD_IO=yes` is `true`, `ORCH_SURFACES` a comma
//!    list) and set on its key, and noted: a flag or its variable wins over the file, which wins
//!    over the default. A secret variable counts as a reference to itself
//!    (`ORCH_MODEL_API_KEY` set is `apiKey: { env: ORCH_MODEL_API_KEY }`). A value that does not
//!    parse is an error naming the variable.
//! 3. **Shape and rules**: `orch_config::check` (every shape error, with reserved keys named),
//!    then `validate` (the rules between keys, the secrets resolved).
//!
//! The valid file is then projected onto [`Args`] and handed to [`Config::load`], the rules the
//! binary has always had (the gate against the agents, a surface this build compiled in, the
//! agents file, the MCP tokens), so the file and the variables cannot disagree on what is valid.
//! Those errors name the legacy variable; they are put in the file's words (the key) and every
//! quoted piece of them is scrubbed, because no error may carry a value.
//!
//! [ADR 0034]: ../../../../docs/decisions/0034-one-yaml-configuration-secrets-by-reference.md
//! [`docs/api/config.md`]: ../../../../docs/api/config.md

use std::collections::{BTreeMap, HashMap};
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use orch_app::{PublicConfig, TaskSettings, ToolServerInfo, UiSettings};
use orch_config::{Resolve, SecretRef, Validated};
use orch_core::{AgentId, LanguageRule, TaskKind};
use secrecy::SecretString;
use serde_json::{Map, Value};

use super::{
    Args, Config, ConfigError, EndpointSettings, LogFormat, ModelsSettings, Resolved, Surface,
    flag, number, parse_surfaces,
};
use crate::artifacts::{ArtifactSettings, S3Settings, StoreSettings};

/// The most a configuration file or an agents file may be.
const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// What startup says about where a setting came from, logged once the logging is up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
    /// A variable or flag that is deprecated is set: it wins over the file (`overrides`) or fills
    /// a key the file leaves out. A warning, naming the variable and the key, never the value.
    Deprecated {
        /// The variable.
        var: &'static str,
        /// The key it is for, such as `tasks.title.model`.
        key: String,
        /// Whether the file had the key (the variable overrides it) or not.
        overrides: bool,
    },
    /// A process override (`ORCH_ROLE`, `ORCH_INSTANCE_ID`, `RUST_LOG`, `HOSTNAME`): not
    /// deprecated, logged at info.
    Process {
        /// The variable.
        var: &'static str,
        /// What it does, in words.
        what: &'static str,
    },
    /// No file: the environment alone configures the process, which is deprecated.
    EnvironmentOnly,
    /// A tool server (`toolServers`) that has a credential is at a plain `http://` URL: the
    /// credential travels in the clear. A warning naming the server's id, never a URL or a value.
    ToolServerCredentialOverHttp {
        /// The server's id.
        id: String,
    },
}

impl Note {
    /// The note in words, without a value.
    pub fn describe(&self) -> String {
        match self {
            Note::Deprecated {
                var,
                key,
                overrides: true,
            } => format!(
                "{var} overrides {key} of the configuration file; set it in the file, the variable is deprecated"
            ),
            Note::Deprecated {
                var,
                key,
                overrides: false,
            } => format!(
                "{var} sets {key}, which the configuration file leaves out; set it in the file, the variable is deprecated"
            ),
            Note::Process { var, what } => format!("{var} {what}"),
            Note::EnvironmentOnly => "no configuration file: the settings come from environment \
                variables alone, which are deprecated; set ORCH_CONFIG_FILE to a YAML file \
                (docs/api/config.md)"
                .to_owned(),
            Note::ToolServerCredentialOverHttp { id } => format!(
                "the tool server {id} has a credential and a plain http:// URL: the credential is \
                 sent in the clear; use https outside development"
            ),
        }
    }

    /// Logs the note: a warning for what is deprecated, info for what is not.
    pub fn log(&self) {
        let message = self.describe();
        match self {
            Note::Deprecated { var, key, .. } => {
                tracing::warn!(variable = *var, key = key.as_str(), "{message}");
            }
            Note::Process { var, .. } => tracing::info!(variable = *var, "{message}"),
            Note::EnvironmentOnly => tracing::warn!("{message}"),
            Note::ToolServerCredentialOverHttp { id } => {
                tracing::warn!(server = id.as_str(), "{message}");
            }
        }
    }
}

/// The secret flags that were given on the command line, by variable name: a flag wins over its
/// variable, so a `{ env: DATABASE_URL }` reference reads the flag's value when there is one.
pub type FlagSecrets = HashMap<&'static str, String>;

/// What loading produced.
#[derive(Debug)]
pub struct Loaded {
    /// The configuration to run with.
    pub config: Config,
    /// What to say at startup about where settings came from.
    pub notes: Vec<Note>,
    /// The merged configuration as YAML, secrets as references: what `--print-config` prints.
    /// `None` when the process runs from the environment alone and nothing asked to print.
    pub merged: Option<String>,
}

impl Loaded {
    /// Loads the configuration of this process: the file `ORCH_CONFIG_FILE` names when it is set
    /// (the variables over it), else the environment alone.
    pub fn from_process(args: Args, flag_secrets: FlagSecrets) -> Result<Loaded, ConfigError> {
        if clean(args.config.as_deref()).is_none() && !args.print_config {
            // The environment alone, exactly as before the file existed.
            return Config::from_args(args).map(|config| Loaded {
                config,
                notes: vec![Note::EnvironmentOnly],
                merged: None,
            });
        }
        Self::load(
            args,
            flag_secrets,
            |name| std::env::var(name).ok(),
            |path| read_limited(path, MAX_FILE_BYTES),
        )
    }

    /// [`from_process`](Self::from_process) with the environment and the file system as
    /// parameters, so a test reads neither.
    ///
    /// Without a file and without `--print-config` this is exactly [`Config::load`]: the
    /// environment alone, as before the file existed.
    pub fn load(
        args: Args,
        flag_secrets: FlagSecrets,
        env: impl Fn(&str) -> Option<String>,
        read: impl Fn(&Path) -> io::Result<String>,
    ) -> Result<Loaded, ConfigError> {
        let file = clean(args.config.as_deref());
        if file.is_none() && !args.print_config {
            let config = Config::load(args, &env, &read)?;
            return Ok(Loaded {
                config,
                notes: vec![Note::EnvironmentOnly],
                merged: None,
            });
        }
        let (text, base_dir) = match &file {
            Some(path) => {
                let path = PathBuf::from(path);
                let text = read(&path).map_err(|_| {
                    document(format!(
                        "ORCH_CONFIG_FILE: cannot read the configuration file {}",
                        path.display()
                    ))
                })?;
                let base = path
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .map_or_else(|| PathBuf::from("."), Path::to_owned);
                (text, base)
            }
            // `--print-config` with no file: the environment alone, through the same checks.
            None => ("version: 1\n".to_owned(), PathBuf::from(".")),
        };
        load_file(
            &text,
            &base_dir,
            args,
            flag_secrets,
            env,
            read,
            file.is_none(),
        )
    }
}

fn document(line: impl Into<String>) -> ConfigError {
    ConfigError::Document(vec![line.into()])
}

/// The text of the file at `path`, at most `limit` bytes.
pub(super) fn read_limited(path: &Path, limit: u64) -> io::Result<String> {
    let mut text = String::new();
    std::fs::File::open(path)?
        .take(limit + 1)
        .read_to_string(&mut text)?;
    if text.len() as u64 > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the file is too large",
        ));
    }
    Ok(text)
}

/// A value as the legacy loader sees it: trimmed, and blank is unset.
fn clean(v: Option<&str>) -> Option<String> {
    v.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

// ---- the variables, as keys -------------------------------------------------------------------

/// How the text of a variable becomes the value of its key, with the parser the variable has
/// always had.
#[derive(Debug, Clone, Copy)]
enum Kind {
    /// A string as it is.
    Text,
    /// A whole number (the range is the key's, checked with its path).
    Uint,
    /// `true`/`false`, `1`/`0`, `yes`/`no`, `on`/`off`.
    Bool,
    /// A comma list, at least one name.
    List,
    /// `adam-host`'s role names.
    Role,
    /// `ORCH_SURFACES`: known, compiled in, no repeats, the removed `chat-api` named.
    Surfaces,
    /// `ORCH_GATE`: known sources.
    Gate,
    /// `LOG_FORMAT`: `text`, and anything else is JSON.
    LogFormat,
    /// The variable is the secret: the key is `{ env: VAR }`.
    Secret,
    /// A list of one reference to this variable, which keeps its comma rule.
    SecretList,
}

/// One legacy variable and the key it became.
struct Setting {
    /// The variable.
    var: &'static str,
    /// The key, as the path of its members.
    key: &'static [&'static str],
    kind: Kind,
    /// The flag's value (the variable's value when there is no flag).
    get: fn(&Args) -> &Option<String>,
    /// A process override: not deprecated, logged at info.
    process: bool,
    /// The key that must be there for this one to mean anything: the rest of a group whose
    /// first key is not set is read (so a typo is heard of) and left out, as it always was.
    needs: Option<&'static [&'static str]>,
}

const fn setting(
    var: &'static str,
    key: &'static [&'static str],
    kind: Kind,
    get: fn(&Args) -> &Option<String>,
) -> Setting {
    Setting {
        var,
        key,
        kind,
        get,
        process: false,
        needs: None,
    }
}

const REGISTRY_URL: &[&str] = &["agents", "registry", "url"];
const MODEL_BASE_URL: &[&str] = &["models", "endpoints", "default", "baseUrl"];
const GENERIC_SECRETS: &[&str] = &["webhooks", "generic", "secrets"];
const GITHUB_SECRETS: &[&str] = &["webhooks", "github", "secrets"];

/// Every variable that has a key (51 of the 53 names; `HOSTNAME` and `RUST_LOG` have none), the
/// first of a group before the rest of it.
const SETTINGS: &[Setting] = &[
    setting("DATABASE_URL", &["database", "url"], Kind::Secret, |a| {
        &a.database_url
    }),
    setting("AGENTS_FILE", &["agents", "file"], Kind::Text, |a| {
        &a.agents_file
    }),
    setting("AGENT_REGISTRY_URL", REGISTRY_URL, Kind::Text, |a| {
        &a.registry_url
    }),
    Setting {
        needs: Some(REGISTRY_URL),
        ..setting(
            "AGENT_REGISTRY_TOKEN",
            &["agents", "registry", "token"],
            Kind::Secret,
            |a| &a.registry_token,
        )
    },
    Setting {
        needs: Some(REGISTRY_URL),
        ..setting(
            "AGENT_REGISTRY_AGENT_TOKEN",
            &["agents", "registry", "agentToken"],
            Kind::Secret,
            |a| &a.registry_agent_token,
        )
    },
    Setting {
        needs: Some(REGISTRY_URL),
        ..setting(
            "AGENT_REGISTRY_TIMEOUT_SECS",
            &["agents", "registry", "timeoutSecs"],
            Kind::Uint,
            |a| &a.registry_timeout_secs,
        )
    },
    Setting {
        needs: Some(REGISTRY_URL),
        ..setting(
            "AGENT_REGISTRY_MAX_AGE_SECS",
            &["agents", "registry", "maxAgeSecs"],
            Kind::Uint,
            |a| &a.registry_max_age_secs,
        )
    },
    setting("LISTEN_ADDR", &["server", "listen"], Kind::Text, |a| {
        &a.listen_addr
    }),
    Setting {
        process: true,
        ..setting("ORCH_ROLE", &["server", "role"], Kind::Role, |a| &a.role)
    },
    setting(
        "ORCH_SURFACES",
        &["server", "surfaces"],
        Kind::Surfaces,
        |a| &a.surfaces,
    ),
    setting("ORCH_GATE", &["gate", "require"], Kind::Gate, |a| &a.gate),
    setting(
        "ORCH_MAX_ATTEMPTS",
        &["gate", "maxAttempts"],
        Kind::Uint,
        |a| &a.max_attempts,
    ),
    setting(
        "ORCH_MAX_ATTEMPTS_CAP",
        &["gate", "maxAttemptsCap"],
        Kind::Uint,
        |a| &a.max_attempts_cap,
    ),
    setting("ORCH_VERIFIER", &["gate", "verifier"], Kind::Text, |a| {
        &a.verifier
    }),
    setting("MCP_TOKENS_FILE", &["mcp", "tokensFile"], Kind::Text, |a| {
        &a.mcp_tokens_file
    }),
    setting(
        "MCP_ALLOWED_HOSTS",
        &["mcp", "allowedHosts"],
        Kind::List,
        |a| &a.mcp_allowed_hosts,
    ),
    setting(
        "ORCH_PUBLIC_URL",
        &["server", "publicUrl"],
        Kind::Text,
        |a| &a.public_url,
    ),
    setting(
        "MCP_WAIT_MAX_SECS",
        &["mcp", "waitMaxSecs"],
        Kind::Uint,
        |a| &a.mcp_wait_max_secs,
    ),
    setting(
        "MCP_WAIT_MAX_CONCURRENT",
        &["mcp", "waitMaxConcurrent"],
        Kind::Uint,
        |a| &a.mcp_wait_max_concurrent,
    ),
    setting(
        "MCP_WAIT_MAX_PER_USER",
        &["mcp", "waitMaxPerUser"],
        Kind::Uint,
        |a| &a.mcp_wait_max_per_user,
    ),
    setting(
        "MCP_ALLOWED_ORIGINS",
        &["mcp", "allowedOrigins"],
        Kind::List,
        |a| &a.mcp_allowed_origins,
    ),
    setting(
        "THREAD_TOOLS_SECRET",
        &["threadTools", "secret"],
        Kind::Secret,
        |a| &a.thread_tools_secret,
    ),
    setting(
        "THREAD_TOOLS_SECRET_PREVIOUS",
        &["threadTools", "previousSecret"],
        Kind::Secret,
        |a| &a.thread_tools_secret_previous,
    ),
    setting(
        "THREAD_TOOLS_URL",
        &["threadTools", "url"],
        Kind::Text,
        |a| &a.thread_tools_url,
    ),
    setting(
        "THREAD_TOOLS_TOKEN_TTL_SECS",
        &["threadTools", "tokenTtlSecs"],
        Kind::Uint,
        |a| &a.thread_tools_token_ttl_secs,
    ),
    setting(
        "THREAD_TOOLS_ALLOWED_HOSTS",
        &["threadTools", "allowedHosts"],
        Kind::List,
        |a| &a.thread_tools_allowed_hosts,
    ),
    setting(
        "ORCH_VERIFIER_TIMEOUT_SECS",
        &["gate", "verifierTimeoutSecs"],
        Kind::Uint,
        |a| &a.verifier_timeout_secs,
    ),
    setting(
        "ORCH_VERIFIER_WATCH_SECS",
        &["gate", "verifierWatchSecs"],
        Kind::Uint,
        |a| &a.verifier_watch_secs,
    ),
    setting(
        "ORCH_STEPS_RECORD_IO",
        &["steps", "recordToolIo"],
        Kind::Bool,
        |a| &a.steps_record_io,
    ),
    setting(
        "ORCH_TITLE_MODEL",
        &["tasks", "title", "model"],
        Kind::Text,
        |a| &a.title_model,
    ),
    setting("ORCH_MODEL_BASE_URL", MODEL_BASE_URL, Kind::Text, |a| {
        &a.model_base_url
    }),
    Setting {
        needs: Some(MODEL_BASE_URL),
        ..setting(
            "ORCH_MODEL_API_KEY",
            &["models", "endpoints", "default", "apiKey"],
            Kind::Secret,
            |a| &a.model_api_key,
        )
    },
    Setting {
        needs: Some(MODEL_BASE_URL),
        ..setting(
            "ORCH_MODEL_TIMEOUT_SECS",
            &["models", "endpoints", "default", "timeoutSecs"],
            Kind::Uint,
            |a| &a.model_timeout_secs,
        )
    },
    setting(
        "ORCH_CI_TIMEOUT_SECS",
        &["gate", "ci", "timeoutSecs"],
        Kind::Uint,
        |a| &a.ci_timeout_secs,
    ),
    setting(
        "ORCH_CI_REQUIRED",
        &["gate", "ci", "required"],
        Kind::List,
        |a| &a.ci_required,
    ),
    setting(
        "WEBHOOK_GENERIC_SECRETS",
        GENERIC_SECRETS,
        Kind::SecretList,
        |a| &a.webhook_generic_secrets,
    ),
    setting(
        "WEBHOOK_GITHUB_SECRETS",
        GITHUB_SECRETS,
        Kind::SecretList,
        |a| &a.webhook_github_secrets,
    ),
    Setting {
        needs: Some(GITHUB_SECRETS),
        ..setting(
            "WEBHOOK_GITHUB_MAX_AGE_SECS",
            &["webhooks", "github", "maxAgeSecs"],
            Kind::Uint,
            |a| &a.webhook_github_max_age_secs,
        )
    },
    Setting {
        needs: Some(GENERIC_SECRETS),
        ..setting(
            "WEBHOOK_GENERIC_MAX_SKEW_SECS",
            &["webhooks", "generic", "maxSkewSecs"],
            Kind::Uint,
            |a| &a.webhook_generic_max_skew_secs,
        )
    },
    setting("AUTH_DEV_USER", &["auth", "devUser"], Kind::Text, |a| {
        &a.auth_dev_user
    }),
    setting(
        "DATABASE_MAX_CONNECTIONS",
        &["database", "maxConnections"],
        Kind::Uint,
        |a| &a.database_max_connections,
    ),
    setting(
        "DISPATCHER_CONCURRENCY",
        &["dispatcher", "concurrency"],
        Kind::Uint,
        |a| &a.dispatcher_concurrency,
    ),
    #[cfg(feature = "agent-local")]
    setting(
        "AGENT_LOCAL_CONCURRENCY",
        &["agents", "localConcurrency"],
        Kind::Uint,
        |a| &a.agent_local_concurrency,
    ),
    setting(
        "OUTBOX_LEASE_SECS",
        &["dispatcher", "outboxLeaseSecs"],
        Kind::Uint,
        |a| &a.outbox_lease_secs,
    ),
    setting(
        "INBOX_LEASE_SECS",
        &["inbox", "leaseSecs"],
        Kind::Uint,
        |a| &a.inbox_lease_secs,
    ),
    setting("INBOX_POLL_SECS", &["inbox", "pollSecs"], Kind::Uint, |a| {
        &a.inbox_poll_secs
    }),
    setting(
        "INBOX_PARKED_TTL_SECS",
        &["inbox", "parkedTtlSecs"],
        Kind::Uint,
        |a| &a.inbox_parked_ttl_secs,
    ),
    setting(
        "INBOX_MAX_ATTEMPTS",
        &["inbox", "maxAttempts"],
        Kind::Uint,
        |a| &a.inbox_max_attempts,
    ),
    setting(
        "SHUTDOWN_GRACE_SECS",
        &["server", "shutdownGraceSecs"],
        Kind::Uint,
        |a| &a.shutdown_grace_secs,
    ),
    Setting {
        process: true,
        ..setting(
            "ORCH_INSTANCE_ID",
            &["server", "instanceId"],
            Kind::Text,
            |a| &a.instance_id,
        )
    },
    setting("LOG_FORMAT", &["log", "format"], Kind::LogFormat, |a| {
        &a.log_format
    }),
];

impl Setting {
    /// The key as written in a message: `database.url`.
    fn dotted(&self) -> String {
        self.key.join(".")
    }
}

/// The arguments that carry a secret, by the variable: the name clap knows the flag by, and how
/// to read it. `main` asks clap which of them came from the command line.
pub fn secret_flags() -> impl Iterator<Item = (&'static str, &'static str)> {
    [
        ("DATABASE_URL", "database_url"),
        ("AGENT_REGISTRY_TOKEN", "registry_token"),
        ("AGENT_REGISTRY_AGENT_TOKEN", "registry_agent_token"),
        ("THREAD_TOOLS_SECRET", "thread_tools_secret"),
        (
            "THREAD_TOOLS_SECRET_PREVIOUS",
            "thread_tools_secret_previous",
        ),
        ("ORCH_MODEL_API_KEY", "model_api_key"),
        ("WEBHOOK_GENERIC_SECRETS", "webhook_generic_secrets"),
        ("WEBHOOK_GITHUB_SECRETS", "webhook_github_secrets"),
    ]
    .into_iter()
}

impl Args {
    /// The value of the secret variable `var` as this command line holds it (flag or variable).
    pub fn secret_value(&self, var: &str) -> Option<String> {
        SETTINGS
            .iter()
            .find(|s| s.var == var)
            .and_then(|s| clean((s.get)(self).as_deref()))
    }
}

// ---- the tree ---------------------------------------------------------------------------------

fn get<'a>(tree: &'a Value, key: &[&str]) -> Option<&'a Value> {
    key.iter().try_fold(tree, |node, name| node.get(*name))
}

/// Sets `key` to `value`, making the mappings on the way. `false` when something on the way is
/// there and is not a mapping: the shape pass reports that, and the file's own value stays.
fn set(tree: &mut Value, key: &[&str], value: Value) -> bool {
    let Some((last, parents)) = key.split_last() else {
        return false;
    };
    let mut node = tree;
    for name in parents {
        let Value::Object(map) = node else {
            return false;
        };
        node = map
            .entry((*name).to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    match node {
        Value::Object(map) => {
            map.insert((*last).to_owned(), value);
            true
        }
        _ => false,
    }
}

/// Whether the file names the environment variable `var` as a secret anywhere: then the file
/// reads it itself, and it is not an override of anything.
fn references_env(node: &Value, var: &str) -> bool {
    match node {
        Value::Object(map) => {
            (map.len() == 1 && map.get("env").and_then(Value::as_str) == Some(var))
                || map.values().any(|v| references_env(v, var))
        }
        Value::Array(items) => items.iter().any(|v| references_env(v, var)),
        _ => false,
    }
}

/// A reference to `var`, as the tree holds one.
fn env_reference(var: &str) -> Value {
    serde_json::to_value(SecretRef::Env(var.to_owned())).unwrap_or(Value::Null)
}

/// `raw` as the value of the key of `setting`, read the way the variable has always been read.
fn convert(setting: &Setting, raw: &str) -> Result<Value, ConfigError> {
    let var = setting.var;
    let invalid = |reason: String| ConfigError::Invalid { var, reason };
    let names = |raw: &str| -> Vec<String> {
        raw.split(',')
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_owned)
            .collect()
    };
    Ok(match setting.kind {
        Kind::Text => Value::String(raw.to_owned()),
        Kind::Uint => Value::from(number::<u64>(Some(raw.to_owned()), var, 0, 0)?),
        Kind::Bool => Value::Bool(flag(Some(raw.to_owned()), var, true)?),
        Kind::List => {
            let list = names(raw);
            if list.is_empty() {
                return Err(invalid("the list is empty".to_owned()));
            }
            Value::from(list)
        }
        Kind::Role => Value::String(
            adam_host::Role::from_optional(Some(raw))
                .map_err(|e| invalid(e.to_string()))?
                .as_str()
                .to_owned(),
        ),
        Kind::Surfaces => Value::from(
            parse_surfaces(raw)?
                .into_iter()
                .map(|s: Surface| s.name().to_owned())
                .collect::<Vec<_>>(),
        ),
        Kind::Gate => {
            let list = names(raw);
            if list.is_empty() {
                return Err(invalid(format!(
                    "names no source (one of: {})",
                    orch_app::known_sources()
                )));
            }
            for name in &list {
                if orch_core::CheckSource::from_config_name(name).is_none() {
                    return Err(invalid(format!(
                        "unknown source {name:?} (one of: {})",
                        orch_app::known_sources()
                    )));
                }
            }
            Value::from(list)
        }
        Kind::LogFormat => Value::String(
            match LogFormat::parse(Some(raw)) {
                LogFormat::Json => "json",
                LogFormat::Text => "text",
            }
            .to_owned(),
        ),
        Kind::Secret => env_reference(var),
        Kind::SecretList => Value::Array(vec![env_reference(var)]),
    })
}

/// Lays the legacy variables over the tree. Returns what to say about them, and the errors.
fn overlay(tree: &mut Value, args: &Args) -> (Vec<Note>, Vec<String>) {
    let mut notes = Vec::new();
    let mut errors = Vec::new();
    if !tree.is_object() {
        // The shape pass says so; there is nothing to lay a variable over.
        return (notes, errors);
    }
    for setting in SETTINGS {
        let Some(raw) = clean((setting.get)(args).as_deref()) else {
            continue;
        };
        let value = match convert(setting, &raw) {
            Ok(value) => value,
            Err(e) => {
                errors.push(variable_line(&e));
                continue;
            }
        };
        if setting
            .needs
            .is_some_and(|needs| get(tree, needs).is_none())
        {
            continue;
        }
        let is_secret = matches!(setting.kind, Kind::Secret | Kind::SecretList);
        if is_secret && references_env(tree, setting.var) {
            continue;
        }
        let existing = get(tree, setting.key);
        if existing == Some(&value) {
            continue;
        }
        let overrides = existing.is_some();
        if !set(tree, setting.key, value) {
            continue;
        }
        if setting.var == "ORCH_TITLE_MODEL" && get(tree, &["tasks", "title", "endpoint"]).is_none()
        {
            set(
                tree,
                &["tasks", "title", "endpoint"],
                Value::String("default".to_owned()),
            );
        }
        notes.push(if setting.process {
            Note::Process {
                var: setting.var,
                what: match setting.var {
                    "ORCH_ROLE" => "overrides server.role for this process",
                    _ => "overrides server.instanceId for this process",
                },
            }
        } else {
            Note::Deprecated {
                var: setting.var,
                key: setting.dotted(),
                overrides,
            }
        });
    }
    (notes, errors)
}

// ---- the process notes ------------------------------------------------------------------------

fn process_notes(args: &Args, env: &impl Fn(&str) -> Option<String>) -> Vec<Note> {
    let mut notes = Vec::new();
    if env("RUST_LOG").is_some_and(|v| !v.trim().is_empty()) {
        notes.push(Note::Process {
            var: "RUST_LOG",
            what: "sets the log filter (it is not a configuration key)",
        });
    }
    if clean(args.hostname.as_deref()).is_some() {
        notes.push(Note::Process {
            var: "HOSTNAME",
            what: "is the prefix of the default server.instanceId (it is not a configuration key)",
        });
    }
    notes
}

// ---- the loader -------------------------------------------------------------------------------

/// The resolver `orch-config` is given: the environment (a flag wins over its variable), and the
/// file system.
struct ProcessResolver<'a, E, R> {
    env: &'a E,
    read: &'a R,
    flags: &'a FlagSecrets,
}

impl<E, R> Resolve for ProcessResolver<'_, E, R>
where
    E: Fn(&str) -> Option<String>,
    R: Fn(&Path) -> io::Result<String>,
{
    fn env(&self, name: &str) -> Option<String> {
        self.flags.get(name).cloned().or_else(|| (self.env)(name))
    }

    fn read_file(&self, path: &Path) -> io::Result<String> {
        (self.read)(path)
    }
}

/// The three passes on the text of the file (or `version: 1`, with no file), the variables over
/// it, and then the rules the binary has always had.
fn load_file(
    text: &str,
    base_dir: &Path,
    args: Args,
    flag_secrets: FlagSecrets,
    env: impl Fn(&str) -> Option<String>,
    read: impl Fn(&Path) -> io::Result<String>,
    environment_only: bool,
) -> Result<Loaded, ConfigError> {
    // Pass 1: syntax, alone.
    let mut tree = orch_config::parse_yaml(text).map_err(file_errors)?;
    // The variables over the file.
    let (mut notes, mut errors) = overlay(&mut tree, &args);
    if environment_only {
        notes.insert(0, Note::EnvironmentOnly);
    }
    if !errors.is_empty() {
        // The shape of what is left is checked too, so one run lists what it can.
        if let Err(more) = orch_config::check(&tree) {
            errors.extend(more.iter().map(ToString::to_string));
        }
        return Err(ConfigError::Document(errors));
    }
    // Pass 2: the shape, every error.
    let config = orch_config::check(&tree).map_err(file_errors)?;
    // Pass 3: the rules, and the secrets.
    let resolver = ProcessResolver {
        env: &env,
        read: &read,
        flags: &flag_secrets,
    };
    let valid = config.validate(base_dir, &resolver).map_err(file_errors)?;
    // `agents.localConcurrency` is the key of a build that has local agents: refused, never ignored.
    #[cfg(not(feature = "agent-local"))]
    if get(&tree, &["agents", "localConcurrency"]).is_some() {
        return Err(document(
            "agents.localConcurrency: needs the Cargo feature \"agent-local\", which this build \
             does not have (ADR 0015)",
        ));
    }

    // `artifacts.store` names only what this build compiled in: refused, never ignored (ADR 0032).
    if let Some(artifacts) = &valid.config.artifacts {
        let (compiled, feature) = match artifacts.store {
            orch_config::ArtifactStoreKind::Fs => (cfg!(feature = "artifacts-fs"), "artifacts-fs"),
            orch_config::ArtifactStoreKind::S3 => (cfg!(feature = "artifacts-s3"), "artifacts-s3"),
        };
        if !compiled {
            return Err(document(format!(
                "artifacts.store: {} needs the Cargo feature \"{feature}\", which this build does not have \
                 (ADR 0032)",
                artifacts.store.as_str()
            )));
        }
    }

    let (file_args, resolved) = project(&valid, &tree, args.hostname.clone());
    let merged = serde_norway::to_string(&valid.config.effective())
        .map_err(|_| document("the configuration cannot be written as YAML (a bug)"))?;
    let loaded = Config::load_with(file_args, resolved, &env, &read)
        .map_err(|e| ConfigError::Document(vec![legacy_line(&e)]))?;
    tool_server_agents(&valid.config.tool_servers, &loaded)?;
    notes.extend(tool_server_notes(&valid));
    notes.extend(process_notes(&args, &env));
    Ok(Loaded {
        config: loaded,
        notes,
        merged: Some(merged),
    })
}

fn file_errors(errors: Vec<orch_config::ConfigError>) -> ConfigError {
    ConfigError::Document(errors.iter().map(ToString::to_string).collect())
}

// ---- the file, as the legacy arguments --------------------------------------------------------

/// The valid file as the [`Args`] [`Config::load`] reads, and the secrets that cannot go through
/// a string. `tree` says which keys the file (or a variable) *set*, for the defaults that depend
/// on whether a key is there.
fn project(valid: &Validated, tree: &Value, hostname: Option<String>) -> (Args, Resolved) {
    let c = &valid.config;
    let s = &valid.secrets;
    let set_in_tree = |key: &[&str]| get(tree, key).is_some();
    let some = |v: String| Some(v);
    let join = |v: &[String]| (!v.is_empty()).then(|| v.join(","));
    let mut a = Args {
        hostname,
        database_url: some(s.database_url.expose().to_owned()),
        agents_file: valid.agents_file().map(|p| p.display().to_string()),
        listen_addr: some(c.server.listen.clone()),
        role: some(c.server.role.as_str().to_owned()),
        surfaces: set_in_tree(&["server", "surfaces"]).then(|| {
            c.mounted_surfaces()
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(",")
        }),
        public_url: c.server.public_url.clone(),
        shutdown_grace_secs: some(c.server.shutdown_grace_secs.to_string()),
        instance_id: c.server.instance_id.clone(),
        log_format: some(c.log.format.as_str().to_owned()),
        database_max_connections: some(c.database.max_connections.to_string()),
        dispatcher_concurrency: some(c.dispatcher.concurrency.to_string()),
        outbox_lease_secs: some(c.dispatcher.outbox_lease_secs.to_string()),
        inbox_lease_secs: some(c.inbox.lease_secs.to_string()),
        inbox_poll_secs: some(c.inbox.poll_secs.to_string()),
        inbox_parked_ttl_secs: some(c.inbox.parked_ttl_secs.to_string()),
        inbox_max_attempts: some(c.inbox.max_attempts.to_string()),
        gate: join(
            &c.gate
                .require
                .iter()
                .map(|g| g.as_str().to_owned())
                .collect::<Vec<_>>(),
        ),
        max_attempts: c.gate.max_attempts.map(|n| n.to_string()),
        max_attempts_cap: some(c.gate.max_attempts_cap.to_string()),
        verifier: c.gate.verifier.clone(),
        verifier_timeout_secs: some(c.gate.verifier_timeout_secs.to_string()),
        verifier_watch_secs: some(c.gate.verifier_watch_secs.to_string()),
        ci_timeout_secs: some(c.gate.ci.timeout_secs.to_string()),
        ci_required: join(&c.gate.ci.required),
        steps_record_io: some(c.steps.record_tool_io.to_string()),
        mcp_tokens_file: valid.mcp_tokens_file().map(|p| p.display().to_string()),
        mcp_allowed_hosts: c.mcp.allowed_hosts.as_deref().and_then(join),
        mcp_allowed_origins: join(&c.mcp.allowed_origins),
        mcp_wait_max_secs: some(c.mcp.wait_max_secs.to_string()),
        mcp_wait_max_concurrent: some(c.mcp.wait_max_concurrent.to_string()),
        mcp_wait_max_per_user: some(c.mcp.wait_max_per_user.to_string()),
        thread_tools_url: c.thread_tools.url.clone(),
        thread_tools_secret: s
            .thread_tools_secret
            .as_ref()
            .map(|x| x.expose().to_owned()),
        thread_tools_secret_previous: s
            .thread_tools_previous_secret
            .as_ref()
            .map(|x| x.expose().to_owned()),
        thread_tools_token_ttl_secs: some(c.thread_tools.token_ttl_secs.to_string()),
        thread_tools_allowed_hosts: c.thread_tools.allowed_hosts.as_deref().and_then(join),
        auth_dev_user: c.auth.dev_user.clone(),
        ..Args::default()
    };
    if let Some(registry) = &c.agents.registry {
        a.registry_url = some(registry.url.clone());
        a.registry_token = s.registry_token.as_ref().map(|x| x.expose().to_owned());
        a.registry_agent_token = s
            .registry_agent_token
            .as_ref()
            .map(|x| x.expose().to_owned());
        a.registry_timeout_secs = some(registry.timeout_secs.to_string());
        a.registry_max_age_secs = some(registry.max_age_secs.to_string());
    }
    #[cfg(feature = "agent-local")]
    {
        a.agent_local_concurrency = c.agents.local_concurrency.map(|n| n.to_string());
    }
    if let Some(generic) = &c.webhooks.generic {
        a.webhook_generic_max_skew_secs = some(generic.max_skew_secs.to_string());
    }
    if let Some(github) = &c.webhooks.github {
        a.webhook_github_max_age_secs = some(github.max_age_secs.to_string());
    }
    let resolved = Resolved {
        artifacts: artifact_settings(valid),
        auth: super::AuthSettings::from_file(&c.auth),
        models: Some(models_of(valid)),
        tool_servers: tool_servers_of(&c.tool_servers),
        public: Some(PublicConfig {
            ui: UiSettings {
                show_descriptions: c.ui.show_descriptions,
            },
        }),
        #[cfg(feature = "surface-webhook")]
        webhook_generic: webhook_values(&s.webhook_generic, "WEBHOOK_GENERIC_SECRETS"),
        #[cfg(feature = "surface-webhook")]
        webhook_github: webhook_values(&s.webhook_github, "WEBHOOK_GITHUB_SECRETS"),
    };
    (a, resolved)
}

/// The artifact store of the valid file, in the terms of this process: the directory resolved
/// against the file's directory, the credentials as resolved secrets. `None` without the section.
fn artifact_settings(valid: &Validated) -> Option<ArtifactSettings> {
    let artifacts = valid.config.artifacts.as_ref()?;
    let store = match artifacts.store {
        orch_config::ArtifactStoreKind::Fs => StoreSettings::Fs {
            root: valid.artifacts_fs_root()?,
        },
        orch_config::ArtifactStoreKind::S3 => {
            let s3 = artifacts.s3.as_ref()?;
            let secrets = &valid.secrets;
            StoreSettings::S3(S3Settings {
                bucket: s3.bucket.clone(),
                region: s3.region.clone(),
                endpoint: s3.endpoint.clone(),
                prefix: s3.prefix.clone(),
                access_key_id: SecretString::from(secrets.s3_access_key_id.as_ref()?.expose()),
                secret_access_key: SecretString::from(
                    secrets.s3_secret_access_key.as_ref()?.expose(),
                ),
                timeout: Duration::from_secs(s3.timeout_secs),
            })
        }
    };
    Some(ArtifactSettings {
        store,
        max_file_bytes: artifacts.max_file_bytes,
        max_per_job_bytes: artifacts.max_per_job_bytes,
        fetch_hosts: artifacts.fetch_hosts.clone(),
    })
}

/// The models of a valid file: its endpoints with their keys read, and the tasks that are on, each
/// with its prompt read and the timeout of its endpoint as the bound of a try (ADR 0035).
fn models_of(valid: &Validated) -> ModelsSettings {
    let c = &valid.config;
    let endpoints: BTreeMap<String, EndpointSettings> = c
        .models
        .endpoints
        .iter()
        .map(|(name, e)| {
            (
                name.clone(),
                EndpointSettings {
                    base_url: e.base_url.trim().trim_end_matches('/').to_owned(),
                    api_key: valid
                        .secrets
                        .model_api_keys
                        .get(name)
                        .map(|key| SecretString::from(key.expose().to_owned())),
                    timeout: Duration::from_secs(e.timeout_secs),
                },
            )
        })
        .collect();
    // The rules pass checked that a task names an endpoint of the file; a name that is not there
    // would be a task that is off, which no file can say.
    let timeout_of = |endpoint: &str| endpoints.get(endpoint).map(|e| e.timeout);
    let mut tasks = BTreeMap::new();
    if let Some(t) = &c.tasks.title
        && let Some(timeout) = timeout_of(&t.endpoint)
    {
        tasks.insert(
            TaskKind::Title,
            TaskSettings {
                guidance: valid.prompts.title.clone(),
                max_tokens: t.max_tokens,
                language: language_rule(t.language),
                ..TaskSettings::new(TaskKind::Title, &t.endpoint, &t.model).with_timeout(timeout)
            },
        );
    }
    if let Some(t) = &c.tasks.description
        && let Some(timeout) = timeout_of(&t.endpoint)
    {
        tasks.insert(
            TaskKind::Description,
            TaskSettings {
                guidance: valid.prompts.description.clone(),
                max_tokens: t.max_tokens,
                language: language_rule(t.language),
                max_chars: usize::try_from(t.max_chars).unwrap_or(usize::MAX),
                min_new_messages: t.recompute.min_new_messages,
                ..TaskSettings::new(TaskKind::Description, &t.endpoint, &t.model)
                    .with_timeout(timeout)
            },
        );
    }
    ModelsSettings { endpoints, tasks }
}

/// The tool servers of the valid file, as far as the application knows them (ADR 0024): the public
/// part. The URL and the credentials stay in the validated file, which only the relay will read;
/// nothing the application keeps, writes or serves holds them.
fn tool_servers_of(servers: &[orch_config::ToolServer]) -> Vec<ToolServerInfo> {
    servers
        .iter()
        .map(|server| ToolServerInfo {
            id: server.id.clone(),
            name: server.name.trim().to_owned(),
            description: server
                .description
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .map(str::to_owned),
            icon: server.icon.clone(),
            tools: server.tools.clone(),
            agents: server
                .agents
                .as_ref()
                .map(|agents| agents.iter().map(AgentId::new).collect()),
            timeout: Duration::from_secs(server.timeout_secs),
        })
        .collect()
}

/// A server's `agents` list names agents that exist: with a platform registry the agents are not
/// all known at startup, so the check is made only when the file's own list is the whole list. A
/// typo would otherwise be a server that is never offered, with nothing to say so.
fn tool_server_agents(
    servers: &[orch_config::ToolServer],
    loaded: &Config,
) -> Result<(), ConfigError> {
    #[cfg(feature = "registry-platform")]
    if loaded.registry.is_some() {
        return Ok(());
    }
    let known: std::collections::BTreeSet<&str> = loaded
        .agents
        .iter()
        .map(|a| a.endpoint.id.as_str())
        .collect();
    let mut errors = Vec::new();
    for (i, server) in servers.iter().enumerate() {
        for (j, agent) in server.agents.iter().flatten().enumerate() {
            if !known.contains(agent.as_str()) {
                errors.push(format!(
                    "toolServers[{i}].agents[{j}]: names no agent of the agents file"
                ));
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(ConfigError::Document(errors))
    }
}

/// What startup says about the tool servers: a credential that would travel over plain `http://`.
fn tool_server_notes(valid: &Validated) -> Vec<Note> {
    valid
        .config
        .tool_servers
        .iter()
        .filter(|server| {
            valid.secrets.tool_servers.contains_key(&server.id)
                && server
                    .url
                    .trim_start()
                    .to_ascii_lowercase()
                    .starts_with("http://")
        })
        .map(|server| Note::ToolServerCredentialOverHttp {
            id: server.id.clone(),
        })
        .collect()
}

/// The core's rule for a language the file names (the same names: a test asserts they agree).
fn language_rule(language: orch_config::Language) -> LanguageRule {
    LanguageRule::from_config_name(language.as_str()).unwrap_or_default()
}

/// The values of a webhook's secrets, separated. A secret that is read through the legacy
/// variable keeps that variable's comma rule: it may hold one or two secrets.
#[cfg(feature = "surface-webhook")]
fn webhook_values(secrets: &[orch_config::Secret], legacy_var: &str) -> Option<Vec<String>> {
    if secrets.is_empty() {
        return None;
    }
    let mut values = Vec::new();
    for secret in secrets {
        if secret.reference() == &SecretRef::Env(legacy_var.to_owned()) {
            values.extend(
                secret
                    .expose()
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned),
            );
        } else {
            values.push(secret.expose().to_owned());
        }
    }
    Some(values)
}

/// A [`ConfigError`] of a variable that does not parse: it names the variable, as it always has,
/// and nothing it quotes is kept.
fn variable_line(error: &ConfigError) -> String {
    match error {
        ConfigError::Invalid { var, reason } => format!("{var} is invalid: {}", scrub(reason)),
        // The name that was refused is the value of the variable: not repeated.
        ConfigError::UnknownSurface { known, .. } => {
            format!("ORCH_SURFACES is invalid: unknown surface (known: {known})")
        }
        other => other.to_string(),
    }
}

// ---- the legacy errors, in the file's words ---------------------------------------------------

/// A [`ConfigError`] of the binary as a line for the operator of a file: the variable it names
/// is the key it became, and nothing quoted from a value is kept.
fn legacy_line(error: &ConfigError) -> String {
    let key = |var: &str| {
        SETTINGS
            .iter()
            .find(|s| s.var == var)
            .map_or_else(|| var.to_owned(), Setting::dotted)
    };
    match error {
        ConfigError::Invalid { var, reason } => format!("{}: {}", key(var), scrub(reason)),
        ConfigError::Missing(var) => format!("{}: required", key(var)),
        #[cfg(any(feature = "surface-webhook", feature = "surface-thread-tools"))]
        ConfigError::MissingForSurface { var, surface } => {
            format!(
                "{}: required when server.surfaces mounts {surface:?}",
                key(var)
            )
        }
        other => in_keys(&other.to_string()),
    }
}

/// `message` with each variable name replaced by its key.
fn in_keys(message: &str) -> String {
    let mut out = message.to_owned();
    let mut names: Vec<&Setting> = SETTINGS.iter().collect();
    // The longest first: `THREAD_TOOLS_SECRET` is the start of `THREAD_TOOLS_SECRET_PREVIOUS`.
    names.sort_by_key(|s| std::cmp::Reverse(s.var.len()));
    for setting in names {
        out = replace_word(&out, setting.var, &setting.dotted());
    }
    out
}

fn replace_word(text: &str, word: &str, with: &str) -> String {
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(word) {
        let before = rest[..at].chars().next_back();
        let after = rest[at + word.len()..].chars().next();
        out.push_str(&rest[..at]);
        if before.is_some_and(is_word) || after.is_some_and(is_word) {
            out.push_str(word);
        } else {
            out.push_str(with);
        }
        rest = &rest[at + word.len()..];
    }
    out.push_str(rest);
    out
}

/// `text` with whatever is between double quotes replaced by an ellipsis: a message of a library
/// or of today's parsers may quote the value it refused.
fn scrub(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        out.push(c);
        if c == '"' {
            out.push('…');
            let mut escaped = false;
            for q in chars.by_ref() {
                if escaped {
                    escaped = false;
                } else if q == '\\' {
                    escaped = true;
                } else if q == '"' {
                    out.push('"');
                    break;
                }
            }
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::collections::HashMap;
    #[cfg(feature = "registry-platform")]
    use std::time::Duration;

    use super::super::AuthMode;
    use super::super::tests::{args_of, env_of};
    use super::*;

    const AGENTS: &str = "\
- id: coder
  name: Coder
  cardUrl: https://coder.example.com/.well-known/agent-card.json
  tokenEnv: CODER_A2A_TOKEN
";
    const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef";

    /// A file that configures everything the environment can, beside the secrets.
    const FILE: &str = "\
version: 1
server:
  listen: 0.0.0.0:9000
log: { format: text }
database:
  url: { env: DATABASE_URL }
  maxConnections: 7
agents:
  file: agents.yaml
steps: { recordToolIo: false }
models:
  endpoints:
    default: { baseUrl: 'http://mock-model:8080/v1', apiKey: { env: ORCH_MODEL_API_KEY }, timeoutSecs: 9 }
tasks:
  title: { endpoint: default, model: small-model }
threadTools:
  url: http://orchestrator:8080
  secret: { env: THREAD_TOOLS_SECRET }
";

    /// The variables the secrets of [`FILE`] come from, and the agents' own.
    fn base<'a>() -> Vec<(&'a str, &'a str)> {
        vec![
            ("ORCH_CONFIG_FILE", "/etc/orch/config.yaml"),
            ("DATABASE_URL", "postgres://u:hunter2@db/orch"),
            ("ORCH_MODEL_API_KEY", "model-key-hunter2"),
            ("THREAD_TOOLS_SECRET", KEY),
            ("CODER_A2A_TOKEN", "tok-123"),
        ]
    }

    /// Loads `pairs` (the variables) with `file` as the configuration file.
    fn load_with_file(
        pairs: &[(&str, &str)],
        file: &str,
        flags: FlagSecrets,
    ) -> Result<Loaded, ConfigError> {
        let files: HashMap<&str, String> = HashMap::from([
            ("/etc/orch/config.yaml", file.to_owned()),
            ("/etc/orch/agents.yaml", AGENTS.to_owned()),
            ("/run/secrets/key", format!("{KEY}\n")),
            (
                "/etc/orch/prompts/description.md",
                "Say what the person wants.\n".to_owned(),
            ),
        ]);
        Loaded::load(args_of(pairs), flags, env_of(pairs), move |path| {
            files
                .get(path.to_str().unwrap())
                .cloned()
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        })
    }

    fn load_file_only(pairs: &[(&str, &str)], file: &str) -> Result<Loaded, ConfigError> {
        load_with_file(pairs, file, FlagSecrets::new())
    }

    fn lines(result: Result<Loaded, ConfigError>) -> Vec<String> {
        match result.unwrap_err() {
            ConfigError::Document(lines) => lines,
            other => vec![other.to_string()],
        }
    }

    const JWT: &str = "\
auth:
  mode: jwt
  jwt:
    issuer: https://idp.example/realms/main
    audiences: [orchestrator-web]
    userClaim: preferred_username
    rolesClaim: realm_access.roles
";

    #[test]
    fn without_an_auth_section_the_proxy_header_is_how_requests_are_authenticated() {
        let c = load_file_only(&base(), FILE).unwrap().config;
        assert_eq!(c.auth, super::super::AuthSettings::default());
        assert_eq!(c.auth.mode, AuthMode::ProxyHeader);
        // And without a file at all.
        let env = Config::load(
            args_of(&[
                ("DATABASE_URL", "postgres://u:hunter2@db/orch"),
                ("AGENTS_FILE", "/etc/orch/agents.yaml"),
                ("CODER_A2A_TOKEN", "tok-123"),
            ]),
            env_of(&[("CODER_A2A_TOKEN", "tok-123")]),
            |_| Ok(AGENTS.to_owned()),
        )
        .unwrap();
        assert_eq!(env.auth.mode, AuthMode::ProxyHeader);
    }

    #[test]
    #[cfg(feature = "auth-jwt")]
    fn the_auth_section_reaches_the_configuration() {
        let c = load_file_only(&base(), &format!("{FILE}{JWT}"))
            .unwrap()
            .config;
        assert_eq!(c.auth.mode, AuthMode::Jwt);
        let jwt = c.auth.jwt.unwrap();
        assert_eq!(jwt.issuer, "https://idp.example/realms/main");
        assert_eq!(jwt.audiences, ["orchestrator-web"]);
        assert_eq!(jwt.user_claim, "preferred_username");
        assert_eq!(jwt.roles_claim.as_deref(), Some("realm_access.roles"));
        assert!(jwt.jwks_url.is_none());
    }

    #[test]
    fn the_roles_of_the_file_become_the_policy() {
        use orch_app::{Permission, Resource, Scope};
        let user = orch_core::UserId::new("a@example.com");
        let other = orch_core::UserId::new("b@example.com");
        let mine = Resource::Thread { owner: &user };
        let theirs = Resource::Thread { owner: &other };
        let principal = |roles: &[&str]| orch_ports::Principal {
            roles: roles.iter().map(|r| orch_ports::Role::new(*r)).collect(),
            ..orch_ports::Principal::of(user.clone())
        };
        // Without roles in the file: the built-in pair, and everyone without a role is a user.
        let policy = load_file_only(&base(), FILE).unwrap().config.auth.policy;
        assert!(policy.allows(&principal(&[]), Permission::ThreadWrite, &mine));
        assert!(!policy.allows(&principal(&[]), Permission::ThreadRead, &theirs));
        assert!(policy.allows(&principal(&["admin"]), Permission::ThreadRead, &theirs));
        assert!(!policy.allows(&principal(&["admin"]), Permission::ThreadWrite, &theirs));
        // `defaultRole: null` without roles: the built-ins, and nobody without a role is let in.
        let strict = load_file_only(&base(), &format!("{FILE}auth: {{ defaultRole: null }}\n"))
            .unwrap()
            .config
            .auth
            .policy;
        assert!(strict.access(&principal(&[])).is_empty());
        assert!(strict.allows(&principal(&["user"]), Permission::ThreadRead, &mine));
        // Roles in the file replace the built-ins, and with no `defaultRole` there is none.
        let text = format!(
            "{FILE}auth:\n  roles:\n    reader: {{ permissions: [thread.read], scope: any }}\n    \
             clerk: {{ permissions: [agent.invoke, thread.write], agents: [coder] }}\n"
        );
        let policy = load_file_only(&base(), &text).unwrap().config.auth.policy;
        assert!(policy.default_role().is_none());
        assert!(policy.access(&principal(&[])).is_empty());
        assert!(
            policy.access(&principal(&["user"])).is_empty(),
            "the built-ins are gone"
        );
        let reader = policy.access(&principal(&["reader"]));
        assert_eq!(reader.scope(Permission::ThreadRead), Some(Scope::Any));
        let clerk = policy.access(&principal(&["clerk"]));
        assert_eq!(clerk.agents(Permission::AgentInvoke).patterns(), ["coder"]);
        assert!(!clerk.has(Permission::ThreadRead));
        // A default role of the file is the one a person with no known role gets.
        let text = format!("{text}  defaultRole: reader\n");
        let policy = load_file_only(&base(), &text).unwrap().config.auth.policy;
        assert!(policy.allows(&principal(&["wizard"]), Permission::ThreadRead, &theirs));
        // A default role nobody defined is a rule of the file.
        let errors = lines(load_file_only(
            &base(),
            &format!("{FILE}auth: {{ defaultRole: nobody }}\n"),
        ));
        assert!(
            errors
                .iter()
                .any(|l| l.starts_with("auth.defaultRole: not one of the roles")),
            "{errors:?}"
        );
    }

    #[test]
    fn auth_mode_and_the_development_user_are_refused_together_through_the_variable_too() {
        // AUTH_DEV_USER is the variable of auth.devUser: the rule of the file applies to it.
        let mut pairs = base();
        pairs.push(("AUTH_DEV_USER", "dev@example.com"));
        let errors = lines(load_file_only(&pairs, &format!("{FILE}{JWT}")));
        assert!(
            errors
                .iter()
                .any(|l| l.starts_with("auth.devUser: only with auth.mode proxy_header")),
            "{errors:?}"
        );
    }

    #[test]
    fn a_mode_whose_feature_is_not_in_the_build_is_refused() {
        let jwt = load_file_only(&base(), &format!("{FILE}{JWT}"));
        let jwt_or = load_file_only(
            &base(),
            &format!(
                "{FILE}auth: {{ mode: jwt_or_proxy_header, jwt: {{ issuer: 'https://i.example', audiences: [a] }} }}\n"
            ),
        );
        if cfg!(feature = "auth-jwt") {
            assert!(jwt.is_ok());
        } else {
            let line = lines(jwt).join("\n");
            assert!(
                line.contains("auth.mode jwt is not in this build") && line.contains("auth-jwt"),
                "{line}"
            );
        }
        if cfg!(all(feature = "auth-jwt", feature = "auth-header")) {
            assert!(jwt_or.is_ok());
        } else {
            assert!(jwt_or.is_err());
        }
        // The default mode needs the header authenticator.
        let header = load_file_only(&base(), FILE);
        if cfg!(feature = "auth-header") {
            assert!(header.is_ok());
        } else {
            let line = lines(header).join("\n");
            assert!(
                line.contains("auth.mode proxy_header is not in this build")
                    && line.contains("auth-header"),
                "{line}"
            );
        }
    }

    #[test]
    fn a_worker_needs_no_authenticator() {
        let mut pairs = base();
        pairs.push(("ORCH_ROLE", "worker"));
        let c = load_file_only(&pairs, FILE).unwrap().config;
        assert!(!c.role.runs_control_plane());
    }

    #[test]
    #[cfg(feature = "auth-jwt")]
    fn print_config_shows_the_auth_section() {
        let merged = load_file_only(&base(), &format!("{FILE}{JWT}"))
            .unwrap()
            .merged
            .unwrap();
        assert!(
            merged.contains("mode: jwt")
                && merged.contains("issuer: https://idp.example/realms/main"),
            "{merged}"
        );
        assert!(merged.contains("userClaim: preferred_username"), "{merged}");
    }

    #[test]
    fn a_file_gives_the_configuration_the_variables_would_have_given() {
        let from_file = load_file_only(&base(), FILE).unwrap().config;
        let by_variables = Loaded::load(
            args_of(&[
                ("DATABASE_URL", "postgres://u:hunter2@db/orch"),
                ("AGENTS_FILE", "/etc/orch/agents.yaml"),
                ("CODER_A2A_TOKEN", "tok-123"),
                ("LISTEN_ADDR", "0.0.0.0:9000"),
                ("LOG_FORMAT", "text"),
                ("DATABASE_MAX_CONNECTIONS", "7"),
                ("ORCH_STEPS_RECORD_IO", "false"),
                ("ORCH_MODEL_BASE_URL", "http://mock-model:8080/v1"),
                ("ORCH_MODEL_API_KEY", "model-key-hunter2"),
                ("ORCH_MODEL_TIMEOUT_SECS", "9"),
                ("ORCH_TITLE_MODEL", "small-model"),
                ("THREAD_TOOLS_URL", "http://orchestrator:8080"),
                ("THREAD_TOOLS_SECRET", KEY),
            ]),
            FlagSecrets::new(),
            env_of(&[("CODER_A2A_TOKEN", "tok-123")]),
            |_| Ok(AGENTS.to_owned()),
        )
        .unwrap()
        .config;
        // The instance id is a fresh uuid each time; everything else is the same.
        let unlike = |c: &Config| format!("{c:?}").replace(&c.instance_id, "<id>");
        assert_eq!(unlike(&from_file), unlike(&by_variables));
        assert_eq!(from_file.listen_addr.port(), 9000);
        assert!(!from_file.steps_record_io);
        assert_eq!(from_file.log_format, LogFormat::Text);
    }

    #[test]
    fn without_a_file_the_environment_alone_configures_the_process_as_before() {
        let pairs = [
            ("DATABASE_URL", "postgres://u:pw@db/orch"),
            ("AGENTS_FILE", "/etc/orch/agents.yaml"),
            ("CODER_A2A_TOKEN", "tok-123"),
            ("LISTEN_ADDR", "0.0.0.0:9001"),
        ];
        let loaded = Loaded::load(args_of(&pairs), FlagSecrets::new(), env_of(&pairs), |_| {
            Ok(AGENTS.to_owned())
        })
        .unwrap();
        assert_eq!(loaded.config.listen_addr.port(), 9001);
        assert_eq!(loaded.notes, [Note::EnvironmentOnly]);
        assert!(loaded.merged.is_none());
    }

    #[test]
    fn a_variable_wins_over_the_file_and_the_note_names_the_variable_and_the_key_never_the_value() {
        let mut pairs = base();
        pairs.extend([
            ("ORCH_TITLE_MODEL", "other-model-hunter2"),
            ("LISTEN_ADDR", "0.0.0.0:9100"),
            ("DATABASE_MAX_CONNECTIONS", "12"),
        ]);
        let loaded = load_file_only(&pairs, FILE).unwrap();
        assert_eq!(loaded.config.listen_addr.port(), 9100);
        assert_eq!(loaded.config.database_max_connections, 12);
        assert_eq!(
            loaded.config.models.tasks[&TaskKind::Title].model,
            "other-model-hunter2"
        );
        let deprecated: Vec<(&str, &str, bool)> = loaded
            .notes
            .iter()
            .filter_map(|n| match n {
                Note::Deprecated {
                    var,
                    key,
                    overrides,
                } => Some((*var, key.as_str(), *overrides)),
                _ => None,
            })
            .collect();
        assert_eq!(
            deprecated,
            [
                ("LISTEN_ADDR", "server.listen", true),
                ("ORCH_TITLE_MODEL", "tasks.title.model", true),
                ("DATABASE_MAX_CONNECTIONS", "database.maxConnections", true),
            ]
        );
        let said: Vec<String> = loaded.notes.iter().map(Note::describe).collect();
        assert!(
            said.contains(
                &"ORCH_TITLE_MODEL overrides tasks.title.model of the configuration file; \
                  set it in the file, the variable is deprecated"
                    .to_owned()
            ),
            "{said:?}"
        );
        assert!(
            !said.join(" ").contains("hunter2"),
            "a value is never in a note"
        );
    }

    #[test]
    fn a_variable_that_says_what_the_file_says_is_not_an_override_and_a_secret_variable_the_file_names_is_not_either()
     {
        let mut pairs = base();
        // The variables of the file's own references are set (they hold the secrets), and
        // LISTEN_ADDR equals the file's value.
        pairs.push(("LISTEN_ADDR", "0.0.0.0:9000"));
        let loaded = load_file_only(&pairs, FILE).unwrap();
        assert!(
            loaded
                .notes
                .iter()
                .all(|n| !matches!(n, Note::Deprecated { .. })),
            "{:?}",
            loaded.notes
        );
    }

    #[test]
    fn a_secret_variable_counts_as_a_reference_to_itself_and_a_file_secret_is_overridden_by_it() {
        let file = FILE.replace(
            "secret: { env: THREAD_TOOLS_SECRET }",
            "secret: { file: /run/secrets/key }",
        );
        // The file's secret is a file; the variable overrides it, with a warning.
        let other = "another-key-0123456789abcdef0123456789abcdef";
        let mut pairs = base();
        pairs.push(("THREAD_TOOLS_SECRET", other));
        let loaded = load_file_only(&pairs, &file).unwrap();
        assert!(loaded.notes.contains(&Note::Deprecated {
            var: "THREAD_TOOLS_SECRET",
            key: "threadTools.secret".to_owned(),
            overrides: true,
        }));
        let merged = loaded.merged.unwrap();
        assert!(merged.contains("env: THREAD_TOOLS_SECRET"), "{merged}");
        assert!(!merged.contains(other) && !merged.contains(KEY), "{merged}");
        // Without the variable the file's own `{ file }` reference is read.
        let pairs: Vec<_> = base()
            .into_iter()
            .filter(|(k, _)| *k != "THREAD_TOOLS_SECRET")
            .collect();
        let loaded = load_file_only(&pairs, &file).unwrap();
        assert!(loaded.merged.unwrap().contains("file: /run/secrets/key"));
    }

    #[test]
    fn a_flag_wins_over_its_variable_for_a_secret_the_file_names_as_a_variable() {
        let flags = FlagSecrets::from([("DATABASE_URL", "postgres://flag@db/orch".to_owned())]);
        let loaded = load_with_file(&base(), FILE, flags).unwrap();
        assert_eq!(loaded.config.database_url, "postgres://flag@db/orch");
        assert_eq!(
            load_file_only(&base(), FILE).unwrap().config.database_url,
            "postgres://u:hunter2@db/orch"
        );
    }

    #[test]
    fn the_process_overrides_are_noted_at_info_and_are_not_deprecated() {
        let mut pairs = base();
        pairs.extend([
            ("ORCH_ROLE", "worker"),
            ("ORCH_INSTANCE_ID", "w1"),
            ("HOSTNAME", "host-a"),
        ]);
        let loaded = load_file_only(&pairs, FILE).unwrap();
        assert_eq!(loaded.config.role.as_str(), "worker");
        assert_eq!(loaded.config.instance_id, "w1");
        let process: Vec<&str> = loaded
            .notes
            .iter()
            .filter_map(|n| match n {
                Note::Process { var, .. } => Some(*var),
                _ => None,
            })
            .collect();
        assert_eq!(process, ["ORCH_ROLE", "ORCH_INSTANCE_ID", "HOSTNAME"]);
        assert!(
            loaded
                .notes
                .iter()
                .all(|n| !matches!(n, Note::Deprecated { .. }))
        );
        let mut with_rust_log = pairs.clone();
        with_rust_log.push(("RUST_LOG", "debug"));
        let loaded = Loaded::load(
            args_of(&with_rust_log),
            FlagSecrets::new(),
            env_of(&with_rust_log),
            |p| {
                Ok(if p.ends_with("agents.yaml") {
                    AGENTS.to_owned()
                } else {
                    FILE.to_owned()
                })
            },
        )
        .unwrap();
        assert!(loaded.notes.iter().any(|n| matches!(
            n,
            Note::Process {
                var: "RUST_LOG",
                ..
            }
        )));
    }

    #[test]
    fn a_variable_is_read_with_its_own_parser_before_the_shape_is_checked() {
        let surfaces = if cfg!(feature = "surface-thread-tools") {
            " agui , thread-tools "
        } else {
            " agui "
        };
        let mut pairs = base();
        pairs.extend([
            ("ORCH_STEPS_RECORD_IO", "yes"),
            ("THREAD_TOOLS_ALLOWED_HOSTS", "orchestrator:8080, localhost"),
            ("LOG_FORMAT", "anything-but-text"),
        ]);
        // A comma list, with blanks, of as many surfaces as this build has.
        if cfg!(feature = "surface-agui") {
            pairs.push(("ORCH_SURFACES", surfaces));
        }
        let loaded = load_file_only(&pairs, FILE).unwrap();
        assert!(loaded.config.steps_record_io, "`yes` is true");
        let merged = loaded.merged.unwrap();
        assert!(merged.contains("recordToolIo: true"), "{merged}");
        assert!(merged.contains("format: json"), "{merged}");
        if cfg!(feature = "surface-agui") {
            assert!(merged.contains("- agui"), "{merged}");
            assert_eq!(
                merged.contains("- thread-tools"),
                cfg!(feature = "surface-thread-tools"),
                "{merged}"
            );
        }
        assert!(
            merged.contains("- orchestrator:8080") && merged.contains("- localhost"),
            "{merged}"
        );
    }

    #[test]
    fn a_variable_that_does_not_parse_is_an_error_naming_the_variable_and_no_value() {
        let mut pairs = base();
        pairs.extend([
            ("ORCH_STEPS_RECORD_IO", "maybe-hunter2"),
            ("DISPATCHER_CONCURRENCY", "many-hunter2"),
            ("ORCH_SURFACES", "a2a-hunter2"),
            ("ORCH_ROLE", "boss-hunter2"),
            ("ORCH_GATE", "magic-hunter2"),
            ("MCP_ALLOWED_HOSTS", " , "),
        ]);
        let errors = lines(load_file_only(&pairs, FILE));
        let joined = errors.join("\n");
        for var in [
            "ORCH_STEPS_RECORD_IO",
            "DISPATCHER_CONCURRENCY",
            "ORCH_SURFACES",
            "ORCH_ROLE",
            "ORCH_GATE",
            "MCP_ALLOWED_HOSTS",
        ] {
            assert!(
                errors
                    .iter()
                    .any(|l| l.starts_with(&format!("{var} is invalid"))),
                "{var}: {joined}"
            );
        }
        assert!(!joined.contains("hunter2"), "{joined}");
    }

    #[test]
    fn the_rest_of_a_group_whose_first_key_is_not_set_is_read_and_left_out() {
        // As ever: a registry timeout without a registry URL, a model key without a base URL and
        // a webhook age without secrets are heard of if they are not numbers, and mean nothing.
        let file = FILE.replace(
            "models:\n  endpoints:\n    default: { baseUrl: 'http://mock-model:8080/v1', apiKey: { env: ORCH_MODEL_API_KEY }, timeoutSecs: 9 }\ntasks:\n  title: { endpoint: default, model: small-model }\n",
            "",
        );
        let mut pairs = base();
        pairs.extend([
            ("AGENT_REGISTRY_TIMEOUT_SECS", "5"),
            ("ORCH_MODEL_TIMEOUT_SECS", "5"),
            ("WEBHOOK_GITHUB_MAX_AGE_SECS", "5"),
        ]);
        let loaded = load_file_only(&pairs, &file).unwrap();
        assert!(loaded.config.models.tasks.is_empty());
        assert!(
            loaded
                .notes
                .iter()
                .all(|n| !matches!(n, Note::Deprecated { .. }))
        );
        pairs.push(("WEBHOOK_GITHUB_MAX_AGE_SECS", "soon"));
        let errors = lines(load_file_only(&pairs, &file));
        assert!(
            errors[0].starts_with("WEBHOOK_GITHUB_MAX_AGE_SECS is invalid"),
            "{errors:?}"
        );
    }

    #[test]
    fn the_legacy_model_variables_are_the_endpoint_named_default() {
        let file = "\
version: 1
database: { url: { env: DATABASE_URL } }
agents: { file: agents.yaml }
";
        let pairs = [
            ("ORCH_CONFIG_FILE", "/etc/orch/config.yaml"),
            ("DATABASE_URL", "postgres://u:pw@db/orch"),
            ("CODER_A2A_TOKEN", "tok-123"),
            ("ORCH_MODEL_BASE_URL", "http://mock-model:8080/v1/"),
            ("ORCH_MODEL_API_KEY", "model-key-hunter2"),
            ("ORCH_TITLE_MODEL", "small-model"),
        ];
        let loaded = load_file_only(&pairs, file).unwrap();
        let models = &loaded.config.models;
        assert_eq!(models.tasks[&TaskKind::Title].model, "small-model");
        assert_eq!(models.tasks[&TaskKind::Title].endpoint, "default");
        assert_eq!(
            models.endpoints["default"].base_url, "http://mock-model:8080/v1",
            "a trailing slash is cut"
        );
        let merged = loaded.merged.unwrap();
        assert!(merged.contains("endpoint: default"), "{merged}");
        assert!(merged.contains("env: ORCH_MODEL_API_KEY"), "{merged}");
        assert!(!merged.contains("hunter2"), "{merged}");
        // With no ORCH_TITLE_MODEL there is no `tasks.title`: titles are off.
        let no_title: Vec<_> = pairs
            .iter()
            .copied()
            .filter(|(k, _)| *k != "ORCH_TITLE_MODEL")
            .collect();
        assert!(
            load_file_only(&no_title, file)
                .unwrap()
                .config
                .models
                .tasks
                .is_empty()
        );
    }

    #[test]
    fn the_model_variables_set_the_endpoint_default_and_leave_the_files_other_endpoints_be() {
        let file = FILE
            .replace("default: {", "main: {")
            .replace("endpoint: default", "endpoint: main");
        let mut pairs = base();
        pairs.push(("ORCH_MODEL_BASE_URL", "http://other:8080/v1"));
        let loaded = load_file_only(&pairs, &file).unwrap();
        let models = &loaded.config.models;
        assert_eq!(
            models.endpoints.keys().collect::<Vec<_>>(),
            ["default", "main"]
        );
        assert_eq!(models.endpoints["default"].base_url, "http://other:8080/v1");
        assert_eq!(
            models.endpoints["main"].base_url, "http://mock-model:8080/v1",
            "the file's endpoint is as it was"
        );
        assert_eq!(
            models.tasks[&TaskKind::Title].endpoint,
            "main",
            "the title task is the file's, on the file's endpoint"
        );
    }

    /// A file with several endpoints and both tasks, each with its own prompt (one inline, one a
    /// file), reaches the application as the settings it runs with.
    #[test]
    fn a_files_endpoints_and_tasks_reach_the_application_with_their_prompts_and_limits() {
        let file = "\
version: 1
database: { url: { env: DATABASE_URL } }
agents: { file: agents.yaml }
models:
  endpoints:
    default: { baseUrl: 'http://mock-model:8080/v1', apiKey: { env: ORCH_MODEL_API_KEY }, timeoutSecs: 9 }
    small: { baseUrl: 'http://small:8080/v1/', timeoutSecs: 4 }
tasks:
  title: { endpoint: small, model: small-model, system: { inline: 'Be brief.' }, maxTokens: 16, language: english }
  description:
    endpoint: default
    model: big-model
    system: { file: prompts/description.md }
    maxChars: 200
    recompute: { minNewMessages: 6 }
ui: { showDescriptions: false }
";
        let mut pairs = base();
        pairs.retain(|(k, _)| *k != "THREAD_TOOLS_SECRET");
        let loaded = load_file_only(&pairs, file).unwrap();
        let app = loaded.config.app_config();
        let title = &app.tasks[&TaskKind::Title];
        assert_eq!(
            (title.endpoint.as_str(), title.model.as_str()),
            ("small", "small-model")
        );
        assert_eq!(title.guidance.as_deref(), Some("Be brief."));
        assert_eq!(title.max_tokens, 16);
        assert_eq!(
            title.language,
            LanguageRule::Fixed(orch_core::Lang::English)
        );
        assert_eq!(title.timeout, Duration::from_secs(4), "its endpoint's");
        let description = &app.tasks[&TaskKind::Description];
        assert_eq!(description.endpoint, "default");
        assert_eq!(
            description.guidance.as_deref(),
            Some("Say what the person wants.")
        );
        assert_eq!(
            (description.max_chars, description.min_new_messages),
            (200, 6)
        );
        assert_eq!(description.max_tokens, 160);
        assert_eq!(description.timeout, Duration::from_secs(9));
        assert!(!app.public.ui.show_descriptions);
        let endpoints = &loaded.config.models.endpoints;
        assert_eq!(endpoints["small"].base_url, "http://small:8080/v1");
        assert!(endpoints["small"].api_key.is_none());
        assert!(endpoints["default"].api_key.is_some());
        let merged = loaded.merged.unwrap();
        assert!(merged.contains("showDescriptions: false"), "{merged}");
        assert!(merged.contains("maxChars: 200"), "{merged}");
    }

    const TOOL_SERVERS: &str = "\
toolServers:
  - id: websearch
    name: ' Web search '
    description: Search the web.
    url: http://search.internal:8080/mcp
    icon: 'data:image/svg+xml;base64,PHN2Zy8+'
    bearer: { env: SEARCH_TOKEN }
    tools: [search]
    agents: [coder]
    timeoutSecs: 45
  - id: docs
    name: Documentation
    url: https://docs.example.com/mcp
";

    /// The servers of the file reach the application as their public part (ADR 0024): the names,
    /// the icon, the agents and the timeout; no URL, no credential, in any `Debug`.
    #[test]
    fn the_tool_servers_reach_the_application_without_their_urls_or_credentials() {
        let mut pairs = base();
        pairs.push(("SEARCH_TOKEN", "search-token-must-not-leak"));
        let loaded = load_file_only(&pairs, &format!("{FILE}{TOOL_SERVERS}")).unwrap();
        let app = loaded.config.app_config();
        assert_eq!(app.tool_servers.len(), 2);
        let web = &app.tool_servers[0];
        assert_eq!(web.id, "websearch");
        assert_eq!(web.name, "Web search", "trimmed");
        assert_eq!(web.description.as_deref(), Some("Search the web."));
        assert_eq!(
            web.icon.as_deref(),
            Some("data:image/svg+xml;base64,PHN2Zy8+")
        );
        assert_eq!(web.tools.as_deref(), Some(&["search".to_owned()][..]));
        assert_eq!(web.agents.as_deref(), Some(&[AgentId::new("coder")][..]));
        assert_eq!(web.timeout, Duration::from_secs(45));
        assert_eq!(app.tool_servers[1].timeout, Duration::from_secs(120));
        assert!(app.tool_servers[1].agents.is_none());
        // nothing that says where a server is, or how to get in, is kept by the application
        let shown = format!("{:?} {:?} {:?}", loaded.config, app, loaded.notes);
        for hidden in [
            "search-token-must-not-leak",
            "search.internal",
            "docs.example.com",
            "PHN2Zy8+",
        ] {
            assert!(!shown.contains(hidden), "{hidden} is in a Debug: {shown}");
        }
        let merged = loaded.merged.unwrap();
        assert!(
            merged.contains("toolServers:") && merged.contains("env: SEARCH_TOKEN"),
            "{merged}"
        );
        assert!(!merged.contains("search-token-must-not-leak"), "{merged}");
    }

    /// The default is no server, and the variables alone cannot name one.
    #[test]
    fn without_the_key_nothing_is_attachable() {
        let loaded = load_file_only(&base(), FILE).unwrap();
        assert!(loaded.config.app_config().tool_servers.is_empty());
        assert!(!loaded.merged.unwrap().contains("toolServers"));
    }

    /// An agent a server names that the agents file does not have is a typo that would otherwise
    /// make a server nobody is offered (checked when the file's agents are the whole list).
    #[test]
    fn a_server_for_an_agent_the_agents_file_lacks_is_refused_naming_the_key() {
        let file = format!(
            "{FILE}toolServers:\n  - id: a\n    name: A\n    url: https://a.example.com/mcp\n    agents: [coder, ghost]\n"
        );
        let got = lines(load_file_only(&base(), &file));
        assert_eq!(
            got,
            ["toolServers[0].agents[1]: names no agent of the agents file"]
        );
    }

    /// A credential at a plain `http://` URL is a warning that names the server and nothing else.
    #[test]
    fn a_credential_over_plain_http_is_a_note_naming_the_server() {
        let mut pairs = base();
        pairs.push(("SEARCH_TOKEN", "search-token-must-not-leak"));
        let loaded = load_file_only(&pairs, &format!("{FILE}{TOOL_SERVERS}")).unwrap();
        assert!(
            loaded.notes.contains(&Note::ToolServerCredentialOverHttp {
                id: "websearch".to_owned()
            }),
            "{:?}",
            loaded.notes
        );
        // `docs` is https and has none; a server with no credential over http says nothing
        assert_eq!(
            loaded
                .notes
                .iter()
                .filter(|n| matches!(n, Note::ToolServerCredentialOverHttp { .. }))
                .count(),
            1
        );
        let said = loaded
            .notes
            .iter()
            .map(Note::describe)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !said.contains("search.internal") && !said.contains("must-not-leak"),
            "{said}"
        );
    }

    /// The names the file spells a language with are the core's: none falls back to the person's
    /// language by accident.
    #[test]
    fn every_language_of_the_file_is_a_rule_of_the_core() {
        use orch_config::Language;
        for language in [
            Language::Conversation,
            Language::English,
            Language::French,
            Language::German,
            Language::Spanish,
            Language::Portuguese,
            Language::Italian,
            Language::Chinese,
            Language::Japanese,
            Language::Korean,
            Language::Cyrillic,
            Language::Arabic,
            Language::Hebrew,
            Language::Greek,
            Language::Devanagari,
            Language::Thai,
        ] {
            let rule = LanguageRule::from_config_name(language.as_str());
            assert!(rule.is_some(), "{language:?}");
            assert_eq!(
                rule == Some(LanguageRule::Conversation),
                language == Language::Conversation
            );
        }
    }

    /// A file that mounts everything, for the webhook secrets.
    #[cfg(feature = "surface-webhook")]
    const WEBHOOKS: &str = "\
version: 1
server: { surfaces: [agui, webhook-generic, webhook-github] }
database: { url: { env: DATABASE_URL } }
agents: { file: agents.yaml }
webhooks:
  generic: { secrets: [{ env: WEBHOOK_GENERIC_SECRETS }] }
  github: { secrets: [{ env: GH_ONE }, { file: /run/secrets/key }] }
";

    #[cfg(feature = "surface-webhook")]
    #[test]
    fn webhook_secrets_keep_the_comma_rule_of_their_variable_and_a_file_secret_is_one_secret() {
        let two = format!("{KEY}-one,{KEY}-two");
        let pairs = [
            ("ORCH_CONFIG_FILE", "/etc/orch/config.yaml"),
            ("DATABASE_URL", "postgres://u:pw@db/orch"),
            ("CODER_A2A_TOKEN", "tok-123"),
            ("WEBHOOK_GENERIC_SECRETS", two.as_str()),
            ("GH_ONE", KEY),
        ];
        let loaded = load_file_only(&pairs, WEBHOOKS).unwrap();
        assert!(loaded.config.webhook_generic.is_some());
        assert!(loaded.config.webhook_github.is_some());
        // The variable holds three: the rule of the variable, which refuses it, as before.
        let three = format!("{KEY}-1,{KEY}-2,{KEY}-3");
        let mut bad = pairs.to_vec();
        bad[3] = ("WEBHOOK_GENERIC_SECRETS", three.as_str());
        let errors = lines(load_file_only(&bad, WEBHOOKS));
        assert!(
            errors
                .iter()
                .any(|l| l.starts_with("webhooks.generic.secrets")),
            "{errors:?}"
        );
    }

    #[cfg(feature = "surface-webhook")]
    #[test]
    fn errors_of_the_rules_the_binary_has_always_had_name_keys_and_carry_no_value() {
        // A gate that requires `ci` names a check; a verifier is another configured agent.
        let file = "\
version: 1
server: { surfaces: [agui, webhook-generic] }
database: { url: { env: DATABASE_URL } }
agents: { file: agents.yaml }
gate: { require: [ci, verifier], verifier: nobody }
webhooks: { generic: { secrets: [{ env: GH_ONE }] } }
";
        let pairs = [
            ("ORCH_CONFIG_FILE", "/etc/orch/config.yaml"),
            ("DATABASE_URL", "postgres://u:hunter2@db/orch"),
            ("CODER_A2A_TOKEN", "tok-123"),
            ("GH_ONE", KEY),
        ];
        let errors = lines(load_file_only(&pairs, file));
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("gate"), "{errors:?}");
        assert!(
            !errors[0].contains("ORCH_GATE") && !errors[0].contains("hunter2"),
            "{errors:?}"
        );
    }

    #[test]
    fn a_missing_file_a_syntax_error_and_every_shape_error_are_exit_78_errors() {
        let pairs = base();
        let missing = Loaded::load(args_of(&pairs), FlagSecrets::new(), env_of(&pairs), |_| {
            Err(io::ErrorKind::NotFound.into())
        });
        assert_eq!(
            lines(missing),
            ["ORCH_CONFIG_FILE: cannot read the configuration file /etc/orch/config.yaml"]
        );
        let errors = lines(load_file_only(&pairs, "version: 1\ndatabase: [\n"));
        assert_eq!(errors.len(), 1);
        assert!(
            errors[0].starts_with("the YAML cannot be read (line "),
            "{errors:?}"
        );
        let errors = lines(load_file_only(
            &pairs,
            "version: 1\nnonsense: 1\nserver: { role: boss }\n",
        ));
        assert_eq!(
            errors,
            [
                "nonsense: unknown key",
                "server.role: not an allowed value (allowed: all, control-plane, worker)"
            ],
            "every shape error is listed at once"
        );
        assert!(matches!(
            load_file_only(&pairs, "version: 1\n").unwrap_err(),
            ConfigError::Document(_)
        ));
    }

    #[test]
    fn print_config_shows_the_merged_configuration_with_references_and_no_value() {
        let mut args = args_of(&base());
        args.print_config = true;
        let pairs = base();
        let files: HashMap<&str, String> = HashMap::from([
            ("/etc/orch/config.yaml", FILE.to_owned()),
            ("/etc/orch/agents.yaml", AGENTS.to_owned()),
        ]);
        let loaded = Loaded::load(args, FlagSecrets::new(), env_of(&pairs), move |p| {
            files
                .get(p.to_str().unwrap())
                .cloned()
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        })
        .unwrap();
        let merged = loaded.merged.unwrap();
        for reference in [
            "env: DATABASE_URL",
            "env: ORCH_MODEL_API_KEY",
            "env: THREAD_TOOLS_SECRET",
        ] {
            assert!(merged.contains(reference), "{reference} in {merged}");
        }
        for value in ["hunter2", KEY, "tok-123", "postgres://"] {
            assert!(!merged.contains(value), "{value} in {merged}");
        }
        // Defaults are filled in.
        assert!(merged.contains("shutdownGraceSecs: 15") && merged.contains("maxAttempts: 3"));
        // And the merged text is itself a configuration the loader reads back.
        let again = orch_config::parse_yaml(&merged).unwrap();
        assert!(orch_config::check(&again).is_ok(), "{merged}");
    }

    #[test]
    fn print_config_without_a_file_prints_what_the_environment_alone_says() {
        let pairs = [
            ("DATABASE_URL", "postgres://u:hunter2@db/orch"),
            ("AGENTS_FILE", "/etc/orch/agents.yaml"),
            ("CODER_A2A_TOKEN", "tok-123"),
            ("LISTEN_ADDR", "0.0.0.0:9300"),
        ];
        let mut args = args_of(&pairs);
        args.print_config = true;
        let loaded = Loaded::load(args, FlagSecrets::new(), env_of(&pairs), |_| {
            Ok(AGENTS.to_owned())
        })
        .unwrap();
        let merged = loaded.merged.unwrap();
        assert!(
            merged.contains("listen: 0.0.0.0:9300") && merged.contains("env: DATABASE_URL"),
            "{merged}"
        );
        assert!(!merged.contains("hunter2"));
        assert_eq!(loaded.notes[0], Note::EnvironmentOnly);
    }

    #[cfg(feature = "agent-local")]
    #[test]
    fn local_concurrency_is_a_key_of_a_build_with_local_agents() {
        let file = FILE.replace(
            "agents:\n  file: agents.yaml",
            "agents:\n  file: agents.yaml\n  localConcurrency: 2",
        );
        let loaded = load_file_only(&base(), &file).unwrap();
        assert_eq!(loaded.config.agent_local_concurrency, 2);
        let mut pairs = base();
        pairs.push(("AGENT_LOCAL_CONCURRENCY", "6"));
        let loaded = load_file_only(&pairs, &file).unwrap();
        assert_eq!(loaded.config.agent_local_concurrency, 6);
        assert!(loaded.notes.contains(&Note::Deprecated {
            var: "AGENT_LOCAL_CONCURRENCY",
            key: "agents.localConcurrency".to_owned(),
            overrides: true,
        }));
    }

    #[cfg(not(feature = "agent-local"))]
    #[test]
    fn local_concurrency_is_refused_in_a_build_without_local_agents() {
        let file = FILE.replace(
            "agents:\n  file: agents.yaml",
            "agents:\n  file: agents.yaml\n  localConcurrency: 2",
        );
        let errors = lines(load_file_only(&base(), &file));
        assert_eq!(errors.len(), 1);
        assert!(
            errors[0]
                .starts_with("agents.localConcurrency: needs the Cargo feature \"agent-local\""),
            "{errors:?}"
        );
    }

    #[test]
    fn a_registry_is_read_with_its_secrets_and_a_relative_agents_file_is_next_to_the_file() {
        let file = "\
version: 1
database: { url: { env: DATABASE_URL } }
agents:
  file: agents.yaml
  registry: { url: 'https://r.example.com/agents', agentToken: { file: /run/secrets/key }, timeoutSecs: 5 }
";
        let mut pairs = base();
        pairs.retain(|(k, _)| *k != "THREAD_TOOLS_SECRET");
        let loaded = load_file_only(&pairs, file);
        #[cfg(feature = "registry-platform")]
        {
            let loaded = loaded.unwrap();
            let registry = loaded.config.registry.unwrap();
            assert_eq!(registry.url, "https://r.example.com/agents");
            assert_eq!(registry.timeout, Duration::from_secs(5));
            assert!(registry.agent_token.is_some() && registry.token.is_none());
            assert_eq!(
                loaded.config.agents.len(),
                1,
                "the file beside config.yaml was read"
            );
        }
        #[cfg(not(feature = "registry-platform"))]
        {
            let errors = lines(loaded);
            assert!(errors[0].contains("agents.registry.url"), "{errors:?}");
        }
    }

    #[test]
    fn a_variable_in_a_message_is_replaced_by_its_key_by_whole_words_only() {
        assert_eq!(
            in_keys(
                "THREAD_TOOLS_SECRET_PREVIOUS is not THREAD_TOOLS_SECRET; XTHREAD_TOOLS_SECRET"
            ),
            "threadTools.previousSecret is not threadTools.secret; XTHREAD_TOOLS_SECRET"
        );
        assert_eq!(
            scrub(r#"a "secret" and "an \"escaped\" one" end"#),
            r#"a "…" and "…" end"#
        );
    }

    #[test]
    fn the_table_covers_every_variable_that_has_a_key_and_each_key_is_in_the_contract() {
        let doc = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs/api/config.md"),
        )
        .unwrap();
        let mut seen = std::collections::HashSet::new();
        for s in SETTINGS {
            assert!(seen.insert(s.var), "{} twice", s.var);
            let documented = s.dotted().replace("endpoints.default", "endpoints.<name>");
            assert!(
                doc.contains(&format!("| `{documented}` |")),
                "{} ({documented}) is not a row of docs/api/config.md",
                s.var,
            );
            assert!(
                doc.contains(s.var),
                "{} is not in docs/api/config.md",
                s.var
            );
        }
        // 51 variables have a key; the build without `agent-local` has no flag for one of them.
        assert_eq!(
            SETTINGS.len(),
            if cfg!(feature = "agent-local") {
                51
            } else {
                50
            }
        );
    }

    /// The artifact store (ADR 0032): the section reaches the `Config`, resolved, and a build
    /// without the store's Cargo feature refuses it.
    mod artifacts {
        use super::*;
        #[cfg(any(feature = "artifacts-fs", feature = "artifacts-s3"))]
        use crate::artifacts::StoreSettings;

        fn with(section: &str) -> String {
            format!("{FILE}{section}")
        }

        fn s3_env<'a>() -> Vec<(&'a str, &'a str)> {
            let mut pairs = base();
            pairs.push(("S3_ACCESS_KEY_ID", "AKIDEXAMPLE"));
            pairs.push(("S3_SECRET_ACCESS_KEY", "s3cr3t-access-key"));
            pairs
        }

        const S3: &str = "\
artifacts:
  store: s3
  maxFileBytes: 2048
  s3:
    bucket: orchestrator-files
    region: eu-central-1
    endpoint: https://minio.example.com:9000
    prefix: prod
    accessKeyId: { env: S3_ACCESS_KEY_ID }
    secretAccessKey: { env: S3_SECRET_ACCESS_KEY }
    timeoutSecs: 7
";

        #[test]
        fn no_section_and_no_file_are_no_store() {
            let loaded = load_file_only(&base(), FILE).unwrap();
            assert!(loaded.config.artifacts.is_none());
            assert!(!loaded.merged.unwrap().contains("artifacts"));
            let by_variables = Loaded::load(
                args_of(&[("DATABASE_URL", "postgres://u:hunter2@db/orch")]),
                FlagSecrets::new(),
                env_of(&[]),
                |_| Ok(AGENTS.to_owned()),
            );
            assert!(by_variables.is_err() || by_variables.unwrap().config.artifacts.is_none());
        }

        #[cfg(feature = "artifacts-fs")]
        #[test]
        fn a_directory_store_reaches_the_configuration_with_its_root_resolved() {
            let loaded = load_file_only(
                &base(),
                &with("artifacts: { store: fs, fs: { root: files }, maxFileBytes: 4096 }\n"),
            )
            .unwrap();
            let artifacts = loaded.config.artifacts.unwrap();
            assert_eq!(artifacts.max_file_bytes, 4096);
            match artifacts.store {
                StoreSettings::Fs { root } => {
                    assert_eq!(root, Path::new("/etc/orch/files"), "relative to the file")
                }
                other @ StoreSettings::S3(_) => panic!("{other:?}"),
            }
            assert!(
                loaded.merged.unwrap().contains("root: files"),
                "--print-config shows the key as written"
            );
        }

        /// The limits and the fetch hosts of the ingest (ADR 0032) reach the application's
        /// configuration and the A2A client's.
        #[cfg(feature = "artifacts-fs")]
        #[test]
        fn the_ingest_limits_and_the_fetch_hosts_reach_the_configuration() {
            let loaded = load_file_only(
                &base(),
                &with(
                    "artifacts: { store: fs, fs: { root: files }, maxFileBytes: 4096, \
                     maxPerJobBytes: 8192, fetchHosts: [files.example.com, \"10.0.0.5:8080\"] }\n",
                ),
            )
            .unwrap();
            let artifacts = loaded.config.artifacts.as_ref().unwrap();
            assert_eq!(artifacts.max_per_job_bytes, 8192);
            assert_eq!(
                artifacts.fetch_hosts,
                ["files.example.com", "10.0.0.5:8080"]
            );
            let limits = loaded.config.app_config().files;
            assert_eq!(
                (
                    limits.max_file_bytes,
                    limits.max_per_job_bytes,
                    limits.max_files_per_job
                ),
                (4096, 8192, 50)
            );
            // without the section the defaults stand, and nothing is fetched
            let none = load_file_only(&base(), &with("")).unwrap();
            assert!(none.config.artifacts.is_none());
            assert_eq!(
                none.config.app_config().files,
                orch_app::FileLimits::default()
            );
        }

        #[cfg(feature = "artifacts-s3")]
        #[test]
        fn a_bucket_store_reaches_the_configuration_with_its_credentials_resolved_and_hidden() {
            let loaded = load_file_only(&s3_env(), &with(S3)).unwrap();
            let artifacts = loaded.config.artifacts.as_ref().unwrap();
            assert_eq!(artifacts.max_file_bytes, 2048);
            let StoreSettings::S3(s3) = &artifacts.store else {
                panic!("{artifacts:?}")
            };
            assert_eq!(
                (s3.bucket.as_str(), s3.region.as_str(), s3.prefix.as_deref()),
                ("orchestrator-files", "eu-central-1", Some("prod"))
            );
            assert_eq!(
                s3.endpoint.as_deref(),
                Some("https://minio.example.com:9000")
            );
            assert_eq!(s3.timeout, Duration::from_secs(7));
            use secrecy::ExposeSecret as _;
            assert_eq!(s3.access_key_id.expose_secret(), "AKIDEXAMPLE");
            assert_eq!(s3.secret_access_key.expose_secret(), "s3cr3t-access-key");
            // no credential in a Debug of the configuration or in what --print-config prints
            let debug = format!("{:?}", loaded.config);
            let printed = loaded.merged.unwrap();
            for text in [&debug, &printed] {
                assert!(
                    !text.contains("AKIDEXAMPLE") && !text.contains("s3cr3t"),
                    "{text}"
                );
            }
            assert!(printed.contains("S3_SECRET_ACCESS_KEY"), "{printed}");
        }

        #[cfg(feature = "artifacts-s3")]
        #[test]
        fn a_bucket_store_whose_credentials_are_missing_is_78_naming_the_variable() {
            let errors = lines(load_file_only(&base(), &with(S3)));
            assert_eq!(
                errors,
                [
                    "artifacts.s3.accessKeyId: the environment variable S3_ACCESS_KEY_ID is unset or empty",
                    "artifacts.s3.secretAccessKey: the environment variable S3_SECRET_ACCESS_KEY is unset or empty",
                ]
            );
        }

        /// A key that selects an implementation names only what the build has: exit 78 naming the
        /// feature, never ignored.
        #[cfg(not(feature = "artifacts-fs"))]
        #[test]
        fn a_directory_store_is_refused_by_a_build_without_it() {
            let errors = lines(load_file_only(
                &base(),
                &with("artifacts: { store: fs, fs: { root: files } }\n"),
            ));
            assert_eq!(errors.len(), 1, "{errors:?}");
            assert!(
                errors[0]
                    .starts_with("artifacts.store: fs needs the Cargo feature \"artifacts-fs\""),
                "{errors:?}"
            );
        }

        #[cfg(not(feature = "artifacts-s3"))]
        #[test]
        fn a_bucket_store_is_refused_by_a_build_without_it() {
            let errors = lines(load_file_only(&s3_env(), &with(S3)));
            assert_eq!(errors.len(), 1, "{errors:?}");
            assert!(
                errors[0]
                    .starts_with("artifacts.store: s3 needs the Cargo feature \"artifacts-s3\""),
                "{errors:?}"
            );
            assert!(!errors[0].contains("s3cr3t"));
        }

        /// The rules of the file are checked first, whatever the build has.
        #[test]
        fn a_store_without_its_section_is_refused_by_every_build() {
            let errors = lines(load_file_only(&base(), &with("artifacts: { store: s3 }\n")));
            assert_eq!(
                errors,
                ["artifacts.s3: required when artifacts.store is s3"]
            );
        }
    }
}
