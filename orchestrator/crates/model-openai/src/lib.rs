//! [`ChatModel`] over OpenAI-compatible chat completions endpoints (ADR 0005, ADR 0035).
//!
//! One JSON `POST {base}/chat/completions` with `stream: false`, and the text of the first choice
//! as the answer. It is the one protocol every model endpoint speaks (a hosted model, a gateway, a
//! local server), so nothing here is specific to a vendor: no host SDK, no gateway product
//! (ADR 0007).
//!
//! [`OpenAiChat`] holds **one client configuration per endpoint name** (`models.endpoints.<name>`
//! of the configuration file: a base URL, a credential, a timeout) and sends each request to the
//! endpoint it names ([`ChatRequest::endpoint`]). A name it does not hold is
//! [`ModelError::NotConfigured`]: the configuration checks that every task names an endpoint, so
//! that is a bug and never a configuration outcome, and nothing is sent anywhere else.
//!
//! The credential is a bearer token. It is marked sensitive in the request, never part of an error
//! or of a `Debug` ([`OpenAiChat`] prints `<redacted>` for it, and for the address too: a deployment may keep it in a secret store), and a redirect is never followed:
//! the token goes to the endpoint it was configured for and nowhere else.
//!
//! How an answer maps to the port's errors: a refusal of the request for good (4xx other than the
//! cases below) is [`ModelError::Rejected`]; 401 and 403 are [`ModelError::Unauthenticated`]; 429 is
//! [`ModelError::RateLimited`] with the `Retry-After` the endpoint gave; 408, 5xx, a timeout and a
//! connection that failed are [`ModelError::Unreachable`]; a 2xx that is not a chat completion is
//! [`ModelError::Protocol`]. The last two are transient: the caller may ask again.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use orch_core::BoxError;
use orch_ports::{ChatModel, ChatRequest, ModelError};
use reqwest::StatusCode;
use reqwest::header::{AUTHORIZATION, HeaderValue, RETRY_AFTER};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};

/// The longest `Retry-After` honoured: an endpoint cannot park a request for longer.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(3600);
/// Most bytes of an answer that are read: a title is a few words.
const MAX_BODY_BYTES: usize = 256 * 1024;
/// Most characters of an endpoint's own error message that are kept.
const MAX_ERROR_CHARS: usize = 300;

/// How to reach the endpoint.
#[derive(Clone)]
pub struct OpenAiConfig {
    /// The endpoint's base URL, up to and not including `/chat/completions` (for example
    /// `https://api.openai.com/v1`). `http` or `https`.
    pub base_url: String,
    /// The bearer token, when the endpoint wants one.
    pub api_key: Option<SecretString>,
    /// How long one question may take, connecting and answering together.
    pub timeout: Duration,
    /// Whether to honour `HTTP(S)_PROXY` from the environment (off by default: the endpoint is
    /// reached directly).
    pub use_system_proxy: bool,
}

impl OpenAiConfig {
    /// A configuration for `base_url` with no credential, a 20 second timeout and no proxy.
    pub fn new(base_url: impl Into<String>) -> Self {
        OpenAiConfig {
            base_url: base_url.into(),
            api_key: None,
            timeout: Duration::from_secs(20),
            use_system_proxy: false,
        }
    }

    /// Sets the bearer token.
    #[must_use]
    pub fn with_api_key(mut self, key: SecretString) -> Self {
        self.api_key = Some(key);
        self
    }

    /// Sets the time one question may take.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

impl fmt::Debug for OpenAiConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAiConfig")
            // the address may be one a deployment keeps in a secret store (`baseUrl: { file }`)
            .field("base_url", &"<redacted>")
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("timeout", &self.timeout)
            .field("use_system_proxy", &self.use_system_proxy)
            .finish()
    }
}

/// The adapter could not be built. Each error names the endpoint, by the name the configuration
/// gives it (never a URL or a credential).
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    /// The base URL is not an `http` or `https` URL.
    #[error("the base URL of the model endpoint `{endpoint}` must start with http:// or https://")]
    BadBaseUrl {
        /// The endpoint's name.
        endpoint: String,
    },
    /// The token cannot be sent in a header (a newline in it).
    #[error("the API key of the model endpoint `{endpoint}` cannot be sent as a header")]
    BadApiKey {
        /// The endpoint's name.
        endpoint: String,
    },
    /// The HTTP client could not be built (TLS backend initialisation).
    #[error("cannot build the HTTP client of the model endpoint `{endpoint}`")]
    Http {
        /// The endpoint's name.
        endpoint: String,
        /// The cause.
        #[source]
        source: BoxError,
    },
}

/// A [`ChatModel`] that asks OpenAI-compatible endpoints, one per name.
#[derive(Clone)]
pub struct OpenAiChat {
    endpoints: Arc<BTreeMap<String, Endpoint>>,
}

/// One endpoint: its client (with its timeout), its URL and its credential.
struct Endpoint {
    http: reqwest::Client,
    url: String,
    authorization: Option<HeaderValue>,
}

impl fmt::Debug for OpenAiChat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map()
            .entries(self.endpoints.iter().map(|(name, e)| {
                (
                    name,
                    format!(
                        "<redacted address> (authorization: {})",
                        if e.authorization.is_some() {
                            "<redacted>"
                        } else {
                            "none"
                        }
                    ),
                )
            }))
            .finish()
    }
}

impl OpenAiChat {
    /// Builds the adapter over the endpoints `endpoints` (name and configuration) and installs the
    /// `rustls` crypto provider if none is installed yet. No endpoints is a model that holds none:
    /// every question is [`ModelError::NotConfigured`].
    ///
    /// # Errors
    /// [`BuildError`] for a base URL that is not `http(s)`, a token that is not a header value,
    /// or a TLS backend that fails to start; the first endpoint (by name) that is wrong.
    pub fn new(
        endpoints: impl IntoIterator<Item = (String, OpenAiConfig)>,
    ) -> Result<Self, BuildError> {
        let mut built = BTreeMap::new();
        for (name, cfg) in endpoints {
            let endpoint = Endpoint::new(&name, &cfg)?;
            built.insert(name, endpoint);
        }
        Ok(OpenAiChat {
            endpoints: Arc::new(built),
        })
    }

    /// The names of the endpoints this model holds, in order.
    pub fn endpoint_names(&self) -> impl Iterator<Item = &str> {
        self.endpoints.keys().map(String::as_str)
    }
}

impl Endpoint {
    fn new(name: &str, cfg: &OpenAiConfig) -> Result<Self, BuildError> {
        let base = cfg.base_url.trim().trim_end_matches('/');
        if !(base.starts_with("http://") || base.starts_with("https://")) || base.len() < 9 {
            return Err(BuildError::BadBaseUrl {
                endpoint: name.to_owned(),
            });
        }
        let authorization = match &cfg.api_key {
            Some(key) if !key.expose_secret().is_empty() => {
                let mut value = HeaderValue::from_str(&format!("Bearer {}", key.expose_secret()))
                    .map_err(|_| BuildError::BadApiKey {
                    endpoint: name.to_owned(),
                })?;
                value.set_sensitive(true);
                Some(value)
            }
            Some(_) | None => None,
        };
        // Err means a provider is already installed, which is what we want.
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let builder = reqwest::Client::builder()
            .timeout(cfg.timeout)
            .redirect(reqwest::redirect::Policy::none());
        let builder = if cfg.use_system_proxy {
            builder
        } else {
            builder.no_proxy()
        };
        let http = builder.build().map_err(|e| BuildError::Http {
            endpoint: name.to_owned(),
            source: e.without_url().into(),
        })?;
        Ok(Endpoint {
            http,
            url: format!("{base}/chat/completions"),
            authorization,
        })
    }
}

#[derive(Serialize)]
struct WireMessage<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(Serialize)]
struct WireRequest<'a> {
    model: &'a str,
    messages: [WireMessage<'a>; 2],
    max_tokens: u32,
    stream: bool,
}

#[derive(Deserialize)]
struct WireResponse {
    #[serde(default)]
    choices: Vec<WireChoice>,
}

#[derive(Deserialize)]
struct WireChoice {
    message: Option<WireAnswer>,
}

#[derive(Deserialize)]
struct WireAnswer {
    content: Option<String>,
}

#[derive(Deserialize)]
struct WireErrorBody {
    error: Option<WireError>,
}

#[derive(Deserialize)]
struct WireError {
    message: Option<String>,
}

/// `Retry-After` as delta-seconds, capped at [`MAX_RETRY_AFTER`] (an HTTP date is not read: the
/// caller backs off by its own schedule then).
fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let secs: u64 = headers
        .get(RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(Duration::from_secs(secs).min(MAX_RETRY_AFTER))
}

/// What a transport failure says, without the URL.
fn transport(err: reqwest::Error) -> ModelError {
    let detail = if err.is_timeout() {
        "the model did not answer in time"
    } else if err.is_connect() {
        "could not connect to the model"
    } else {
        "the request to the model failed"
    };
    ModelError::unreachable(detail).with_source(err.without_url())
}

/// The endpoint's own message for a refusal, cut, or the status when it gave none.
fn refusal(status: StatusCode, body: &[u8]) -> String {
    let said = serde_json::from_slice::<WireErrorBody>(body)
        .ok()
        .and_then(|b| b.error)
        .and_then(|e| e.message)
        .map(|m| m.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|m| !m.is_empty());
    match said {
        Some(message) => {
            let cut: String = message.chars().take(MAX_ERROR_CHARS).collect();
            format!("{status}: {cut}")
        }
        None => status.to_string(),
    }
}

/// Reads at most [`MAX_BODY_BYTES`] of the answer.
async fn read_body(mut response: reqwest::Response) -> Result<Vec<u8>, ModelError> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(transport)? {
        if body.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(ModelError::protocol("the model's answer is too large"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

impl ChatModel for OpenAiChat {
    async fn complete(&self, request: &ChatRequest) -> Result<String, ModelError> {
        let Some(endpoint) = self.endpoints.get(&request.endpoint) else {
            return Err(ModelError::NotConfigured);
        };
        endpoint.complete(request).await
    }
}

impl Endpoint {
    async fn complete(&self, request: &ChatRequest) -> Result<String, ModelError> {
        let body = WireRequest {
            model: &request.model,
            messages: [
                WireMessage {
                    role: "system",
                    content: &request.system,
                },
                WireMessage {
                    role: "user",
                    content: &request.user,
                },
            ],
            max_tokens: request.max_tokens,
            stream: false,
        };
        let mut call = self.http.post(&self.url).json(&body);
        if let Some(authorization) = &self.authorization {
            call = call.header(AUTHORIZATION, authorization.clone());
        }
        let response = call.send().await.map_err(transport)?;
        let status = response.status();
        let wait = retry_after(response.headers());
        let bytes = read_body(response).await?;
        match status {
            s if s.is_success() => {}
            StatusCode::TOO_MANY_REQUESTS => {
                return Err(ModelError::RateLimited { retry_after: wait });
            }
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                return Err(ModelError::Unauthenticated);
            }
            StatusCode::REQUEST_TIMEOUT => {
                return Err(ModelError::unreachable("the model timed out the request"));
            }
            s if s.is_server_error() => {
                return Err(ModelError::unreachable(format!(
                    "the model answered {}",
                    s.as_u16()
                )));
            }
            s if s.is_redirection() => {
                return Err(ModelError::Rejected(format!(
                    "the model endpoint redirected ({}): configure its final URL",
                    s.as_u16()
                )));
            }
            s => return Err(ModelError::Rejected(refusal(s, &bytes))),
        }
        let parsed: WireResponse = serde_json::from_slice(&bytes)
            .map_err(|_| ModelError::protocol("the model's answer is not a chat completion"))?;
        parsed
            .choices
            .into_iter()
            .next()
            .and_then(|choice| choice.message)
            .and_then(|message| message.content)
            .ok_or_else(|| ModelError::protocol("the model's answer has no text"))
    }
}
