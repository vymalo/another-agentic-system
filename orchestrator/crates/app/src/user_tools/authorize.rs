//! Authorizing an `oauth2` server of a person's own, and the callback that finishes it (ADR 0046,
//! slice 4c): [`App::authorize_user_tool_server`], [`App::finish_user_tool_authorization`] and the
//! types they speak. The parent module's docs ("Authorizing an `oauth2` server") say how the two
//! halves fit together and why the callback takes no person.

use std::fmt;

use orch_core::{Classify, Timestamp, report};
use orch_ports::{
    Clock, CredentialBroker, CredentialError, CredentialRef, OAuthState, PendingAuthorizationError,
    PendingAuthorizations, Ports, StoreError, StoredServer, ToolSecret, UserAuth, UserToolPorts,
    UserToolServer, UserToolServerState, UserToolServerStore, UserToolServerStoreError,
};

use super::{EDIT_ATTEMPTS, UserToolRefusal, parse_id, same_grant, store_error};
use crate::{App, AppError, Requester};

/// The longest `code` a callback may carry, in bytes. Authorization codes are opaque and some
/// authorization servers make them long (a signed token); the query of a request is bounded by the
/// API far below the token endpoint's own limits, and this is the application's.
pub const MAX_CALLBACK_CODE_BYTES: usize = 8 * 1024;

// ------------------------------------------------------------------------------ authorization

/// An authorization that has begun, as the person is told of it (`POST …/authorize`). `Debug` shows
/// neither the URL nor the state in it.
#[derive(Clone, PartialEq, Eq)]
pub struct StartedAuthorization {
    authorize_url: String,
    issuer: String,
    expires_at: Timestamp,
}

impl StartedAuthorization {
    /// Where to send the person: the authorization server's endpoint, with the PKCE challenge, the
    /// `resource`, the `redirect_uri` and the `state` in its query. It is a capability for the length
    /// of the flow, so it is never logged and the answer that carries it is never cached.
    pub fn authorize_url(&self) -> &str {
        &self.authorize_url
    }

    /// The issuer identifier (RFC 8414) of the authorization server the person is sent to: a public
    /// `https` identifier, so that the screen can show which server a stranger's MCP server chose.
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// When the flow ends: a callback at or after it finds nothing.
    pub fn expires_at(&self) -> Timestamp {
        self.expires_at
    }
}

impl fmt::Debug for StartedAuthorization {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StartedAuthorization")
            .field("authorize_url", &"[redacted]")
            .field("issuer", &self.issuer)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// What the browser brought back to the OAuth callback, as it came: nothing of it is interpreted
/// here but by the checks below. `Debug` shows which parts were there and none of their text.
#[derive(Default)]
pub struct OAuthCallback {
    /// The `state` parameter. A bearer value: it finds the person and the server.
    pub state: Option<String>,
    /// The `code` parameter. A secret until it is exchanged.
    pub code: Option<ToolSecret>,
    /// The `iss` parameter (RFC 9207), decoded, **as it came**: the broker compares it.
    pub iss: Option<String>,
    /// Whether the redirect carried an `error` (the person declined, or the authorization server
    /// refused). Its text is not read, and neither is `error_description`: it is a stranger's text,
    /// and without `iss` checked it may not even come from the authorization server (RFC 9207, 2.4).
    pub error: bool,
}

impl fmt::Debug for OAuthCallback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthCallback")
            .field("state", &self.state.is_some())
            .field("code", &self.code.is_some())
            .field("iss", &self.iss.is_some())
            .field("error", &self.error)
            .finish()
    }
}

/// How a callback ended: the page the browser is shown, and nothing else. A closed enum, so an
/// outcome added later fails to compile in the route that writes the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuthorizationOutcome {
    /// The server is `connected`: the person can return to the app.
    Connected,
    /// The link is not one: no `state`, or one that was never put, was used, was replaced by a newer
    /// authorization of the server, has ended, whose server is no longer an `oauth2` server of the
    /// person's, or whose `iss` the broker refuses (a mix-up).
    InvalidLink,
    /// The authorization was not completed: the person declined or the authorization server reported
    /// an error, the redirect carried no usable `code`, the authorization server refused the code, or
    /// the server was changed or deleted while the person authorized. The server is as it was.
    NotCompleted,
    /// The authorization could not be finished now: the store, the broker or the authorization
    /// server could not be reached. The state is spent; the person begins again.
    Unavailable,
}

impl AuthorizationOutcome {
    /// Whether the server is connected.
    pub const fn is_connected(self) -> bool {
        matches!(self, AuthorizationOutcome::Connected)
    }

    /// A fixed word for the log.
    pub const fn as_str(self) -> &'static str {
        match self {
            AuthorizationOutcome::Connected => "connected",
            AuthorizationOutcome::InvalidLink => "invalid_link",
            AuthorizationOutcome::NotCompleted => "not_completed",
            AuthorizationOutcome::Unavailable => "unavailable",
        }
    }
}

impl<P: Ports> App<P> {
    /// Starts the OAuth authorization of a server of the person's own
    /// (`POST /api/user-tool-servers/{id}/authorize`). Needs `tools.user.add`, a deployment that keeps
    /// credentials and has an address for the callback, and an `oauth2` server that is the person's.
    ///
    /// The broker discovers the server's authorization server, registers the client if it must and
    /// makes the PKCE verifier (which stays in the broker); the application keeps `state ↦ (owner,
    /// server, until)` for the callback and tells the person where to go. A server that is already
    /// `connected` can be authorized again. **Nothing of the definition is written**: the server
    /// stays what it was until a callback finishes the flow.
    ///
    /// The route limits how often a person may call this (it makes the broker fetch, from the
    /// network, URLs a stranger's metadata names) and runs it in a task of its own.
    ///
    /// # Errors
    /// [`UserToolRefusal::Off`]; [`AppError::Forbidden`] without the permission;
    /// [`UserToolRefusal::NoCredentials`]; [`UserToolRefusal::OAuthNotConfigured`];
    /// [`UserToolRefusal::NotFound`] for an id that is not the person's, a malformed one and a server
    /// of another kind; [`UserToolRefusal::EgressRefused`] when the guard stops a URL the broker was
    /// to fetch; [`UserToolRefusal::AuthorizationUnavailable`]; [`AppError::Store`].
    pub async fn authorize_user_tool_server(
        &self,
        who: &impl Requester,
        id: &str,
    ) -> Result<StartedAuthorization, AppError> {
        let mode = self.enabled_user_tools()?;
        self.require_add_own(who)?;
        let id = parse_id(id)?;
        if !mode.keeps_credentials() {
            return Err(UserToolRefusal::NoCredentials.into());
        }
        let Some(redirect) = self.user_tool_redirect() else {
            return Err(UserToolRefusal::OAuthNotConfigured.into());
        };
        let ports = self.user_tools();
        let owner = who.user();
        match ports.servers().get(owner, &id).await {
            Ok(Some(stored)) if matches!(stored.server().auth(), UserAuth::OAuth2 { .. }) => {}
            // never added, deleted, another person's, or a kind with no authorization: all "no such
            // server" to the person who asked
            Ok(Some(_) | None) => return Err(UserToolRefusal::NotFound.into()),
            Err(e) => return Err(store_error(e)),
        }
        let start = ports
            .broker()
            .begin_oauth(owner, &id, redirect)
            .await
            .map_err(|e| authorization_failure(&e))?;
        ports
            .pending()
            .put(
                &start.state,
                owner,
                &id,
                start.expires_at,
                self.ports().clock().now(),
            )
            .await
            .map_err(pending_failure)?;
        Ok(StartedAuthorization {
            authorize_url: start.authorize_url,
            issuer: start.issuer,
            expires_at: start.expires_at,
        })
    }

    /// Finishes an authorization with what the browser brought back to the callback
    /// (`GET /api/user-tool-servers/oauth/callback`). **No person is asked for**: see the module's
    /// docs. The sequence, whatever the outcome, spends the `state`:
    ///
    /// 1. the `state` is taken (once; an unknown, used, replaced or ended one is
    ///    [`AuthorizationOutcome::InvalidLink`]);
    /// 2. a redirect with `error`, or without a usable `code`, is
    ///    [`AuthorizationOutcome::NotCompleted`] and calls nothing: the broker's pending flow
    ///    expires by itself;
    /// 3. the definition is read: a server that is gone or is no longer an `oauth2` server is
    ///    `InvalidLink`;
    /// 4. [`complete_oauth`](CredentialBroker::complete_oauth) exchanges the code (the broker checks
    ///    `iss` first, and a refusal is `InvalidLink` and sends the code to nobody);
    /// 5. the definition is replaced as `connected` with the reference the broker gave, on the
    ///    revision it read. When another writer got in first the definition is read again: if what
    ///    the credential was issued for (the URL, the scopes, the client) is unchanged the write is
    ///    made again (a rename, the broker marking the grant); if it changed or the server is gone,
    ///    the credential has no definition that it fits and is revoked (best effort), and the
    ///    outcome is `NotCompleted`.
    ///
    /// The route runs this in a task of its own, so that a browser that closes the connection cannot
    /// leave it between the broker keeping the grant and the definition naming it.
    pub async fn finish_user_tool_authorization(
        &self,
        callback: OAuthCallback,
    ) -> AuthorizationOutcome {
        let outcome = self.finish_authorization(callback).await;
        tracing::info!(
            outcome = outcome.as_str(),
            "the callback of an OAuth authorization of a person's own MCP server was answered"
        );
        outcome
    }

    async fn finish_authorization(&self, callback: OAuthCallback) -> AuthorizationOutcome {
        let ports = self.user_tools();
        // `Off` has no routes, and a deployment that keeps no credentials has nothing pending
        if !ports.mode().keeps_credentials() {
            return AuthorizationOutcome::InvalidLink;
        }
        let Some(state) = callback
            .state
            .as_deref()
            .and_then(|state| OAuthState::new(state).ok())
        else {
            return AuthorizationOutcome::InvalidLink;
        };
        let pending = match ports
            .pending()
            .take(&state, self.ports().clock().now())
            .await
        {
            Ok(Some(pending)) => pending,
            Ok(None) => return AuthorizationOutcome::InvalidLink,
            Err(e) => {
                tracing::warn!(
                    error = %report(&e),
                    "the pending authorizations could not be read for a callback"
                );
                return AuthorizationOutcome::Unavailable;
            }
        };
        let (owner, id) = pending.into_parts();
        let OAuthCallback {
            code, iss, error, ..
        } = callback;
        if error {
            return AuthorizationOutcome::NotCompleted;
        }
        let Some(code) = code.filter(|code| {
            let len = code.expose().len();
            len > 0 && len <= MAX_CALLBACK_CODE_BYTES
        }) else {
            return AuthorizationOutcome::NotCompleted;
        };

        // the definition the credential will be named by, as it is now
        let stored = match ports.servers().get(&owner, &id).await {
            Ok(Some(stored)) if matches!(stored.server().auth(), UserAuth::OAuth2 { .. }) => stored,
            Ok(Some(_) | None) => return AuthorizationOutcome::InvalidLink,
            Err(e) => {
                tracing::warn!(
                    error = %report(&e),
                    "the definition of a server could not be read for a callback"
                );
                return AuthorizationOutcome::Unavailable;
            }
        };

        let credential = match ports
            .broker()
            .complete_oauth(&owner, &id, &state, &code, iss.as_deref())
            .await
        {
            Ok(credential) => credential,
            // not a flow of this person and server, or an `iss` the broker refuses: nothing was sent
            Err(CredentialError::Unknown) => return AuthorizationOutcome::InvalidLink,
            // the authorization server refused the code
            Err(CredentialError::NeedsReauth) => return AuthorizationOutcome::NotCompleted,
            Err(e) => {
                tracing::warn!(
                    class = ?e.class(),
                    error = %report(&e),
                    "the broker could not finish an OAuth authorization"
                );
                return AuthorizationOutcome::Unavailable;
            }
        };
        self.name_the_credential(&owner, stored, credential).await
    }

    /// Writes `connected` and the reference into the definition the credential was issued for, and
    /// settles a credential that cannot be named (see [`finish_user_tool_authorization`]).
    ///
    /// [`finish_user_tool_authorization`]: Self::finish_user_tool_authorization
    async fn name_the_credential(
        &self,
        owner: &orch_core::UserId,
        mut stored: StoredServer,
        credential: CredentialRef,
    ) -> AuthorizationOutcome {
        let ports = self.user_tools();
        let id = stored.server().id().clone();
        for _ in 0..EDIT_ATTEMPTS {
            let connected = stored
                .server()
                .clone()
                .with_state(UserToolServerState::Connected)
                .with_credential(credential.clone());
            match ports
                .servers()
                .replace(owner, &connected, stored.revision())
                .await
            {
                Ok(_) => return AuthorizationOutcome::Connected,
                Err(UserToolServerStoreError::Conflict) => {
                    match ports.servers().get(owner, &id).await {
                        // renamed, or marked by the broker: what the credential was issued for is
                        // the same, so name it on the revision that is there now
                        Ok(Some(now)) if issued_for_the_same(stored.server(), now.server()) => {
                            stored = now;
                        }
                        // moved, or deleted: the credential fits no definition
                        Ok(Some(_) | None) => {
                            self.forget_unnamed(
                                owner,
                                &id,
                                "a credential kept for a server that was changed or deleted under its authorization",
                            )
                            .await;
                            return AuthorizationOutcome::NotCompleted;
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %report(&e),
                                "a server changed under its authorization and could not be read again"
                            );
                            self.forget_unnamed(
                                owner,
                                &id,
                                "a credential kept for a server that could not be read again",
                            )
                            .await;
                            return AuthorizationOutcome::Unavailable;
                        }
                    }
                }
                Err(UserToolServerStoreError::NotFound) => {
                    self.forget_unnamed(
                        owner,
                        &id,
                        "a credential kept for a server that was deleted under its authorization",
                    )
                    .await;
                    return AuthorizationOutcome::NotCompleted;
                }
                Err(e) => {
                    tracing::warn!(
                        error = %report(&e),
                        "the definition of a server could not be written after its authorization"
                    );
                    self.forget_unnamed(
                        owner,
                        &id,
                        "a credential kept for a server whose authorization could not be written",
                    )
                    .await;
                    return AuthorizationOutcome::Unavailable;
                }
            }
        }
        // the definition kept changing under us (the same grant, a new revision each time)
        self.forget_unnamed(
            owner,
            &id,
            "a credential kept for a server that kept changing under its authorization",
        )
        .await;
        AuthorizationOutcome::Unavailable
    }
}

/// Whether `now` is the definition `before` was, as far as a credential issued for `before` is
/// concerned: the same URL and the same grant (kind, scopes as a set, client). A rename is.
fn issued_for_the_same(before: &UserToolServer, now: &UserToolServer) -> bool {
    before.url() == now.url() && same_grant(before.auth(), now.auth())
}

/// A failure of the broker at the start of an authorization, as the application says it. A refusal of
/// the egress guard is the person's to be told, as the guard's one public sentence; the cause of any
/// other failure is logged by class and chain (the port promises neither holds a secret or a URL).
fn authorization_failure(error: &CredentialError) -> AppError {
    if let Some(refusal) = error.egress_refusal() {
        return UserToolRefusal::EgressRefused {
            reason: refusal.public_reason(),
        }
        .into();
    }
    match error {
        // the server is gone, or is not an `oauth2` one: someone changed it under the request
        CredentialError::Unknown => UserToolRefusal::NotFound.into(),
        _ => {
            tracing::warn!(
                class = ?error.class(),
                error = %report(error),
                "the broker could not start an OAuth authorization"
            );
            UserToolRefusal::AuthorizationUnavailable.into()
        }
    }
}

/// A failure of the pending authorizations' store, as the application says it: a storage failure, as
/// every store's is.
fn pending_failure(error: PendingAuthorizationError) -> AppError {
    AppError::Store(StoreError::unavailable(error))
}
