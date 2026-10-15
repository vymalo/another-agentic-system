//! A person's own MCP servers on the relay (ADR 0046, decisions 4 and 5).
//!
//! The thread's **owner's** `u-` servers, attached with `PUT /api/threads/{id}/tools`, are relayed
//! beside the deployment's through the same [`ToolServerClient`] port, by a client that judges every
//! connection (`McpToolClient::guarded`). What differs from the deployment's servers is everything
//! about trust, and it is all here:
//!
//! ```mermaid
//! sequenceDiagram
//!   autonumber
//!   participant A as Agent
//!   participant R as RelayTools
//!   participant D as Definitions (UserToolServerStore)
//!   participant B as CredentialBroker
//!   participant C as Guarded client
//!   participant S as The person's server
//!
//!   A->>R: tools/list, or tools/call u-…__tool
//!   R->>R: mode() first: off, or no client, serves nothing
//!   R->>D: get/list(owner): the owner still owns it, and it is connected
//!   R->>R: egress.check_url(url): today's lists
//!   R->>B: lease(owner, server): fresh, for this one request, never kept
//!   B-->>R: Bearer token, or a header and its value
//!   R->>C: list_tools / call_tool(endpoint with the lease)
//!   C->>C: check_url, then the resolved address: the guard again
//!   C->>S: the request, no redirect followed
//!   alt the server answers 401 or 403
//!     S-->>C: Unauthenticated
//!     C-->>R: Unauthenticated
//!     R->>D: set_state(needs_reauth) on the revision read
//!     R-->>A: isError "the MCP server needs the person to sign in again" (a failed step)
//!   else it answers
//!     S-->>R: the result
//!     R-->>A: the result, one step
//!   end
//! ```
//!
//! # The rules
//!
//! - **Ask the mode first** ([`UserToolPorts::mode`]): with the feature off the refusing store would
//!   fail even a read (ADR 0046, departure 78), and a thread's `u-` servers are not on the endpoint.
//! - **A fresh lease per `tools/list` and per `tools/call`**, from the broker, and none is kept: not in
//!   the relay, not in a cache, not between two calls of one request. A lease past its end is not
//!   used. A server of a kind that wants no credential (`none`) is connected to with none.
//! - **A server whose credential cannot be had is not offered**: left out of `tools/list` (and logged,
//!   by id and reason), and a `tools/call` that was already attached fails as a `failed` step with
//!   `isError`, never a silent skip. The sentences are [`NEEDS_SIGN_IN`] for a credential that ended
//!   (the person's call to action, not an error of the agent), and a fixed sentence each for a broker
//!   that cannot be reached and for a server the egress guard refuses; none holds a URL, an address or
//!   a secret.
//! - **A `401`, or a `403` that carries a `WWW-Authenticate` challenge** (a scope step-up), from a server
//!   of a kind that holds a credential, sets the definition to `needs_reauth` (`set_state` on the
//!   revision the endpoint was made from: a definition edited since is left as it is), so the picker
//!   shows `needs_auth` / `reauthorize` and the way back is to authorize or key the server again. A
//!   **403 with no challenge** (`ToolServerError::Forbidden`), and a 401 to a server that was added with
//!   no credential, fail the step with their own words and change no state: no sign-in of the person's
//!   would change them.
//! - **A place first** ([`limits`](super::limits)): at most [`MAX_PER_OWNER`](super::MAX_PER_OWNER)
//!   requests to one person's servers and [`MAX_IN_FLIGHT`](super::MAX_IN_FLIGHT) in the process run at
//!   once, a request waits [`PERMIT_WAIT`](super::PERMIT_WAIT) and is `busy` after. The place comes
//!   **before the lease**: a request that waits holds no credential.
//! - **The lease and the URL are of one revision**: the definition is read again after the lease, and a
//!   revision that moved drops the lease and starts over once (`prepare`).
//! - **What a stranger says is untrusted**: a tool's `annotations` are dropped, its `description` is
//!   cut at [`MAX_OWN_DESCRIPTION_BYTES`] (2 KiB) and its `title` at 256 bytes, the icon is none (a
//!   person supplies none), a tool whose input schema is over 64 KiB, or whose name holds the
//!   credential it was lent, or that is past the 200th is left out (counts in the log, never words),
//!   and the step's icon is `mcp-server:<u-id>`, which the web draws as the generic glyph of a tool.
//! - **What the server says back is scrubbed of the credential it was lent.** A server that echoes what
//!   it was sent (a debugging tool that returns the request's headers, an error that quotes the bad
//!   token) would otherwise put the credential into the step's output and detail, the thread's log, the
//!   export, the AG-UI stream and the agent's context. Every place the relay passes a stranger's words
//!   on (a result, a JSON-RPC error's message, a listed tool's title, description and schemas) has the
//!   exact value of each secret it sent replaced by `[redacted]` ([`scrub_output`]). Only the exact
//!   value, and only a value of at least 8 bytes (a shorter one would redact ordinary words, so the
//!   add API refuses to keep one and a lease of fewer bytes is not lent): a server that encodes the
//!   credential first (base64, hashed) is not caught, and nothing could catch it. A result cut at
//!   256 KiB loses a beginning of a credential at the end of its text.
//! - **Re-checked at each call**: that the server is still attached to the thread, that its owner
//!   still owns it, that the feature is still on. A server detached or deleted after the agent listed
//!   its tools is not on the endpoint any more (`-32602`, no step), as for the deployment's.
//!
//! A revoked `tools.user.add` does not stop a call here: the relay has the owner's identity from the
//! thread and not their roles. It is the application that takes the owner's servers off the thread at
//! the next message or job the person starts (`App::post_message`, `submit`, a fork), so the "still
//! attached" check above then refuses the call; a job already running keeps its servers until it ends
//! (ADR 0046, slice 5a review). `userToolServers.enabled: false` stops every call at once.
//!
//! Nothing here puts a secret in a step, an error or a log line: the lease is moved into the endpoint
//! (a [`ToolSecret`](orch_ports::ToolSecret), which prints `<redacted>`), the broker's errors are
//! logged by class and chain (the port promises neither holds a secret), and a server's own words are
//! its own.

use std::time::Duration;

use futures::future::join_all;
use orch_core::{Classify, UserId, report};
use orch_ports::{
    Clock, CredentialBroker, CredentialError, CredentialLease, MAX_USER_SERVER_TOOLS,
    MIN_SCRUBBABLE_SECRET_BYTES, Ports, ServerRevision, StoredServer, ToolCall, ToolServerClient,
    ToolServerEndpoint, ToolServerError, UnavailableReason, UserToolPorts, UserToolServerId,
    UserToolServerState, UserToolServerStore,
};
use orch_ports::{ToolCallOutput, ToolDef, ToolSecret};
use rmcp::ErrorData;
use rmcp::model::{CallToolResult, JsonObject, Tool};
use serde_json::{Map, Value};

use super::limits::Busy;
use super::{
    CLIENT_GRACE, Ended, Live, Presented, RelayTools, answered, canceled, definition, error_result,
    failure, finish, first_line, relayed_name,
};
use crate::ToolCtx;
use orch_core::StepState;

/// What the agent is told when a call cannot be made because the person's credential ended, and
/// what the step's `detail` says: the person's call to action, and no secret.
pub const NEEDS_SIGN_IN: &str = "the MCP server needs the person to sign in again";

/// The longest one call to a person's server may take, the lease included, before the relay gives
/// up (a person cannot set it yet): 60 seconds, plus [`TIMEOUT_MARGIN`](super::TIMEOUT_MARGIN) in the
/// tool's `timeoutSecs`.
pub const USER_SERVER_TIMEOUT: Duration = Duration::from_secs(60);

/// Why a server of a person's cannot be used for this request. Not an error type: each case is a
/// decision of the relay with its own words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cannot {
    /// It is not on the endpoint: the owner has no such server (deleted), or the feature is off.
    Gone,
    /// Its credential ended or was never given: the person signs in again.
    NeedsSignIn,
    /// The credential could not be had now (the broker or the secret store behind it is down, the
    /// definitions cannot be read, or this deployment keeps none).
    BrokerDown,
    /// Today's egress lists refuse the server, or the broker's guard refused a URL of its own.
    EgressRefused,
    /// There was no place for the request within the wait: the process, or this person, has as
    /// many requests to people's servers in flight as it is allowed ([`limits`](super::limits)).
    Busy,
    /// The server answered 403 with no challenge, or 401 to a server that was added with no
    /// credential: it will not do it, and no sign-in of the person's changes that.
    Refused,
    /// The server wants credentials and was added with none (a 401 to a server of the kind `none`).
    WantsCredentials,
}

impl Cannot {
    /// The name the operator's log gives the reason: those of the picker where there is one.
    const fn reason(self) -> &'static str {
        match self {
            Cannot::Gone => "gone",
            Cannot::NeedsSignIn => "needs_reauth",
            Cannot::BrokerDown => UnavailableReason::BrokerDown.as_str(),
            Cannot::EgressRefused => UnavailableReason::EgressRefused.as_str(),
            Cannot::Busy => "busy",
            Cannot::Refused => "refused",
            Cannot::WantsCredentials => "wants_credentials",
        }
    }
}

/// A server ready to be called, with the lease in its endpoint.
struct Prepared {
    endpoint: ToolServerEndpoint,
    revision: ServerRevision,
    /// Whether the definition's kind holds a credential (every kind but `none`): only a refusal of
    /// a credential can be a credential that ended.
    holds_credential: bool,
}

/// What one pass of [`prepare`](RelayTools::prepare) came to.
enum Leased {
    Ready(Prepared),
    /// The definition's revision changed between the read and the lease.
    Moved,
}

/// How a call failed.
enum Failed {
    Cannot(Cannot),
    Server(ToolServerError),
    /// The server refused the credential (a 401, or a 403 that carries a challenge).
    Unauthenticated {
        holds_credential: bool,
        /// The revision of the definition the credential was leased for.
        revision: ServerRevision,
    },
}

/// The words for a call that was not made, as a failed step. The step's `detail` is the first line.
fn unobtainable(name: &str, why: Cannot) -> Ended {
    let message = match why {
        Cannot::NeedsSignIn => NEEDS_SIGN_IN.to_owned(),
        Cannot::BrokerDown => format!(
            "the credential for the MCP server '{name}' could not be had now; the call did not run"
        ),
        Cannot::EgressRefused => {
            format!("the MCP server '{name}' is not reachable from here; the call did not run")
        }
        Cannot::Gone => format!("the MCP server '{name}' is no longer there; the call did not run"),
        Cannot::Busy => format!("the MCP server '{name}' is busy; the call did not run, try again"),
        Cannot::Refused => {
            format!("the MCP server '{name}' refused the call; the call did not run")
        }
        Cannot::WantsCredentials => format!(
            "the MCP server '{name}' wants credentials and was added with none; the call did not run"
        ),
    };
    Ended {
        detail: Some(first_line(&message)),
        result: error_result(message),
        state: StepState::Failed,
        output: None,
    }
}

impl<P: Ports, C: ToolServerClient> RelayTools<P, C> {
    /// The tools of the thread owner's own servers attached to `record`, for `tools/list`. A server
    /// that cannot be used is left out, and the log says which and why.
    pub(super) async fn list_own(
        &self,
        ctx: &ToolCtx,
        record: &orch_core::ThreadRecord,
    ) -> Vec<Tool> {
        let attached: Vec<&str> = record
            .job
            .tools
            .iter()
            .map(String::as_str)
            .filter(|id| self.is_own(id))
            .collect();
        if attached.is_empty() {
            return Vec::new();
        }
        let ports = self.app.user_tools();
        // the mode first: with the feature off the store refuses even a read
        if !ports.mode().is_enabled() {
            tracing::info!(
                attached = attached.len(),
                "people's own servers are off here: the servers of a person's attached to the thread are left out"
            );
            return Vec::new();
        }
        let Some(client) = &self.own else {
            tracing::warn!(
                attached = attached.len(),
                "this relay has no client for people's own servers: those attached to the thread are left out"
            );
            return Vec::new();
        };
        let rows = match ports.servers().list(&ctx.owner).await {
            Ok(rows) => rows,
            Err(error) => {
                tracing::warn!(
                    class = ?error.class(),
                    error = %report(&error),
                    "the definitions of a person's own servers could not be read: those attached to the thread are left out"
                );
                return Vec::new();
            }
        };
        let listed = join_all(attached.iter().map(|id| {
            let stored = rows.iter().find(|row| row.server().id().as_str() == *id);
            async move {
                match stored {
                    Some(stored) => self.list_one_own(ctx, client, stored.clone()).await,
                    None => {
                        tracing::info!(
                            server = id,
                            reason = Cannot::Gone.reason(),
                            "a server of a person's attached to the thread is left out"
                        );
                        Vec::new()
                    }
                }
            }
        }))
        .await;
        listed.into_iter().flatten().collect()
    }

    /// The tools of one server of a person's: a place among the requests in flight, its definition,
    /// a fresh lease, the server's listing.
    async fn list_one_own(&self, ctx: &ToolCtx, client: &C, stored: StoredServer) -> Vec<Tool> {
        let id = stored.server().id().clone();
        let name = stored.server().name().to_owned();
        // a place first: a request that waits holds no credential
        let Ok(_places) = self.limits.acquire(&ctx.owner).await else {
            self.left_out(&id, Cannot::Busy);
            return Vec::new();
        };
        let prepared = match self.prepare(&ctx.owner, stored).await {
            Ok(prepared) => prepared,
            Err(why) => {
                self.left_out(&id, why);
                return Vec::new();
            }
        };
        let secrets = secrets_of(&prepared.endpoint);
        let mut upstream = match client.list_tools(&prepared.endpoint).await {
            Ok(upstream) => upstream,
            // only a refused credential is a credential that ended
            Err(ToolServerError::Unauthenticated) if prepared.holds_credential => {
                self.mark_needs_reauth(&ctx.owner, &id, prepared.revision)
                    .await;
                self.left_out(&id, Cannot::NeedsSignIn);
                return Vec::new();
            }
            Err(ToolServerError::Unauthenticated) => {
                self.left_out(&id, Cannot::WantsCredentials);
                return Vec::new();
            }
            Err(ToolServerError::Forbidden) => {
                self.left_out(&id, Cannot::Refused);
                return Vec::new();
            }
            Err(error) => {
                // No URL, no credential, no transport error, and none of the server's words.
                tracing::warn!(
                    server = %id,
                    reason = loggable(&error),
                    rule = refusal_rule(&error),
                    "a server of a person's could not be listed: its tools are left out"
                );
                return Vec::new();
            }
        };
        // what a tool is called is the stranger's word too: one named with the credential it was
        // lent is not offered (the name is what the agent calls it by, so it cannot be redacted)
        let listed = upstream.len();
        upstream.retain(|tool| !holds_secret(&tool.name, &secrets));
        for tool in &mut upstream {
            scrub_tool(tool, &secrets);
        }
        let presented = Presented {
            id: id.as_str(),
            name: &name,
            icon: None,
            timeout: USER_SERVER_TIMEOUT,
            trusted: false,
        };
        let mut seen: Vec<&str> = Vec::new();
        let mut tools = Vec::new();
        for tool in &upstream {
            if seen.contains(&tool.name.as_str()) {
                continue;
            }
            if tools.len() >= MAX_USER_SERVER_TOOLS {
                break;
            }
            // `None`: a name that does not fit, or a schema over the limit
            if let Some(relayed) = definition(&presented, tool) {
                seen.push(&tool.name);
                tools.push(relayed);
            }
        }
        // counts only: what is left out is the stranger's choice of words
        let left_out = listed - tools.len();
        if left_out > 0 {
            tracing::warn!(
                server = %id,
                listed,
                left_out,
                "tools of a server of a person's were left out: a name or a schema that does not fit, a copy of the credential, a duplicate or more than the limit"
            );
        }
        tools
    }

    /// A call of `name` (`<u-id>__<tool>`), when `server_id` is one of the thread owner's own
    /// servers attached to the thread; `None` when the name is not on the endpoint.
    pub(super) async fn call_own(
        &self,
        ctx: &ToolCtx,
        name: &str,
        server_id: &str,
        tool: &str,
        args: JsonObject,
    ) -> Option<Result<CallToolResult, ErrorData>> {
        let id = UserToolServerId::parse(server_id).ok()?;
        let ports = self.app.user_tools();
        // the mode first, and a relay built without the client serves none
        if !ports.mode().is_enabled() {
            return None;
        }
        let client = self.own.as_ref()?;
        if relayed_name(server_id, tool).as_deref() != Some(name) {
            return None;
        }
        match self.live_thread(ctx, server_id).await {
            Live::Ours => {}
            Live::NotOurs => return None,
            Live::Answer(result) => return Some(Ok(result)),
        }
        // the owner still owns it: a server deleted since the agent listed its tools is not on the
        // endpoint any more
        let stored = match ports.servers().get(&ctx.owner, &id).await {
            Ok(Some(stored)) => stored,
            Ok(None) => return None,
            Err(error) => {
                tracing::warn!(
                    class = ?error.class(),
                    error = %report(&error),
                    "the definition of a person's own server could not be read"
                );
                // Attached, so the agent was offered it: the call fails as a step, named by its id
                // (the name is in the definition that cannot be read).
                let step = self
                    .start_step(ctx, id.as_str(), id.as_str(), tool, &args)
                    .await;
                let ended = unobtainable(id.as_str(), Cannot::BrokerDown);
                return Some(Ok(finish(step, ended).await));
            }
        };
        Some(Ok(self
            .relay_own(ctx, client, &id, stored, tool, args)
            .await))
    }

    /// The call, as one step: a place, the lease, the request, the end of the step. The place and
    /// the lease are taken inside the call's bound and its cancellation, so a wait for either costs
    /// the call's time and a caller that gives up does not wait for it.
    async fn relay_own(
        &self,
        ctx: &ToolCtx,
        client: &C,
        id: &UserToolServerId,
        stored: StoredServer,
        tool: &str,
        args: JsonObject,
    ) -> CallToolResult {
        let name = stored.server().name().to_owned();
        let step = self.start_step(ctx, id.as_str(), &name, tool, &args).await;
        let call = ToolCall::new(tool).with_arguments(args);
        let bound = USER_SERVER_TIMEOUT + CLIENT_GRACE;
        let work = async {
            // the place first, so that a call that waits holds no credential
            let _places = self
                .limits
                .acquire(&ctx.owner)
                .await
                .map_err(|Busy| Failed::Cannot(Cannot::Busy))?;
            let prepared = self
                .prepare(&ctx.owner, stored)
                .await
                .map_err(Failed::Cannot)?;
            let secrets = secrets_of(&prepared.endpoint);
            match tokio::time::timeout(bound, client.call_tool(&prepared.endpoint, &call)).await {
                Ok(Ok(mut output)) => {
                    scrub_output(&mut output, &secrets);
                    Ok(output)
                }
                Ok(Err(ToolServerError::Unauthenticated)) => Err(Failed::Unauthenticated {
                    holds_credential: prepared.holds_credential,
                    revision: prepared.revision,
                }),
                Ok(Err(error)) => Err(Failed::Server(scrub_error(error, &secrets))),
                Err(_) => Err(Failed::Server(ToolServerError::TimedOut {
                    after: USER_SERVER_TIMEOUT,
                })),
            }
        };
        let ended = tokio::select! {
            () = ctx.cancel.cancelled() => canceled(),
            outcome = tokio::time::timeout(bound, work) => match outcome {
                Ok(Ok(output)) => answered(output),
                Ok(Err(Failed::Cannot(why))) => {
                    self.left_out(id, why);
                    unobtainable(&name, why)
                }
                // a refused credential is a credential that ended, for a kind that holds one
                Ok(Err(Failed::Unauthenticated { holds_credential: true, revision })) => {
                    self.mark_needs_reauth(&ctx.owner, id, revision).await;
                    // For the operator: the server's id, never a credential.
                    tracing::warn!(
                        server = %id,
                        "a server of a person's refused the credential: it needs the person to sign in again"
                    );
                    unobtainable(&name, Cannot::NeedsSignIn)
                }
                // a 401 to a server added with no credential: nothing to sign in again
                Ok(Err(Failed::Unauthenticated { holds_credential: false, .. })) => {
                    self.left_out(id, Cannot::WantsCredentials);
                    unobtainable(&name, Cannot::WantsCredentials)
                }
                // a 403 with no challenge: the server will not do it, and no state changes
                Ok(Err(Failed::Server(ToolServerError::Forbidden))) => {
                    self.left_out(id, Cannot::Refused);
                    unobtainable(&name, Cannot::Refused)
                }
                Ok(Err(Failed::Server(error))) => {
                    let rule = refusal_rule(&error);
                    if !rule.is_empty() {
                        tracing::warn!(server = %id, rule, "the egress guard refused a call to a server of a person's");
                    }
                    failure(&name, &error)
                }
                Err(_) => failure(
                    &name,
                    &ToolServerError::TimedOut { after: USER_SERVER_TIMEOUT },
                ),
            },
        };
        finish(step, ended).await
    }

    /// Whether `id` is a server of a person's own for this relay: not one the deployment lists, and
    /// shaped like one of the person's.
    fn is_own(&self, id: &str) -> bool {
        id.starts_with(UserToolServerId::PREFIX) && !self.servers.iter().any(|s| s.info.id == id)
    }

    /// The server as it is called now: its state, today's egress lists, and a **fresh lease** in the
    /// endpoint. `first` is the definition just read.
    ///
    /// **The lease and the URL belong to one revision.** The URL is read from the definition and the
    /// credential is leased by the broker from the definition as *it* reads it: a person who edits
    /// the URL, or re-keys the server, between the two would have the credential of one revision
    /// sent to the URL of another. So the definition is read again after the lease and must have the
    /// revision the endpoint was made from. If it moved, the lease is dropped (it is wiped) and the
    /// whole is done once more on what was read; if it moves again, the call is not made
    /// (`could not be had now`).
    async fn prepare(&self, owner: &UserId, first: StoredServer) -> Result<Prepared, Cannot> {
        let id = first.server().id().clone();
        let mut stored = first;
        for attempt in 0..2 {
            if let Leased::Ready(prepared) = self.prepare_once(owner, &stored).await? {
                return Ok(prepared);
            }
            tracing::info!(
                server = %id,
                attempt = attempt + 1,
                "the definition of a person's server moved while its credential was leased: the lease is dropped"
            );
            if attempt == 0 {
                stored = match self.app.user_tools().servers().get(owner, &id).await {
                    Ok(Some(now)) => now,
                    Ok(None) => return Err(Cannot::Gone),
                    Err(error) => {
                        tracing::warn!(
                            class = ?error.class(),
                            error = %report(&error),
                            "the definition of a person's own server could not be read"
                        );
                        return Err(Cannot::BrokerDown);
                    }
                };
            }
        }
        tracing::warn!(server = %id, "the definition of a person's server moved twice while its credential was leased: the call is not made");
        Err(Cannot::BrokerDown)
    }

    /// One pass of [`prepare`](Self::prepare) over the definition `stored`.
    async fn prepare_once(&self, owner: &UserId, stored: &StoredServer) -> Result<Leased, Cannot> {
        let ports = self.app.user_tools();
        let mode = ports.mode();
        let server = stored.server();
        // a credential that is not there is not offered, and the broker is not asked about it
        if !server.state().is_offered() {
            return Err(Cannot::NeedsSignIn);
        }
        let wants_credential = server.auth().needs_credential();
        if wants_credential && !mode.keeps_credentials() {
            return Err(Cannot::BrokerDown);
        }
        // today's lists, before a token is refreshed for a server that will not be connected to
        if ports.egress().check_url(server.url()).is_err() {
            return Err(Cannot::EgressRefused);
        }
        let endpoint =
            ToolServerEndpoint::new(server.id().as_str(), server.url(), USER_SERVER_TIMEOUT);
        let revision = stored.revision();
        if !wants_credential {
            return Ok(Leased::Ready(Prepared {
                endpoint,
                revision,
                holds_credential: false,
            }));
        }
        let id = server.id().clone();
        let lease = match ports.broker().lease(owner, &id).await {
            Ok(lease) => lease,
            Err(CredentialError::NeedsReauth) => {
                // The broker learned it when it can (a refresh refused); a credential that is gone
                // while the definition says `connected` is marked here, so the picker agrees.
                self.mark_needs_reauth(owner, &id, revision).await;
                return Err(Cannot::NeedsSignIn);
            }
            Err(CredentialError::Unknown) => return Err(Cannot::Gone),
            Err(error) => {
                let refusal = error.egress_refusal();
                tracing::warn!(
                    server = %id,
                    class = ?error.class(),
                    rule = refusal.map_or("", |r| r.rule()),
                    error = %report(&error),
                    "the credential broker could not lend a credential"
                );
                return Err(if refusal.is_some() {
                    Cannot::EgressRefused
                } else {
                    Cannot::BrokerDown
                });
            }
        };
        // a lease is short and the broker gives a fresh one; one that has ended is not used
        if lease.is_expired_at(self.app.ports().clock().now()) {
            tracing::warn!(server = %id, "the credential broker lent a credential that had ended");
            return Err(Cannot::BrokerDown);
        }
        // A credential shorter than the shortest the relay can take out of a server's words would
        // be said back whole and not found: the add API refuses to keep one, and one that comes
        // from elsewhere (an OAuth token) is not lent.
        let lent = match &lease {
            CredentialLease::Bearer { token, .. } => token.expose().len(),
            CredentialLease::Header { value, .. } => value.expose().len(),
        };
        if lent < MIN_SCRUBBABLE_SECRET_BYTES {
            tracing::warn!(server = %id, "the credential broker lent a credential too short to be kept out of what a server says back");
            return Err(Cannot::BrokerDown);
        }
        let endpoint = match lease {
            CredentialLease::Bearer { token, .. } => endpoint.with_bearer(token),
            CredentialLease::Header { name, value, .. } => endpoint.with_header(name, value),
        };
        // the definition again: the lease was made for the revision the URL was read from, or it is
        // not used (dropping `endpoint` wipes the credential)
        match ports.servers().get(owner, &id).await {
            Ok(Some(now)) if now.revision() == revision => Ok(Leased::Ready(Prepared {
                endpoint,
                revision,
                holds_credential: true,
            })),
            Ok(Some(_)) => Ok(Leased::Moved),
            Ok(None) => Err(Cannot::Gone),
            Err(error) => {
                tracing::warn!(
                    class = ?error.class(),
                    error = %report(&error),
                    "the definition of a person's own server could not be read"
                );
                Err(Cannot::BrokerDown)
            }
        }
    }

    /// The server answered that the credential is not accepted (or the broker says it ended): its
    /// state becomes `needs_reauth`, on the revision just read. A definition edited, re-keyed or
    /// deleted since is left as it is: the state of a server that has changed is not this request's
    /// to write.
    async fn mark_needs_reauth(
        &self,
        owner: &UserId,
        id: &UserToolServerId,
        revision: ServerRevision,
    ) {
        let servers = self.app.user_tools().servers();
        if let Err(error) = servers
            .set_state(owner, id, UserToolServerState::NeedsReauth, revision)
            .await
        {
            tracing::debug!(
                server = %id,
                class = ?error.class(),
                "a server's state was not set to needs_reauth: it changed meanwhile or the store is down"
            );
        }
    }

    /// Says in the log that a server of a person's was left out of a listing, and why.
    fn left_out(&self, id: impl std::fmt::Display, why: Cannot) {
        match why {
            // the person's call to action, or a server that is not there: expected
            Cannot::NeedsSignIn | Cannot::Gone => tracing::info!(
                server = %id,
                reason = why.reason(),
                "a server of a person's attached to the thread is not offered"
            ),
            Cannot::BrokerDown
            | Cannot::EgressRefused
            | Cannot::Busy
            | Cannot::Refused
            | Cannot::WantsCredentials => tracing::warn!(
                server = %id,
                reason = why.reason(),
                "a server of a person's attached to the thread is not offered"
            ),
        }
    }
}

/// The rule of an egress refusal, for the operator's log; empty for any other error.
fn refusal_rule(error: &ToolServerError) -> &'static str {
    match error {
        ToolServerError::EgressRefused(rule) => rule.rule(),
        _ => "",
    }
}

/// What replaces a credential in a server's words (the marker the step's input uses too).
const REDACTED: &str = "[redacted]";

/// The secrets an endpoint sends: its bearer and the values of its headers. None is shorter than
/// [`MIN_SCRUBBABLE_SECRET_BYTES`]: the add API refuses to keep one and `prepare` refuses to lend
/// one. Clones are wiped when they are dropped, as the originals are.
fn secrets_of(endpoint: &ToolServerEndpoint) -> Vec<ToolSecret> {
    endpoint
        .bearer
        .iter()
        .chain(endpoint.headers.iter().map(|(_, value)| value))
        .cloned()
        .collect()
}

/// Whether `text` holds the exact value of one of the secrets.
fn holds_secret(text: &str, secrets: &[ToolSecret]) -> bool {
    secrets.iter().any(|secret| text.contains(secret.expose()))
}

/// `text` with the exact value of each secret replaced.
fn scrub_text(text: &mut String, secrets: &[ToolSecret]) {
    for secret in secrets {
        if text.contains(secret.expose()) {
            *text = text.replace(secret.expose(), REDACTED);
        }
    }
}

/// Every string of `value`, and every key of an object, scrubbed.
fn scrub_value(value: &mut Value, secrets: &[ToolSecret]) {
    match value {
        Value::String(text) => scrub_text(text, secrets),
        Value::Array(items) => items.iter_mut().for_each(|item| scrub_value(item, secrets)),
        Value::Object(map) => scrub_map(map, secrets),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn scrub_map(map: &mut Map<String, Value>, secrets: &[ToolSecret]) {
    let scrubbed: Map<String, Value> = std::mem::take(map)
        .into_iter()
        .map(|(mut key, mut value)| {
            scrub_text(&mut key, secrets);
            scrub_value(&mut value, secrets);
            (key, value)
        })
        .collect();
    *map = scrubbed;
}

/// A result with the credential taken out of its content and its structured part.
///
/// The port hands a result back already cut at 256 KiB, so the relay cannot scrub before the cut,
/// and a credential that straddles it is only its beginning at the end of a text block, which
/// [`scrub_text`] cannot see. A result that was cut therefore also loses, from the end of each text
/// block, whatever is a **beginning** of a credential.
fn scrub_output(output: &mut ToolCallOutput, secrets: &[ToolSecret]) {
    for block in &mut output.content {
        scrub_value(block, secrets);
    }
    if let Some(structured) = &mut output.structured {
        scrub_value(structured, secrets);
    }
    if output.truncated {
        for block in &mut output.content {
            if let Some(Value::String(text)) = block.get_mut("text") {
                trim_partial(text, secrets);
            }
        }
    }
}

/// `text` without a proper beginning of one of the secrets at its end (the longest that is there).
fn trim_partial(text: &mut String, secrets: &[ToolSecret]) {
    for secret in secrets {
        let value = secret.expose();
        // the proper prefixes of the secret, longest first, at character boundaries
        let ends = value
            .char_indices()
            .map(|(i, _)| i)
            .skip(1)
            .collect::<Vec<_>>();
        if let Some(end) = ends
            .iter()
            .rev()
            .find(|end| text.ends_with(&value[..**end]))
        {
            text.truncate(text.len() - end);
        }
    }
}

/// A tool as the server listed it, with the credential taken out of the words it chose.
fn scrub_tool(tool: &mut ToolDef, secrets: &[ToolSecret]) {
    if let Some(title) = &mut tool.title {
        scrub_text(title, secrets);
    }
    if let Some(description) = &mut tool.description {
        scrub_text(description, secrets);
    }
    scrub_map(&mut tool.input_schema, secrets);
    if let Some(schema) = &mut tool.output_schema {
        scrub_map(schema, secrets);
    }
    if let Some(annotations) = &mut tool.annotations {
        scrub_value(annotations, secrets);
    }
}

/// An error of the server with the credential taken out of the message it chose.
fn scrub_error(error: ToolServerError, secrets: &[ToolSecret]) -> ToolServerError {
    match error {
        ToolServerError::Remote { code, mut message } => {
            scrub_text(&mut message, secrets);
            ToolServerError::Remote { code, message }
        }
        other => other,
    }
}

/// What the operator's log says of an error of a person's server: the words of `public_detail`, but
/// never a stranger's own (a JSON-RPC message may quote anything).
fn loggable(error: &ToolServerError) -> String {
    match error {
        ToolServerError::Remote { code, .. } => {
            format!("the server answered a JSON-RPC error ({code})")
        }
        other => other.public_detail(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use serde_json::json;

    use super::*;

    fn secrets(values: &[&str]) -> Vec<ToolSecret> {
        values.iter().map(|v| ToolSecret::new(*v)).collect()
    }

    #[test]
    fn the_exact_value_is_replaced_wherever_it_is_in_the_json() {
        let secrets = secrets(&["sentinel-token-123456", "key-abcdefgh"]);
        let mut value = json!({
            "text": "you sent Bearer sentinel-token-123456 and key-abcdefgh, twice: sentinel-token-123456",
            "nested": [{"deep": "x sentinel-token-123456"}, 7, null, true],
            "sentinel-token-123456": "a key that is the secret",
        });
        scrub_value(&mut value, &secrets);
        let shown = value.to_string();
        assert!(
            !shown.contains("sentinel-token-123456") && !shown.contains("key-abcdefgh"),
            "{shown}"
        );
        assert_eq!(
            value["text"],
            "you sent Bearer [redacted] and [redacted], twice: [redacted]"
        );
        assert_eq!(value["nested"][0]["deep"], "x [redacted]");
        assert_eq!(value["[redacted]"], "a key that is the secret");
        assert_eq!(value["nested"][1], 7, "what is not text is left alone");
    }

    #[test]
    fn every_secret_an_endpoint_sends_is_one_to_scrub() {
        let endpoint = ToolServerEndpoint::new(
            "u-abcdefghij",
            "https://x.example/mcp",
            Duration::from_secs(1),
        )
        .with_bearer(ToolSecret::new("long-enough-bearer"))
        .with_header("X-Key", ToolSecret::new("long-enough-key"));
        let kept = secrets_of(&endpoint);
        assert_eq!(kept.len(), 2);
        let mut text = "a word, long-enough-bearer and long-enough-key".to_owned();
        scrub_text(&mut text, &kept);
        assert_eq!(text, "a word, [redacted] and [redacted]");
        assert!(holds_secret("long-enough-key", &kept));
        assert!(holds_secret("a_long-enough-bearer_tool", &kept));
        assert!(!holds_secret("echo", &kept));
    }

    #[test]
    fn a_beginning_of_a_secret_at_the_end_of_a_cut_text_is_taken_out_and_only_there() {
        let secrets = secrets(&["sentinel-token-123456"]);
        let cut = |text: &str, truncated: bool| {
            let mut output = ToolCallOutput {
                content: vec![json!({"type": "text", "text": text})],
                truncated,
                ..ToolCallOutput::default()
            };
            scrub_output(&mut output, &secrets);
            output.text_of()
        };
        // the cut fell inside the credential: only its beginning is left at the end
        assert_eq!(cut("some words sentinel-tok", true), "some words ");
        assert_eq!(cut("some words s", true), "some words ");
        // a whole one is replaced as ever, and a text that merely ends like it does not lose more
        assert_eq!(
            cut("x sentinel-token-123456 sentinel-t", true),
            "x [redacted] "
        );
        // a result that was not cut is a result: its last word is its own
        assert_eq!(
            cut("it ends with sentinel-tok", false),
            "it ends with sentinel-tok"
        );
        // a beginning that is not the end of the text stays
        assert_eq!(
            cut("sentinel-tok then more", true),
            "sentinel-tok then more"
        );
    }

    #[test]
    fn a_beginning_is_taken_out_at_a_character_boundary() {
        let secrets = secrets(&["clé-secrète-123"]);
        let mut text = "voici clé-sec".to_owned();
        trim_partial(&mut text, &secrets);
        assert_eq!(text, "voici ");
        // the cut inside the multi-byte character of the secret leaves a text that is not a prefix
        let mut text = "voici cl".to_owned();
        trim_partial(&mut text, &secrets);
        assert_eq!(text, "voici ");
    }

    #[test]
    fn a_remote_message_and_a_result_and_a_listed_tool_are_scrubbed_and_other_errors_are_not_touched()
     {
        let secrets = secrets(&["sentinel-token-123456"]);
        let ToolServerError::Remote { message, .. } = scrub_error(
            ToolServerError::remote(-32000, "bad token sentinel-token-123456"),
            &secrets,
        ) else {
            panic!("a remote error stays one");
        };
        assert_eq!(message, "bad token [redacted]");
        assert!(matches!(
            scrub_error(ToolServerError::Unauthenticated, &secrets),
            ToolServerError::Unauthenticated
        ));

        let mut output = ToolCallOutput {
            content: vec![json!({"type": "text", "text": "headers: sentinel-token-123456"})],
            structured: Some(json!({"auth": "sentinel-token-123456"})),
            ..ToolCallOutput::default()
        };
        scrub_output(&mut output, &secrets);
        assert_eq!(output.text_of(), "headers: [redacted]");
        assert_eq!(output.structured, Some(json!({"auth": "[redacted]"})));

        let mut tool = ToolDef {
            name: "t".to_owned(),
            title: Some("sentinel-token-123456".to_owned()),
            description: Some("uses sentinel-token-123456".to_owned()),
            input_schema: json!({"type": "object", "default": "sentinel-token-123456"})
                .as_object()
                .unwrap()
                .clone(),
            output_schema: None,
            annotations: Some(json!({"hint": "sentinel-token-123456"})),
        };
        scrub_tool(&mut tool, &secrets);
        assert!(!format!("{tool:?}").contains("sentinel-token-123456"));
    }

    #[test]
    fn the_log_never_quotes_the_words_of_a_server() {
        let error = ToolServerError::remote(-32000, "your token sentinel-token-123456 is bad");
        let said = loggable(&error);
        assert!(!said.contains("sentinel"), "{said}");
        assert_eq!(
            loggable(&ToolServerError::Unauthenticated),
            "the server refused the credentials"
        );
    }
}
