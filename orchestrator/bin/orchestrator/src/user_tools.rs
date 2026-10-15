//! People's own MCP servers in the composition root (ADR 0046, slice 4a, ADR 0009): the one place
//! that knows which adapters stand behind the two ports, and that chooses between them from the
//! configuration (`userToolServers` and `secrets`) and the role. The application sees only the ports.
//!
//! Three cases, one build (static dispatch over the variants, as [`ConfiguredArtifacts`] does):
//!
//! | `userToolServers.enabled` | `secrets` | definitions | broker | [`UserToolsMode`] |
//! |---|---|---|---|---|
//! | false or absent | any | refuse | refuse | `Off` |
//! | true | absent | Postgres | refuse | `WithoutCredentials` |
//! | true | `store: pg` | Postgres | `orch-credential-broker` over a sealed Postgres store | `Full` |
//!
//! The mode is not chosen here: it is what the constructor of the bundle was (`off`,
//! `without_credentials`, `full`), so it cannot disagree with the ports. The secret store is built
//! here and given to the broker, which holds the only handle: **the bundle has no secret store**, so
//! no handler can read secret material through it.
//!
//! **Which roles build it.** Only a process that serves routes: `all` and `control-plane`. People's
//! servers are used in two places, the API of slice 4b (routes of the control plane) and the relay of
//! `thread-tools/v1` (`RelayTools`, a provider of the thread-tools surface, mounted by
//! `control_plane_router`; an agent calls it over HTTP at `threadTools.url`). A `worker` process
//! serves neither, so it gets the refusing bundle (`Off`) and opens nothing for the feature: no pool,
//! no migration of the secret store, no HTTP client ([`settings_for`]).
//!
//! When the feature is off, or the role is a worker, **nothing is built and nothing is opened**: no
//! pool, no HTTP client, no migration of the secret store. The definitions' table is created by the
//! thread store's migrations like every other table, and is not read.
//!
//! The routes of slice 4b (`/api/user-tool-servers`, the listing `GET /api/tool-servers`) call them through `App`; the relay's lease is slice 5.
//!
//! [`ConfiguredArtifacts`]: crate::artifacts::ConfiguredArtifacts

use adam_host::Role;
use jiff::Timestamp;
use orch_core::UserId;
#[cfg(feature = "secrets-pg")]
use orch_credential_broker::Broker;
use orch_credential_broker::BrokerConfig;
use orch_ports::{
    CredentialBroker, CredentialError, CredentialLease, CredentialRef, EgressPolicy, NoBroker,
    NoPendingAuthorizations, NoUserToolServers, OAuthStart, OAuthState, PendingAuthorization,
    PendingAuthorizationError, PendingAuthorizations, RedirectUri, SecretError, ServerRevision,
    StoredServer, ToolSecret, UserToolPorts as _, UserToolServer, UserToolServerId,
    UserToolServerState, UserToolServerStore, UserToolServerStoreError, UserTools,
};
#[cfg(feature = "secrets-pg")]
use orch_store_postgres::PgPendingAuthorizations;
use orch_store_postgres::{PgStore, PgUserToolServers};

/// What `userToolServers` (and, when the file has it, `secrets`) says, in the terms of this
/// process: present only when the feature is **enabled**. Its `Debug` shows the lists of hosts and
/// the ids of the keys, never a key.
#[derive(Debug, Clone)]
pub struct UserToolsSettings {
    /// `userToolServers.egress`: the lists of hosts, and plain `http` outside production. The
    /// broker is composed with a clone of it and the bundle exposes it, so a URL the API accepts is
    /// a URL the broker may fetch.
    pub egress: EgressPolicy,
    /// `userToolServers.egress.allowHttp`: also in `egress`, kept to say so at startup.
    pub allow_http: bool,
    /// `userToolServers.oauth.clientName`: what a client registered dynamically calls itself.
    pub client_name: String,
    /// `userToolServers.oauth.clientMetadataUrl`: the URL of this orchestrator's Client ID Metadata
    /// Document, used as the `client_id` where an authorization server supports it. The control
    /// plane serves the document at the path of this URL while credentials are kept
    /// ([`client_metadata`](Self::client_metadata)).
    pub client_metadata_url: Option<String>,
    /// Where the browser comes back to after a person authorizes a server: `userToolServers.oauth
    /// .callbackUrl`, or `server.publicUrl` and the callback's path. `None` only when the file gives
    /// neither, which the file's rules refuse while credentials are kept.
    pub callback: Option<RedirectUri>,
    /// The keys of `secrets.pg`, with `secrets.pg.current` the one that seals. `None`: the file has
    /// no `secrets` section, and credentials cannot be kept.
    #[cfg(feature = "secrets-pg")]
    pub keyring: Option<orch_secrets_pg::Keyring>,
}

/// The redirect URI the broker is given, from the address the file names (`userToolServers.oauth
/// .callbackUrl`, or `server.publicUrl` and the callback's path). The file's rules have checked the
/// shape and the path; this is the second look, where the address becomes the `redirect_uri` of every
/// authorization and the only redirect of the Client ID Metadata Document: **its path is the one the
/// router serves the callback at** ([`orch_api::OAUTH_CALLBACK_PATH`]) or the process does not start,
/// because any other path ends every authorization at a 404.
///
/// # Errors
/// The words for the operator, naming the key and the path that is served.
fn callback_redirect(url: Option<String>) -> Result<Option<RedirectUri>, String> {
    let served = orch_api::OAUTH_CALLBACK_PATH;
    url.map(|url| {
        let redirect = RedirectUri::parse(&url).map_err(|e| {
            format!("userToolServers.oauth.callbackUrl (or server.publicUrl and {served}): {e}")
        })?;
        match url::Url::parse(redirect.as_str()) {
            Ok(parsed) if parsed.path() == served => Ok(redirect),
            _ => Err(format!(
                "userToolServers.oauth.callbackUrl: its path must be {served}, the only path the \
                 orchestrator serves the callback at"
            )),
        }
    })
    .transpose()
}

impl UserToolsSettings {
    /// The settings of a valid file, or `None` when `userToolServers` is absent or not enabled.
    ///
    /// The file's rules have checked the host patterns, the OAuth client and the keys, so an error
    /// here is not a path a valid file takes: it is refused, never ignored.
    ///
    /// # Errors
    /// The words for the operator, naming the key.
    pub fn from_file(valid: &orch_config::Validated) -> Result<Option<Self>, String> {
        let Some(section) = valid
            .config
            .user_tool_servers
            .as_ref()
            .filter(|section| section.enabled)
        else {
            return Ok(None);
        };
        let mut egress = EgressPolicy::from_lists(&section.egress.allow, &section.egress.deny)
            .map_err(|e| format!("userToolServers.egress: {e}"))?;
        if section.egress.allow_http {
            egress = egress.allowing_http();
        }
        let callback = callback_redirect(valid.config.user_tool_callback_url())?;
        #[cfg(feature = "secrets-pg")]
        let keyring = match valid.config.secrets.as_ref().and_then(|s| s.pg.as_ref()) {
            Some(pg) => {
                Some(keyring(pg, &valid.secrets.user_secret_keys).map_err(|e| e.to_string())?)
            }
            None => None,
        };
        Ok(Some(UserToolsSettings {
            egress,
            allow_http: section.egress.allow_http,
            client_name: section.oauth.client_name.clone(),
            client_metadata_url: section.oauth.client_metadata_url.clone(),
            callback,
            #[cfg(feature = "secrets-pg")]
            keyring,
        }))
    }

    /// The Client ID Metadata Document this orchestrator serves, when it serves one: the file names
    /// its URL, credentials are kept (there is a `secrets` section, so a broker that would use the
    /// document exists) and the callback it lists as its redirect URI is known. `None` otherwise,
    /// and then no route is mounted.
    ///
    /// # Errors
    /// The words for the operator, naming the key, for a document that cannot be made (the file's
    /// rules have checked the URL and the origin, so this is not a path a valid file takes).
    pub fn client_metadata(&self) -> Result<Option<orch_api::ClientMetadata>, String> {
        #[cfg(feature = "secrets-pg")]
        let keeps_credentials = self.keyring.is_some();
        #[cfg(not(feature = "secrets-pg"))]
        let keeps_credentials = false;
        let (true, Some(url), Some(callback)) = (
            keeps_credentials,
            self.client_metadata_url.as_deref(),
            self.callback.as_ref(),
        ) else {
            return Ok(None);
        };
        orch_api::ClientMetadata::new(url, &self.client_name, callback.as_str())
            .map(Some)
            .map_err(|e| format!("userToolServers.oauth.clientMetadataUrl: {e}"))
    }
}

/// Why the keys of `secrets.pg` do not make a ring: the key path that is wrong and what is wrong
/// with it. Quotes nothing of a key (`orch-secrets-pg`'s `KeyError` holds none).
#[cfg(feature = "secrets-pg")]
#[derive(Debug)]
pub struct KeyringError {
    /// `secrets.pg.keys[i].id`, `secrets.pg.keys[i].key` or `secrets.pg`.
    pub at: String,
    /// What is wrong.
    pub error: orch_secrets_pg::KeyError,
}

#[cfg(feature = "secrets-pg")]
impl std::fmt::Display for KeyringError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.at, self.error)
    }
}

/// The keyring of a valid `secrets.pg`: `keys` as the file's rules resolved them, `pg.current` the
/// one that seals. The rules have checked every id and every key; what `orch-secrets-pg` says here
/// is the same sentence, and it quotes nothing.
///
/// # Errors
/// [`KeyringError`] naming the key.
#[cfg(feature = "secrets-pg")]
pub fn keyring(
    pg: &orch_config::UserSecretsPg,
    keys: &[orch_config::UserSecretKeyValue],
) -> Result<orch_secrets_pg::Keyring, KeyringError> {
    use orch_secrets_pg::{KeyBytes, KeyId, Keyring};

    let at = |at: String| move |error| KeyringError { at, error };
    let mut ring = Vec::new();
    for (i, key) in keys.iter().enumerate() {
        let path = format!("secrets.pg.keys[{i}]");
        ring.push((
            KeyId::parse(&key.id).map_err(at(format!("{path}.id")))?,
            KeyBytes::from_hex(key.key.expose()).map_err(at(format!("{path}.key")))?,
        ));
    }
    let current = KeyId::parse(&pg.current).map_err(at("secrets.pg.current".to_owned()))?;
    Keyring::new(current, ring).map_err(at("secrets.pg".to_owned()))
}

// --------------------------------------------------------------------------------- the adapters

/// The definitions store of the binary: the sentence of [`NoUserToolServers`], or the table of the
/// orchestrator's own Postgres.
#[derive(Debug, Clone)]
pub enum ConfiguredServers {
    /// The feature is off: every call is refused.
    Off(NoUserToolServers),
    /// `user_tool_servers`, in the pool of the thread store.
    Pg(PgUserToolServers),
}

impl From<NoUserToolServers> for ConfiguredServers {
    fn from(store: NoUserToolServers) -> Self {
        ConfiguredServers::Off(store)
    }
}

impl UserToolServerStore for ConfiguredServers {
    async fn insert(
        &self,
        owner: &UserId,
        server: &UserToolServer,
    ) -> Result<ServerRevision, UserToolServerStoreError> {
        match self {
            ConfiguredServers::Off(store) => store.insert(owner, server).await,
            ConfiguredServers::Pg(store) => store.insert(owner, server).await,
        }
    }

    async fn get(
        &self,
        owner: &UserId,
        id: &UserToolServerId,
    ) -> Result<Option<StoredServer>, UserToolServerStoreError> {
        match self {
            ConfiguredServers::Off(store) => store.get(owner, id).await,
            ConfiguredServers::Pg(store) => store.get(owner, id).await,
        }
    }

    async fn list(&self, owner: &UserId) -> Result<Vec<StoredServer>, UserToolServerStoreError> {
        match self {
            ConfiguredServers::Off(store) => store.list(owner).await,
            ConfiguredServers::Pg(store) => store.list(owner).await,
        }
    }

    async fn replace(
        &self,
        owner: &UserId,
        server: &UserToolServer,
        expected: ServerRevision,
    ) -> Result<ServerRevision, UserToolServerStoreError> {
        match self {
            ConfiguredServers::Off(store) => store.replace(owner, server, expected).await,
            ConfiguredServers::Pg(store) => store.replace(owner, server, expected).await,
        }
    }

    async fn set_state(
        &self,
        owner: &UserId,
        id: &UserToolServerId,
        state: UserToolServerState,
        expected: ServerRevision,
    ) -> Result<ServerRevision, UserToolServerStoreError> {
        match self {
            ConfiguredServers::Off(store) => store.set_state(owner, id, state, expected).await,
            ConfiguredServers::Pg(store) => store.set_state(owner, id, state, expected).await,
        }
    }

    async fn delete(
        &self,
        owner: &UserId,
        id: &UserToolServerId,
    ) -> Result<(), UserToolServerStoreError> {
        match self {
            ConfiguredServers::Off(store) => store.delete(owner, id).await,
            ConfiguredServers::Pg(store) => store.delete(owner, id).await,
        }
    }
}

/// The broker of the binary: the sentence of [`NoBroker`], or `orch-credential-broker` over the
/// sealed table and the definitions.
#[derive(Debug, Clone)]
pub enum ConfiguredBroker {
    /// The feature is off, or there is no secret store: every call is refused.
    Off(NoBroker),
    /// The broker, over `secrets.store: pg` and the definitions.
    #[cfg(feature = "secrets-pg")]
    Full(Broker<orch_secrets_pg::PgSecretStore, PgUserToolServers>),
}

impl From<NoBroker> for ConfiguredBroker {
    fn from(broker: NoBroker) -> Self {
        ConfiguredBroker::Off(broker)
    }
}

#[cfg(feature = "secrets-pg")]
impl From<Broker<orch_secrets_pg::PgSecretStore, PgUserToolServers>> for ConfiguredBroker {
    fn from(broker: Broker<orch_secrets_pg::PgSecretStore, PgUserToolServers>) -> Self {
        ConfiguredBroker::Full(broker)
    }
}

impl CredentialBroker for ConfiguredBroker {
    async fn store(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
        material: &ToolSecret,
    ) -> Result<CredentialRef, CredentialError> {
        match self {
            ConfiguredBroker::Off(broker) => broker.store(owner, server, material).await,
            #[cfg(feature = "secrets-pg")]
            ConfiguredBroker::Full(broker) => broker.store(owner, server, material).await,
        }
    }

    async fn begin_oauth(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
        redirect: &RedirectUri,
    ) -> Result<OAuthStart, CredentialError> {
        match self {
            ConfiguredBroker::Off(broker) => broker.begin_oauth(owner, server, redirect).await,
            #[cfg(feature = "secrets-pg")]
            ConfiguredBroker::Full(broker) => broker.begin_oauth(owner, server, redirect).await,
        }
    }

    async fn complete_oauth(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
        state: &OAuthState,
        code: &ToolSecret,
        iss: Option<&str>,
    ) -> Result<CredentialRef, CredentialError> {
        match self {
            ConfiguredBroker::Off(broker) => {
                broker.complete_oauth(owner, server, state, code, iss).await
            }
            #[cfg(feature = "secrets-pg")]
            ConfiguredBroker::Full(broker) => {
                broker.complete_oauth(owner, server, state, code, iss).await
            }
        }
    }

    async fn lease(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
    ) -> Result<CredentialLease, CredentialError> {
        match self {
            ConfiguredBroker::Off(broker) => broker.lease(owner, server).await,
            #[cfg(feature = "secrets-pg")]
            ConfiguredBroker::Full(broker) => broker.lease(owner, server).await,
        }
    }

    async fn revoke(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
    ) -> Result<(), CredentialError> {
        match self {
            ConfiguredBroker::Off(broker) => broker.revoke(owner, server).await,
            #[cfg(feature = "secrets-pg")]
            ConfiguredBroker::Full(broker) => broker.revoke(owner, server).await,
        }
    }
}

/// The pending authorizations of the binary: the sentence of [`NoPendingAuthorizations`], or the
/// table of the orchestrator's own Postgres.
#[derive(Debug, Clone)]
pub enum ConfiguredPending {
    /// The feature is off, or no credentials are kept, so nothing can be authorized: every call is
    /// refused.
    Off(NoPendingAuthorizations),
    /// `user_tool_oauth_pending`, in the pool of the thread store. Built where the broker is, and
    /// only a build with `secrets-pg` has one.
    #[cfg(feature = "secrets-pg")]
    Pg(PgPendingAuthorizations),
}

impl From<NoPendingAuthorizations> for ConfiguredPending {
    fn from(store: NoPendingAuthorizations) -> Self {
        ConfiguredPending::Off(store)
    }
}

impl PendingAuthorizations for ConfiguredPending {
    async fn put(
        &self,
        state: &OAuthState,
        owner: &UserId,
        server: &UserToolServerId,
        expires_at: Timestamp,
        now: Timestamp,
    ) -> Result<(), PendingAuthorizationError> {
        match self {
            ConfiguredPending::Off(store) => store.put(state, owner, server, expires_at, now).await,
            #[cfg(feature = "secrets-pg")]
            ConfiguredPending::Pg(store) => store.put(state, owner, server, expires_at, now).await,
        }
    }

    async fn take(
        &self,
        state: &OAuthState,
        now: Timestamp,
    ) -> Result<Option<PendingAuthorization>, PendingAuthorizationError> {
        match self {
            ConfiguredPending::Off(store) => store.take(state, now).await,
            #[cfg(feature = "secrets-pg")]
            ConfiguredPending::Pg(store) => store.take(state, now).await,
        }
    }
}

/// Whether a failure of the secret store, at startup, is the database being down or too busy to
/// answer (exit 69, as the thread store's outage) and not a statement it refused (exit 1, as the
/// thread store's `Internal`). The port's [`SecretError::Unavailable`] is both, and its class
/// `Transient` says neither; `orch-secrets-pg` reads the driver's error it kept. A build without
/// that adapter has no store that could fail, and any `Unavailable` is taken for an outage.
pub fn is_outage(error: &SecretError) -> bool {
    #[cfg(feature = "secrets-pg")]
    {
        orch_secrets_pg::is_outage(error)
    }
    #[cfg(not(feature = "secrets-pg"))]
    {
        use orch_core::{Classify as _, ErrorClass};
        error.class() == ErrorClass::Transient
    }
}

/// The one `UserToolPorts` type of the binary's `PortSet`: three enums by static dispatch, and the mode
/// the constructor of the bundle fixed. It has no secret store (the broker holds the only handle).
pub type ConfiguredUserTools = UserTools<ConfiguredServers, ConfiguredBroker, ConfiguredPending>;

/// The sealed store the broker was built over, kept for a test to look at (a process drops it: the
/// broker holds its own handle, and the bundle gives out none).
#[cfg(feature = "secrets-pg")]
type Sealed = Option<orch_secrets_pg::PgSecretStore>;
#[cfg(not(feature = "secrets-pg"))]
type Sealed = ();

/// The settings this process builds the ports from: those of the file when it serves routes, and
/// none in a process that does not.
///
/// People's own servers are used by the API (slice 4b) and by the relay of `thread-tools/v1` (slice
/// 5), and both are routes of the control plane: `control_plane_router` mounts them, and an agent
/// that wants a tool calls the control plane over HTTP at `threadTools.url`. A `worker` role serves
/// only its probes, so it is given the refusing bundle and opens nothing for the feature.
pub fn settings_for(
    role: Role,
    settings: Option<&UserToolsSettings>,
) -> Option<&UserToolsSettings> {
    settings.filter(|_| role.runs_control_plane())
}

/// [`build`] for the role of this process: the ports of people's own servers where the process serves
/// routes ([`settings_for`]), the refusing bundle where it does not. A process that is given a file
/// that enables the feature and does not serve routes says so once, so an operator who looks for the
/// feature in a worker's log finds why it is not there.
///
/// # Errors
/// As [`build`].
pub async fn build_for_role(
    role: Role,
    settings: Option<&UserToolsSettings>,
    database_url: &str,
    store: &PgStore,
) -> anyhow::Result<ConfiguredUserTools> {
    let serving = settings_for(role, settings);
    if settings.is_some() && serving.is_none() {
        tracing::info!(
            role = %role,
            "userToolServers is enabled, and this process serves no routes: people's servers are \
             used by the control plane (the API and the thread-tools endpoint), so it builds nothing for them"
        );
    }
    build(serving, database_url, store).await
}

/// Builds the ports for `settings`: the feature off (`None`) is the refusing bundle and opens
/// nothing; on, the definitions are the table of the thread store's pool, the secret store is the
/// sealed table in a pool of its own (so a lock that is held in it can never keep a connection of the
/// thread store) when the file has the `secrets` section, and the broker is built over both with the
/// egress policy, the system clock and the OAuth client of the file. The secret store is given to the
/// broker and to nobody else.
///
/// # Errors
/// A configuration error (exit 78) for settings the broker refuses (a client metadata URL it cannot
/// use); the secret store's database being down (exit 69, the same outage as the thread store's), a
/// statement it refuses or its migrations not matching this build (exit 1).
pub async fn build(
    settings: Option<&UserToolsSettings>,
    database_url: &str,
    store: &PgStore,
) -> anyhow::Result<ConfiguredUserTools> {
    assemble(settings, database_url, store)
        .await
        .map(|(tools, _sealed)| tools)
}

/// [`build`], and the sealed store the broker holds, for the tests of this module.
async fn assemble(
    settings: Option<&UserToolsSettings>,
    database_url: &str,
    store: &PgStore,
) -> anyhow::Result<(ConfiguredUserTools, Sealed)> {
    let Some(settings) = settings else {
        return Ok((UserTools::off(), Sealed::default()));
    };
    let servers = PgUserToolServers::from_pool(store.pool().clone());
    let mut oauth = BrokerConfig::default().with_client_name(&settings.client_name);
    if let Some(url) = &settings.client_metadata_url {
        oauth = oauth.with_client_metadata_url(url);
    }

    #[cfg(feature = "secrets-pg")]
    if let Some(keyring) = &settings.keyring {
        use anyhow::Context as _;
        use orch_secrets_pg::PgSecretStore;

        let secrets = PgSecretStore::connect(database_url, keyring.clone())
            .await
            .context("cannot connect the secret store to Postgres (secrets)")?;
        // Idempotent and serialised by sqlx's advisory lock: every replica may run it at boot.
        secrets
            .migrate()
            .await
            .context("cannot apply the migrations of the secret store (secrets)")?;
        let broker = Broker::builder(secrets.clone(), servers.clone(), settings.egress.clone())
            .config(oauth)
            .build()
            .map_err(|e| crate::config::ConfigError::Invalid {
                var: "userToolServers",
                reason: e.to_string(),
            })?;
        tracing::info!(
            keys = ?keyring.ids().map(ToString::to_string).collect::<Vec<_>>(),
            current = %keyring.current(),
            "the credentials of people's own MCP servers are kept sealed in Postgres (secrets.store pg)"
        );
        let tools = UserTools::full(
            settings.egress.clone(),
            ConfiguredServers::Pg(servers),
            broker,
            ConfiguredPending::Pg(PgPendingAuthorizations::from_pool(store.pool().clone())),
        );
        announce(&tools, settings);
        return Ok((tools, Some(secrets)));
    }

    // no `secrets` section (a store this build lacks was refused when the file was read)
    let _ = (database_url, oauth);
    warn_about_stranded_credentials(&servers).await;
    let tools =
        UserTools::without_credentials(settings.egress.clone(), ConfiguredServers::Pg(servers));
    announce(&tools, settings);
    Ok((tools, Sealed::default()))
}

/// In a deployment that keeps no credentials, says once how many definitions already hold one: the
/// `secrets` section was taken away after they were stored (a key lost, a response to a compromise).
/// Those servers are unavailable until the section is back with its keys, and their owners can still
/// delete them. The count is all that is read and all that is said: no owner, no id, no URL.
///
/// A failure to count is a warning too and does not stop the process: the count is a courtesy to the
/// operator, and the database was reachable a moment ago, when the migrations ran.
async fn warn_about_stranded_credentials(servers: &PgUserToolServers) {
    match servers.count_with_credential().await {
        Ok(0) => {}
        Ok(count) => tracing::warn!(
            servers = count,
            "{count} of people's own MCP servers hold a credential, and this deployment keeps none \
             (userToolServers is enabled with no secrets section): they stay unavailable until the \
             secrets section is back with its keys, and their owners can still delete them"
        ),
        Err(error) => tracing::warn!(
            %error,
            "cannot count the people's MCP servers that hold a credential (userToolServers is enabled \
             with no secrets section)"
        ),
    }
}

/// Says once, at startup, what a deployment that turned the feature on has: never a URL, a key or
/// a credential.
fn announce(tools: &ConfiguredUserTools, settings: &UserToolsSettings) {
    tracing::info!(
        credentials_kept = tools.mode().keeps_credentials(),
        client_name = %settings.client_name,
        client_metadata_url = settings.client_metadata_url.is_some(),
        "people's own MCP servers are on (userToolServers.enabled)"
    );
    if settings.allow_http {
        tracing::warn!(
            "userToolServers.egress.allowHttp is on: a person's server may be at an http URL, and \
             its credential travels in the clear; use it in development only"
        );
    }
    if !tools.mode().keeps_credentials() {
        tracing::warn!(
            "userToolServers is enabled with no secrets section: only servers that need no \
             authentication can be added; a token, a key or an OAuth grant has nowhere to be kept"
        );
    }
}

#[cfg(test)]
mod tests;
