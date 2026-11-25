//! [`DevCredentials`]: the [`PlatformCredentials`] of a development stack.
//!
//! It asks the development issuer (`mock-oidc`, `dev/mock-oidc/`) for a token with a
//! `client_credentials` request, the way `dev/auth-header.sh` does, and gives it to the
//! provisioner. A token is asked for on every call (the issuer is a mock; nothing is cached), so
//! a restarted issuer is followed at once. **Not for a deployment**: the configuration refuses it
//! in production.

use std::fmt;
use std::time::Duration;

use orch_ports::{CredentialsError, PlatformCredentials, PlatformToken};
use reqwest::StatusCode;
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;
use url::Url;

/// How to reach the development issuer.
#[derive(Clone)]
pub struct DevConfig {
    /// The issuer's base URL, `http` or `https`, with a host and no user name or password.
    /// `/token` is appended.
    pub url: String,
    /// The client the issuer knows the orchestrator by.
    pub client_id: String,
    /// The client's secret.
    pub client_secret: SecretString,
    /// The `audience` to ask for, when the issuer needs it named.
    pub audience: Option<String>,
    /// How long one request may take.
    pub timeout: Duration,
}

impl fmt::Debug for DevConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DevConfig")
            .field("url", &self.url)
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .finish_non_exhaustive()
    }
}

/// The adapter could not be built.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    /// The URL is not an absolute `http(s)` URL with a host and no user name or password.
    #[error(
        "the issuer URL must be an absolute http:// or https:// URL with a host and no user name or password"
    )]
    BadUrl,
    /// The HTTP client could not be built.
    #[error("cannot build the issuer HTTP client")]
    Http,
}

/// A token from the development issuer.
#[derive(Clone)]
pub struct DevCredentials {
    http: reqwest::Client,
    token_url: Url,
    client_id: String,
    client_secret: SecretString,
    audience: Option<String>,
}

impl fmt::Debug for DevCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DevCredentials")
            .field("client_id", &self.client_id)
            .finish_non_exhaustive()
    }
}

impl DevCredentials {
    /// Builds the adapter; nothing is sent until a token is asked for.
    ///
    /// # Errors
    /// [`BuildError`] for a URL that is not plain `http(s)` or a client that cannot be built.
    pub fn new(cfg: DevConfig) -> Result<Self, BuildError> {
        let base = Url::parse(&cfg.url).map_err(|_| BuildError::BadUrl)?;
        if !matches!(base.scheme(), "http" | "https")
            || !base.has_host()
            || !base.username().is_empty()
            || base.password().is_some()
        {
            return Err(BuildError::BadUrl);
        }
        let mut token_url = base;
        let path = format!("{}/token", token_url.path().trim_end_matches('/'));
        token_url.set_path(&path);
        // Err means a provider is already installed, which is what we want.
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let http = reqwest::Client::builder()
            .timeout(cfg.timeout)
            .connect_timeout(cfg.timeout)
            // The client secret goes to the configured URL and nowhere else.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| BuildError::Http)?;
        Ok(DevCredentials {
            http,
            token_url,
            client_id: cfg.client_id,
            client_secret: cfg.client_secret,
            audience: cfg.audience,
        })
    }
}

impl PlatformCredentials for DevCredentials {
    async fn token(&self) -> Result<PlatformToken, CredentialsError> {
        let body = {
            let mut form = url::form_urlencoded::Serializer::new(String::new());
            form.append_pair("grant_type", "client_credentials");
            if let Some(audience) = &self.audience {
                form.append_pair("audience", audience);
            }
            form.finish()
        };
        let response = self
            .http
            .post(self.token_url.clone())
            .basic_auth(&self.client_id, Some(self.client_secret.expose_secret()))
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(body)
            .send()
            .await
            .map_err(|e| CredentialsError::unavailable(e.without_url()))?;
        match response.status() {
            s if s.is_success() => {}
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                return Err(CredentialsError::Refused);
            }
            s => {
                return Err(CredentialsError::unavailable(format!(
                    "the issuer answered {s}"
                )));
            }
        }
        let body: Value = response
            .json()
            .await
            .map_err(|_| CredentialsError::Invalid)?;
        let token = body
            .get("access_token")
            .and_then(Value::as_str)
            .ok_or(CredentialsError::Invalid)?;
        PlatformToken::new(token)
    }
}
