//! The authenticator this process asks (ADR 0033, ADR 0009): the proxy header, the JWT
//! authenticator, both, or none, chosen by `auth.mode` and by the Cargo features compiled in. A
//! composition detail of the binary: the application and the API see only the `Authenticator`
//! port.

use orch_ports::{AuthError, Authenticator, Credentials, Principal, RefuseAll};

use crate::config::{AuthMode, Config, ConfigError};

/// The one authenticator type of the binary's `PortSet`, so that every mode is the same build
/// (static dispatch over the variants, not a runtime plugin).
#[derive(Debug)]
pub enum ConfiguredAuth {
    /// A process that serves no routes (a worker) authenticates nobody.
    Off(RefuseAll),
    /// `auth.mode: proxy_header`.
    #[cfg(feature = "auth-header")]
    Header(orch_auth_header::HeaderAuth),
    /// `auth.mode: jwt`.
    #[cfg(feature = "auth-jwt")]
    Jwt(orch_auth_jwt::JwtAuth),
    /// `auth.mode: jwt_or_proxy_header`: the token when the request has one, else the header.
    #[cfg(all(feature = "auth-header", feature = "auth-jwt"))]
    Either(orch_ports::ByCredential<orch_auth_jwt::JwtAuth, orch_auth_header::HeaderAuth>),
}

#[cfg(feature = "auth-header")]
fn header(cfg: &Config) -> orch_auth_header::HeaderAuth {
    let auth = orch_auth_header::HeaderAuth::new();
    match &cfg.auth_dev_user {
        Some(user) => auth.with_dev_user(user.clone()),
        None => auth,
    }
}

#[cfg(feature = "auth-jwt")]
fn jwt(cfg: &Config) -> Result<orch_auth_jwt::JwtAuth, ConfigError> {
    let settings = cfg.auth.jwt.as_ref().ok_or_else(|| ConfigError::Invalid {
        var: "auth.jwt",
        reason: "required when auth.mode reads tokens".to_owned(),
    })?;
    let mut jwt =
        orch_auth_jwt::JwtConfig::new(settings.issuer.clone(), settings.audiences.clone())
            .with_user_claim(settings.user_claim.clone());
    if let Some(url) = &settings.jwks_url {
        jwt = jwt.with_jwks_url(url.clone());
    }
    if let Some(path) = &settings.roles_claim {
        jwt = jwt.with_roles_claim(path.clone());
    }
    if let Some(dpop) = &cfg.auth.dpop {
        jwt = jwt.with_dpop(
            orch_auth_jwt::DpopConfig::new(dpop.public_origins.iter().cloned())
                .with_window(dpop.max_age, dpop.future_skew),
        );
    }
    orch_auth_jwt::JwtAuth::new(jwt).map_err(|e| ConfigError::Invalid {
        var: "auth.jwt",
        reason: e.to_string(),
    })
}

impl ConfiguredAuth {
    /// The authenticator `cfg` asks for. A process that serves no routes gets none.
    ///
    /// The configuration refuses a mode whose feature is not compiled in, so the error arms below
    /// are a second line of defence, not paths a running service takes.
    pub fn build(cfg: &Config) -> Result<Self, ConfigError> {
        if !cfg.role.runs_control_plane() {
            return Ok(ConfiguredAuth::Off(RefuseAll));
        }
        let mode = cfg.auth.mode;
        #[cfg_attr(
            all(feature = "auth-header", feature = "auth-jwt"),
            allow(unused_variables)
        )]
        let missing = |feature: &'static str| ConfigError::AuthNotCompiled {
            mode: mode.as_str(),
            feature,
        };
        match mode {
            #[cfg(feature = "auth-header")]
            AuthMode::ProxyHeader => Ok(ConfiguredAuth::Header(header(cfg))),
            #[cfg(not(feature = "auth-header"))]
            AuthMode::ProxyHeader => Err(missing("auth-header")),
            #[cfg(feature = "auth-jwt")]
            AuthMode::Jwt => Ok(ConfiguredAuth::Jwt(jwt(cfg)?)),
            #[cfg(not(feature = "auth-jwt"))]
            AuthMode::Jwt => Err(missing("auth-jwt")),
            #[cfg(all(feature = "auth-header", feature = "auth-jwt"))]
            AuthMode::JwtOrProxyHeader => Ok(ConfiguredAuth::Either(orch_ports::ByCredential {
                bearer: jwt(cfg)?,
                header: header(cfg),
            })),
            #[cfg(not(all(feature = "auth-header", feature = "auth-jwt")))]
            AuthMode::JwtOrProxyHeader => Err(missing(if cfg!(feature = "auth-jwt") {
                "auth-header"
            } else {
                "auth-jwt"
            })),
        }
    }

    /// A line for the log: which authenticator this process runs.
    pub fn describe(&self) -> &'static str {
        match self {
            ConfiguredAuth::Off(_) => "none (this process serves no routes)",
            #[cfg(feature = "auth-header")]
            ConfiguredAuth::Header(_) => "proxy_header",
            #[cfg(feature = "auth-jwt")]
            ConfiguredAuth::Jwt(_) => "jwt",
            #[cfg(all(feature = "auth-header", feature = "auth-jwt"))]
            ConfiguredAuth::Either(_) => "jwt_or_proxy_header",
        }
    }
}

impl Authenticator for ConfiguredAuth {
    async fn authenticate(&self, credentials: &Credentials<'_>) -> Result<Principal, AuthError> {
        match self {
            ConfiguredAuth::Off(auth) => auth.authenticate(credentials).await,
            #[cfg(feature = "auth-header")]
            ConfiguredAuth::Header(auth) => auth.authenticate(credentials).await,
            #[cfg(feature = "auth-jwt")]
            ConfiguredAuth::Jwt(auth) => auth.authenticate(credentials).await,
            #[cfg(all(feature = "auth-header", feature = "auth-jwt"))]
            ConfiguredAuth::Either(auth) => auth.authenticate(credentials).await,
        }
    }

    async fn ready(&self) -> Result<(), AuthError> {
        match self {
            ConfiguredAuth::Off(auth) => auth.ready().await,
            #[cfg(feature = "auth-header")]
            ConfiguredAuth::Header(auth) => auth.ready().await,
            #[cfg(feature = "auth-jwt")]
            ConfiguredAuth::Jwt(auth) => auth.ready().await,
            #[cfg(all(feature = "auth-header", feature = "auth-jwt"))]
            ConfiguredAuth::Either(auth) => auth.ready().await,
        }
    }

    fn accepts_bearer(&self) -> bool {
        match self {
            ConfiguredAuth::Off(auth) => auth.accepts_bearer(),
            #[cfg(feature = "auth-header")]
            ConfiguredAuth::Header(auth) => auth.accepts_bearer(),
            #[cfg(feature = "auth-jwt")]
            ConfiguredAuth::Jwt(auth) => auth.accepts_bearer(),
            #[cfg(all(feature = "auth-header", feature = "auth-jwt"))]
            ConfiguredAuth::Either(auth) => auth.accepts_bearer(),
        }
    }

    fn accepts_dpop(&self) -> bool {
        match self {
            ConfiguredAuth::Off(auth) => auth.accepts_dpop(),
            #[cfg(feature = "auth-header")]
            ConfiguredAuth::Header(auth) => auth.accepts_dpop(),
            #[cfg(feature = "auth-jwt")]
            ConfiguredAuth::Jwt(auth) => auth.accepts_dpop(),
            #[cfg(all(feature = "auth-header", feature = "auth-jwt"))]
            ConfiguredAuth::Either(auth) => auth.accepts_dpop(),
        }
    }
}
