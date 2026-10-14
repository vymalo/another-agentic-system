//! The one HTTP client of the broker, and the egress guard in front of it.
//!
//! Every request the broker makes (resource metadata, authorization-server metadata, the token,
//! registration and revocation endpoints) goes through [`Http`], which
//!
//! 1. asks [`EgressPolicy::check_url`] about the URL and sends to the **normal form** it returns;
//! 2. for an address written in the URL, asks [`EgressPolicy::check_resolved`] about it;
//! 3. for a name, lets the client resolve it only through [`GuardedResolver`], which resolves the
//!    name **once**, asks [`EgressPolicy::check_resolved`] about the answer and hands the client
//!    only the addresses that passed, so the address that was judged is the address connected to
//!    (no second lookup to rebind);
//! 4. follows no redirect, uses no proxy of the environment, times out, and reads at most a bounded
//!    number of bytes.
//!
//! No error holds a URL, a header or a body: a [`FetchError`] is a fixed phrase and, for a refusal,
//! the rule that refused.

use std::future::Future;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use orch_ports::{EgressPolicy, EgressRefusal, ToolSecret};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderValue};
use url::form_urlencoded;
use zeroize::Zeroizing;

use crate::BuildError;
use crate::config::BrokerConfig;

/// The future of a [`HostLookup`].
pub type LookupFuture<'a> = Pin<Box<dyn Future<Output = io::Result<Vec<IpAddr>>> + Send + 'a>>;

/// How a host name becomes addresses. The default, [`SystemLookup`], asks the operating system's
/// resolver; a composition root or a test may put another here (the guard judges whatever it
/// answers, and the connection is made only to those addresses).
pub trait HostLookup: Send + Sync + 'static {
    /// The addresses `host` (a name, never a URL) resolves to.
    fn lookup<'a>(&'a self, host: &'a str) -> LookupFuture<'a>;
}

/// [`HostLookup`] over the operating system's resolver (`getaddrinfo`).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemLookup;

impl HostLookup for SystemLookup {
    fn lookup<'a>(&'a self, host: &'a str) -> LookupFuture<'a> {
        Box::pin(async move {
            let addrs = tokio::net::lookup_host((host, 0)).await?;
            Ok(addrs.map(|addr| addr.ip()).collect())
        })
    }
}

/// Why a request was not made or did not finish. Fixed phrases only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum FetchError {
    /// The egress guard refused the URL, or an address its host resolved to.
    #[error("the egress guard refused the URL")]
    Refused(#[source] EgressRefusal),
    /// The request did not finish in time.
    #[error("the request timed out")]
    Timeout,
    /// No connection could be made.
    #[error("no connection could be made")]
    Connect,
    /// The response is longer than the limit.
    #[error("the response is longer than the limit")]
    TooLarge,
    /// Any other failure of the exchange.
    #[error("the request failed")]
    Failed,
}

/// A response: the status and the body, wiped on drop (a token response is a secret).
pub(crate) struct Response {
    pub(crate) status: u16,
    pub(crate) body: Zeroizing<Vec<u8>>,
}

/// What a POST carries.
pub(crate) enum Body {
    /// `application/x-www-form-urlencoded`; it may hold a secret.
    Form(Zeroizing<String>),
    /// `application/json`.
    Json(Vec<u8>),
}

impl Body {
    /// A form body of `fields`.
    pub(crate) fn form<'a>(fields: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let mut encoder = form_urlencoded::Serializer::new(String::new());
        for (name, value) in fields {
            encoder.append_pair(name, value);
        }
        Body::Form(Zeroizing::new(encoder.finish()))
    }
}

/// `client_secret_basic` (RFC 6749, section 2.3.1): the id and the secret, form-encoded, then
/// joined with a colon and base64-encoded.
pub(crate) struct BasicAuth<'a> {
    pub(crate) id: &'a str,
    pub(crate) secret: &'a ToolSecret,
}

impl BasicAuth<'_> {
    fn header(&self) -> Option<HeaderValue> {
        let encode =
            |text: &str| form_urlencoded::byte_serialize(text.as_bytes()).collect::<String>();
        let pair = Zeroizing::new(format!(
            "{}:{}",
            encode(self.id),
            encode(self.secret.expose())
        ));
        let value = Zeroizing::new(format!("Basic {}", STANDARD.encode(pair.as_bytes())));
        let mut header = HeaderValue::from_str(&value).ok()?;
        header.set_sensitive(true);
        Some(header)
    }
}

/// The client, the policy and the limits.
pub(crate) struct Http {
    client: reqwest::Client,
    policy: EgressPolicy,
    max_body: usize,
}

impl Http {
    pub(crate) fn new(
        policy: EgressPolicy,
        lookup: Arc<dyn HostLookup>,
        config: &BrokerConfig,
    ) -> Result<Self, BuildError> {
        // as the other HTTP clients of this workspace do: the one TLS provider of the process
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let resolver = GuardedResolver {
            policy: policy.clone(),
            lookup,
        };
        let client = reqwest::Client::builder()
            // every request is its own exchange: a connection kept for later is a connection to
            // an address the guard judged once
            .pool_max_idle_per_host(0)
            .connect_timeout(config.connect_timeout)
            .timeout(config.request_timeout)
            // a redirect is a new URL the guard has not seen
            .redirect(reqwest::redirect::Policy::none())
            // a proxy connects where it likes
            .no_proxy()
            .dns_resolver(resolver)
            .user_agent(concat!(
                "orch-credential-broker/",
                env!("CARGO_PKG_VERSION")
            ))
            .build()
            .map_err(|e| BuildError::Http(e.without_url()))?;
        Ok(Http {
            client,
            policy,
            max_body: config.max_response_bytes,
        })
    }

    /// The URL in the normal form the guard accepts, for an address that is written in it also
    /// judged now; a name is judged when the client resolves it.
    pub(crate) fn admit(&self, url: &str) -> Result<String, FetchError> {
        let target = self.policy.check_url(url).map_err(FetchError::Refused)?;
        if let Some(addr) = target.literal_addr() {
            self.policy
                .check_resolved(&target, &[addr])
                .map_err(FetchError::Refused)?;
        }
        Ok(target.url().to_owned())
    }

    /// The guard's verdict on the URL's own rules only (scheme, host, user information, lists), for
    /// a URL the broker sends a person to and never fetches.
    pub(crate) fn admit_shape(&self, url: &str) -> Result<(), FetchError> {
        self.policy
            .check_url(url)
            .map(|_| ())
            .map_err(FetchError::Refused)
    }

    pub(crate) async fn get(&self, url: &str) -> Result<Response, FetchError> {
        let url = self.admit(url)?;
        let request = self.client.get(url).header(ACCEPT, "application/json");
        self.send(request).await
    }

    pub(crate) async fn post(
        &self,
        url: &str,
        body: Body,
        basic: Option<BasicAuth<'_>>,
    ) -> Result<Response, FetchError> {
        let url = self.admit(url)?;
        let mut request = self.client.post(url).header(ACCEPT, "application/json");
        if let Some(basic) = basic {
            request = request.header(AUTHORIZATION, basic.header().ok_or(FetchError::Failed)?);
        }
        request = match body {
            Body::Form(text) => request
                .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(text.to_string()),
            Body::Json(bytes) => request.header(CONTENT_TYPE, "application/json").body(bytes),
        };
        self.send(request).await
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<Response, FetchError> {
        let mut response = request.send().await.map_err(classify)?;
        let status = response.status().as_u16();
        let limit = u64::try_from(self.max_body).unwrap_or(u64::MAX);
        if response.content_length().is_some_and(|n| n > limit) {
            return Err(FetchError::TooLarge);
        }
        let mut body = Zeroizing::new(Vec::new());
        while let Some(chunk) = response.chunk().await.map_err(classify)? {
            if body.len() + chunk.len() > self.max_body {
                return Err(FetchError::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(Response { status, body })
    }
}

/// What a failed exchange was, without its URL: a refusal of the resolver hook is found in the
/// error's chain.
fn classify(error: reqwest::Error) -> FetchError {
    let error = error.without_url();
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&error);
    while let Some(e) = source {
        if let Some(refusal) = e.downcast_ref::<EgressRefusal>() {
            return FetchError::Refused(*refusal);
        }
        source = e.source();
    }
    if error.is_timeout() {
        FetchError::Timeout
    } else if error.is_connect() {
        FetchError::Connect
    } else {
        FetchError::Failed
    }
}

/// The resolver of the client: resolves once, judges the answer, returns the judged addresses.
struct GuardedResolver {
    policy: EgressPolicy,
    lookup: Arc<dyn HostLookup>,
}

#[derive(Debug, thiserror::Error)]
#[error("the host name could not be resolved")]
struct NotResolved;

impl Resolve for GuardedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let policy = self.policy.clone();
        let lookup = Arc::clone(&self.lookup);
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let bare = host.trim_start_matches('[').trim_end_matches(']');
            let literal = bare.parse::<IpAddr>().ok();
            let ips = match literal {
                Some(ip) => vec![ip],
                None => lookup.lookup(bare).await.map_err(|_| NotResolved)?,
            };
            // the host again as the guard reads it, so the lists apply to the name the client
            // is about to connect with
            let probe = match literal {
                Some(IpAddr::V6(_)) => format!("https://[{bare}]/"),
                _ => format!("https://{bare}/"),
            };
            let target = policy.check_url(&probe)?;
            let resolved: Vec<SocketAddr> =
                ips.into_iter().map(|ip| SocketAddr::new(ip, 0)).collect();
            let checked = policy.check_resolved(&target, &resolved)?;
            let addrs: Addrs = Box::new(checked.into_vec().into_iter());
            Ok(addrs)
        })
    }
}
