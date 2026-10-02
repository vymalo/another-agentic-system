//! Pass 3: the rules the schema cannot say, and the secrets resolved.
//!
//! [`Config::validate`] checks the cross-key rules (both or neither of `threadTools.url` and
//! `secret`; a surface that is mounted has what it needs), the URLs and hosts, and resolves every
//! secret reference, collecting every error. It reads nothing itself: the environment and the
//! files come through a [`Resolve`].
//!
//! What needs the agents file, the build's features or the gate rules of the application (a gate
//! that requires `ci` names a check, a verifier is another configured agent, a surface this build
//! compiled in) is not here: it needs more than the file, and the binary checks it.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use url::Url;

use crate::error::{ConfigError, ErrorKind};
use crate::secret::{MAX_SECRET_FILE_BYTES, Resolve, Secret};
use crate::types::{
    ArtifactStoreKind, Artifacts, AuthMode, Config, Environment, Prompt, SecretRef, Surface,
};

/// What `gate.maxAttempts` is when it is not set and the cap allows it (the core's default).
pub const DEFAULT_MAX_ATTEMPTS: u64 = 3;

/// The shortest HMAC key or webhook secret: 32 bytes, what `openssl rand -hex 32` gives.
pub const MIN_SECRET_BYTES: usize = 32;

/// The longest a task's prompt may be, 4 KiB: the guidance of a model call that costs a few tokens.
pub const MAX_PROMPT_BYTES: usize = 4 * 1024;

/// The secrets of a valid configuration, resolved. `Debug` shows references, never values.
#[derive(Debug, Clone)]
pub struct Secrets {
    /// `database.url`.
    pub database_url: Secret,
    /// `agents.registry.token`.
    pub registry_token: Option<Secret>,
    /// `agents.registry.agentToken`.
    pub registry_agent_token: Option<Secret>,
    /// `threadTools.secret`.
    pub thread_tools_secret: Option<Secret>,
    /// `threadTools.previousSecret`.
    pub thread_tools_previous_secret: Option<Secret>,
    /// `artifacts.s3.accessKeyId`; `None` unless `artifacts.store` is `s3`.
    pub s3_access_key_id: Option<Secret>,
    /// `artifacts.s3.secretAccessKey`; `None` unless `artifacts.store` is `s3`.
    pub s3_secret_access_key: Option<Secret>,
    /// `models.endpoints.<name>.apiKey`, by endpoint name.
    pub model_api_keys: BTreeMap<String, Secret>,
    /// `webhooks.generic.secrets`; empty when the section is absent or this process serves no
    /// routes (a worker is not asked for secrets it would never use).
    pub webhook_generic: Vec<Secret>,
    /// `webhooks.github.secrets`; empty as above.
    pub webhook_github: Vec<Secret>,
}

/// The prompts of the tasks (`tasks.<task>.system`), read: the text itself, whether it was written
/// inline or in a file. A task with no `system` has none here and runs on the core's guidance.
/// Not secrets: they are shown by `--print-config` as the reference the file has.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Prompts {
    /// `tasks.title.system`.
    pub title: Option<String>,
    /// `tasks.description.system`.
    pub description: Option<String>,
}

/// A configuration that passed the three passes: the keys (defaults filled in), the secrets and
/// the prompts, resolved.
#[derive(Debug, Clone)]
pub struct Validated {
    /// The keys.
    pub config: Config,
    /// The secrets.
    pub secrets: Secrets,
    /// The prompts of the tasks.
    pub prompts: Prompts,
    /// The directory the relative paths of the file are relative to.
    pub base_dir: PathBuf,
}

impl Validated {
    /// `agents.file`, resolved against the directory of the configuration file.
    pub fn agents_file(&self) -> Option<PathBuf> {
        self.config
            .agents
            .file
            .as_deref()
            .map(|p| join(&self.base_dir, p))
    }

    /// `mcp.tokensFile`, resolved against the directory of the configuration file.
    pub fn mcp_tokens_file(&self) -> Option<PathBuf> {
        self.config
            .mcp
            .tokens_file
            .as_deref()
            .map(|p| join(&self.base_dir, p))
    }
}

impl Validated {
    /// `artifacts.fs.root`, resolved against the directory of the configuration file; `None`
    /// unless `artifacts.store` is `fs`.
    pub fn artifacts_fs_root(&self) -> Option<PathBuf> {
        let artifacts = self.config.artifacts.as_ref()?;
        match artifacts.store {
            ArtifactStoreKind::Fs => artifacts
                .fs
                .as_ref()
                .map(|fs| join(&self.base_dir, &fs.root)),
            ArtifactStoreKind::S3 => None,
        }
    }
}

fn join(base: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_owned()
    } else {
        base.join(path)
    }
}

impl Config {
    /// The surfaces this configuration mounts: the list, or `[agui]` when none is given.
    pub fn mounted_surfaces(&self) -> Vec<Surface> {
        self.server
            .surfaces
            .clone()
            .unwrap_or_else(|| vec![Surface::Agui])
    }

    /// This configuration with the defaults that depend on other keys filled in, as
    /// `--print-config` shows it: the surfaces, and the attempts a gate allows.
    #[must_use]
    pub fn effective(&self) -> Config {
        let mut config = self.clone();
        config.server.surfaces = Some(self.mounted_surfaces());
        config.gate.max_attempts = Some(
            self.gate
                .max_attempts
                .unwrap_or_else(|| DEFAULT_MAX_ATTEMPTS.min(self.gate.max_attempts_cap)),
        );
        config
    }

    /// Pass 3: checks the rules between keys and resolves every secret through `resolver`.
    /// `base_dir` is the directory of the configuration file, which relative paths are relative
    /// to.
    ///
    /// # Errors
    ///
    /// Every error, sorted by key path. None carries a value.
    pub fn validate(
        self,
        base_dir: &Path,
        resolver: &dyn Resolve,
    ) -> Result<Validated, Vec<ConfigError>> {
        let mut checker = Checker {
            errors: Vec::new(),
            base_dir,
            resolver,
        };
        checker.rules(&self);
        let secrets = checker.secrets(&self);
        let prompts = checker.prompts(&self);
        let mut errors = checker.errors;
        match secrets {
            Some(secrets) if errors.is_empty() => Ok(Validated {
                config: self,
                secrets,
                prompts,
                base_dir: base_dir.to_owned(),
            }),
            _ => {
                errors.sort();
                errors.dedup();
                Err(errors)
            }
        }
    }
}

struct Checker<'a> {
    errors: Vec<ConfigError>,
    base_dir: &'a Path,
    resolver: &'a dyn Resolve,
}

impl Checker<'_> {
    fn invalid(&mut self, path: impl Into<String>, reason: impl Into<String>) {
        self.errors.push(ConfigError::invalid(path, reason));
    }

    fn rules(&mut self, cfg: &Config) {
        let surfaces = cfg.mounted_surfaces();
        let mounts = |s: Surface| surfaces.contains(&s);
        let serves = cfg.server.role.runs_control_plane();

        // server
        if cfg.server.listen.parse::<SocketAddr>().is_err() {
            self.invalid("server.listen", "not a socket address like 0.0.0.0:8080");
        }
        if let Some(listed) = &cfg.server.surfaces {
            for (i, surface) in listed.iter().enumerate() {
                if listed[..i].contains(surface) {
                    self.invalid(
                        format!("server.surfaces[{i}]"),
                        "a surface is listed more than once",
                    );
                }
            }
        }
        if let Some(url) = &cfg.server.public_url
            && !is_origin(url)
        {
            self.invalid(
                "server.publicUrl",
                "must be an origin such as https://chat.example.com: http or https, a host, no \
                 credentials, path, query or fragment",
            );
        }

        // agents
        let registry_url = cfg.agents.registry.as_ref().map(|r| r.url.as_str());
        if cfg.agents.file.is_none() && registry_url.is_none() {
            self.invalid(
                "agents.file",
                "required unless agents.registry.url is set (the agents have to come from somewhere)",
            );
        }
        if let Some(url) = registry_url
            && !is_plain_http_url(url)
        {
            self.invalid(
                "agents.registry.url",
                "expected an absolute http:// or https:// URL with a host and no user name or \
                 password, like https://platform.example.com/registry/v1/agents",
            );
        }

        // gate
        if let Some(attempts) = cfg.gate.max_attempts
            && attempts > cfg.gate.max_attempts_cap
        {
            self.invalid("gate.maxAttempts", "is above gate.maxAttemptsCap");
        }

        // models and tasks
        for (name, endpoint) in &cfg.models.endpoints {
            let at = format!("models.endpoints.{}", crate::tree::display_key(name));
            if !is_slug(name) {
                self.invalid(
                    &at,
                    "an endpoint name is a slug: a to z, 0 to 9 and -, 1 to 32 characters",
                );
            }
            if !is_http_url(&endpoint.base_url) {
                self.invalid(
                    format!("{at}.baseUrl"),
                    "expected an http:// or https:// URL, like https://api.example.com/v1",
                );
            }
        }
        // a task names an endpoint that exists: the model port answers `NotConfigured` for any
        // other, so this check is what makes that a bug and never a configuration outcome
        let tasks = [
            ("tasks.title", cfg.tasks.title.as_ref().map(|t| &t.endpoint)),
            (
                "tasks.description",
                cfg.tasks.description.as_ref().map(|t| &t.endpoint),
            ),
        ];
        for (path, endpoint) in tasks {
            if let Some(endpoint) = endpoint
                && !cfg.models.endpoints.contains_key(endpoint)
            {
                self.invalid(
                    format!("{path}.endpoint"),
                    "names no endpoint of models.endpoints",
                );
            }
        }

        // artifacts
        if let Some(artifacts) = &cfg.artifacts {
            self.artifacts(artifacts);
        }

        // threadTools
        let tools = &cfg.thread_tools;
        match (&tools.url, &tools.secret) {
            (Some(_), Some(_)) => {}
            (None, None) if mounts(Surface::ThreadTools) => {
                for key in ["url", "secret"] {
                    self.invalid(
                        format!("threadTools.{key}"),
                        "required when server.surfaces mounts thread-tools (in every role)",
                    );
                }
            }
            (None, None) => {}
            (Some(_), None) => self.invalid(
                "threadTools.secret",
                "required with threadTools.url: set both or neither",
            ),
            (None, Some(_)) => self.invalid(
                "threadTools.url",
                "required with threadTools.secret: set both or neither",
            ),
        }
        if tools.previous_secret.is_some() && tools.secret.is_none() {
            self.invalid(
                "threadTools.previousSecret",
                "needs threadTools.secret: the previous key only verifies",
            );
        }
        if let Some(url) = &tools.url
            && !is_base_url(url)
        {
            self.invalid(
                "threadTools.url",
                "expected an http:// or https:// URL with a host, without credentials, query or fragment",
            );
        }
        self.hosts("threadTools.allowedHosts", tools.allowed_hosts.as_deref());

        // mcp
        if mounts(Surface::Mcp) && serves {
            if cfg.mcp.tokens_file.is_none() {
                self.invalid("mcp.tokensFile", "required when server.surfaces mounts mcp");
            }
            if cfg.mcp.allowed_hosts.is_none() {
                self.invalid(
                    "mcp.allowedHosts",
                    "required when server.surfaces mounts mcp",
                );
            }
        }
        self.hosts("mcp.allowedHosts", cfg.mcp.allowed_hosts.as_deref());
        for (i, origin) in cfg.mcp.allowed_origins.iter().enumerate() {
            if !is_allowed_origin(origin) {
                self.invalid(
                    format!("mcp.allowedOrigins[{i}]"),
                    "not an origin: write https://host or https://host:port, with no path",
                );
            }
        }

        // webhooks
        if mounts(Surface::WebhookGeneric) && serves && cfg.webhooks.generic.is_none() {
            self.invalid(
                "webhooks.generic",
                "required when server.surfaces mounts webhook-generic",
            );
        }
        if mounts(Surface::WebhookGithub) && serves && cfg.webhooks.github.is_none() {
            self.invalid(
                "webhooks.github",
                "required when server.surfaces mounts webhook-github",
            );
        }

        self.auth(cfg);
    }

    /// `auth` (ADR 0033): the mode has what it needs, nothing is set that the mode ignores, and
    /// a production process does not trust the identity header.
    fn auth(&mut self, cfg: &Config) {
        let auth = &cfg.auth;
        if let Some(user) = &auth.dev_user {
            if !user.contains('@') {
                self.invalid("auth.devUser", "expected an e-mail address");
            }
            if auth.mode != AuthMode::ProxyHeader {
                self.invalid(
                    "auth.devUser",
                    "only with auth.mode proxy_header: a development identity beside token validation would let anyone in",
                );
            }
        }
        if cfg.server.environment == Environment::Production && auth.mode == AuthMode::ProxyHeader {
            self.invalid(
                "auth.mode",
                "proxy_header is for a single user on a local machine and is refused when \
                 server.environment is production: use jwt (or jwt_or_proxy_header for the \
                 one release of migration)",
            );
        }
        match (&auth.jwt, auth.mode.reads_tokens()) {
            (None, true) => self.invalid(
                "auth.jwt",
                format!("required when auth.mode is {}", auth.mode.as_str()),
            ),
            (Some(_), false) => self.invalid(
                "auth.jwt",
                "only with auth.mode jwt or jwt_or_proxy_header: it would silently do nothing",
            ),
            (None, false) => {}
            (Some(jwt), true) => {
                if !is_base_url(&jwt.issuer) {
                    self.invalid(
                        "auth.jwt.issuer",
                        "expected an http:// or https:// URL with a host, without credentials, \
                         query or fragment, like https://idp.example/realms/main",
                    );
                }
                for (i, audience) in jwt.audiences.iter().enumerate() {
                    if audience.trim().is_empty() {
                        self.invalid(
                            format!("auth.jwt.audiences[{i}]"),
                            "an audience is not empty",
                        );
                    }
                }
                if jwt.audiences.is_empty() {
                    self.invalid("auth.jwt.audiences", "at least one audience is needed");
                }
                if let Some(url) = &jwt.jwks_url
                    && !is_plain_http_url(url)
                {
                    self.invalid(
                        "auth.jwt.jwksUrl",
                        "expected an absolute http:// or https:// URL with a host and no user name \
                         or password",
                    );
                }
                // In production the keys that decide who is calling come over TLS only: anyone on
                // the path to a plain-http issuer could serve their own keys.
                if cfg.server.environment == Environment::Production {
                    let plain = |url: &str| url.trim().to_ascii_lowercase().starts_with("http://");
                    if plain(&jwt.issuer) {
                        self.invalid(
                            "auth.jwt.issuer",
                            "an https:// URL when server.environment is production: the issuer's \
                             keys decide who is calling",
                        );
                    }
                    if jwt.jwks_url.as_deref().is_some_and(plain) {
                        self.invalid(
                            "auth.jwt.jwksUrl",
                            "an https:// URL when server.environment is production: the issuer's \
                             keys decide who is calling",
                        );
                    }
                }
                if jwt.user_claim.trim().is_empty() {
                    self.invalid("auth.jwt.userClaim", "a claim name is not empty");
                }
                if jwt
                    .roles_claim
                    .as_deref()
                    .is_some_and(|c| c.trim().is_empty())
                {
                    self.invalid("auth.jwt.rolesClaim", "a claim name is not empty");
                }
            }
        }
    }

    /// The store `artifacts.store` names has its section, the other one has none, and what the
    /// section holds can be used.
    fn artifacts(&mut self, artifacts: &Artifacts) {
        let (wanted, other, other_name) = match artifacts.store {
            ArtifactStoreKind::Fs => (artifacts.fs.is_some(), artifacts.s3.is_some(), "s3"),
            ArtifactStoreKind::S3 => (artifacts.s3.is_some(), artifacts.fs.is_some(), "fs"),
        };
        let store = artifacts.store.as_str();
        if !wanted {
            self.invalid(
                format!("artifacts.{store}"),
                format!("required when artifacts.store is {store}"),
            );
        }
        if other {
            self.invalid(
                format!("artifacts.{other_name}"),
                format!("only with artifacts.store: {other_name}; this file's store is {store}"),
            );
        }
        if let Some(fs) = &artifacts.fs
            && fs.root.trim().is_empty()
        {
            self.invalid("artifacts.fs.root", "must name a directory");
        }
        if let Some(s3) = &artifacts.s3 {
            if !is_bucket_name(&s3.bucket) {
                self.invalid(
                    "artifacts.s3.bucket",
                    "a bucket name is 3 to 63 characters: lower case letters, digits, - and ., \
                     starting and ending with a letter or a digit",
                );
            }
            if !is_slug(&s3.region) {
                self.invalid(
                    "artifacts.s3.region",
                    "a region is a slug: a to z, 0 to 9 and -, 1 to 32 characters (us-east-1)",
                );
            }
            if let Some(endpoint) = &s3.endpoint
                && !is_base_url(endpoint)
            {
                self.invalid(
                    "artifacts.s3.endpoint",
                    "expected an http:// or https:// URL with a host, without credentials, query \
                     or fragment, like https://minio.example.com:9000",
                );
            }
            if let Some(prefix) = &s3.prefix
                && !is_key_prefix(prefix)
            {
                self.invalid(
                    "artifacts.s3.prefix",
                    "a prefix is made of a to z, A to Z, 0 to 9, ., _, - and /, has no .. part and \
                     is at most 128 characters",
                );
            }
        }
    }

    fn hosts(&mut self, path: &str, hosts: Option<&[String]>) {
        for (i, host) in hosts.unwrap_or_default().iter().enumerate() {
            if !is_authority(host) {
                self.invalid(
                    format!("{path}[{i}]"),
                    "not a host name or address, with or without a port (no scheme, path, \
                     wildcard or credentials): write orch.example.com or orch.example.com:8443",
                );
            }
        }
    }

    /// Resolves every secret the process uses. `None` when `database.url` cannot be.
    fn secrets(&mut self, cfg: &Config) -> Option<Secrets> {
        let surfaces = cfg.mounted_surfaces();
        let serves = cfg.server.role.runs_control_plane();
        let generic_mounted = serves && surfaces.contains(&Surface::WebhookGeneric);
        let github_mounted = serves && surfaces.contains(&Surface::WebhookGithub);

        let database_url = self.resolve(&cfg.database.url, "database.url", true);
        let (registry_token, registry_agent_token) = match &cfg.agents.registry {
            Some(registry) => (
                self.optional(&registry.token, "agents.registry.token"),
                self.optional(&registry.agent_token, "agents.registry.agentToken"),
            ),
            None => (None, None),
        };
        let thread_tools_secret = self.optional(&cfg.thread_tools.secret, "threadTools.secret");
        let thread_tools_previous_secret = self.optional(
            &cfg.thread_tools.previous_secret,
            "threadTools.previousSecret",
        );
        for (secret, path) in [
            (&thread_tools_secret, "threadTools.secret"),
            (&thread_tools_previous_secret, "threadTools.previousSecret"),
        ] {
            if let Some(secret) = secret {
                self.at_least(secret, path);
            }
        }
        if let (Some(current), Some(previous)) =
            (&thread_tools_secret, &thread_tools_previous_secret)
            && current.expose() == previous.expose()
        {
            self.invalid(
                "threadTools.previousSecret",
                "is the same as threadTools.secret",
            );
        }
        // the credentials of the S3 store; the section of another store is already an error
        let (s3_access_key_id, s3_secret_access_key) = match &cfg.artifacts {
            Some(artifacts) if artifacts.store == ArtifactStoreKind::S3 => match &artifacts.s3 {
                Some(s3) => (
                    self.resolve(&s3.access_key_id, "artifacts.s3.accessKeyId", true),
                    self.resolve(&s3.secret_access_key, "artifacts.s3.secretAccessKey", true),
                ),
                None => (None, None),
            },
            _ => (None, None),
        };
        let mut model_api_keys = BTreeMap::new();
        for (name, endpoint) in &cfg.models.endpoints {
            let path = format!("models.endpoints.{}.apiKey", crate::tree::display_key(name));
            if let Some(secret) = self.optional(&endpoint.api_key, &path) {
                model_api_keys.insert(name.clone(), secret);
            }
        }
        let webhook_generic = match &cfg.webhooks.generic {
            Some(section) if serves => self.list(
                &section.secrets,
                "webhooks.generic.secrets",
                generic_mounted,
            ),
            _ => Vec::new(),
        };
        let webhook_github = match &cfg.webhooks.github {
            Some(section) if serves => {
                self.list(&section.secrets, "webhooks.github.secrets", github_mounted)
            }
            _ => Vec::new(),
        };
        Some(Secrets {
            database_url: database_url?,
            registry_token,
            registry_agent_token,
            thread_tools_secret,
            thread_tools_previous_secret,
            s3_access_key_id,
            s3_secret_access_key,
            model_api_keys,
            webhook_generic,
            webhook_github,
        })
    }

    fn optional(&mut self, reference: &Option<SecretRef>, path: &str) -> Option<Secret> {
        reference.as_ref().and_then(|r| self.resolve(r, path, true))
    }

    /// The secrets of a webhook. A reference that cannot be resolved is an error only when the
    /// route is mounted; one that can be is checked either way.
    fn list(&mut self, references: &[SecretRef], path: &str, mounted: bool) -> Vec<Secret> {
        let mut secrets = Vec::new();
        for (i, reference) in references.iter().enumerate() {
            let at = format!("{path}[{i}]");
            if let Some(secret) = self.resolve(reference, &at, mounted) {
                self.at_least(&secret, &at);
                secrets.push(secret);
            }
        }
        secrets
    }

    fn at_least(&mut self, secret: &Secret, path: &str) {
        if secret.expose().len() < MIN_SECRET_BYTES {
            self.invalid(
                path,
                format!(
                    "the secret is shorter than {MIN_SECRET_BYTES} bytes (generate one with \
                     `openssl rand -hex 32`)"
                ),
            );
        }
    }

    /// Reads the prompts of the tasks: inline text as it is, a file through the resolver. Each is
    /// UTF-8 and, with the space around it cut, 1 to [`MAX_PROMPT_BYTES`] bytes. An error names
    /// the key and, for a file, its path; never the text.
    fn prompts(&mut self, cfg: &Config) -> Prompts {
        let title = cfg.tasks.title.as_ref().and_then(|t| t.system.as_ref());
        let description = cfg
            .tasks
            .description
            .as_ref()
            .and_then(|t| t.system.as_ref());
        Prompts {
            title: title.and_then(|p| self.prompt(p, "tasks.title.system")),
            description: description.and_then(|p| self.prompt(p, "tasks.description.system")),
        }
    }

    fn prompt(&mut self, prompt: &Prompt, path: &str) -> Option<String> {
        let (text, from) = match prompt {
            Prompt::Inline(text) => (text.clone(), "the prompt".to_owned()),
            Prompt::File(file) => {
                if file.trim().is_empty() {
                    self.invalid(path, "`file` must name a file");
                    return None;
                }
                match self.resolver.read_file(&join(self.base_dir, file)) {
                    Ok(text) => (text, format!("the file {file}")),
                    Err(_) => {
                        self.invalid(path, format!("the file {file} cannot be read"));
                        return None;
                    }
                }
            }
        };
        let text = text.trim();
        if text.is_empty() {
            self.invalid(path, format!("{from} is empty"));
            return None;
        }
        if text.len() > MAX_PROMPT_BYTES {
            self.invalid(path, format!("{from} is larger than 4 KiB"));
            return None;
        }
        Some(text.to_owned())
    }

    /// Reads one reference. Whatever is wrong is an error naming the key and the variable or
    /// the path, when `required`; never a value.
    fn resolve(&mut self, reference: &SecretRef, path: &str, required: bool) -> Option<Secret> {
        let outcome = match reference {
            SecretRef::Env(name) => self.env_value(name),
            SecretRef::File(file) => self.file_value(file),
        };
        match outcome {
            Ok(value) => Some(Secret::new(reference.clone(), value)),
            Err(message) => {
                if required {
                    self.errors
                        .push(ConfigError::new(path, ErrorKind::Unresolved(message)));
                }
                None
            }
        }
    }

    fn env_value(&self, name: &str) -> Result<String, String> {
        if name.trim().is_empty() || name.contains(['=', '\0']) {
            return Err("`env` must name an environment variable".to_owned());
        }
        match self.resolver.env(name).map(|v| v.trim().to_owned()) {
            Some(value) if !value.is_empty() => Ok(value),
            _ => Err(format!("the environment variable {name} is unset or empty")),
        }
    }

    fn file_value(&self, file: &str) -> Result<String, String> {
        if file.trim().is_empty() {
            return Err("`file` must name a file".to_owned());
        }
        let text = self
            .resolver
            .read_file(&join(self.base_dir, file))
            .map_err(|_| format!("the file {file} cannot be read"))?;
        if text.len() > MAX_SECRET_FILE_BYTES {
            return Err(format!("the file {file} is larger than 64 KiB"));
        }
        let text = text
            .strip_suffix("\r\n")
            .or_else(|| text.strip_suffix('\n'))
            .unwrap_or(&text);
        if text.is_empty() {
            return Err(format!("the file {file} is empty"));
        }
        Ok(text.to_owned())
    }
}

fn is_slug(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// An S3 bucket name as the services take it: 3 to 63 characters of `a-z`, `0-9`, `-` and `.`,
/// starting and ending with a letter or a digit.
fn is_bucket_name(name: &str) -> bool {
    let edge = |b: Option<&u8>| b.is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    (3..=63).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
        && edge(name.as_bytes().first())
        && edge(name.as_bytes().last())
}

/// A key prefix: letters, digits, `.`, `_`, `-` and `/`, something besides `/`, no `..` part, at
/// most 128 characters.
fn is_key_prefix(prefix: &str) -> bool {
    prefix.bytes().any(|b| b != b'/')
        && prefix.len() <= 128
        && prefix
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/'))
        && !prefix.split('/').any(|part| part == "..")
}

/// `http` or `https` with a host.
fn is_http_url(raw: &str) -> bool {
    Url::parse(raw).is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.has_host())
}

/// `http` or `https` with a host, and no user name or password.
fn is_plain_http_url(raw: &str) -> bool {
    Url::parse(raw).is_ok_and(|u| {
        matches!(u.scheme(), "http" | "https")
            && u.has_host()
            && u.username().is_empty()
            && u.password().is_none()
    })
}

/// An origin: `http(s)://host[:port]`, no credentials, path, query or fragment.
fn is_origin(raw: &str) -> bool {
    Url::parse(raw).is_ok_and(|u| {
        matches!(u.scheme(), "http" | "https")
            && u.has_host()
            && u.username().is_empty()
            && u.password().is_none()
            && u.path() == "/"
            && u.query().is_none()
            && u.fragment().is_none()
    })
}

/// An entry of `mcp.allowedOrigins`: an origin, written without a trailing slash.
fn is_allowed_origin(raw: &str) -> bool {
    is_origin(raw) && !raw.ends_with('/')
}

/// The base URL of the thread tools: an http(s) URL with a host, optionally a port and a path
/// prefix, and no credentials, query or fragment.
fn is_base_url(raw: &str) -> bool {
    Url::parse(raw.trim()).is_ok_and(|u| {
        matches!(u.scheme(), "http" | "https")
            && u.has_host()
            && u.username().is_empty()
            && u.password().is_none()
            && u.query().is_none()
            && u.fragment().is_none()
    })
}

/// A `Host` header value: a name or an address, with or without a port. The binary checks it
/// again with the HTTP library's own parser; this is the part that does not need one.
fn is_authority(host: &str) -> bool {
    if host.is_empty() || host.contains(['@', '*', '/', '?', '#', ' ', '\\']) {
        return false;
    }
    let (name, port) = match host.rsplit_once(':') {
        // An IPv6 literal has colons inside brackets; its port, if any, follows the bracket.
        Some((name, port)) if !port.contains(']') => (name, Some(port)),
        _ => (host, None),
    };
    if name.is_empty() || name.contains("://") {
        return false;
    }
    match port {
        None => true,
        Some(port) => !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()),
    }
}
