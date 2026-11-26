//! [`OidcClientCredentials`]: the development adapter of [`PlatformCredentials`]. It asks an OIDC
//! issuer's token endpoint for a token with the OAuth `client_credentials` grant and keeps it until
//! it is about to expire. Made for the `mock-oidc` of the local stack (its extension parameters
//! `user`, `audience` and `roles`); a real issuer ignores or refuses them, so use a real adapter
//! there.

use std::fmt;
use std::sync::Mutex;
use std::time::Duration;

use orch_ports::{
    PlatformCredentials, PlatformCredentialsError, PlatformToken, PlatformTokenFuture,
};
use reqwest::header::CONTENT_TYPE;
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;
use tokio::time::Instant;
use url::Url;

use crate::BuildError;

/// A token is asked for again this long before it expires.
const SKEW: Duration = Duration::from_secs(30);
/// A token that says it lives less than this is used once and asked for again.
const FALLBACK_TTL: Duration = Duration::from_secs(60);

/// How to ask the issuer.
#[derive(Clone)]
pub struct OidcClientConfig {
    /// The token endpoint, `http` or `https`, with a host and no user name or password.
    pub token_url: String,
    /// The client id.
    pub client_id: String,
    /// The client secret.
    pub client_secret: SecretString,
    /// Extension parameters of `mock-oidc`: the user the token is for, the audience and the roles.
    pub user: Option<String>,
    /// The `audience` parameter.
    pub audience: Option<String>,
    /// The `roles` parameter (comma separated by the adapter); `None` leaves the user's own.
    pub roles: Option<Vec<String>>,
    /// How long one request may take.
    pub timeout: Duration,
}

impl fmt::Debug for OidcClientConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OidcClientConfig")
            .field("client_id", &self.client_id)
            .finish_non_exhaustive()
    }
}

/// [`PlatformCredentials`] from an issuer's `client_credentials` grant.
pub struct OidcClientCredentials {
    http: reqwest::Client,
    url: Url,
    body: SecretString,
    cache: Mutex<Option<(PlatformToken, Instant)>>,
}

impl fmt::Debug for OidcClientCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OidcClientCredentials")
            .finish_non_exhaustive()
    }
}

impl OidcClientCredentials {
    /// Checks the endpoint and builds the client.
    pub fn new(cfg: OidcClientConfig) -> Result<Self, BuildError> {
        let url = Url::parse(cfg.token_url.trim()).map_err(|_| BuildError::BadUrl)?;
        let ok = matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none();
        if !ok {
            return Err(BuildError::BadUrl);
        }
        let mut form = url::form_urlencoded::Serializer::new(String::new());
        form.append_pair("grant_type", "client_credentials");
        form.append_pair("client_id", &cfg.client_id);
        form.append_pair("client_secret", cfg.client_secret.expose_secret());
        if let Some(user) = &cfg.user {
            form.append_pair("user", user);
        }
        if let Some(audience) = &cfg.audience {
            form.append_pair("audience", audience);
        }
        if let Some(roles) = &cfg.roles {
            form.append_pair("roles", &roles.join(","));
        }
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let http = reqwest::Client::builder()
            .timeout(cfg.timeout)
            .connect_timeout(cfg.timeout)
            // The client secret goes to the configured endpoint and nowhere else.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| BuildError::Http(e.without_url().into()))?;
        Ok(Self {
            http,
            url,
            body: SecretString::from(form.finish()),
            cache: Mutex::new(None),
        })
    }

    fn cached(&self) -> Option<PlatformToken> {
        let cache = self.cache.lock().ok()?;
        let (token, until) = cache.as_ref()?;
        (Instant::now() < *until).then(|| token.clone())
    }

    async fn fetch(&self) -> Result<PlatformToken, PlatformCredentialsError> {
        let response = self
            .http
            .post(self.url.clone())
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(self.body.expose_secret().to_owned())
            .send()
            .await
            .map_err(|_| PlatformCredentialsError::Unavailable)?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::BAD_REQUEST
        {
            return Err(PlatformCredentialsError::Refused);
        }
        if !status.is_success() {
            return Err(PlatformCredentialsError::Unavailable);
        }
        let answer: Value = response
            .json()
            .await
            .map_err(|_| PlatformCredentialsError::Unavailable)?;
        let access = answer
            .get("access_token")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .ok_or(PlatformCredentialsError::Unavailable)?;
        let lives = answer
            .get("expires_in")
            .and_then(Value::as_u64)
            .map_or(FALLBACK_TTL, Duration::from_secs);
        let token = PlatformToken::new(access);
        let until = Instant::now() + lives.saturating_sub(SKEW);
        if let Ok(mut cache) = self.cache.lock() {
            *cache = Some((token.clone(), until));
        }
        Ok(token)
    }
}

impl PlatformCredentials for OidcClientCredentials {
    fn token(&self) -> PlatformTokenFuture<'_> {
        Box::pin(async move {
            match self.cached() {
                Some(token) => Ok(token),
                None => self.fetch().await,
            }
        })
    }
}
