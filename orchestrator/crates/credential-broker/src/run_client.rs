//! The orchestrator's client of the broker's run routes (ADR 0084 decisions 1 and 6): `bind` at a
//! run's creation, `unbind` at its end, over `POST /internal/runs` and `DELETE /internal/runs/{run}`
//! (`src/routes.rs` is the other side of the same wire).
//!
//! The broker's address is the deployment's internal one. This client deliberately does **not** go
//! through the crate's egress guard ([`crate::HostLookup`] and the policy that guards people's own
//! MCP servers): the broker is ours, reached on the cluster's internal listener, and its address is
//! configuration. It follows no redirect, times out, and reads nothing of the answer — the routes
//! answer `204`, and a body would only be a refusal.
//!
//! The token is the orchestrator's, in a [`ToolSecret`]: `Debug` on the client and on every error
//! shows no token, no URL and no body.

use std::time::Duration;

use jiff::Timestamp;
use orch_ports::{RunBinding, RunBindingError, RunBindings, RunId, ToolSecret};
use serde::Serialize;
use url::Url;

use crate::BuildError;

/// How long one call to the broker may take. A run does not wait long for a binding it can live
/// without today (S9 makes it park instead), and an unbind that times out is retried by the next
/// sweep's idempotence rather than held.
pub const DEFAULT_BIND_TIMEOUT: Duration = Duration::from_secs(5);

/// The client of the broker's run routes.
pub struct RunBindingClient {
    base: Url,
    token: ToolSecret,
    client: reqwest::Client,
}

/// The bind body, as `src/routes.rs` reads it: camelCase, `expiresAt` in RFC 3339.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BindBody<'a> {
    run: &'a str,
    agent: &'a str,
    person: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<&'a str>,
    expires_at: String,
}

impl RunBindingClient {
    /// A client of the broker at `base` (for example `http://another-agentic-broker.ns.svc:8080`),
    /// presenting `token`.
    ///
    /// # Errors
    /// [`BuildError::BrokerUrl`] when `base` is not an `http(s)` URL with a host, no query and no
    /// fragment; [`BuildError::Http`] when the client does not start.
    pub fn new(base: &str, token: ToolSecret) -> Result<Self, BuildError> {
        Self::with_timeout(base, token, DEFAULT_BIND_TIMEOUT)
    }

    /// [`RunBindingClient::new`] with a timeout of its own.
    ///
    /// # Errors
    /// As [`RunBindingClient::new`].
    pub fn with_timeout(
        base: &str,
        token: ToolSecret,
        timeout: Duration,
    ) -> Result<Self, BuildError> {
        if timeout.is_zero() {
            return Err(BuildError::Config);
        }
        let url = Url::parse(base).map_err(|_| BuildError::BrokerUrl)?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host().is_none()
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(BuildError::BrokerUrl);
        }
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let client = reqwest::Client::builder()
            // A redirect is an address the deployment did not name.
            .redirect(reqwest::redirect::Policy::none())
            .timeout(timeout)
            .connect_timeout(timeout)
            // The broker answers `204`; nothing of an answer is kept.
            .build()
            .map_err(BuildError::Http)?;
        Ok(RunBindingClient {
            base: url,
            token,
            client,
        })
    }

    /// `base` with `path` appended, keeping any path the base already carries.
    fn url(&self, path: &str) -> Url {
        let mut url = self.base.clone();
        let base_path = url.path().trim_end_matches('/').to_owned();
        url.set_path(&format!("{base_path}{path}"));
        url
    }

    fn authorization(&self) -> String {
        format!("Bearer {}", self.token.expose())
    }
}

impl std::fmt::Debug for RunBindingClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunBindingClient")
            .field("base", &self.base.as_str())
            .finish_non_exhaustive()
    }
}

impl RunBindings for RunBindingClient {
    async fn bind(&self, binding: RunBinding) -> Result<(), RunBindingError> {
        let body = BindBody {
            run: binding.run.as_str(),
            agent: &binding.agent.0,
            person: binding.person.as_str(),
            model: binding.model.as_deref(),
            expires_at: binding.expires_at.to_string(),
        };
        let answer = self
            .client
            .post(self.url("/internal/runs"))
            .header(reqwest::header::AUTHORIZATION, self.authorization())
            .json(&body)
            .send()
            .await
            .map_err(|_| RunBindingError::Unreachable)?;
        refuse(answer.status())
    }

    async fn unbind(&self, run: &RunId) -> Result<(), RunBindingError> {
        let answer = self
            .client
            .delete(self.url(&format!("/internal/runs/{}", encode_path(run.as_str()))))
            .header(reqwest::header::AUTHORIZATION, self.authorization())
            .send()
            .await
            .map_err(|_| RunBindingError::Unreachable)?;
        refuse(answer.status())
    }
}

/// One run id as a path segment: percent-encoded, so an id the orchestrator generated with a slash
/// or a space cannot address another route.
fn encode_path(segment: &str) -> String {
    let mut encoded = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// The broker's answer: `204` is done, `409` is a run already bound, anything else is a refusal.
fn refuse(status: reqwest::StatusCode) -> Result<(), RunBindingError> {
    match status {
        reqwest::StatusCode::NO_CONTENT | reqwest::StatusCode::OK => Ok(()),
        reqwest::StatusCode::CONFLICT => Err(RunBindingError::Conflict),
        other => Err(RunBindingError::Refused(other.as_u16())),
    }
}

/// The moment a binding for `run` should end: now, for a test.
#[must_use]
pub fn deadline_in(seconds: i64) -> Timestamp {
    Timestamp::now() + Duration::from_secs(seconds.unsigned_abs())
}
