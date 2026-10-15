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

use orch_app::{PublicConfig, TaskSettings, ToolServerInfo, UiHistory, UiSettings};
use orch_config::{Resolve, SecretRef, UrlRef, Validated};
use orch_core::{AgentId, LanguageRule, TaskKind};
use orch_ports::{ToolSecret, ToolServerEndpoint};
use secrecy::SecretString;
use serde_json::{Map, Value};

use super::{
    Args, Config, ConfigError, EndpointSettings, LogFormat, ModelsSettings, Resolved, Surface,
    flag, number, parse_surfaces,
};
use crate::artifacts::{ArtifactSettings, S3Settings, StoreSettings, UploadLimits};
use crate::user_tools::UserToolsSettings;

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

/// The artifact store of a configuration file, and nothing else of it: what
/// `orchestrator artifacts migrate` runs with (ADR 0058 A5.4).
#[cfg(all(feature = "artifacts-fs", feature = "artifacts-s3"))]
#[derive(Debug)]
pub struct StoreOnly {
    /// The S3 store of the file, built as the service builds it. It has no public endpoint: a
    /// migration signs nothing for anybody.
    pub settings: ArtifactSettings,
    /// `log.format`.
    pub log_format: LogFormat,
}

/// The text of the configuration file `ORCH_CONFIG_FILE` or `--config` names, and the directory
/// its relative paths are relative to, for a command that reads the file and not the service's
/// environment. `wants` says what the command reads it for, in the error when there is no file.
///
/// # Errors
/// A configuration error (exit 78): no file, or a file that cannot be read.
#[cfg(any(
    feature = "secrets-pg",
    all(feature = "artifacts-fs", feature = "artifacts-s3")
))]
fn config_file(args: &Args, wants: &str) -> Result<(String, PathBuf), ConfigError> {
    let Some(path) = clean(args.config.as_deref()) else {
        return Err(document(format!(
            "ORCH_CONFIG_FILE: {wants}; set ORCH_CONFIG_FILE or --config"
        )));
    };
    let path = PathBuf::from(path);
    let text = read_limited(&path, MAX_FILE_BYTES).map_err(|_| {
        document(format!(
            "ORCH_CONFIG_FILE: cannot read the configuration file {}",
            path.display()
        ))
    })?;
    let base = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_owned);
    Ok((text, base))
}

/// Loads the store of the file `ORCH_CONFIG_FILE` or `--config` names, for a command that migrates
/// into it: the file's syntax and shape are checked whole, then **only** the rules of the
/// `artifacts` section and the two credentials of the store are checked and read. The database, the
/// agents, the thread-tools key and the model keys are not (the Job that runs the command is not
/// given them). Variables and flags are not laid over the file: `artifacts` has none.
///
/// # Errors
/// A configuration error (exit 78): no file, a file that cannot be read or is wrong, a store that is
/// not `s3`, a credential that does not resolve.
#[cfg(all(feature = "artifacts-fs", feature = "artifacts-s3"))]
pub fn load_store(args: &Args) -> Result<StoreOnly, ConfigError> {
    let (text, base) = config_file(
        args,
        "artifacts migrate reads the artifact store from the configuration file",
    )?;
    load_store_from(
        &text,
        &base,
        |name| std::env::var(name).ok(),
        |path| read_limited(path, MAX_FILE_BYTES),
    )
}

/// [`load_store`] with the text of the file, the environment and the file system as parameters, so a
/// test reads none of them.
#[cfg(all(feature = "artifacts-fs", feature = "artifacts-s3"))]
fn load_store_from(
    text: &str,
    base_dir: &Path,
    env: impl Fn(&str) -> Option<String>,
    read: impl Fn(&Path) -> io::Result<String>,
) -> Result<StoreOnly, ConfigError> {
    let tree = orch_config::parse_yaml(text).map_err(file_errors)?;
    let config = orch_config::check(&tree).map_err(file_errors)?;
    let no_flags = FlagSecrets::new();
    let resolver = ProcessResolver {
        env: &env,
        read: &read,
        flags: &no_flags,
    };
    let valid = config
        .validate_artifacts(base_dir, &resolver)
        .map_err(file_errors)?;
    let (Some(s3), Some(access_key_id), Some(secret_access_key)) = (
        valid.artifacts.s3.as_ref(),
        valid.s3_access_key_id.as_ref(),
        valid.s3_secret_access_key.as_ref(),
    ) else {
        return Err(document(format!(
            "artifacts.store: artifacts migrate writes to the store of the configuration, which must be s3 \
             (the file says {})",
            valid.artifacts.store.as_str()
        )));
    };
    let store = S3Settings {
        // nothing is presigned for anybody here
        public_endpoint: None,
        ..s3_settings(s3, access_key_id.expose(), secret_access_key.expose())
    };
    Ok(StoreOnly {
        settings: ArtifactSettings {
            store: StoreSettings::S3(store),
            max_file_bytes: valid.artifacts.max_file_bytes,
            max_per_job_bytes: valid.artifacts.max_per_job_bytes,
            fetch_hosts: Vec::new(),
            upload: None,
            downloads: orch_app::DownloadMode::Stream,
        },
        log_format: match valid.log_format {
            orch_config::LogFormat::Json => LogFormat::Json,
            orch_config::LogFormat::Text => LogFormat::Text,
        },
    })
}

/// The database and the secret store of a configuration file, and nothing else of it: what
/// `orchestrator secrets rewrap` runs with (ADR 0046, slice 3).
#[cfg(feature = "secrets-pg")]
pub struct UserSecretsOnly {
    /// `database.url`.
    pub database_url: SecretString,
    /// The keys of `secrets.pg`, with `secrets.pg.current` the one that seals.
    pub keyring: orch_secrets_pg::Keyring,
    /// `log.format`.
    pub log_format: LogFormat,
}

#[cfg(feature = "secrets-pg")]
impl std::fmt::Debug for UserSecretsOnly {
    /// The ids of the keys and nothing that opens them.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserSecretsOnly")
            .field("database_url", &"<redacted>")
            .field("keyring", &self.keyring)
            .field("log_format", &self.log_format)
            .finish()
    }
}

/// Loads the database and the secret store of the file `ORCH_CONFIG_FILE` or `--config` names, for
/// a command that re-seals the secrets: the file's syntax and shape are checked whole, then
/// **only** `database.url`, the rules of the `secrets` section and its keys are checked and read.
/// The agents, the thread-tools key and the model keys are not (the Job that runs the command is
/// not given them). Variables and flags are not laid over the file: `secrets` has none.
///
/// # Errors
/// A configuration error (exit 78): no file, a file that cannot be read or is wrong, no `secrets`
/// section, a key or the database URL that does not resolve.
#[cfg(feature = "secrets-pg")]
pub fn load_user_secrets(args: &Args) -> Result<UserSecretsOnly, ConfigError> {
    let (text, base) = config_file(
        args,
        "secrets rewrap reads the database and the secret store's keys from the configuration file",
    )?;
    load_user_secrets_from(
        &text,
        &base,
        |name| std::env::var(name).ok(),
        |path| read_limited(path, MAX_FILE_BYTES),
    )
}

/// [`load_user_secrets`] with the text of the file, the environment and the file system as
/// parameters, so a test reads none of them.
#[cfg(feature = "secrets-pg")]
fn load_user_secrets_from(
    text: &str,
    base_dir: &Path,
    env: impl Fn(&str) -> Option<String>,
    read: impl Fn(&Path) -> io::Result<String>,
) -> Result<UserSecretsOnly, ConfigError> {
    let tree = orch_config::parse_yaml(text).map_err(file_errors)?;
    let config = orch_config::check(&tree).map_err(file_errors)?;
    let no_flags = FlagSecrets::new();
    let resolver = ProcessResolver {
        env: &env,
        read: &read,
        flags: &no_flags,
    };
    let valid = config
        .validate_user_secrets(base_dir, &resolver)
        .map_err(file_errors)?;
    let Some(pg) = valid.secrets.pg.as_ref() else {
        // the rules require the section of the store the file names
        return Err(document("secrets.pg: required when secrets.store is pg"));
    };
    // The rules have checked every id and every key; what `orch-secrets-pg` says here is the same
    // sentence, and it quotes nothing.
    let keyring =
        crate::user_tools::keyring(pg, &valid.keys).map_err(|e| document(e.to_string()))?;
    Ok(UserSecretsOnly {
        database_url: SecretString::from(valid.database_url.expose()),
        keyring,
        log_format: match valid.log_format {
            orch_config::LogFormat::Json => LogFormat::Json,
            orch_config::LogFormat::Text => LogFormat::Text,
        },
    })
}

/// `artifacts.s3` in the terms of this process, with its two credentials as they were read.
fn s3_settings(
    s3: &orch_config::ArtifactsS3,
    access_key_id: &str,
    secret_access_key: &str,
) -> S3Settings {
    S3Settings {
        bucket: s3.bucket.clone(),
        region: s3.region.clone(),
        endpoint: s3.endpoint.clone(),
        public_endpoint: s3.public_endpoint.clone(),
        prefix: s3.prefix.clone(),
        access_key_id: SecretString::from(access_key_id),
        secret_access_key: SecretString::from(secret_access_key),
        timeout: Duration::from_secs(s3.timeout_secs),
    }
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
        "ORCH_ASK_MAX_DEPTH",
        &["asks", "maxDepth"],
        Kind::Uint,
        |a| &a.ask_max_depth,
    ),
    setting(
        "ORCH_ASK_MAX_PER_JOB",
        &["asks", "maxPerJob"],
        Kind::Uint,
        |a| &a.ask_max_per_job,
    ),
    setting(
        "ORCH_ASK_MAX_RUNNING",
        &["asks", "maxRunning"],
        Kind::Uint,
        |a| &a.ask_max_running,
    ),
    setting(
        "ORCH_ASK_TIMEOUT_SECS",
        &["asks", "timeoutSecs"],
        Kind::Uint,
        |a| &a.ask_timeout_secs,
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

    // `secrets.store` names only what this build compiled in: refused, never ignored (ADR 0046).
    if let Some(secrets) = &valid.config.secrets {
        let compiled = match secrets.store {
            orch_config::SecretStoreKind::Pg => cfg!(feature = "secrets-pg"),
        };
        if !compiled {
            return Err(document(format!(
                "secrets.store: {} needs the Cargo feature \"{}\", which this build does not have \
                 (ADR 0046)",
                secrets.store.as_str(),
                secrets.store.feature()
            )));
        }
    }

    let (file_args, mut resolved) = project(&valid, &tree, args.hostname.clone())?;
    (resolved.sharing, resolved.public_limits) = sharing_of(&valid)?;
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

/// `sharing` of a valid file (ADR 0040) as the application and the API take it: the cap, the keys
/// links are made under, what a public reader sees, the address links are shown under
/// (`server.publicUrl`) and the limits of the public routes.
///
/// The file's rules have checked the secrets (present for a cap above `disabled`, long enough, not
/// the thread tools' key) and that `public` and `rateLimit` come only with `mode: public`, so the
/// errors here are not paths a valid file takes: they are refused, never ignored.
fn sharing_of(
    valid: &Validated,
) -> Result<(orch_app::SharingSettings, orch_api::PublicLimits), ConfigError> {
    use orch_app::{PublicView, ShareKeys, SharingMode, SharingSettings};
    use secrecy::SecretString;

    let sharing = &valid.config.sharing;
    let mode = match sharing.mode {
        orch_config::SharingMode::Disabled => SharingMode::Disabled,
        orch_config::SharingMode::Internal => SharingMode::Internal,
        orch_config::SharingMode::Public => SharingMode::Public,
    };
    let keys = valid
        .secrets
        .sharing_secret
        .as_ref()
        .map(|current| {
            ShareKeys::new(
                SecretString::from(current.expose().to_owned()),
                valid
                    .secrets
                    .sharing_previous_secret
                    .as_ref()
                    .map(|previous| SecretString::from(previous.expose().to_owned())),
            )
        })
        .transpose()
        .map_err(|e| document(format!("sharing.secret: {e}")))?;
    let public = sharing
        .public
        .as_ref()
        .map_or_else(PublicView::default, |p| PublicView {
            step_io: p.step_io,
            files: p.files,
        });
    let settings = SharingSettings::new(mode, keys, public)
        .map_err(|e| document(format!("sharing: {e}")))?
        .with_base_url(valid.config.server.public_url.clone());
    let limits = sharing.rate_limit.clone().unwrap_or_default();
    let narrow = |n: u64| u32::try_from(n).unwrap_or(u32::MAX);
    Ok((
        settings,
        orch_api::PublicLimits {
            per_link_per_second: narrow(limits.per_link_per_second),
            total_per_second: narrow(limits.total_per_second),
            streams_per_link: narrow(limits.streams_per_link),
            streams_total: narrow(limits.streams_total),
        },
    ))
}

fn file_errors(errors: Vec<orch_config::ConfigError>) -> ConfigError {
    ConfigError::Document(errors.iter().map(ToString::to_string).collect())
}

// ---- the file, as the legacy arguments --------------------------------------------------------

/// The valid file as the [`Args`] [`Config::load`] reads, and the secrets that cannot go through
/// a string. `tree` says which keys the file (or a variable) *set*, for the defaults that depend
/// on whether a key is there.
fn project(
    valid: &Validated,
    tree: &Value,
    hostname: Option<String>,
) -> Result<(Args, Resolved), ConfigError> {
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
        ask_max_depth: some(c.asks.max_depth.to_string()),
        ask_max_per_job: some(c.asks.max_per_job.to_string()),
        ask_max_running: some(c.asks.max_running.to_string()),
        ask_timeout_secs: some(c.asks.timeout_secs.to_string()),
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
        artifacts: artifact_settings(valid)?,
        // People's own MCP servers (ADR 0046): the egress policy and the keys, only when enabled.
        user_tools: UserToolsSettings::from_file(valid).map_err(document)?,
        auth: super::AuthSettings::from_file(&c.auth),
        models: Some(models_of(valid)),
        tool_servers: tool_servers_of(&c.tool_servers),
        tool_endpoints: tool_endpoints_of(valid),
        public: Some(PublicConfig {
            ui: UiSettings {
                show_descriptions: c.ui.show_descriptions,
                // `projection` is the build's, set where the surfaces are known
                // (`Config::public_config`)
                history: Some(UiHistory {
                    initial_turns: c.ui.history.initial_turns,
                    page_turns: c.ui.history.page_turns,
                    max_turns: c.server.history.max_turns,
                    projection: 0,
                    windowed: c.ui.history.windowed,
                }),
            },
        }),
        // Built by `sharing_of`, which can refuse; the defaults stand for a disabled deployment.
        sharing: orch_app::SharingSettings::default(),
        public_limits: orch_api::PublicLimits::default(),
        history: orch_app::HistorySettings {
            max_turns: c.server.history.max_turns as usize,
            max_page_bytes: usize::try_from(c.server.history.max_page_bytes).unwrap_or(usize::MAX),
        },
        cors_allowed_origins: c
            .server
            .cors
            .as_ref()
            .map(|cors| cors.allowed_origins.clone())
            .unwrap_or_default(),
        #[cfg(feature = "surface-webhook")]
        webhook_generic: webhook_values(&s.webhook_generic, "WEBHOOK_GENERIC_SECRETS"),
        #[cfg(feature = "surface-webhook")]
        webhook_github: webhook_values(&s.webhook_github, "WEBHOOK_GITHUB_SECRETS"),
    };
    Ok((a, resolved))
}

/// The artifact store of the valid file, in the terms of this process: the directory resolved
/// against the file's directory, the credentials as resolved secrets. `None` without the section.
///
/// # Errors
/// A configuration error (exit 78) for a download lifetime the application cannot sign.
fn artifact_settings(valid: &Validated) -> Result<Option<ArtifactSettings>, ConfigError> {
    let Some(artifacts) = valid.config.artifacts.as_ref() else {
        return Ok(None);
    };
    let (store, downloads) = match artifacts.store {
        orch_config::ArtifactStoreKind::Fs => (
            valid
                .artifacts_fs_root()
                .map(|root| StoreSettings::Fs { root }),
            orch_app::DownloadMode::Stream,
        ),
        orch_config::ArtifactStoreKind::S3 => {
            let s3 = artifacts.s3.as_ref();
            let secrets = &valid.secrets;
            let store = s3.and_then(|s3| {
                Some(StoreSettings::S3(s3_settings(
                    s3,
                    secrets.s3_access_key_id.as_ref()?.expose(),
                    secrets.s3_secret_access_key.as_ref()?.expose(),
                )))
            });
            // only an S3 store has the mode (the file's rules refuse the section for any other)
            let downloads = s3.map_or(Ok(orch_app::DownloadMode::Stream), download_mode)?;
            (store, downloads)
        }
    };
    let Some(store) = store else {
        return Ok(None);
    };
    Ok(Some(ArtifactSettings {
        store,
        max_file_bytes: artifacts.max_file_bytes,
        max_per_job_bytes: artifacts.max_per_job_bytes,
        fetch_hosts: artifacts.fetch_hosts.clone(),
        upload: artifacts.upload.as_ref().map(|upload| UploadLimits {
            max_bytes: upload.max_bytes,
            ttl: Duration::from_secs(upload.ttl_secs),
        }),
        downloads,
    }))
}

/// `artifacts.s3.downloads` and `presignTtlSecs` as the application's [`orch_app::DownloadMode`].
/// The file's rules check the lifetime's range and the mode checks it again (it is what signs): a
/// lifetime the rules let through and the mode refuses is a configuration error, never a store that
/// is quietly streamed instead of redirected.
fn download_mode(s3: &orch_config::ArtifactsS3) -> Result<orch_app::DownloadMode, ConfigError> {
    match s3.downloads {
        orch_config::ArtifactDownloads::Stream => Ok(orch_app::DownloadMode::Stream),
        orch_config::ArtifactDownloads::Redirect => orch_app::DownloadMode::redirect(
            Duration::from_secs(s3.presign_ttl_secs),
        )
        .map_err(|e| {
            document(format!(
                "artifacts.s3.presignTtlSecs: {e} (it is {})",
                s3.presign_ttl_secs
            ))
        }),
    }
}

/// The models of a valid file: its endpoints with their keys read, and the tasks that are on, each
/// with its prompt read and the timeout of its endpoint as the bound of a try (ADR 0035).
fn models_of(valid: &Validated) -> ModelsSettings {
    let c = &valid.config;
    let endpoints: BTreeMap<String, EndpointSettings> = c
        .models
        .endpoints
        .iter()
        .filter_map(|(name, e)| {
            // Invariant: the rules pass gave every endpoint of a valid file an address (written,
            // or read through its reference), so this never skips one. An endpoint with none
            // would be left out, and a task naming it is off (see below), never an empty URL.
            let base_url = valid.model_base_url(name)?;
            Some((
                name.clone(),
                EndpointSettings {
                    base_url: base_url.trim().trim_end_matches('/').to_owned(),
                    base_url_ref: match &e.base_url {
                        UrlRef::Ref(reference) => Some(reference.clone()),
                        UrlRef::Literal(_) => None,
                    },
                    api_key: valid
                        .secrets
                        .model_api_keys
                        .get(name)
                        .map(|key| SecretString::from(key.expose().to_owned())),
                    timeout: Duration::from_secs(e.timeout_secs),
                },
            ))
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

/// The tool servers of the valid file as the relay calls them (ADR 0024): where each is and the
/// credentials the file refers to, resolved, as the port's endpoint type, whose `Debug` shows no
/// value. Held by the configuration and read by nothing but the relay (feature `tool-relay`).
fn tool_endpoints_of(valid: &Validated) -> Vec<ToolServerEndpoint> {
    valid
        .config
        .tool_servers
        .iter()
        .map(|server| {
            let mut endpoint = ToolServerEndpoint::new(
                &server.id,
                server.url.trim(),
                Duration::from_secs(server.timeout_secs),
            );
            if let Some(secrets) = valid.secrets.tool_servers.get(&server.id) {
                if let Some(bearer) = &secrets.bearer {
                    endpoint = endpoint.with_bearer(ToolSecret::new(bearer.expose()));
                }
                for (name, value) in &secrets.headers {
                    endpoint = endpoint.with_header(name, ToolSecret::new(value.expose()));
                }
            }
            endpoint
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
    // An alias names an agent too (ADR 0049).
    let known: std::collections::BTreeSet<&str> = loaded
        .agents
        .iter()
        .map(|a| a.endpoint.id.as_str())
        .chain(loaded.agent_aliases.keys().map(AgentId::as_str))
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
    #[cfg(feature = "auth-jwt")]
    fn dpop_and_browser_sign_in_reach_the_configuration() {
        let text = format!(
            "{FILE}{JWT}  dpop:
    publicOrigins: [https://chat.example.com, 'http://localhost:3000']
    maxAgeSeconds: 30
    futureSkewSeconds: 2
  browser: {{ clientId: another-agentic-web }}
"
        );
        let c = load_file_only(&base(), &text).unwrap().config;
        let dpop = c.auth.dpop.unwrap();
        assert_eq!(
            dpop.public_origins,
            ["https://chat.example.com", "http://localhost:3000"]
        );
        assert_eq!(dpop.max_age, std::time::Duration::from_secs(30));
        assert_eq!(dpop.future_skew, std::time::Duration::from_secs(2));
        let browser = c.auth.browser.unwrap();
        assert_eq!(browser.client_id, "another-agentic-web");
        assert_eq!(browser.scope, "openid email profile offline_access");
        // Off by default.
        let plain = load_file_only(&base(), &format!("{FILE}{JWT}"))
            .unwrap()
            .config;
        assert!(plain.auth.dpop.is_none() && plain.auth.browser.is_none());
    }

    #[test]
    #[cfg(feature = "auth-jwt")]
    fn browser_sign_in_without_dpop_is_refused_through_the_file() {
        let text = format!("{FILE}{JWT}  browser: {{ clientId: web }}\n");
        let errors = lines(load_file_only(&base(), &text));
        assert!(
            errors
                .iter()
                .any(|l| l.starts_with("auth.browser: needs auth.dpop")),
            "{errors:?}"
        );
    }

    #[test]
    fn a_roles_max_tier_reaches_the_policy_and_a_wrong_one_is_a_rule_of_the_file() {
        use orch_core::AccessTier;
        let user = orch_core::UserId::new("a@example.com");
        let principal = |roles: &[&str]| orch_ports::Principal {
            roles: roles.iter().map(|r| orch_ports::Role::new(*r)).collect(),
            ..orch_ports::Principal::of(user.clone())
        };
        // The built-in roles name no cap: nobody is limited by default.
        let policy = load_file_only(&base(), FILE).unwrap().config.auth.policy;
        assert_eq!(policy.access(&principal(&["user"])).max_tier(), None);
        // A cap of the file is the role's, for the agents the role lets the person invoke: a person
        // with several roles has the highest of those that cover the agent, one of them without a
        // cap lifts it, and a role that cannot invoke the agent lifts nothing.
        let text = format!(
            "{FILE}auth:\n  roles:\n    reader: {{ permissions: [agent.invoke], maxTier: plan }}\n    \
             clerk: {{ permissions: [agent.invoke], agents: [coder], maxTier: auto }}\n    \
             free: {{ permissions: [agent.invoke], agents: [chat] }}\n    \
             looker: {{ permissions: [agent.read] }}\n"
        );
        let policy = load_file_only(&base(), &text).unwrap().config.auth.policy;
        let any = |roles: &[&str]| policy.access(&principal(roles)).max_tier();
        let on = |roles: &[&str], agent: &str| {
            policy
                .access(&principal(roles))
                .max_tier_for(&orch_core::AgentId::new(agent))
        };
        assert_eq!(any(&["reader"]), Some(AccessTier::Plan));
        assert_eq!(any(&["reader", "clerk"]), Some(AccessTier::Auto));
        assert_eq!(on(&["reader", "clerk"], "coder"), Some(AccessTier::Auto));
        assert_eq!(on(&["reader", "clerk"], "chat"), Some(AccessTier::Plan));
        assert_eq!(on(&["clerk", "free"], "coder"), Some(AccessTier::Auto));
        assert_eq!(
            on(&["clerk", "free"], "chat"),
            None,
            "free has no cap on chat"
        );
        assert_eq!(any(&["clerk", "free"]), None);
        assert_eq!(on(&["reader", "looker"], "coder"), Some(AccessTier::Plan));
        // A value that is not a tier stops the service with the file's key.
        let errors = lines(load_file_only(
            &base(),
            &format!(
                "{FILE}auth:\n  roles:\n    r: {{ permissions: [agent.invoke], maxTier: root }}\n"
            ),
        ));
        assert!(
            errors.iter().any(|l| l.starts_with("auth.roles.r.maxTier")),
            "{errors:?}"
        );
        // and so does a cap on a role that cannot invoke an agent: it would do nothing
        let errors = lines(load_file_only(
            &base(),
            &format!(
                "{FILE}auth:\n  roles:\n    r: {{ permissions: [thread.read], maxTier: plan }}\n"
            ),
        ));
        assert!(
            errors
                .iter()
                .any(|l| l
                    .starts_with("auth.roles.r.maxTier: only with a role that holds agent.invoke")),
            "{errors:?}"
        );
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
        // The administrator reaches no one else's thread, to read or to change (ADR 0039).
        assert!(policy.allows(&principal(&["admin"]), Permission::ThreadRead, &mine));
        assert!(!policy.allows(&principal(&["admin"]), Permission::ThreadRead, &theirs));
        assert!(!policy.allows(&principal(&["admin"]), Permission::ThreadWrite, &theirs));
        assert!(policy.allows(
            &principal(&["admin"]),
            Permission::Admin,
            &Resource::Anything
        ));
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
            "{FILE}auth:\n  roles:\n    reader: {{ permissions: [thread.read], scope: own }}\n    \
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
        assert_eq!(reader.scope(Permission::ThreadRead), Some(Scope::Own));
        let clerk = policy.access(&principal(&["clerk"]));
        assert_eq!(clerk.agents(Permission::AgentInvoke).patterns(), ["coder"]);
        assert!(!clerk.has(Permission::ThreadRead));
        // A default role of the file is the one a person with no known role gets.
        let text = format!("{text}  defaultRole: reader\n");
        let policy = load_file_only(&base(), &text).unwrap().config.auth.policy;
        assert!(policy.allows(&principal(&["wizard"]), Permission::ThreadRead, &mine));
        assert!(!policy.allows(&principal(&["wizard"]), Permission::ThreadRead, &theirs));
        // A role that asks for another person's threads is a rule of the file (ADR 0039).
        let errors = lines(load_file_only(
            &base(),
            &format!(
                "{FILE}auth:\n  roles:\n    spy: {{ permissions: [thread.read], scope: any }}\n"
            ),
        ));
        assert!(
            errors
                .iter()
                .any(|l| l.starts_with("auth.roles.spy.scope: any is refused")
                    && l.contains("ADR 0039")),
            "{errors:?}"
        );
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
    fn the_limits_on_asks_come_from_the_file_and_print_back() {
        // without the section: the owner's defaults
        let c = load_file_only(&base(), FILE).unwrap().config;
        assert_eq!((c.asks.depth, c.asks.per_job, c.asks.running), (2, 16, 4));
        assert_eq!(c.asks.timeout.as_secs(), 1800);
        // with it, each key moves its own limit, and `--print-config` says them
        let file = format!(
            "{FILE}asks: {{ maxDepth: 3, maxPerJob: 9, maxRunning: 2, timeoutSecs: 600 }}\n"
        );
        let loaded = load_file_only(&base(), &file).unwrap();
        let c = &loaded.config;
        assert_eq!((c.asks.depth, c.asks.per_job, c.asks.running), (3, 9, 2));
        assert_eq!(c.asks.timeout.as_secs(), 600);
        assert_eq!(c.app_config().asks, c.asks);
        let merged = loaded.merged.unwrap();
        for line in [
            "maxDepth: 3",
            "maxPerJob: 9",
            "maxRunning: 2",
            "timeoutSecs: 600",
        ] {
            assert!(merged.contains(line), "{line} in {merged}");
        }
        // a value out of its range is the file's error at its key
        let bad = format!("{FILE}asks: {{ maxDepth: 5 }}\n");
        let errors = lines(load_file_only(&base(), &bad));
        assert!(
            errors.iter().any(|l| l.starts_with("asks.maxDepth: ")),
            "{errors:?}"
        );
        // a variable moves the limit the file did not set
        let mut pairs = base();
        pairs.push(("ORCH_ASK_TIMEOUT_SECS", "120"));
        let c = load_file_only(&pairs, FILE).unwrap().config;
        assert_eq!(c.asks.timeout.as_secs(), 120);
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
ui: { showDescriptions: false, history: { initialTurns: 5, pageTurns: 8, windowed: false } }
server: { history: { maxTurns: 30, maxPageBytes: 1048576 } }
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
        // `ui.history` and `server.history` (ADR 0059) reach the application: the key of the
        // endpoint is the file's, with the build's projection; the bounds are the server's
        // the capability is the route's: a build without the AG-UI surface does not claim it
        #[cfg(feature = "surface-agui")]
        {
            let history = app.public.ui.history.expect("agui is mounted by default");
            assert_eq!(
                (history.initial_turns, history.page_turns, history.windowed),
                (5, 8, false)
            );
            assert_eq!(
                history.max_turns, 30,
                "what the server accepts, for the pages that grow"
            );
        }
        #[cfg(not(feature = "surface-agui"))]
        assert!(app.public.ui.history.is_none());
        assert_eq!(
            app.history,
            orch_app::HistorySettings {
                max_turns: 30,
                max_page_bytes: 1_048_576
            }
        );
        let endpoints = &loaded.config.models.endpoints;
        assert_eq!(endpoints["small"].base_url, "http://small:8080/v1");
        assert!(endpoints["small"].api_key.is_none());
        assert!(endpoints["default"].api_key.is_some());
        let merged = loaded.merged.unwrap();
        assert!(merged.contains("showDescriptions: false"), "{merged}");
        assert!(merged.contains("maxChars: 200"), "{merged}");
    }

    /// A model endpoint whose address a deployment keeps in a secret store (`{ env }` or
    /// `{ file }`) reaches the application as the address read, without a trailing `/`; what is
    /// shown of it, in a `Debug` and a log line, is the reference and never the address.
    #[test]
    fn a_base_url_read_through_a_reference_reaches_the_application_and_is_shown_as_the_reference() {
        let file = "\
version: 1
database: { url: { env: DATABASE_URL } }
agents: { file: agents.yaml }
models:
  endpoints:
    default: { baseUrl: 'http://written:8080/v1' }
    small: { baseUrl: { env: SMALL_MODEL_BASE_URL } }
tasks:
  title: { endpoint: small, model: small-model }
";
        let mut pairs = base();
        pairs.retain(|(k, _)| *k != "THREAD_TOOLS_SECRET");
        pairs.push(("SMALL_MODEL_BASE_URL", "http://private-gateway:8080/v1/\n"));
        let loaded = load_file_only(&pairs, file).unwrap();
        let endpoints = &loaded.config.models.endpoints;
        assert_eq!(
            endpoints["small"].base_url,
            "http://private-gateway:8080/v1"
        );
        assert_eq!(
            endpoints["small"].shown_url(),
            "{ env: SMALL_MODEL_BASE_URL }"
        );
        assert_eq!(endpoints["default"].base_url, "http://written:8080/v1");
        assert_eq!(endpoints["default"].shown_url(), "http://written:8080/v1");
        let shown = format!("{:?}", loaded.config.models);
        assert!(!shown.contains("private-gateway"), "{shown}");
        // the merged file, which `--print-config` prints, has the reference and not the address
        let merged = loaded.merged.unwrap();
        assert!(merged.contains("SMALL_MODEL_BASE_URL"), "{merged}");
        assert!(!merged.contains("private-gateway"), "{merged}");
        // unset: an error naming the key
        pairs.retain(|(k, _)| *k != "SMALL_MODEL_BASE_URL");
        let errors = lines(load_file_only(&pairs, file));
        assert!(
            errors.iter().any(|l| l.starts_with(
                "models.endpoints.small.baseUrl: the environment variable SMALL_MODEL_BASE_URL is unset or empty"
            )),
            "{errors:?}"
        );
    }

    const SHARING_SECRET: &str = "sharing-secret-sharing-secret-sharing-secret-0000";

    /// ADR 0040: a file with no `sharing` section shares nothing, and one that has it reaches the
    /// application and the API as the cap, the keys, what a public reader sees, the address links
    /// are shown under and the limits of the public routes; no secret is in any `Debug` or in the
    /// merged file.
    #[test]
    fn sharing_reaches_the_application_as_the_file_says_and_keeps_its_secret() {
        use orch_app::SharingMode;

        let loaded = load_file_only(&base(), FILE).unwrap();
        let app = loaded.config.app_config();
        assert_eq!(app.sharing.mode(), SharingMode::Disabled);
        assert!(app.sharing.keys().is_none());
        assert_eq!(
            loaded.config.public_limits,
            orch_api::PublicLimits::default()
        );

        let mut pairs = base();
        pairs.push(("SHARING_SECRET", SHARING_SECRET));
        let file = format!(
            "{FILE}sharing:\n  mode: public\n  secret: {{ env: SHARING_SECRET }}\n  public: {{ stepIo: true }}\n  rateLimit: {{ perLinkPerSecond: 3, streamsTotal: 7 }}\n"
        );
        let loaded = load_file_only(
            &pairs,
            &file.replace(
                "listen: 0.0.0.0:9000",
                "listen: 0.0.0.0:9000\n  publicUrl: https://chat.example.com/",
            ),
        )
        .unwrap();
        let app = loaded.config.app_config();
        assert_eq!(app.sharing.mode(), SharingMode::Public);
        assert!(app.sharing.keys().is_some());
        assert!(app.sharing.public().step_io && !app.sharing.public().files);
        assert_eq!(
            app.sharing.url_of("T"),
            "https://chat.example.com/s/T",
            "the address server.publicUrl gives"
        );
        assert_eq!(
            loaded.config.public_limits,
            orch_api::PublicLimits {
                per_link_per_second: 3,
                total_per_second: 100,
                streams_per_link: 5,
                streams_total: 7,
            }
        );
        let shown = format!("{:?} {:?} {:?}", loaded.config, app, loaded.notes);
        assert!(!shown.contains(SHARING_SECRET), "{shown}");
        let merged = loaded.merged.unwrap();
        assert!(merged.contains("env: SHARING_SECRET"), "{merged}");
        assert!(!merged.contains(SHARING_SECRET), "{merged}");

        // a refusal is a refusal of the file, naming the key
        let bad = format!("{FILE}sharing: {{ mode: internal }}\n");
        assert_eq!(
            lines(load_file_only(&base(), &bad)),
            ["sharing.secret: required unless sharing.mode is disabled (a link is made with it)"]
        );
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

    /// The relay's part of the servers: the URL, the timeout and the credentials resolved, as the
    /// port's endpoint type, in the order of the file, and in no `Debug`.
    #[test]
    fn the_tool_servers_reach_the_relay_as_endpoints_with_their_credentials() {
        let file =
            format!("{FILE}{TOOL_SERVERS}    headers:\n      X-Api-Key: {{ env: DOCS_KEY }}\n");
        let mut pairs = base();
        pairs.push(("SEARCH_TOKEN", "search-token-must-not-leak"));
        pairs.push(("DOCS_KEY", "docs-key-must-not-leak"));
        let loaded = load_file_only(&pairs, &file).unwrap();
        let endpoints = &loaded.config.tool_endpoints;
        assert_eq!(
            endpoints.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            ["websearch", "docs"]
        );
        let web = &endpoints[0];
        assert_eq!(web.url, "http://search.internal:8080/mcp");
        assert_eq!(web.timeout, Duration::from_secs(45));
        assert_eq!(
            web.bearer.as_ref().map(ToolSecret::expose),
            Some("search-token-must-not-leak")
        );
        assert!(web.headers.is_empty());
        let docs = &endpoints[1];
        assert_eq!(docs.url, "https://docs.example.com/mcp");
        assert_eq!(docs.timeout, Duration::from_secs(120));
        assert!(docs.bearer.is_none());
        assert_eq!(docs.headers.len(), 1);
        assert_eq!(docs.headers[0].0, "X-Api-Key");
        assert_eq!(docs.headers[0].1.expose(), "docs-key-must-not-leak");
        // an endpoint's own `Debug` hides every value, and the configuration's lists ids only
        let shown = format!("{:?} {:?}", loaded.config, endpoints);
        for hidden in ["search-token-must-not-leak", "docs-key-must-not-leak"] {
            assert!(!shown.contains(hidden), "{hidden} is in a Debug: {shown}");
        }
        assert!(!format!("{:?}", loaded.config).contains("search.internal"));
    }

    /// The default is no server, and the variables alone cannot name one.
    #[test]
    fn without_the_key_nothing_is_attachable() {
        let loaded = load_file_only(&base(), FILE).unwrap();
        assert!(loaded.config.app_config().tool_servers.is_empty());
        assert!(loaded.config.tool_endpoints.is_empty());
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
        // 55 variables have a key; the build without `agent-local` has no flag for one of them.
        assert_eq!(
            SETTINGS.len(),
            if cfg!(feature = "agent-local") {
                55
            } else {
                54
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

        /// Large files by a presigned upload (ADR 0058): `artifacts.upload` and the public endpoint
        /// reach the process, and the application is given the limits with the thread-tools keys the
        /// ids are signed with, so a replica that serves the endpoint reads an id a worker issued.
        #[cfg(all(feature = "artifacts-s3", feature = "surface-thread-tools"))]
        #[test]
        fn uploads_reach_the_application_with_the_thread_tools_keys() {
            let file = FILE.replace(
                "  listen: 0.0.0.0:9000\n",
                "  listen: 0.0.0.0:9000\n  surfaces: [agui, thread-tools]\n",
            );
            let section = "\
artifacts:
  store: s3
  maxPerJobBytes: 2147483648
  s3:
    bucket: orchestrator-files
    endpoint: http://release-rustfs:9000
    publicEndpoint: https://agentic-files.example.com
    accessKeyId: { env: S3_ACCESS_KEY_ID }
    secretAccessKey: { env: S3_SECRET_ACCESS_KEY }
  upload: { maxBytes: 1073741824, ttlSecs: 120 }
";
            let loaded = load_file_only(&s3_env(), &format!("{file}{section}")).unwrap();
            let artifacts = loaded.config.artifacts.as_ref().unwrap();
            assert_eq!(
                artifacts.upload,
                Some(UploadLimits {
                    max_bytes: 1 << 30,
                    ttl: Duration::from_secs(120)
                })
            );
            let StoreSettings::S3(s3) = &artifacts.store else {
                panic!("{artifacts:?}")
            };
            assert_eq!(
                s3.public_endpoint.as_deref(),
                Some("https://agentic-files.example.com")
            );
            assert_eq!(s3.endpoint.as_deref(), Some("http://release-rustfs:9000"));

            let app = loaded.config.app_config();
            let uploads = app.uploads.expect("uploads are on");
            assert_eq!(
                uploads.offer(),
                orch_core::UploadOffer {
                    max_bytes: 1 << 30,
                    ttl_secs: 120
                }
            );
            let issuer = &loaded.config.thread_tools.as_ref().unwrap().issuer;
            assert_eq!(
                uploads.keys().current_kid(),
                issuer.keys().current_kid(),
                "the ids are signed with the key of the thread-tools tokens"
            );

            // without the section the extension is off, whatever the store
            let off = section.replace("  upload: { maxBytes: 1073741824, ttlSecs: 120 }\n", "");
            let loaded = load_file_only(&s3_env(), &format!("{file}{off}")).unwrap();
            assert!(loaded.config.artifacts.as_ref().unwrap().upload.is_none());
            assert!(loaded.config.app_config().uploads.is_none());
        }

        /// Downloads by a redirect (ADR 0058, A1) reach the application as a mode and a lifetime; the
        /// default streams, and a public endpoint alone (which uploads need) redirects nothing.
        #[cfg(feature = "artifacts-s3")]
        #[test]
        fn downloads_reach_the_application_as_a_mode_and_stream_unless_asked() {
            let section = |extra: &str| {
                format!(
                    "\
artifacts:
  store: s3
  s3:
    bucket: orchestrator-files
    endpoint: http://release-rustfs:9000
    publicEndpoint: https://agentic-files.example.com
{extra}    accessKeyId: {{ env: S3_ACCESS_KEY_ID }}
    secretAccessKey: {{ env: S3_SECRET_ACCESS_KEY }}
"
                )
            };
            let app_of = |extra: &str| {
                load_file_only(&s3_env(), &with(&section(extra)))
                    .unwrap()
                    .config
                    .app_config()
            };
            // a public endpoint alone: the web's `fetch` cannot follow a redirect to a host its CSP lacks
            assert_eq!(app_of("").downloads, orch_app::DownloadMode::Stream);
            assert_eq!(
                app_of("    downloads: stream\n").downloads,
                orch_app::DownloadMode::Stream
            );
            // asked for: the default lifetime, then the owner's
            assert_eq!(
                app_of("    downloads: redirect\n").downloads,
                orch_app::DownloadMode::Redirect {
                    ttl: Duration::from_secs(300)
                }
            );
            assert_eq!(
                app_of("    downloads: redirect\n    presignTtlSecs: 90\n").downloads,
                orch_app::DownloadMode::Redirect {
                    ttl: Duration::from_secs(90)
                }
            );
            // a redirect with no public endpoint is the file's error, naming the key
            let no_public = section("    downloads: redirect\n").replace(
                "    publicEndpoint: https://agentic-files.example.com\n",
                "",
            );
            let errors = lines(load_file_only(&s3_env(), &with(&no_public)));
            assert!(
                errors.iter().any(|l| l.starts_with(
                    "artifacts.s3.downloads: redirect needs artifacts.s3.publicEndpoint"
                )),
                "{errors:?}"
            );
        }

        /// The file's rules and the application's mode both say what a lifetime may be, and they agree
        /// today. If they ever did not, a lifetime the rules let through but the mode refuses is a
        /// configuration error (exit 78), never a deployment that quietly streams what it was told to
        /// redirect.
        #[cfg(feature = "artifacts-s3")]
        #[test]
        fn a_lifetime_the_application_refuses_is_a_startup_error_and_never_a_quiet_stream() {
            let s3 = |mode: &str, secs: u64| -> orch_config::ArtifactsS3 {
                serde_json::from_value(serde_json::json!({
                    "bucket": "files",
                    "accessKeyId": { "env": "A" },
                    "secretAccessKey": { "env": "B" },
                    "downloads": mode,
                    "presignTtlSecs": secs,
                }))
                .unwrap()
            };
            for secs in [0, 1, 29, 3601, u64::MAX] {
                match super::super::download_mode(&s3("redirect", secs)) {
                    Err(ConfigError::Document(lines)) => {
                        assert_eq!(lines.len(), 1, "{lines:?}");
                        assert!(
                            lines[0].starts_with("artifacts.s3.presignTtlSecs: "),
                            "{lines:?}"
                        );
                    }
                    other => panic!("{secs}: expected a configuration error, got {other:?}"),
                }
            }
            for secs in [30, 300, 3600] {
                assert_eq!(
                    super::super::download_mode(&s3("redirect", secs)).unwrap(),
                    orch_app::DownloadMode::Redirect {
                        ttl: Duration::from_secs(secs)
                    }
                );
            }
            // a store that streams has no use for the lifetime
            assert_eq!(
                super::super::download_mode(&s3("stream", 300)).unwrap(),
                orch_app::DownloadMode::Stream
            );
        }

        /// The file's rules, not the process's: an upload needs everything it rides on.
        #[cfg(feature = "artifacts-s3")]
        #[test]
        fn an_upload_without_what_it_rides_on_is_78_naming_the_key() {
            let section = "\
artifacts:
  store: s3
  s3:
    bucket: orchestrator-files
    endpoint: http://release-rustfs:9000
    accessKeyId: { env: S3_ACCESS_KEY_ID }
    secretAccessKey: { env: S3_SECRET_ACCESS_KEY }
  upload: { maxBytes: 1048576 }
";
            let errors = lines(load_file_only(&s3_env(), &with(section)));
            assert!(
                errors
                    .iter()
                    .any(|l| l.starts_with("artifacts.upload: needs artifacts.s3.publicEndpoint")),
                "{errors:?}"
            );
            assert!(
                errors.iter().any(|l| l.starts_with(
                    "artifacts.upload: needs the thread-tools surface in server.surfaces"
                )),
                "{errors:?}"
            );
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

    /// The secret store (ADR 0046, slice 3): the service reads and checks the section in every role
    /// and does not use it yet; a build without the store's Cargo feature refuses it.
    mod user_secrets {
        use super::*;

        const K1: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

        const SECTION: &str = "\
secrets:
  store: pg
  pg:
    keys: [{ id: k1, key: { env: USER_SECRETS_KEY } }]
    current: k1
";

        fn with_key<'a>() -> Vec<(&'a str, &'a str)> {
            let mut pairs = base();
            pairs.push(("USER_SECRETS_KEY", K1));
            pairs
        }

        #[test]
        fn no_section_is_no_store_and_changes_nothing() {
            let loaded = load_file_only(&base(), FILE).unwrap();
            assert!(!loaded.merged.unwrap().contains("secrets:"));
        }

        #[cfg(feature = "secrets-pg")]
        #[test]
        fn a_section_is_read_and_checked_and_printed_as_references() {
            let loaded = load_file_only(&with_key(), &format!("{FILE}{SECTION}")).unwrap();
            let printed = loaded.merged.unwrap();
            assert!(
                printed.contains("secrets:") && printed.contains("USER_SECRETS_KEY"),
                "{printed}"
            );
            assert!(!printed.contains(K1), "{printed}");
            assert!(!format!("{:?}", loaded.config).contains(K1));
        }

        #[cfg(not(feature = "secrets-pg"))]
        #[test]
        fn a_build_without_the_store_refuses_it_naming_the_feature() {
            let errors = lines(load_file_only(&with_key(), &format!("{FILE}{SECTION}")));
            assert_eq!(
                errors,
                [
                    "secrets.store: pg needs the Cargo feature \"secrets-pg\", which this build does \
                     not have (ADR 0046)"
                ]
            );
        }

        #[test]
        fn a_key_that_is_not_a_key_is_refused_in_every_build_and_never_quoted() {
            let mut pairs = base();
            pairs.push(("USER_SECRETS_KEY", "not-a-key-0123456789abcdef"));
            let errors = lines(load_file_only(&pairs, &format!("{FILE}{SECTION}")));
            assert_eq!(errors.len(), 1, "{errors:?}");
            assert!(
                errors[0].starts_with(
                    "secrets.pg.keys[0].key: the secret is not 64 hexadecimal characters"
                ),
                "{errors:?}"
            );
            assert!(!errors[0].contains("not-a-key"));
        }
    }

    /// The database and the keys alone (`orchestrator secrets rewrap`, ADR 0046): the file whole in
    /// its syntax and shape, the secrets and the rules of the store only.
    #[cfg(feature = "secrets-pg")]
    mod the_keys_alone {
        use orch_secrets_pg::KeyId;

        use super::*;

        const K1: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        const K2: &str = "ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100";

        const SECTION: &str = "\
secrets:
  store: pg
  pg:
    keys:
      - { id: k1, key: { file: /run/secrets/orchestrator/k1 } }
      - { id: k2, key: { file: /run/secrets/orchestrator/k2 } }
    current: k2
";

        /// What the Job has: the database URL and the keys, and not the thread-tools key or a model key.
        fn only_the_keys(path: &Path) -> io::Result<String> {
            match path.to_str() {
                Some("/run/secrets/orchestrator/k1") => Ok(format!("{K1}\n")),
                Some("/run/secrets/orchestrator/k2") => Ok(format!("{K2}\n")),
                _ => Err(io::Error::from(io::ErrorKind::NotFound)),
            }
        }

        fn env(name: &str) -> Option<String> {
            (name == "DATABASE_URL").then(|| "postgres://u:hunter2@db/orch".to_owned())
        }

        fn load(text: &str) -> Result<UserSecretsOnly, ConfigError> {
            load_user_secrets_from(text, Path::new("/etc/orch"), env, only_the_keys)
        }

        #[test]
        fn the_keys_are_built_from_a_file_whose_other_secrets_cannot_be_read() {
            // `FILE` names a thread-tools key and a model key: neither is here
            let text = format!("{FILE}{SECTION}");
            assert!(
                load_file_only(&[("ORCH_CONFIG_FILE", "/etc/orch/config.yaml")], &text).is_err(),
                "the whole file cannot be loaded without them"
            );
            let loaded = load(&text).unwrap();
            let ring = &loaded.keyring;
            assert_eq!(ring.current().as_str(), "k2");
            assert_eq!(
                ring.ids().map(ToString::to_string).collect::<Vec<_>>(),
                ["k1", "k2"]
            );
            assert_eq!(loaded.log_format, LogFormat::Text);
            use secrecy::ExposeSecret as _;
            assert_eq!(
                loaded.database_url.expose_secret(),
                "postgres://u:hunter2@db/orch"
            );
            let shown = format!("{loaded:?}");
            assert!(
                !shown.contains("hunter2") && !shown.contains(&K1[..16]) && shown.contains("k2"),
                "{shown}"
            );
        }

        #[test]
        fn a_key_the_job_does_not_have_is_named_and_nothing_else_is() {
            let text = format!("{FILE}{SECTION}");
            let err = load_user_secrets_from(&text, Path::new("/etc/orch"), env, |_| {
                Err(io::Error::from(io::ErrorKind::NotFound))
            })
            .unwrap_err();
            let ConfigError::Document(lines) = err else {
                panic!("not a document error");
            };
            assert_eq!(lines.len(), 2, "{lines:?}");
            assert!(
                lines[0].starts_with("secrets.pg.keys[0].key: "),
                "{lines:?}"
            );
            assert!(
                lines[1].starts_with("secrets.pg.keys[1].key: "),
                "{lines:?}"
            );
        }

        #[test]
        fn the_database_is_named_when_it_is_not_given() {
            let text = format!("{FILE}{SECTION}");
            let err =
                load_user_secrets_from(&text, Path::new("/etc/orch"), |_| None, only_the_keys)
                    .unwrap_err();
            let ConfigError::Document(lines) = err else {
                panic!("not a document error");
            };
            assert_eq!(lines.len(), 1, "{lines:?}");
            assert!(lines[0].starts_with("database.url: "), "{lines:?}");
        }

        #[test]
        fn a_file_without_the_section_is_refused_by_name() {
            let ConfigError::Document(lines) = load(FILE).unwrap_err() else {
                panic!("not a document error");
            };
            assert_eq!(lines.len(), 1, "{lines:?}");
            assert!(lines[0].starts_with("secrets: required"), "{lines:?}");
        }

        /// The rule of a key id is written twice, in `orch-config` (what a configuration file is
        /// checked with) and in `orch-secrets-pg` (what builds the ring), because neither crate may
        /// depend on the other. This binary sees both, so here they are made to agree: an id one
        /// accepts and the other refuses would pass the check and fail at the keyring, or the other
        /// way round.
        #[test]
        fn the_configuration_and_the_store_agree_on_what_a_key_id_is() {
            let mut ids: Vec<String> = [
                "",
                "k1",
                "2026-10",
                "a.b_c-D",
                ".",
                "_",
                "-",
                "..",
                "k 1",
                " k1",
                "k1 ",
                "k/1",
                "k\\1",
                "clé",
                "ｋ1",
                "k１",
                "日本語",
                "k\u{200b}",
                "k\u{0301}",
                "ß",
                "k\n",
                "k\t",
                "k\0",
            ]
            .map(str::to_owned)
            .into();
            // 63, 64 and 65 characters; and 64 and 65 bytes of two-byte characters
            ids.extend(["x".repeat(63), "x".repeat(64), "x".repeat(65)]);
            ids.extend(["é".repeat(32), "é".repeat(33)]);
            // every ASCII character, alone and after a letter
            for byte in 0_u8..128 {
                let c = char::from(byte);
                ids.push(c.to_string());
                ids.push(format!("k{c}"));
            }
            let (mut accepted, mut refused) = (0, 0);
            for id in &ids {
                let quoted = serde_json::to_string(id).unwrap();
                let text = format!(
                    "{FILE}secrets:\n  store: pg\n  pg:\n    keys:\n      - {{ id: {quoted}, key: {{ file: /run/secrets/orchestrator/k1 }} }}\n    current: {quoted}\n"
                );
                // what the configuration says of the file, with nothing of the store's in the way
                let no_flags = FlagSecrets::new();
                let resolver = ProcessResolver {
                    env: &env,
                    read: &only_the_keys,
                    flags: &no_flags,
                };
                let configuration = orch_config::parse_yaml(&text)
                    .and_then(|tree| orch_config::check(&tree))
                    .and_then(|config| {
                        config.validate_user_secrets(Path::new("/etc/orch"), &resolver)
                    })
                    .is_ok();
                let store = KeyId::parse(id).is_ok();
                assert_eq!(
                    configuration, store,
                    "{id:?}: the configuration says {configuration}, orch-secrets-pg says {store}"
                );
                if store {
                    accepted += 1;
                } else {
                    refused += 1;
                }
            }
            // the sweep is not trivial in either direction
            assert!(
                accepted > 60 && refused > 100,
                "{accepted} accepted, {refused} refused"
            );
        }

        #[test]
        fn a_current_key_the_file_does_not_list_is_refused() {
            let text = format!("{FILE}{}", SECTION.replace("current: k2", "current: k9"));
            let ConfigError::Document(lines) = load(&text).unwrap_err() else {
                panic!("not a document error");
            };
            assert_eq!(
                lines,
                ["secrets.pg.current: names no key of secrets.pg.keys"]
            );
        }

        #[test]
        fn a_file_that_is_wrong_in_shape_is_refused_whole() {
            let text = format!("{FILE}{SECTION}bogus: 1\n");
            assert!(matches!(load(&text), Err(ConfigError::Document(_))));
            assert!(matches!(
                load("version: 1\n  - not: [yaml"),
                Err(ConfigError::Document(_))
            ));
        }

        #[test]
        fn no_file_is_a_configuration_error_that_says_how_to_give_one() {
            let ConfigError::Document(lines) = load_user_secrets(&Args::default()).unwrap_err()
            else {
                panic!("not a document error");
            };
            assert!(
                lines[0].contains("ORCH_CONFIG_FILE") && lines[0].contains("--config"),
                "{lines:?}"
            );
            let missing = Args {
                config: Some("/definitely/not/there.yaml".to_owned()),
                ..Args::default()
            };
            let ConfigError::Document(lines) = load_user_secrets(&missing).unwrap_err() else {
                panic!("not a document error");
            };
            assert!(
                lines[0].contains("cannot read the configuration file"),
                "{lines:?}"
            );
        }
    }

    /// The store alone (`orchestrator artifacts migrate`, ADR 0058 A5.4): the file whole in its syntax
    /// and shape, the secrets and the rules of the store only.
    #[cfg(all(feature = "artifacts-fs", feature = "artifacts-s3"))]
    mod the_store_alone {
        use super::*;
        use crate::artifacts::StoreSettings;

        const S3: &str = "\
artifacts:
  store: s3
  maxFileBytes: 2048
  s3:
    bucket: orchestrator-files
    region: eu-central-1
    endpoint: http://rustfs:9000
    publicEndpoint: https://files.example.com
    prefix: prod
    accessKeyId: { file: /run/secrets/orchestrator/id }
    secretAccessKey: { file: /run/secrets/orchestrator/key }
    timeoutSecs: 7
";

        /// What the Job has: the S3 credentials, and not the database, the thread-tools key or a token.
        fn only_the_s3_files(path: &Path) -> io::Result<String> {
            match path.to_str() {
                Some("/run/secrets/orchestrator/id") => Ok("AKIDEXAMPLE\n".to_owned()),
                Some("/run/secrets/orchestrator/key") => Ok("s3cr3t-access-key\n".to_owned()),
                _ => Err(io::Error::from(io::ErrorKind::NotFound)),
            }
        }

        fn load(text: &str) -> Result<StoreOnly, ConfigError> {
            load_store_from(text, Path::new("/etc/orch"), |_| None, only_the_s3_files)
        }

        #[test]
        fn the_store_is_built_from_a_file_whose_database_and_other_secrets_cannot_be_read() {
            // `FILE` names DATABASE_URL, THREAD_TOOLS_SECRET and a model key: none is here
            let text = format!("{FILE}{S3}");
            assert!(
                load_file_only(&[("ORCH_CONFIG_FILE", "/etc/orch/config.yaml")], &text).is_err(),
                "the whole file cannot be loaded without them"
            );
            let store = load(&text).unwrap();
            let StoreSettings::S3(s3) = &store.settings.store else {
                panic!("not an S3 store");
            };
            assert_eq!(s3.bucket, "orchestrator-files");
            assert_eq!(s3.region, "eu-central-1");
            assert_eq!(s3.endpoint.as_deref(), Some("http://rustfs:9000"));
            assert_eq!(s3.prefix.as_deref(), Some("prod"));
            assert_eq!(s3.timeout, Duration::from_secs(7));
            assert_eq!(
                s3.public_endpoint, None,
                "a migration signs nothing for anybody"
            );
            assert_eq!(store.settings.max_file_bytes, 2048);
            assert_eq!(store.log_format, LogFormat::Text);
            let shown = format!("{store:?}");
            assert!(
                !shown.contains("AKIDEXAMPLE") && !shown.contains("s3cr3t"),
                "{shown}"
            );
        }

        #[test]
        fn a_credential_the_job_does_not_have_is_named_and_nothing_else_is() {
            let text = format!("{FILE}{S3}");
            let err = load_store_from(
                &text,
                Path::new("/etc/orch"),
                |_| None,
                |_| Err(io::Error::from(io::ErrorKind::NotFound)),
            )
            .unwrap_err();
            let ConfigError::Document(lines) = err else {
                panic!("not a document error");
            };
            assert_eq!(lines.len(), 2, "{lines:?}");
            assert!(
                lines[0].starts_with("artifacts.s3.accessKeyId: "),
                "{lines:?}"
            );
            assert!(
                lines[1].starts_with("artifacts.s3.secretAccessKey: "),
                "{lines:?}"
            );
        }

        #[test]
        fn a_directory_store_is_not_a_target() {
            let text = format!("{FILE}artifacts: {{ store: fs, fs: {{ root: /var/lib/x }} }}\n");
            let ConfigError::Document(lines) = load(&text).unwrap_err() else {
                panic!("not a document error");
            };
            assert_eq!(lines.len(), 1, "{lines:?}");
            assert!(
                lines[0].starts_with("artifacts.store: ") && lines[0].contains("must be s3"),
                "{lines:?}"
            );
        }

        #[test]
        fn a_file_that_is_wrong_in_shape_is_refused_whole() {
            let text = format!("{FILE}{S3}bogus: 1\n");
            assert!(matches!(load(&text), Err(ConfigError::Document(_))));
            assert!(matches!(
                load("version: 1\n  - not: [yaml"),
                Err(ConfigError::Document(_))
            ));
        }

        #[test]
        fn no_file_is_a_configuration_error_that_says_how_to_give_one() {
            let ConfigError::Document(lines) = load_store(&Args::default()).unwrap_err() else {
                panic!("not a document error");
            };
            assert!(
                lines[0].contains("ORCH_CONFIG_FILE") && lines[0].contains("--config"),
                "{lines:?}"
            );
            let missing = Args {
                config: Some("/definitely/not/there.yaml".to_owned()),
                ..Args::default()
            };
            let ConfigError::Document(lines) = load_store(&missing).unwrap_err() else {
                panic!("not a document error");
            };
            assert!(
                lines[0].contains("cannot read the configuration file"),
                "{lines:?}"
            );
        }
    }

    /// `userToolServers` (ADR 0046, slice 4a) as the service reads it: off unless enabled, and then
    /// the egress policy, the OAuth client and the keys of `secrets` the composition root builds the
    /// ports from.
    mod user_tool_servers {
        use orch_ports::{EgressPolicy, EgressRefusal};

        use super::*;

        const UK1: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        const UK2: &str = "ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100";

        const SECRETS: &str = "\
secrets:
  store: pg
  pg:
    keys:
      - { id: k1, key: { env: UK1 } }
      - { id: k2, key: { env: UK2 } }
    current: k2
";

        /// The `secrets` section, in a build that has the store it names (one without
        /// `secrets-pg` refuses `store: pg` and has no use for it here).
        fn secrets() -> &'static str {
            if cfg!(feature = "secrets-pg") {
                SECRETS
            } else {
                ""
            }
        }

        fn pairs<'a>() -> Vec<(&'a str, &'a str)> {
            let mut pairs = base();
            pairs.extend([("UK1", UK1), ("UK2", UK2)]);
            pairs
        }

        fn load(section: &str) -> Result<Loaded, ConfigError> {
            load_file_only(&pairs(), &format!("{FILE}{section}"))
        }

        #[test]
        fn a_file_without_the_section_has_the_feature_off() {
            let loaded = load("").unwrap();
            assert!(loaded.config.user_tools.is_none());
            // and the keys of a `secrets` section are read and checked as before, and not used
            let loaded = load(secrets()).unwrap();
            assert!(loaded.config.user_tools.is_none());
        }

        #[test]
        fn a_section_that_is_not_enabled_builds_nothing() {
            let text = format!(
                "userToolServers:
  egress: {{ allow: [mcp.example.com], allowHttp: true }}
  oauth: {{ clientName: Acme }}
{}",
                secrets()
            );
            let loaded = load(&text).unwrap();
            assert!(loaded.config.user_tools.is_none());
            let text = format!("userToolServers: {{ enabled: false }}\n{}", secrets());
            assert!(load(&text).unwrap().config.user_tools.is_none());
        }

        #[test]
        fn enabled_without_secrets_keeps_no_credential() {
            let loaded = load("userToolServers: { enabled: true }\n").unwrap();
            let tools = loaded.config.user_tools.as_ref().unwrap();
            assert_eq!(tools.client_name, "another-agentic-system");
            assert_eq!(tools.client_metadata_url, None);
            assert!(!tools.allow_http);
            #[cfg(feature = "secrets-pg")]
            assert!(tools.keyring.is_none(), "no secrets section, no keys");
            // the default policy: https, public hosts
            assert!(
                tools
                    .egress
                    .check_url("https://mcp.example.com/mcp")
                    .is_ok()
            );
            assert!(
                tools
                    .egress
                    .check_url("http://mcp.example.com/mcp")
                    .is_err()
            );
            assert_eq!(tools.egress, EgressPolicy::new());
        }

        #[test]
        fn the_lists_and_the_switch_become_the_egress_policy() {
            let text = "userToolServers:
  enabled: true
  egress:
    allow: [mcp.example.com, '*.tools.example.org']
    deny: [bad.tools.example.org]
    allowHttp: true
  oauth:
    clientName: Acme Chat
    clientMetadataUrl: https://chat.example.com/.well-known/oauth-client.json
";
            let loaded = load(text).unwrap();
            let tools = loaded.config.user_tools.as_ref().unwrap();
            assert!(tools.allow_http);
            assert_eq!(tools.client_name, "Acme Chat");
            assert_eq!(
                tools.client_metadata_url.as_deref(),
                Some("https://chat.example.com/.well-known/oauth-client.json")
            );
            let egress = &tools.egress;
            // allowed, http taken
            assert!(egress.check_url("http://mcp.example.com/mcp").is_ok());
            assert!(egress.check_url("https://a.tools.example.org/mcp").is_ok());
            // denied wins, and the allow list refuses what it does not name
            assert_eq!(
                egress
                    .check_url("https://bad.tools.example.org/mcp")
                    .unwrap_err(),
                EgressRefusal::Denied
            );
            assert_eq!(
                egress
                    .check_url("https://other.example.net/mcp")
                    .unwrap_err(),
                EgressRefusal::NotAllowed
            );
        }

        #[test]
        #[cfg(feature = "secrets-pg")]
        fn enabled_with_secrets_reads_the_keys_into_a_ring() {
            let text = format!("userToolServers: {{ enabled: true }}\n{SECRETS}");
            let loaded = load(&text).unwrap();
            let ring = loaded
                .config
                .user_tools
                .as_ref()
                .unwrap()
                .keyring
                .as_ref()
                .unwrap();
            assert_eq!(ring.current().as_str(), "k2");
            assert_eq!(
                ring.ids().map(ToString::to_string).collect::<Vec<_>>(),
                ["k1", "k2"]
            );
            // nothing of a key is printed, by the settings or by the whole configuration
            let shown = format!("{:?}", loaded.config);
            assert!(shown.contains("k2"), "{shown}");
            for key in [UK1, UK2] {
                assert!(!shown.contains(&key[..16]), "{shown}");
            }
            assert!(!shown.contains("hunter2"), "{shown}");
        }

        #[test]
        #[cfg(feature = "secrets-pg")]
        fn a_key_that_is_not_a_key_is_named_whether_the_feature_is_on_or_not() {
            let bad = SECRETS.replace("env: UK1", "env: NOT_A_KEY");
            let pairs = [pairs(), vec![("NOT_A_KEY", "not-a-key")]].concat();
            for enabled in ["false", "true"] {
                let text = format!("{FILE}userToolServers: {{ enabled: {enabled} }}\n{bad}");
                let errors = lines(load_file_only(&pairs, &text));
                assert_eq!(errors.len(), 1, "{errors:?}");
                assert!(
                    errors[0].starts_with("secrets.pg.keys[0].key: "),
                    "{errors:?}"
                );
                assert!(!errors[0].contains("not-a-key"), "{errors:?}");
            }
        }

        #[test]
        fn a_deployment_server_with_the_id_of_a_persons_is_a_startup_error_only_when_enabled() {
            let servers = "toolServers:
  - { id: u-0123456789, name: Mine, url: 'https://mine.example.com/mcp' }
";
            let errors = lines(load(&format!(
                "userToolServers: {{ enabled: true }}\n{servers}"
            )));
            assert_eq!(errors.len(), 1, "{errors:?}");
            assert!(
                errors[0].starts_with("toolServers[0].id: starts with u-"),
                "{errors:?}"
            );
            assert!(load(&format!("userToolServers: {{ enabled: false }}\n{servers}")).is_ok());
            assert!(load(servers).is_ok());
        }

        /// `tools.user.add` (ADR 0046): no role holds it by default (the built-in `user` and `admin`
        /// do not, and `admin` does not imply it); a file that lists it for a role gives it to that
        /// role and no other, with the feature on or off.
        #[test]
        fn the_permission_to_add_a_server_reaches_the_policy_and_no_built_in_role_holds_it() {
            use orch_app::{Permission, Resource};
            let user = orch_core::UserId::new("a@example.com");
            let principal = |roles: &[&str]| orch_ports::Principal {
                roles: roles.iter().map(|r| orch_ports::Role::new(*r)).collect(),
                ..orch_ports::Principal::of(user.clone())
            };
            let can = |policy: &orch_app::Policy, roles: &[&str]| {
                policy.allows(
                    &principal(roles),
                    Permission::ToolsUserAdd,
                    &Resource::Anything,
                )
            };
            let built_in = load("").unwrap().config.auth.policy;
            assert!(!can(&built_in, &[]));
            assert!(!can(&built_in, &["user"]));
            assert!(!can(&built_in, &["admin"]), "admin does not imply it");

            for enabled in ["false", "true"] {
                let text = format!(
                    "userToolServers: {{ enabled: {enabled} }}
auth:
  defaultRole: plain
  roles:
    plain: {{ permissions: [agent.read, agent.invoke, thread.read, thread.write] }}
    maker: {{ permissions: [tools.user.add] }}
    root: {{ permissions: [admin] }}
"
                );
                let policy = load(&text).unwrap().config.auth.policy;
                assert!(can(&policy, &["maker"]), "enabled: {enabled}");
                assert!(!can(&policy, &["plain"]), "enabled: {enabled}");
                assert!(!can(&policy, &["root"]), "enabled: {enabled}");
                assert!(!can(&policy, &[]), "the default role is plain");
                assert!(can(&policy, &["plain", "maker"]), "roles are unioned");
            }
        }

        /// A host pattern is written twice, in `orch-config` (what a configuration file is checked
        /// with) and in `orch-ports` (what the egress policy is built from), because neither crate
        /// may depend on the other. This binary sees both, so here they are made to agree: an entry
        /// one accepts and the other refuses would pass the check and fail when the policy is
        /// built, or the other way round.
        #[test]
        fn the_configuration_and_the_egress_guard_agree_on_what_a_host_pattern_is() {
            let long_label = "a".repeat(64);
            let long_name = format!("{}com", "a.".repeat(130));
            // the limits (RFC 1035) and the first byte beyond each: a label of 63 and a name of 253
            // bytes are names, 64 and 254 are not
            let label = |n: usize| "a".repeat(n);
            let name = |last: usize| format!("{0}.{0}.{0}.{1}", label(63), label(last));
            let (label63, name253, name254) = (format!("{}.com", label(63)), name(61), name(62));
            let (name253_dotted, name254_dotted) = (format!("{name253}."), format!("{name254}."));
            let (wild253, wild254) = (format!("*.{name253}"), format!("*.{name254}"));
            let label64 = format!("{}.com", label(64));
            assert_eq!((name253.len(), name254.len()), (253, 254));
            let entries = [
                "mcp.example.com",
                "MCP.Example.COM",
                "mcp.example.com.",
                "localhost",
                "a_b.example.com",
                "-",
                "*.example.com",
                "*.internal",
                "*.xn--bcher-kva.example",
                "203.0.113.7",
                "0.0.0.0",
                "[2001:db8::7]",
                "2001:db8::7",
                "::1",
                "[::ffff:8.8.8.8]",
                "xn--bcher-kva.example",
                "münchen.example",
                "",
                " ",
                "mcp.example.com/path",
                "https://mcp.example.com",
                "mcp.example.com:443",
                "user@mcp.example.com",
                "*",
                "*.",
                "*.*.example.com",
                "*.203.0.113.7",
                "mcp.*.com",
                "mcp..example.com",
                ".example.com",
                "mcp example.com",
                "010.0.0.1",
                "127.1",
                "0x7f.1",
                "2130706433",
                "256.1.1.1",
                "[2001:db8::7%eth0]",
                "[1.2.3.4]",
                "[]",
                "%41.example.com",
                "a\u{2028}b.example.com",
                &long_label,
                &long_name,
                &label63,
                &label64,
                &name253,
                &name254,
                &name253_dotted,
                &name254_dotted,
                &wild253,
                &wild254,
            ];
            let within = [&label63, &name253, &name253_dotted, &wild253].map(String::as_str);
            let beyond = [&label64, &name254, &name254_dotted, &wild254].map(String::as_str);
            for entry in entries {
                let in_the_guard = EgressPolicy::from_lists([entry], Vec::<&str>::new()).is_ok();
                // a YAML double-quoted scalar: the entries have only a quote-free, backslash-free
                // alphabet apart from the line separator, which JSON escapes the YAML way
                let quoted = serde_json::to_string(entry).unwrap();
                let text = format!("userToolServers:\n  egress:\n    allow: [{quoted}]\n");
                let in_the_file = load(&text).is_ok();
                assert_eq!(
                    in_the_file, in_the_guard,
                    "{entry:?}: the file says {in_the_file}, the guard says {in_the_guard}"
                );
                // and the limits are where RFC 1035 puts them, not only where the two agree
                if within.contains(&entry) {
                    assert!(in_the_file, "{entry:?} is within the limits");
                }
                if beyond.contains(&entry) {
                    assert!(!in_the_file, "{entry:?} is beyond a limit");
                }
            }
        }
    }
}
