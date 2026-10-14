//! People's own MCP servers in the composition root (ADR 0046, slice 4a, ADR 0009): the one place
//! that knows which adapters stand behind the three ports, and that chooses between them from the
//! configuration (`userToolServers` and `secrets`). The application sees only the ports.
//!
//! Three cases, one build (static dispatch over the variants, as [`ConfiguredArtifacts`] does):
//!
//! | `userToolServers.enabled` | `secrets` | definitions | secret store | broker | [`UserToolsMode`] |
//! |---|---|---|---|---|---|
//! | false or absent | any | refuse | refuse | refuse | `Off` |
//! | true | absent | Postgres | refuse | refuse | `WithoutCredentials` |
//! | true | `store: pg` | Postgres | sealed Postgres | `orch-credential-broker` over both | `Full` |
//!
//! When the feature is off **nothing is built and nothing is opened**: no pool, no HTTP client, no
//! migration of the secret store. The definitions' table is created by the thread store's migrations
//! like every other table, and is not read.
//!
//! Nothing uses the ports yet (no route, no relay): they are built and handed to the bundle.
//!
//! [`ConfiguredArtifacts`]: crate::artifacts::ConfiguredArtifacts

use orch_core::UserId;
#[cfg(feature = "secrets-pg")]
use orch_credential_broker::Broker;
use orch_credential_broker::BrokerConfig;
use orch_ports::{
    CredentialBroker, CredentialError, CredentialLease, CredentialRef, EgressPolicy, NoBroker,
    NoSecrets, NoUserToolServers, OAuthStart, OAuthState, RedirectUri, SecretError, SecretMaterial,
    SecretServer, SecretStore, SecretVersion, ServerRevision, StoredServer, ToolSecret,
    UserToolPorts as _, UserToolServer, UserToolServerId, UserToolServerState, UserToolServerStore,
    UserToolServerStoreError, UserTools, UserToolsMode,
};
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
    /// Document, used as the `client_id` where an authorization server supports it. The route that
    /// serves it is a later slice.
    pub client_metadata_url: Option<String>,
    /// The keys of `secrets.pg`, with `secrets.pg.current` the one that seals. `None`: the file has
    /// no `secrets` section, and credentials cannot be kept.
    #[cfg(feature = "secrets-pg")]
    pub keyring: Option<orch_secrets_pg::Keyring>,
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
            #[cfg(feature = "secrets-pg")]
            keyring,
        }))
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

/// The secret store of the binary: the sentence of [`NoSecrets`], or the sealed table of the
/// orchestrator's own Postgres (`secrets.store: pg`, the feature `secrets-pg`).
#[derive(Debug, Clone)]
pub enum ConfiguredSecrets {
    /// No `secrets` section, or the feature is off: every call is refused.
    Off(NoSecrets),
    /// `user_secrets`, sealed, in a pool of its own.
    #[cfg(feature = "secrets-pg")]
    Pg(orch_secrets_pg::PgSecretStore),
}

impl SecretStore for ConfiguredSecrets {
    async fn put(
        &self,
        owner: &UserId,
        server: &SecretServer,
        material: &SecretMaterial,
    ) -> Result<SecretVersion, SecretError> {
        match self {
            ConfiguredSecrets::Off(store) => store.put(owner, server, material).await,
            #[cfg(feature = "secrets-pg")]
            ConfiguredSecrets::Pg(store) => store.put(owner, server, material).await,
        }
    }

    async fn get(
        &self,
        owner: &UserId,
        server: &SecretServer,
    ) -> Result<Option<(SecretVersion, SecretMaterial)>, SecretError> {
        match self {
            ConfiguredSecrets::Off(store) => store.get(owner, server).await,
            #[cfg(feature = "secrets-pg")]
            ConfiguredSecrets::Pg(store) => store.get(owner, server).await,
        }
    }

    async fn delete(&self, owner: &UserId, server: &SecretServer) -> Result<(), SecretError> {
        match self {
            ConfiguredSecrets::Off(store) => store.delete(owner, server).await,
            #[cfg(feature = "secrets-pg")]
            ConfiguredSecrets::Pg(store) => store.delete(owner, server).await,
        }
    }

    async fn rotate(
        &self,
        owner: &UserId,
        server: &SecretServer,
        expected: SecretVersion,
        material: &SecretMaterial,
    ) -> Result<SecretVersion, SecretError> {
        match self {
            ConfiguredSecrets::Off(store) => store.rotate(owner, server, expected, material).await,
            #[cfg(feature = "secrets-pg")]
            ConfiguredSecrets::Pg(store) => store.rotate(owner, server, expected, material).await,
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

/// The one `UserToolPorts` type of the binary's `PortSet`.
pub type ConfiguredUserTools = UserTools<ConfiguredServers, ConfiguredSecrets, ConfiguredBroker>;

/// Builds the ports for `settings`: the feature off (`None`) is the refusing bundle and opens
/// nothing; on, the definitions are the table of the thread store's pool, the secret store is the sealed table in a
/// pool of its own (so a lock that is held in it can never keep a connection of the thread store)
/// when the file has the `secrets` section, and the broker is built over both with the egress
/// policy, the system clock and the OAuth client of the file.
///
/// # Errors
/// A configuration error (exit 78) for settings the broker refuses (a client metadata URL it cannot
/// use); the secret store's database being down (exit 69, the same outage as the thread store's) or
/// its migrations not matching this build.
pub async fn build(
    settings: Option<&UserToolsSettings>,
    database_url: &str,
    store: &PgStore,
) -> anyhow::Result<ConfiguredUserTools> {
    let Some(settings) = settings else {
        return Ok(UserTools::new(
            UserToolsMode::Off,
            EgressPolicy::new(),
            ConfiguredServers::Off(NoUserToolServers),
            ConfiguredSecrets::Off(NoSecrets),
            ConfiguredBroker::Off(NoBroker),
        ));
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
        let tools = UserTools::new(
            UserToolsMode::Full,
            settings.egress.clone(),
            ConfiguredServers::Pg(servers),
            ConfiguredSecrets::Pg(secrets),
            ConfiguredBroker::Full(broker),
        );
        announce(&tools, settings);
        return Ok(tools);
    }

    // no `secrets` section (a store this build lacks was refused when the file was read)
    let _ = (database_url, oauth);
    let tools = UserTools::new(
        UserToolsMode::WithoutCredentials,
        settings.egress.clone(),
        ConfiguredServers::Pg(servers),
        ConfiguredSecrets::Off(NoSecrets),
        ConfiguredBroker::Off(NoBroker),
    );
    announce(&tools, settings);
    Ok(tools)
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
