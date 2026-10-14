//! The broker itself: [`Broker`] implements [`CredentialBroker`] over a [`SecretStore`] and a
//! [`UserToolServerStore`].

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use orch_core::UserId;
use orch_ports::{
    Clock, CredentialBroker, CredentialError, CredentialLease, CredentialRef, EgressPolicy,
    OAuthStart, OAuthState, RedirectUri, SecretError, SecretServer, SecretStore, SecretVersion,
    StoredServer, SystemClock, ToolSecret, UserAuth, UserToolServer, UserToolServerId,
    UserToolServerState, UserToolServerStore, UserToolServerStoreError,
};
use serde::Deserialize;
use serde_json::json;
use url::Url;

use crate::BuildError;
use crate::config::BrokerConfig;
use crate::discovery::{self, DiscoveryError, Found};
use crate::flights::Flights;
use crate::http::{Body, FetchError, HostLookup, Http, SystemLookup};
use crate::material::{
    AuthMethod, ClientRecord, Endpoints, FlowRecord, KeepError, OAuthGrant, PendingFlow, Record,
    Tokens, open, seal, secret_opt,
};
use crate::oauth::{self, ASSUMED_TTL_SECS, Issued, TokenFailure};

// ------------------------------------------------------------------------------------ errors

fn secret_error(error: SecretError) -> CredentialError {
    let detail = match &error {
        SecretError::Conflict => "the secret store reported a conflict",
        SecretError::Unavailable { .. } => "the secret store is unavailable",
        SecretError::KeyUnknown => "a stored secret is sealed under a key this deployment lacks",
        SecretError::Corrupt(_) => "the secret store holds corrupt data",
        // a kind of failure a later version of the port names
        _ => "the secret store failed",
    };
    CredentialError::unavailable(detail).with_source(error)
}

fn definitions_error(error: UserToolServerStoreError) -> CredentialError {
    let detail = match &error {
        UserToolServerStoreError::Corrupt(_) => "the store of definitions holds corrupt data",
        // the others are a store that is down, or answers a read with what no read gives
        _ => "the store of definitions is unavailable",
    };
    CredentialError::unavailable(detail).with_source(error)
}

fn keep_error(error: KeepError) -> CredentialError {
    CredentialError::unavailable("the credential cannot be kept").with_source(error)
}

/// A request of the broker that did not finish. A refusal of the egress guard is the operator's
/// to read in the log (the rule, never the address) and the API's to find in the error's chain
/// ([`egress_refusal`](crate::egress_refusal)).
fn fetch_failed(step: &'static str, error: FetchError) -> CredentialError {
    let detail = match error {
        FetchError::Refused(refusal) => {
            tracing::warn!(
                rule = refusal.rule(),
                step,
                "the egress guard refused a URL"
            );
            "the egress guard refused a URL"
        }
        FetchError::Timeout | FetchError::Connect | FetchError::TooLarge | FetchError::Failed => {
            step
        }
    };
    CredentialError::unavailable(detail).with_source(error)
}

fn discovery_failed(error: DiscoveryError) -> CredentialError {
    match error {
        DiscoveryError::Fetch(e) => fetch_failed("discovery of the authorization server failed", e),
        DiscoveryError::NotFound => CredentialError::unavailable(
            "the server publishes no metadata that names an authorization server",
        )
        .with_source(error),
        DiscoveryError::Invalid => CredentialError::unavailable(
            "the metadata of the server or its authorization server is not usable",
        )
        .with_source(error),
        DiscoveryError::NoPkce => {
            CredentialError::unavailable("the authorization server does not support PKCE with S256")
                .with_source(error)
        }
    }
}

fn token_failed(failure: TokenFailure) -> CredentialError {
    match failure {
        TokenFailure::Grant(code) => {
            tracing::warn!(error_code = %code, "the authorization server refused the grant");
            CredentialError::NeedsReauth
        }
        TokenFailure::Unavailable(detail, Some(fetch)) => fetch_failed(detail, fetch),
        TokenFailure::Unavailable(detail, None) => CredentialError::unavailable(detail),
    }
}

fn random_failed() -> CredentialError {
    CredentialError::unavailable("the operating system's random source failed")
}

/// The egress refusal that made `error` an `Unavailable`, when that is why: the rule that
/// refused. A person is shown [`EgressRefusal::public_reason`](orch_ports::EgressRefusal), never
/// the precise rule.
pub fn egress_refusal(error: &CredentialError) -> Option<orch_ports::EgressRefusal> {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(e) = source {
        if let Some(refusal) = e.downcast_ref::<orch_ports::EgressRefusal>() {
            return Some(*refusal);
        }
        source = e.source();
    }
    None
}

// -------------------------------------------------------------------------------------- time

/// `at` and `by` later, saturating.
fn after(at: Timestamp, by: Duration) -> Timestamp {
    SignedDuration::try_from(by)
        .ok()
        .and_then(|by| at.checked_add(by).ok())
        .unwrap_or(Timestamp::MAX)
}

fn credential_ref(
    server: &UserToolServerId,
    version: SecretVersion,
) -> Result<CredentialRef, CredentialError> {
    CredentialRef::new(&format!("cred-{server}-v{version}"))
        .map_err(|_| CredentialError::unavailable("the credential has no valid reference"))
}

// --------------------------------------------------------------------------------- the broker

/// The broker (ADR 0046), over the store of secrets `S` and the store of definitions `D`, reading
/// time from `C`. Cheap to clone: clones are one process (they share the single-flight table);
/// build another with [`Broker::builder`] for another replica.
pub struct Broker<S, D, C = SystemClock> {
    inner: Arc<Inner<S, D, C>>,
}

impl<S, D, C> Clone for Broker<S, D, C> {
    fn clone(&self) -> Self {
        Broker {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<S, D, C> fmt::Debug for Broker<S, D, C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Broker").finish_non_exhaustive()
    }
}

impl<S: SecretStore, D: UserToolServerStore> Broker<S, D, SystemClock> {
    /// A broker over `secrets` and `servers` that applies `egress` to every URL it fetches. Set the
    /// clock, the settings and the resolver on the builder.
    pub fn builder(
        secrets: S,
        servers: D,
        egress: EgressPolicy,
    ) -> BrokerBuilder<S, D, SystemClock> {
        BrokerBuilder {
            secrets,
            servers,
            egress,
            clock: SystemClock,
            config: BrokerConfig::default(),
            lookup: Arc::new(SystemLookup),
        }
    }
}

/// Builds a [`Broker`].
pub struct BrokerBuilder<S, D, C> {
    secrets: S,
    servers: D,
    egress: EgressPolicy,
    clock: C,
    config: BrokerConfig,
    lookup: Arc<dyn HostLookup>,
}

impl<S, D, C> fmt::Debug for BrokerBuilder<S, D, C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BrokerBuilder")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl<S: SecretStore, D: UserToolServerStore, C: Clock> BrokerBuilder<S, D, C> {
    /// The clock the broker reads.
    pub fn clock<C2: Clock>(self, clock: C2) -> BrokerBuilder<S, D, C2> {
        BrokerBuilder {
            secrets: self.secrets,
            servers: self.servers,
            egress: self.egress,
            clock,
            config: self.config,
            lookup: self.lookup,
        }
    }

    /// The settings.
    #[must_use]
    pub fn config(mut self, config: BrokerConfig) -> Self {
        self.config = config;
        self
    }

    /// How host names become addresses. The default asks the operating system. Whatever answers,
    /// the egress guard judges the addresses and the connection is made to those alone.
    #[must_use]
    pub fn lookup(mut self, lookup: impl HostLookup) -> Self {
        self.lookup = Arc::new(lookup);
        self
    }

    /// Builds the broker.
    ///
    /// # Errors
    /// [`BuildError`] for settings that cannot work, or an HTTP client that cannot start.
    pub fn build(self) -> Result<Broker<S, D, C>, BuildError> {
        self.config.validate()?;
        let http = Http::new(self.egress, self.lookup, &self.config)?;
        Ok(Broker {
            inner: Arc::new(Inner {
                secrets: self.secrets,
                servers: self.servers,
                clock: self.clock,
                http,
                flights: Flights::default(),
                config: self.config,
            }),
        })
    }
}

struct Inner<S, D, C> {
    secrets: S,
    servers: D,
    clock: C,
    http: Http,
    flights: Flights,
    config: BrokerConfig,
}

/// A credential that is ready to lend, or an OAuth grant whose access token is about to run out.
enum Ready<'a> {
    Lease(CredentialLease),
    Stale(&'a OAuthGrant),
}

impl<S: SecretStore, D: UserToolServerStore, C: Clock> Inner<S, D, C> {
    async fn definition(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
    ) -> Result<StoredServer, CredentialError> {
        self.servers
            .get(owner, server)
            .await
            .map_err(definitions_error)?
            .ok_or(CredentialError::Unknown)
    }

    /// The credential kept for the definition, if it was issued for the definition's URL now.
    async fn credential(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
        definition: &UserToolServer,
    ) -> Result<(SecretVersion, Record), CredentialError> {
        let slot = SecretServer::credential(server);
        let Some((version, material)) =
            self.secrets.get(owner, &slot).await.map_err(secret_error)?
        else {
            return Err(CredentialError::NeedsReauth);
        };
        let Some(record) = open::<Record>(&material) else {
            return Err(CredentialError::unavailable(
                "the stored credential cannot be read",
            ));
        };
        // a credential is for the URL it was issued for, and no other
        if record.url() != definition.url() {
            return Err(CredentialError::NeedsReauth);
        }
        Ok((version, record))
    }

    fn bearer_lease(&self, tokens: &Tokens, now: Timestamp) -> CredentialLease {
        let ceiling = after(now, self.config.lease_ttl);
        let end = tokens
            .expires_at
            .and_then(|s| Timestamp::from_second(s).ok())
            .map_or(ceiling, |expires| expires.min(ceiling));
        CredentialLease::bearer(tokens.access.clone(), end)
    }

    fn ready<'a>(
        &self,
        definition: &UserToolServer,
        record: &'a Record,
    ) -> Result<Ready<'a>, CredentialError> {
        let now = self.clock.now();
        let ceiling = after(now, self.config.lease_ttl);
        match (definition.auth(), record) {
            (UserAuth::Bearer, Record::Bearer { token, .. }) => Ok(Ready::Lease(
                CredentialLease::bearer(token.clone(), ceiling),
            )),
            (UserAuth::ApiKey { header }, Record::ApiKey { value, .. }) => {
                CredentialLease::header(header, value.clone(), ceiling)
                    .map(Ready::Lease)
                    .map_err(|_| {
                        CredentialError::unavailable("the header of the API key is not valid")
                    })
            }
            (UserAuth::OAuth2 { .. }, Record::OAuth2(grant)) => {
                let Some(tokens) = &grant.tokens else {
                    return Err(CredentialError::NeedsReauth);
                };
                let margin = after(now, self.config.refresh_margin);
                let fresh = tokens
                    .expires_at
                    .and_then(|s| Timestamp::from_second(s).ok())
                    .is_none_or(|expires| expires >= margin);
                if fresh {
                    Ok(Ready::Lease(self.bearer_lease(tokens, now)))
                } else {
                    Ok(Ready::Stale(grant))
                }
            }
            (UserAuth::None, _) => Err(CredentialError::Unknown),
            // the credential is of another kind than the definition's now
            (UserAuth::Bearer | UserAuth::ApiKey { .. } | UserAuth::OAuth2 { .. }, _) => {
                Err(CredentialError::NeedsReauth)
            }
        }
    }

    // ------------------------------------------------------------------------ refresh

    /// Refreshes the access token, one caller at a time per `(owner, server)`. Runs to its end even
    /// if the caller gives up (it is spawned): a refresh token that rotated and was not kept is a
    /// grant lost.
    async fn refresh(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
    ) -> Result<CredentialLease, CredentialError> {
        let _flight = self.flights.acquire(owner, server).await;

        // read again inside the flight: whoever held it before may have refreshed
        let stored = self.definition(owner, server).await?;
        let definition = stored.server();
        let (version, record) = self.credential(owner, server, definition).await?;
        let grant = match self.ready(definition, &record)? {
            Ready::Lease(lease) => return Ok(lease),
            Ready::Stale(grant) => grant,
        };
        let Some(tokens) = &grant.tokens else {
            return Err(CredentialError::NeedsReauth);
        };
        let Some(refresh_token) = &tokens.refresh else {
            // it ran out and there is nothing to ask again with: the grant is over
            self.mark_needs_reauth(owner, &stored).await;
            return Err(CredentialError::NeedsReauth);
        };

        let fields = [
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.expose()),
            ("resource", grant.resource.as_str()),
        ];
        let issued =
            oauth::request_token(&self.http, &grant.endpoints.token, &grant.client, &fields).await;
        let issued = match issued {
            Ok(issued) => issued,
            Err(TokenFailure::Grant(code)) => {
                tracing::warn!(error_code = %code, server = %server, "the refresh token was refused");
                if code == "invalid_grant" {
                    self.end_grant(owner, server, version, grant).await;
                }
                self.mark_needs_reauth(owner, &stored).await;
                return Err(CredentialError::NeedsReauth);
            }
            Err(failure) => return Err(token_failed(failure)),
        };

        let now = self.clock.now();
        let renewed = renewed(grant, issued, now);
        let lease = renewed
            .tokens
            .as_ref()
            .map(|tokens| self.bearer_lease(tokens, now))
            .ok_or(CredentialError::NeedsReauth)?;
        let sealed = seal(&Record::OAuth2(Box::new(renewed))).map_err(keep_error)?;
        match self
            .secrets
            .rotate(owner, &SecretServer::credential(server), version, &sealed)
            .await
        {
            Ok(_) => Ok(lease),
            // another replica refreshed first: its tokens are the ones to use, and this refresh is
            // not repeated
            Err(SecretError::Conflict) => self.after_conflict(owner, server, definition).await,
            Err(e) => Err(secret_error(e)),
        }
    }

    async fn after_conflict(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
        definition: &UserToolServer,
    ) -> Result<CredentialLease, CredentialError> {
        let (_, record) = self.credential(owner, server, definition).await?;
        match self.ready(definition, &record)? {
            Ready::Lease(lease) => Ok(lease),
            Ready::Stale(_) => Err(CredentialError::unavailable(
                "the credential changed while it was refreshed",
            )),
        }
    }

    /// The grant is gone: keeps the client and drops the tokens, so that nobody asks the
    /// authorization server with a refresh token it has refused. A compare and swap: a grant the
    /// person made since is not touched.
    async fn end_grant(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
        version: SecretVersion,
        grant: &OAuthGrant,
    ) {
        let ended = OAuthGrant {
            tokens: None,
            ..grant.clone()
        };
        let Ok(sealed) = seal(&Record::OAuth2(Box::new(ended))) else {
            return;
        };
        match self
            .secrets
            .rotate(owner, &SecretServer::credential(server), version, &sealed)
            .await
        {
            Ok(_) | Err(SecretError::Conflict) => {}
            Err(e) => {
                tracing::warn!(server = %server, error = %e, "the ended grant was not recorded")
            }
        }
    }

    /// Records `needs_reauth` on the definition: the one state the broker writes, because it is the
    /// only one that learns of it. A compare and swap on the revision it read: an edit since means
    /// what it saw was about another server, and nothing is written.
    async fn mark_needs_reauth(&self, owner: &UserId, stored: &StoredServer) {
        let definition = stored.server();
        if definition.state() == UserToolServerState::NeedsReauth {
            return;
        }
        match self
            .servers
            .set_state(
                owner,
                definition.id(),
                UserToolServerState::NeedsReauth,
                stored.revision(),
            )
            .await
        {
            Ok(_) => {}
            Err(UserToolServerStoreError::NotFound | UserToolServerStoreError::Conflict) => {
                tracing::debug!(server = %definition.id(), "the definition changed, its state was not written");
            }
            Err(e) => {
                tracing::warn!(server = %definition.id(), error = %e, "the state of the definition was not written");
            }
        }
    }

    // ---------------------------------------------------------------------- registration

    async fn client_for(
        &self,
        found: &Found,
        pre_registered: Option<&str>,
        redirect: &RedirectUri,
    ) -> Result<ClientRecord, CredentialError> {
        // 1. a client the person registered themselves
        if let Some(id) = pre_registered {
            return Ok(ClientRecord::public(id));
        }
        // 2. a Client ID Metadata Document, when the server supports them and we have one
        if found.server.client_id_metadata_document
            && let Some(url) = &self.config.client_metadata_url
        {
            return Ok(ClientRecord::public(url.as_str()));
        }
        // 3. dynamic registration
        let Some(endpoint) = &found.server.registration_endpoint else {
            return Err(CredentialError::unavailable(
                "the authorization server offers no way to register a client; a client id is needed",
            ));
        };
        self.register(endpoint, redirect).await
    }

    /// RFC 7591: registers a public client for `redirect`.
    async fn register(
        &self,
        endpoint: &str,
        redirect: &RedirectUri,
    ) -> Result<ClientRecord, CredentialError> {
        #[derive(Deserialize)]
        struct Registered {
            client_id: String,
            #[serde(default, with = "secret_opt")]
            client_secret: Option<ToolSecret>,
            token_endpoint_auth_method: Option<String>,
        }
        let request = json!({
            "client_name": self.config.client_name,
            "redirect_uris": [redirect.as_str()],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        });
        let body = serde_json::to_vec(&request)
            .map_err(|_| CredentialError::unavailable("the registration could not be written"))?;
        let response = self
            .http
            .post(endpoint, Body::Json(body), None)
            .await
            .map_err(|e| fetch_failed("the registration of the client failed", e))?;
        if !(200..300).contains(&response.status) {
            return Err(CredentialError::unavailable(
                "the authorization server refused to register the client",
            ));
        }
        let registered = serde_json::from_slice::<Registered>(&response.body).map_err(|_| {
            CredentialError::unavailable("the registration answer is not a client registration")
        })?;
        if registered.client_id.is_empty()
            || registered.client_id.len() > 2048
            || !registered.client_id.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err(CredentialError::unavailable(
                "the registration answer is not a client registration",
            ));
        }
        let method = match (
            &registered.client_secret,
            registered.token_endpoint_auth_method.as_deref(),
        ) {
            (None, _) | (Some(_), Some("none")) => AuthMethod::None,
            (Some(_), Some("client_secret_post")) => AuthMethod::Post,
            (Some(_), _) => AuthMethod::Basic,
        };
        let secret = if method == AuthMethod::None {
            None
        } else {
            registered.client_secret
        };
        Ok(ClientRecord {
            id: registered.client_id,
            secret,
            method,
        })
    }

    async fn exchange_code(
        &self,
        flow: &PendingFlow,
        code: &ToolSecret,
    ) -> Result<Issued, CredentialError> {
        let fields = [
            ("grant_type", "authorization_code"),
            ("code", code.expose()),
            ("redirect_uri", flow.redirect_uri.as_str()),
            ("code_verifier", flow.verifier.expose()),
            ("resource", flow.resource.as_str()),
        ];
        oauth::request_token(&self.http, &flow.endpoints.token, &flow.client, &fields)
            .await
            .map_err(token_failed)
    }

    async fn forget_flow(&self, owner: &UserId, server: &UserToolServerId) {
        if let Err(e) = self
            .secrets
            .delete(owner, &SecretServer::pending_flow(server))
            .await
        {
            tracing::warn!(server = %server, error = %e, "a pending flow was not forgotten");
        }
    }

    /// Tells the authorization server to revoke the grant (RFC 7009), once the broker has forgotten
    /// it. A failure is logged, not returned: the person's credential is gone either way.
    async fn revoke_upstream(&self, grant: &OAuthGrant) {
        let (Some(endpoint), Some(tokens)) = (&grant.endpoints.revocation, &grant.tokens) else {
            return;
        };
        // revoking the refresh token takes the access tokens of the grant with it where the
        // server can; without one, the access token is what there is
        let (token, hint) = match &tokens.refresh {
            Some(refresh) => (refresh, "refresh_token"),
            None => (&tokens.access, "access_token"),
        };
        let fields = [("token", token.expose()), ("token_type_hint", hint)];
        match oauth::post_as_client(&self.http, endpoint, &grant.client, &fields).await {
            Ok(response) if (200..300).contains(&response.status) => {}
            Ok(response) => {
                tracing::warn!(
                    status = response.status,
                    "the authorization server did not revoke the grant"
                );
            }
            Err(FetchError::Refused(refusal)) => {
                tracing::warn!(
                    rule = refusal.rule(),
                    "the egress guard refused the revocation endpoint"
                );
            }
            Err(e) => tracing::warn!(error = %e, "the revocation request failed"),
        }
    }
}

/// The grant after a refresh: the new access token, the refresh token the server rotated to (or the
/// one it kept), and when the access token ends.
fn renewed(grant: &OAuthGrant, issued: Issued, now: Timestamp) -> OAuthGrant {
    let refresh = issued
        .refresh
        .or_else(|| grant.tokens.as_ref().and_then(|t| t.refresh.clone()));
    OAuthGrant {
        tokens: Some(tokens_of(issued.access, issued.expires_in, refresh, now)),
        ..grant.clone()
    }
}

/// Tokens with the end of the access token: `expires_in` seconds from `now`; when the server said
/// nothing, an hour if there is a refresh token to ask again with, else no end at all.
fn tokens_of(
    access: ToolSecret,
    expires_in: Option<u64>,
    refresh: Option<ToolSecret>,
    now: Timestamp,
) -> Tokens {
    let seconds = expires_in.or(refresh.is_some().then_some(ASSUMED_TTL_SECS));
    Tokens {
        access,
        expires_at: seconds.map(|s| after(now, Duration::from_secs(s)).as_second()),
        refresh,
    }
}

fn authorization_url(
    endpoint: &Url,
    client_id: &str,
    redirect: &RedirectUri,
    state: &OAuthState,
    challenge: &str,
    resource: &str,
    scope: &str,
) -> String {
    let mut url = endpoint.clone();
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", client_id)
            .append_pair("redirect_uri", redirect.as_str())
            .append_pair("state", state.as_str())
            .append_pair("code_challenge", challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("resource", resource);
        if !scope.is_empty() {
            query.append_pair("scope", scope);
        }
    }
    url.into()
}

// ---------------------------------------------------------------------------------- the port

impl<S: SecretStore, D: UserToolServerStore, C: Clock> CredentialBroker for Broker<S, D, C> {
    async fn store(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
        material: &ToolSecret,
    ) -> Result<CredentialRef, CredentialError> {
        let inner = &self.inner;
        let stored = inner.definition(owner, server).await?;
        let definition = stored.server();
        let url = definition.url().to_owned();
        // a person types a secret for these two kinds; `none` has no credential and `oauth2` is
        // obtained by an authorization, not typed
        let record = match definition.auth() {
            UserAuth::Bearer => Record::Bearer {
                url,
                token: material.clone(),
            },
            UserAuth::ApiKey { .. } => Record::ApiKey {
                url,
                value: material.clone(),
            },
            UserAuth::None | UserAuth::OAuth2 { .. } => return Err(CredentialError::Unknown),
        };
        let sealed = seal(&record).map_err(keep_error)?;
        let version = inner
            .secrets
            .put(owner, &SecretServer::credential(server), &sealed)
            .await
            .map_err(secret_error)?;
        credential_ref(server, version)
    }

    async fn begin_oauth(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
        redirect: &RedirectUri,
    ) -> Result<OAuthStart, CredentialError> {
        let inner = &self.inner;
        let stored = inner.definition(owner, server).await?;
        let definition = stored.server();
        let UserAuth::OAuth2 { scopes, client_id } = definition.auth() else {
            return Err(CredentialError::Unknown);
        };

        let found = discovery::discover(&inner.http, definition.url())
            .await
            .map_err(discovery_failed)?;
        let client = inner
            .client_for(&found, client_id.as_deref(), redirect)
            .await?;

        let verifier = oauth::new_verifier().ok_or_else(random_failed)?;
        let challenge = oauth::challenge_of(&verifier);
        let state = oauth::random_token(32)
            .and_then(|text| OAuthState::new(&text).ok())
            .ok_or_else(random_failed)?;
        // the scopes the person named; else those the server says it offers; else none
        let scope = if scopes.is_empty() {
            found.scopes_supported.join(" ")
        } else {
            scopes.join(" ")
        };
        let authorize_url = authorization_url(
            &found.server.authorization_endpoint,
            &client.id,
            redirect,
            &state,
            &challenge,
            &found.resource,
            &scope,
        );

        let expires_at = after(inner.clock.now(), inner.config.flow_ttl);
        let issuer = found.server.issuer.clone();
        let flow = FlowRecord::Pending(Box::new(PendingFlow {
            state: state.as_str().to_owned(),
            issuer: issuer.clone(),
            iss_advertised: found.server.iss_parameter,
            verifier: ToolSecret::new(verifier),
            expires_at: expires_at.as_second(),
            url: definition.url().to_owned(),
            resource: found.resource,
            redirect_uri: redirect.as_str().to_owned(),
            client,
            endpoints: Endpoints {
                token: found.server.token_endpoint,
                revocation: found.server.revocation_endpoint,
            },
        }));
        let sealed = seal(&flow).map_err(keep_error)?;
        // replaces the flow that was pending: its state is no longer one
        inner
            .secrets
            .put(owner, &SecretServer::pending_flow(server), &sealed)
            .await
            .map_err(secret_error)?;
        Ok(OAuthStart {
            authorize_url,
            state,
            expires_at,
            issuer,
        })
    }

    async fn complete_oauth(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
        state: &OAuthState,
        code: &ToolSecret,
        iss: Option<&str>,
    ) -> Result<CredentialRef, CredentialError> {
        let inner = &self.inner;
        let stored = inner.definition(owner, server).await?;
        if !matches!(stored.server().auth(), UserAuth::OAuth2 { .. }) {
            return Err(CredentialError::Unknown);
        }

        let slot = SecretServer::pending_flow(server);
        let Some((version, material)) = inner
            .secrets
            .get(owner, &slot)
            .await
            .map_err(secret_error)?
        else {
            return Err(CredentialError::Unknown);
        };
        let Some(FlowRecord::Pending(flow)) = open::<FlowRecord>(&material) else {
            return Err(CredentialError::Unknown);
        };
        drop(material);
        // compared in constant time; a state that is not this flow's leaves the flow as it is
        let Ok(began) = OAuthState::new(&flow.state) else {
            return Err(CredentialError::Unknown);
        };
        if began != *state {
            return Err(CredentialError::Unknown);
        }
        let now = inner.clock.now();
        if now.as_second() >= flow.expires_at {
            inner.forget_flow(owner, server).await;
            return Err(CredentialError::Unknown);
        }

        // RFC 9207, section 2.4, before anything is spent: a callback that names another issuer
        // than the one the flow began with is not this flow's (a mix-up, RFC 9700, section 4.4),
        // and its code goes to nobody. The flow stays as it is, so the person's own callback
        // still completes it.
        if let Err(why) = flow.check_issuer(iss) {
            tracing::warn!(
                server = %server,
                why,
                "a callback was refused: its issuer is not the one the flow began with"
            );
            return Err(CredentialError::Unknown);
        }

        // the state is used once, whichever replica gets the callback: the one that swaps the flow
        // for a marker takes it, the others see a conflict
        let used = seal(&FlowRecord::Used).map_err(keep_error)?;
        match inner.secrets.rotate(owner, &slot, version, &used).await {
            Ok(_) => {}
            Err(SecretError::Conflict) => return Err(CredentialError::Unknown),
            Err(e) => return Err(secret_error(e)),
        }

        let kept = async {
            let issued = inner.exchange_code(&flow, code).await?;
            let tokens = tokens_of(issued.access, issued.expires_in, issued.refresh, now);
            // bound to the URL the flow began with
            let record = Record::OAuth2(Box::new(OAuthGrant {
                url: flow.url.clone(),
                resource: flow.resource.clone(),
                client: flow.client.clone(),
                endpoints: flow.endpoints.clone(),
                tokens: Some(tokens),
            }));
            let sealed = seal(&record).map_err(keep_error)?;
            let version = inner
                .secrets
                .put(owner, &SecretServer::credential(server), &sealed)
                .await
                .map_err(secret_error)?;
            credential_ref(server, version)
        }
        .await;
        // the flow is over, whatever came of it
        inner.forget_flow(owner, server).await;
        kept
    }

    async fn lease(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
    ) -> Result<CredentialLease, CredentialError> {
        let inner = &self.inner;
        let stored = inner.definition(owner, server).await?;
        let definition = stored.server();
        if !definition.auth().needs_credential() {
            return Err(CredentialError::Unknown);
        }
        let (_, record) = inner.credential(owner, server, definition).await?;
        match inner.ready(definition, &record)? {
            Ready::Lease(lease) => Ok(lease),
            Ready::Stale(_) => {
                let inner = Arc::clone(&self.inner);
                let (owner, server) = (owner.clone(), server.clone());
                tokio::spawn(async move { inner.refresh(&owner, &server).await })
                    .await
                    .map_err(|_| CredentialError::unavailable("the refresh was interrupted"))?
            }
        }
    }

    async fn revoke(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
    ) -> Result<(), CredentialError> {
        let inner = &self.inner;
        let grant = {
            // not while a refresh is writing the tokens
            let _flight = inner.flights.acquire(owner, server).await;
            let slot = SecretServer::credential(server);
            let kept = match inner.secrets.get(owner, &slot).await {
                Ok(kept) => kept,
                // a row that cannot be opened is still deleted below, and has nothing to revoke
                Err(SecretError::KeyUnknown | SecretError::Corrupt(_)) => None,
                Err(e) => return Err(secret_error(e)),
            };
            let grant = kept.and_then(|(_, material)| match open::<Record>(&material) {
                Some(Record::OAuth2(grant)) => Some(grant),
                Some(Record::Bearer { .. } | Record::ApiKey { .. }) | None => None,
            });
            inner
                .secrets
                .delete(owner, &slot)
                .await
                .map_err(secret_error)?;
            inner
                .secrets
                .delete(owner, &SecretServer::pending_flow(server))
                .await
                .map_err(secret_error)?;
            grant
        };
        if let Some(grant) = grant {
            inner.revoke_upstream(&grant).await;
        }
        Ok(())
    }
}
