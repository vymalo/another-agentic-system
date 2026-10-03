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
use crate::tree::child;
use crate::types::{
    ArtifactStoreKind, Artifacts, Auth, AuthMode, AuthPermission, AuthScope, AuthScopes, Config,
    Environment, Prompt, SecretRef, SharingMode, Surface, ToolServer,
};

/// What `gate.maxAttempts` is when it is not set and the cap allows it (the core's default).
pub const DEFAULT_MAX_ATTEMPTS: u64 = 3;

/// The shortest HMAC key or webhook secret: 32 bytes, what `openssl rand -hex 32` gives.
pub const MIN_SECRET_BYTES: usize = 32;

/// The longest a task's prompt may be, 4 KiB: the guidance of a model call that costs a few tokens.
pub const MAX_PROMPT_BYTES: usize = 4 * 1024;

/// The most bytes of a tool server's icon (`toolServers[].icon`, a `data:` URI): 8 KiB.
pub const MAX_ICON_BYTES: usize = 8 * 1024;

/// The headers a tool server's `headers` may not set: the ones the orchestrator's own MCP client
/// owns (`Authorization` is `bearer`).
const RESERVED_HEADERS: &[&str] = &[
    "authorization",
    "accept",
    "content-type",
    "host",
    "mcp-session-id",
    "mcp-protocol-version",
    "last-event-id",
];

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
    /// `sharing.secret`.
    pub sharing_secret: Option<Secret>,
    /// `sharing.previousSecret`.
    pub sharing_previous_secret: Option<Secret>,
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
    /// The credentials of `toolServers`, by server id; a server that has none has no entry.
    pub tool_servers: BTreeMap<String, ToolServerSecrets>,
}

/// The credentials of one tool server, resolved. `Debug` shows references, never values.
#[derive(Debug, Clone, Default)]
pub struct ToolServerSecrets {
    /// `toolServers[].bearer`.
    pub bearer: Option<Secret>,
    /// `toolServers[].headers`, by header name as written in the file, in the order of the names.
    pub headers: Vec<(String, Secret)>,
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

        // sharing (ADR 0040): a key that does nothing is an error, as ADR 0034 asks
        let sharing = &cfg.sharing;
        if sharing.mode != SharingMode::Disabled && sharing.secret.is_none() {
            self.invalid(
                "sharing.secret",
                "required unless sharing.mode is disabled (a link is made with it)",
            );
        }
        if sharing.previous_secret.is_some() && sharing.secret.is_none() {
            self.invalid(
                "sharing.previousSecret",
                "needs sharing.secret: the previous secret only verifies",
            );
        }
        if sharing.mode != SharingMode::Public {
            for (present, key) in [
                (sharing.public.is_some(), "sharing.public"),
                (sharing.rate_limit.is_some(), "sharing.rateLimit"),
            ] {
                if present {
                    self.invalid(
                        key,
                        "only with sharing.mode public: it would silently do nothing",
                    );
                }
            }
        }

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

        self.tool_servers(&cfg.tool_servers);
        self.auth(cfg);
    }

    /// `toolServers` (ADR 0024): ids that tell the servers apart and make `<id>__<tool>` unambiguous,
    /// a URL with no credential in it, an icon that is a small `data:` image, header names the
    /// client can send, tool and agent lists that name something.
    fn tool_servers(&mut self, servers: &[ToolServer]) {
        let mut ids: Vec<&str> = Vec::new();
        for (i, server) in servers.iter().enumerate() {
            let at = format!("toolServers[{i}]");
            if !is_server_id(&server.id) {
                self.invalid(
                    format!("{at}.id"),
                    "an id is a to z, 0 to 9 and -, starting with a letter or a digit, 1 to 31 \
                     characters",
                );
            } else if ids.contains(&server.id.as_str()) {
                self.invalid(format!("{at}.id"), "another server has this id");
            }
            ids.push(&server.id);
            if server.name.trim().is_empty()
                || server.name.chars().count() > 80
                || server.name.chars().any(char::is_control)
            {
                self.invalid(
                    format!("{at}.name"),
                    "a name is 1 to 80 characters of one line, and not blank",
                );
            }
            if let Some(description) = &server.description
                && (description.chars().count() > 500 || description.chars().any(char::is_control))
            {
                self.invalid(
                    format!("{at}.description"),
                    "a description is at most 500 characters of one line",
                );
            }
            if !is_plain_http_url(&server.url)
                || Url::parse(&server.url)
                    .is_ok_and(|u| u.query().is_some() || u.fragment().is_some())
            {
                self.invalid(
                    format!("{at}.url"),
                    "expected an absolute http:// or https:// URL with a host and no user name, \
                     password, query or fragment: a credential goes in bearer or headers",
                );
            }
            if let Some(icon) = &server.icon
                && !is_icon(icon)
            {
                self.invalid(
                    format!("{at}.icon"),
                    format!(
                        "an icon is a data: URI, data:image/svg+xml;base64,... (or image/png, \
                         image/webp), at most {MAX_ICON_BYTES} bytes: an icon at a URL is never \
                         fetched"
                    ),
                );
            }
            if server.bearer.is_some()
                && server
                    .headers
                    .keys()
                    .any(|h| h.eq_ignore_ascii_case("authorization"))
            {
                self.invalid(
                    format!("{at}.headers"),
                    "Authorization is set by bearer, once",
                );
            }
            let mut names: Vec<String> = Vec::new();
            for name in server.headers.keys() {
                let header = format!("{at}.headers.{}", crate::tree::display_key(name));
                let lower = name.to_ascii_lowercase();
                if !is_header_name(name) {
                    self.invalid(&header, "not a header name: letters, digits and - only");
                } else if RESERVED_HEADERS.contains(&lower.as_str()) {
                    self.invalid(
                        &header,
                        if lower == "authorization" {
                            "Authorization is set by bearer".to_owned()
                        } else {
                            "this header is the orchestrator's own to set".to_owned()
                        },
                    );
                } else if names.contains(&lower) {
                    self.invalid(
                        &header,
                        "the same header is named twice (names do not differ by case)",
                    );
                }
                names.push(lower);
            }
            // (an empty list is a shape error: `minItems`)
            if let Some(tools) = &server.tools {
                for (j, tool) in tools.iter().enumerate() {
                    let tool_at = format!("{at}.tools[{j}]");
                    if !is_tool_name(tool) || server.id.len() + 2 + tool.len() > 64 {
                        self.invalid(
                            &tool_at,
                            "a tool the relay can expose: a to z, A to Z, 0 to 9, _ and -, not \
                             starting with _, and <id>__<tool> at most 64 characters",
                        );
                    } else if tools[..j].contains(tool) {
                        self.invalid(&tool_at, "a tool is listed more than once");
                    }
                }
            }
            if let Some(agents) = &server.agents {
                for (j, agent) in agents.iter().enumerate() {
                    let agent_at = format!("{at}.agents[{j}]");
                    if agent.trim().is_empty() || agent.trim() != agent {
                        self.invalid(
                            &agent_at,
                            "an agent id is not empty and has no space around it",
                        );
                    } else if agents[..j].contains(agent) {
                        self.invalid(&agent_at, "an agent is listed more than once");
                    }
                }
            }
        }
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
        self.roles(auth);
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

    /// `auth.roles` and `auth.defaultRole` (ADR 0033): every role can be told apart, nothing is set
    /// that its role ignores, and the default role is one of the roles.
    fn roles(&mut self, auth: &Auth) {
        use AuthPermission::{AgentInvoke, AgentRead, ArtifactRead, ThreadRead, ThreadWrite};
        let mut names: Vec<&str> = Vec::new();
        match &auth.roles {
            // The built-in roles.
            None => names.extend(["user", "admin"]),
            Some(roles) if roles.is_empty() => self.invalid(
                "auth.roles",
                "at least one role is needed (leave the key out for the built-in user and admin)",
            ),
            Some(roles) => {
                for (name, role) in roles {
                    let at = child("auth.roles", name);
                    // A role is compared exactly: a space around it is a role nobody has.
                    if name.trim().is_empty() || name.trim() != name {
                        self.invalid(&at, "a role name is not empty and has no space around it");
                    }
                    names.push(name);
                    for (i, permission) in role.permissions.iter().enumerate() {
                        if role.permissions[..i].contains(permission) {
                            self.invalid(
                                format!("{at}.permissions[{i}]"),
                                format!("{} is listed twice", permission.as_str()),
                            );
                        }
                    }
                    let holds = |wanted: &[AuthPermission]| {
                        role.permissions.iter().any(|p| wanted.contains(p))
                    };
                    if role.scope.is_some() && !holds(&[ThreadRead, ThreadWrite, ArtifactRead]) {
                        self.invalid(
                            format!("{at}.scope"),
                            "only with a role that holds thread.read, thread.write or \
                             artifact.read: it would silently do nothing",
                        );
                    }
                    if let Some(scopes) = &role.scope {
                        // ADR 0039: no role reaches another person's thread. The key stays for
                        // `version: 1` files, and what it could grant widely is refused by name.
                        let wide = match scopes {
                            AuthScopes::Both(scope) => {
                                vec![(format!("{at}.scope"), *scope)]
                            }
                            AuthScopes::Split(split) => vec![
                                (format!("{at}.scope.read"), split.read),
                                (format!("{at}.scope.write"), split.write),
                            ],
                        };
                        for (key, scope) in wide {
                            if scope == AuthScope::Any {
                                self.invalid(
                                    key,
                                    "any is refused: reading or acting on another person's \
                                     thread is not a permission (ADR 0039); share the thread \
                                     instead",
                                );
                            }
                        }
                    }
                    if let Some(agents) = &role.agents {
                        if !holds(&[AgentRead, AgentInvoke]) {
                            self.invalid(
                                format!("{at}.agents"),
                                "only with a role that holds agent.read or agent.invoke: it \
                                 would silently do nothing",
                            );
                        } else if agents.is_empty() {
                            self.invalid(
                                format!("{at}.agents"),
                                "at least one agent id, or \"*\" for every agent",
                            );
                        }
                        for (i, agent) in agents.iter().enumerate() {
                            if agent.trim().is_empty() {
                                self.invalid(
                                    format!("{at}.agents[{i}]"),
                                    "an agent id is not empty",
                                );
                            }
                        }
                    }
                }
            }
        }
        if let Some(Some(default)) = &auth.default_role
            && !names.contains(&default.as_str())
        {
            self.invalid(
                "auth.defaultRole",
                format!("not one of the roles ({})", names.join(", ")),
            );
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
        self.hosts("artifacts.fetchHosts", Some(&artifacts.fetch_hosts));
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
        // the secrets of a link (ADR 0040): as long as a key, and never the key of another purpose
        let sharing_secret = self.optional(&cfg.sharing.secret, "sharing.secret");
        let sharing_previous_secret =
            self.optional(&cfg.sharing.previous_secret, "sharing.previousSecret");
        for (secret, path) in [
            (&sharing_secret, "sharing.secret"),
            (&sharing_previous_secret, "sharing.previousSecret"),
        ] {
            if let Some(secret) = secret {
                self.at_least(secret, path);
            }
        }
        if let (Some(current), Some(previous)) = (&sharing_secret, &sharing_previous_secret)
            && current.expose() == previous.expose()
        {
            self.invalid("sharing.previousSecret", "is the same as sharing.secret");
        }
        for (secret, path) in [
            (&sharing_secret, "sharing.secret"),
            (&sharing_previous_secret, "sharing.previousSecret"),
        ] {
            let Some(secret) = secret else { continue };
            for (tools, name) in [
                (&thread_tools_secret, "threadTools.secret"),
                (&thread_tools_previous_secret, "threadTools.previousSecret"),
            ] {
                if tools
                    .as_ref()
                    .is_some_and(|t| t.expose() == secret.expose())
                {
                    self.invalid(
                        path,
                        format!(
                            "is the same as {name}: a link and a tool token are not made with one key"
                        ),
                    );
                }
            }
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
        let mut tool_servers = BTreeMap::new();
        for (i, server) in cfg.tool_servers.iter().enumerate() {
            let at = format!("toolServers[{i}]");
            let bearer = self.optional(&server.bearer, &format!("{at}.bearer"));
            let mut headers = Vec::new();
            for (name, reference) in &server.headers {
                let path = format!("{at}.headers.{}", crate::tree::display_key(name));
                if let Some(secret) = self.resolve(reference, &path, true) {
                    // A value the HTTP client cannot send (a newline in a file, a control
                    // character) is refused here, naming the key, never the value.
                    if is_header_value(secret.expose()) {
                        headers.push((name.clone(), secret));
                    } else {
                        self.invalid(&path, "the value is not one a header can hold");
                    }
                }
            }
            if let Some(bearer) = &bearer
                && !is_header_value(bearer.expose())
            {
                self.invalid(
                    format!("{at}.bearer"),
                    "the value is not one a header can hold",
                );
            }
            if bearer.is_some() || !headers.is_empty() {
                tool_servers.insert(server.id.clone(), ToolServerSecrets { bearer, headers });
            }
        }
        Some(Secrets {
            database_url: database_url?,
            registry_token,
            registry_agent_token,
            thread_tools_secret,
            thread_tools_previous_secret,
            sharing_secret,
            sharing_previous_secret,
            s3_access_key_id,
            s3_secret_access_key,
            model_api_keys,
            webhook_generic,
            webhook_github,
            tool_servers,
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

/// A tool server's id: `^[a-z0-9][a-z0-9-]{0,30}$`.
fn is_server_id(id: &str) -> bool {
    let mut bytes = id.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    id.len() <= 31
        && (first.is_ascii_lowercase() || first.is_ascii_digit())
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// An upstream tool name the relay can expose: `^[A-Za-z0-9_-]{1,64}$`, not starting with `_`.
fn is_tool_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && !name.starts_with('_')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// A header name a client can send: an HTTP token of letters, digits and `-`.
fn is_header_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// A header value: visible ASCII, space and tab, nothing that ends a line.
fn is_header_value(value: &str) -> bool {
    value
        .bytes()
        .all(|b| b == b'\t' || (0x20..0x7f).contains(&b))
}

/// An icon: `data:image/(svg+xml|png|webp);base64,<base64>`, at most [`MAX_ICON_BYTES`] bytes.
fn is_icon(icon: &str) -> bool {
    let Some(rest) = icon.strip_prefix("data:image/") else {
        return false;
    };
    let Some((kind, data)) = rest.split_once(";base64,") else {
        return false;
    };
    icon.len() <= MAX_ICON_BYTES
        && matches!(kind, "svg+xml" | "png" | "webp")
        && !data.is_empty()
        && data.len() % 4 == 0
        && data
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
        && data.trim_end_matches('=').len() + 2 >= data.len()
        && !data.trim_end_matches('=').contains('=')
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
