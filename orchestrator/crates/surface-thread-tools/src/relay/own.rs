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
//! - **A `401` or `403` from the server** sets the definition to `needs_reauth` (`set_state` on the
//!   revision just read: a definition edited since is left as it is), so the picker shows
//!   `needs_auth` / `reauthorize` and the way back is to authorize or key the server again. The client
//!   does not tell a 401 from a 403 (`ToolServerError::Unauthenticated`), and a scope step-up is a 403
//!   (ADR 0046, state diagram), so both do.
//! - **What a stranger says is untrusted**: a tool's `annotations` are dropped, its `description` is
//!   cut at [`MAX_OWN_DESCRIPTION_BYTES`] (2 KiB), the icon is none (a person supplies none), and the
//!   step's icon is `mcp-server:<u-id>`, which the web draws as the generic glyph of a tool.
//! - **What the server says back is scrubbed of the credential it was lent.** A server that echoes what
//!   it was sent (a debugging tool that returns the request's headers, an error that quotes the bad
//!   token) would otherwise put the credential into the step's output and detail, the thread's log, the
//!   export, the AG-UI stream and the agent's context. Every place the relay passes a stranger's words
//!   on (a result, a JSON-RPC error's message, a listed tool's title, description and schemas) has the
//!   exact value of each secret it sent replaced by `[redacted]` ([`scrub_output`]). Only the exact
//!   value, and only a value of at least 8 bytes (a shorter one would redact ordinary words): a server
//!   that encodes the credential first (base64, hashed) is not caught, and nothing could catch it.
//! - **Re-checked at each call**: that the server is still attached to the thread, that its owner
//!   still owns it, that the feature is still on. A server detached or deleted after the agent listed
//!   its tools is not on the endpoint any more (`-32602`, no step), as for the deployment's.
//!
//! A revoked `tools.user.add` does not stop a call here: the relay has the owner's identity from the
//! thread and not their roles (ADR 0046, consequences); it stops an attach, and the picker says
//! `not_permitted`. `userToolServers.enabled: false` stops every call at once.
//!
//! Nothing here puts a secret in a step, an error or a log line: the lease is moved into the endpoint
//! (a [`ToolSecret`](orch_ports::ToolSecret), which prints `<redacted>`), the broker's errors are
//! logged by class and chain (the port promises neither holds a secret), and a server's own words are
//! its own.

use std::time::Duration;

use futures::future::join_all;
use orch_core::{Classify, UserId, report};
use orch_ports::{
    Clock, CredentialBroker, CredentialError, CredentialLease, Ports, ServerRevision, StoredServer,
    ToolCall, ToolServerClient, ToolServerEndpoint, ToolServerError, UnavailableReason,
    UserToolPorts, UserToolServerId, UserToolServerState, UserToolServerStore,
};
use orch_ports::{ToolCallOutput, ToolDef, ToolSecret};
use rmcp::ErrorData;
use rmcp::model::{CallToolResult, JsonObject, Tool};
use serde_json::{Map, Value};

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
}

impl Cannot {
    /// The name the operator's log gives the reason: those of the picker where there is one.
    const fn reason(self) -> &'static str {
        match self {
            Cannot::Gone => "gone",
            Cannot::NeedsSignIn => "needs_reauth",
            Cannot::BrokerDown => UnavailableReason::BrokerDown.as_str(),
            Cannot::EgressRefused => UnavailableReason::EgressRefused.as_str(),
        }
    }
}

/// A server ready to be called, with the lease in its endpoint.
struct Prepared {
    endpoint: ToolServerEndpoint,
    revision: ServerRevision,
}

/// How a call failed.
enum Failed {
    Cannot(Cannot),
    Server(ToolServerError),
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

    /// The tools of one server of a person's: its definition, a fresh lease, the server's listing.
    async fn list_one_own(&self, ctx: &ToolCtx, client: &C, stored: StoredServer) -> Vec<Tool> {
        let id = stored.server().id().clone();
        let name = stored.server().name().to_owned();
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
            Err(error) => {
                if matches!(error, ToolServerError::Unauthenticated) {
                    self.mark_needs_reauth(&ctx.owner, &id, prepared.revision)
                        .await;
                    self.left_out(&id, Cannot::NeedsSignIn);
                } else {
                    // No URL, no credential, no transport error, and none of the server's words.
                    tracing::warn!(
                        server = %id,
                        reason = loggable(&error),
                        rule = refusal_rule(&error),
                        "a server of a person's could not be listed: its tools are left out"
                    );
                }
                return Vec::new();
            }
        };
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
            match definition(&presented, tool) {
                Some(relayed) => {
                    seen.push(&tool.name);
                    tools.push(relayed);
                }
                None => tracing::warn!(
                    server = %id,
                    tool = %tool.name,
                    "a tool whose relayed name does not fit is left out"
                ),
            }
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

    /// The call, as one step: the lease, the request, the end of the step. The lease is taken inside
    /// the call's bound and its cancellation, so a refresh that is slow costs the call's time and a
    /// caller that gives up does not wait for it.
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
        let revision = stored.revision();
        let step = self.start_step(ctx, id.as_str(), &name, tool, &args).await;
        let call = ToolCall::new(tool).with_arguments(args);
        let bound = USER_SERVER_TIMEOUT + CLIENT_GRACE;
        let work = async {
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
                Ok(Err(Failed::Server(ToolServerError::Unauthenticated))) => {
                    self.mark_needs_reauth(&ctx.owner, id, revision).await;
                    // For the operator: the server's id, never a credential.
                    tracing::warn!(
                        server = %id,
                        "a server of a person's refused the credential: it needs the person to sign in again"
                    );
                    unobtainable(&name, Cannot::NeedsSignIn)
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
    /// endpoint. `stored` is the definition just read.
    async fn prepare(&self, owner: &UserId, stored: StoredServer) -> Result<Prepared, Cannot> {
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
            return Ok(Prepared { endpoint, revision });
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
        let endpoint = match lease {
            CredentialLease::Bearer { token, .. } => endpoint.with_bearer(token),
            CredentialLease::Header { name, value, .. } => endpoint.with_header(name, value),
        };
        Ok(Prepared { endpoint, revision })
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
            Cannot::BrokerDown | Cannot::EgressRefused => tracing::warn!(
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

/// The shortest secret that is scrubbed from what a server says: a shorter one would redact ordinary
/// words, and nobody's credential is that short.
const MIN_SCRUBBED_BYTES: usize = 8;

/// What replaces a credential in a server's words (the marker the step's input uses too).
const REDACTED: &str = "[redacted]";

/// The secrets an endpoint sends: its bearer and the values of its headers, those long enough to be
/// scrubbed. Clones are wiped when they are dropped, as the originals are.
fn secrets_of(endpoint: &ToolServerEndpoint) -> Vec<ToolSecret> {
    endpoint
        .bearer
        .iter()
        .chain(endpoint.headers.iter().map(|(_, value)| value))
        .filter(|secret| secret.expose().len() >= MIN_SCRUBBED_BYTES)
        .cloned()
        .collect()
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
fn scrub_output(output: &mut ToolCallOutput, secrets: &[ToolSecret]) {
    for block in &mut output.content {
        scrub_value(block, secrets);
    }
    if let Some(structured) = &mut output.structured {
        scrub_value(structured, secrets);
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
    fn a_secret_too_short_to_tell_from_words_is_not_scrubbed() {
        let endpoint = ToolServerEndpoint::new(
            "u-abcdefghij",
            "https://x.example/mcp",
            Duration::from_secs(1),
        )
        .with_bearer(ToolSecret::new("short"))
        .with_header("X-Key", ToolSecret::new("long-enough-key"));
        let kept = secrets_of(&endpoint);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].expose(), "long-enough-key");
        let mut text = "a short word and long-enough-key".to_owned();
        scrub_text(&mut text, &kept);
        assert_eq!(text, "a short word and [redacted]");
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
