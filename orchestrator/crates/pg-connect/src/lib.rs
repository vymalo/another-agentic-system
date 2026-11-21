//! Connects to Postgres on `tokio-postgres` as the orchestrator's crates connected through sqlx 0.9
//! (ADR 0076, sys#382 E1): the same connection string means the same server, the same TLS and the
//! same checks, so moving a crate off sqlx changes no deployment's configuration.
//!
//! * **The connection string** is a `postgres://` or `postgresql://` URL, every parameter
//!   `tokio-postgres` knows (`host`, `port`, `user`, `password`, `dbname`, `application_name`,
//!   `options`, `connect_timeout`, ...) read by it. `sslmode` and `sslrootcert` are read here,
//!   because `tokio-postgres` knows only three of sqlx's five modes. `sslcert`, `sslkey` and
//!   `sslpassword` (a client certificate) are refused: no deployment uses one, and a parameter
//!   that would be ignored must not be.
//! * **TLS is sqlx's**, with rustls on aws-lc-rs and the Mozilla roots (`webpki-roots`) plus the
//!   file `sslrootcert`:
//!
//!   | `sslmode` | TLS | The server's certificate |
//!   |---|---|---|
//!   | `disable`, `allow` | none | |
//!   | `prefer` (the default) | when the server offers it | not checked |
//!   | `require` | always | not checked |
//!   | `verify-ca` | always | its chain, not its name |
//!   | `verify-full` | always | its chain and its name |
//!
//! No error of this crate holds the connection string: it may carry a password.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod, Runtime};
use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::WebPkiSupportedAlgorithms;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{
    CertificateError, ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme,
};
use tokio_postgres::config::SslMode;
use tokio_postgres_rustls::MakeRustlsConnect;

/// What `sslmode` asks for, as sqlx read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TlsMode {
    /// `disable` and `allow`: no TLS.
    Disable,
    /// `prefer`, the default: TLS when the server offers it, its certificate not checked.
    Prefer,
    /// `require`: TLS, its certificate not checked.
    Require,
    /// `verify-ca`: TLS, the certificate's chain checked, not its name.
    VerifyCa,
    /// `verify-full`: TLS, the certificate's chain and name checked.
    VerifyFull,
}

/// A connection string, read: the `tokio-postgres` configuration and the TLS it asks for.
#[derive(Clone, Debug)]
pub struct PgOptions {
    config: tokio_postgres::Config,
    tls: TlsMode,
    root_cert: Option<PathBuf>,
}

impl FromStr for PgOptions {
    type Err = ParseError;

    fn from_str(url: &str) -> Result<Self, ParseError> {
        let mut parsed = url::Url::parse(url).map_err(|_| ParseError::NotAUrl)?;
        if !matches!(parsed.scheme(), "postgres" | "postgresql") {
            return Err(ParseError::Scheme);
        }
        let mut tls = TlsMode::Prefer;
        let mut root_cert = None;
        // The parameters TLS needs are taken out of the query as it is written; every other
        // segment is kept byte for byte, never encoded again (a form encoding would turn a space,
        // `%20`, into a `+`, which `tokio-postgres` reads as a `+`).
        let raw = parsed.query().unwrap_or_default().to_owned();
        let mut kept = Vec::new();
        for segment in raw.split('&').filter(|s| !s.is_empty()) {
            let Some((key, value)) = url::form_urlencoded::parse(segment.as_bytes()).next() else {
                continue;
            };
            match key.as_ref() {
                "sslmode" => {
                    tls = match value.as_ref() {
                        "disable" | "allow" => TlsMode::Disable,
                        "prefer" => TlsMode::Prefer,
                        "require" => TlsMode::Require,
                        "verify-ca" => TlsMode::VerifyCa,
                        "verify-full" => TlsMode::VerifyFull,
                        _ => return Err(ParseError::SslMode),
                    };
                }
                "sslrootcert" => root_cert = Some(PathBuf::from(value.as_ref())),
                "sslcert" | "sslkey" | "sslpassword" => {
                    return Err(ParseError::ClientCertificate);
                }
                _ => kept.push(segment),
            }
        }
        parsed.set_query((!kept.is_empty()).then(|| kept.join("&")).as_deref());
        let mut config =
            tokio_postgres::Config::from_str(parsed.as_str()).map_err(|_| ParseError::Parameter)?;
        config.ssl_mode(match tls {
            TlsMode::Disable => SslMode::Disable,
            TlsMode::Prefer => SslMode::Prefer,
            TlsMode::Require | TlsMode::VerifyCa | TlsMode::VerifyFull => SslMode::Require,
        });
        Ok(Self {
            config,
            tls,
            root_cert,
        })
    }
}

impl PgOptions {
    /// The `tokio-postgres` configuration (its `sslmode` already the one TLS needs).
    pub fn config(&self) -> &tokio_postgres::Config {
        &self.config
    }

    /// The configuration, to change (a test pins a database of its own).
    pub fn config_mut(&mut self) -> &mut tokio_postgres::Config {
        &mut self.config
    }

    /// The TLS the connection string asks for.
    pub fn tls(&self) -> TlsMode {
        self.tls
    }

    /// The rustls connector of these options, built once per pool or connection.
    ///
    /// # Errors
    ///
    /// [`ConnectError::RootCert`] when `sslrootcert` cannot be read as PEM certificates,
    /// [`ConnectError::Tls`] for rustls's own refusals.
    pub fn tls_connector(&self) -> Result<MakeRustlsConnect, ConnectError> {
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let builder = ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .map_err(ConnectError::Tls)?;
        let config = match self.tls {
            TlsMode::Disable | TlsMode::Prefer | TlsMode::Require => builder
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(AnyCertificate(
                    provider.signature_verification_algorithms,
                )))
                .with_no_client_auth(),
            TlsMode::VerifyCa | TlsMode::VerifyFull => {
                let roots = Arc::new(self.roots()?);
                let webpki = WebPkiServerVerifier::builder_with_provider(roots, provider)
                    .build()
                    .map_err(|_| ConnectError::RootCert)?;
                let verifier: Arc<dyn ServerCertVerifier> = if self.tls == TlsMode::VerifyCa {
                    Arc::new(ChainOnly(webpki))
                } else {
                    webpki
                };
                builder
                    .dangerous()
                    .with_custom_certificate_verifier(verifier)
                    .with_no_client_auth()
            }
        };
        Ok(MakeRustlsConnect::new(config))
    }

    /// The Mozilla roots, and the certificates of `sslrootcert`.
    fn roots(&self) -> Result<RootCertStore, ConnectError> {
        let mut roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        if let Some(path) = &self.root_cert {
            let certs = CertificateDer::pem_file_iter(path).map_err(|_| ConnectError::RootCert)?;
            for cert in certs {
                roots
                    .add(cert.map_err(|_| ConnectError::RootCert)?)
                    .map_err(|_| ConnectError::RootCert)?;
            }
        }
        Ok(roots)
    }

    /// One connection, its connection task spawned on the current tokio runtime (it ends when
    /// the client is dropped).
    ///
    /// # Errors
    ///
    /// [`ConnectError::Database`] when the server cannot be reached or refuses, and the TLS
    /// errors of [`tls_connector`](Self::tls_connector).
    pub async fn connect(&self) -> Result<tokio_postgres::Client, ConnectError> {
        let (client, connection) = self.config.connect(self.tls_connector()?).await?;
        tokio::spawn(async move {
            // The client sees the error on its next call; nothing to do with it here.
            let _ = connection.await;
        });
        Ok(client)
    }

    /// A pool of at most `max_size` connections (at least one), each made on demand. `wait`
    /// bounds both a wait for a free connection and the making of a new one, as sqlx's
    /// `acquire_timeout` did.
    ///
    /// # Errors
    ///
    /// The TLS errors of [`tls_connector`](Self::tls_connector), and [`ConnectError::Pool`].
    pub fn pool(&self, max_size: usize, wait: Duration) -> Result<Pool, ConnectError> {
        let manager = Manager::from_config(
            self.config.clone(),
            self.tls_connector()?,
            ManagerConfig {
                recycling_method: RecyclingMethod::Fast,
            },
        );
        Pool::builder(manager)
            .max_size(max_size.max(1))
            .wait_timeout(Some(wait))
            .create_timeout(Some(wait))
            .runtime(Runtime::Tokio1)
            .build()
            .map_err(|e| ConnectError::Pool(e.to_string()))
    }
}

/// `prefer` and `require`: any certificate, as sqlx and libpq take it. The handshake's signature
/// is still checked against the certificate it came with.
#[derive(Debug)]
struct AnyCertificate(WebPkiSupportedAlgorithms);

impl ServerCertVerifier for AnyCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_schemes()
    }
}

/// `verify-ca`: the certificate's chain to a root, not its name, as sqlx's verifier that ignores
/// the host name.
#[derive(Debug)]
struct ChainOnly(Arc<WebPkiServerVerifier>);

impl ServerCertVerifier for ChainOnly {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match self
            .0
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
        {
            Err(rustls::Error::InvalidCertificate(
                CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. },
            )) => Ok(ServerCertVerified::assertion()),
            other => other,
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.0.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.0.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_verify_schemes()
    }
}

/// Why a connection string was refused. Never holds the text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ParseError {
    /// Not a URL.
    #[error("the connection string is not a URL")]
    NotAUrl,
    /// Not `postgres://` or `postgresql://`.
    #[error("the connection string is not a postgres:// or postgresql:// URL")]
    Scheme,
    /// An `sslmode` that is not one of the five.
    #[error("sslmode is not one of disable, allow, prefer, require, verify-ca, verify-full")]
    SslMode,
    /// `sslcert`, `sslkey` or `sslpassword`: a client certificate, which this crate does not offer.
    #[error("a client certificate (sslcert, sslkey, sslpassword) is not supported")]
    ClientCertificate,
    /// A parameter `tokio-postgres` does not know, or a value it refuses.
    #[error("a parameter of the connection string is not known or not valid")]
    Parameter,
}

/// Why a connection or a pool could not be made.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConnectError {
    /// `sslrootcert` cannot be read as PEM certificates.
    #[error("sslrootcert cannot be read as PEM certificates")]
    RootCert,
    /// rustls refused the configuration.
    #[error("the TLS configuration is refused")]
    Tls(#[source] rustls::Error),
    /// The pool could not be built.
    #[error("the pool cannot be built: {0}")]
    Pool(String),
    /// The server could not be reached, or refused.
    #[error("the database cannot be reached")]
    Database(#[from] tokio_postgres::Error),
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn tls(url: &str) -> TlsMode {
        url.parse::<PgOptions>().unwrap().tls()
    }

    #[test]
    fn sslmode_is_read_as_sqlx_reads_it() {
        let base = "postgres://u:p@db.example:5433/orch";
        assert_eq!(tls(base), TlsMode::Prefer, "the default");
        for (mode, want, wire) in [
            ("disable", TlsMode::Disable, SslMode::Disable),
            ("allow", TlsMode::Disable, SslMode::Disable),
            ("prefer", TlsMode::Prefer, SslMode::Prefer),
            ("require", TlsMode::Require, SslMode::Require),
            ("verify-ca", TlsMode::VerifyCa, SslMode::Require),
            ("verify-full", TlsMode::VerifyFull, SslMode::Require),
        ] {
            let options: PgOptions = format!("{base}?sslmode={mode}").parse().unwrap();
            assert_eq!(options.tls(), want, "{mode}");
            assert_eq!(options.config().get_ssl_mode(), wire, "{mode}");
        }
    }

    #[test]
    fn what_is_refused_never_shows_the_text() {
        for (url, want) in [
            ("not a url", ParseError::NotAUrl),
            ("mysql://u:secretpw@h/db", ParseError::Scheme),
            (
                "postgres://u:secretpw@h/db?sslmode=maybe",
                ParseError::SslMode,
            ),
            (
                "postgres://u:secretpw@h/db?sslcert=/c.pem",
                ParseError::ClientCertificate,
            ),
            (
                "postgres://u:secretpw@h/db?sslkey=/k.pem",
                ParseError::ClientCertificate,
            ),
            (
                "postgres://u:secretpw@h/db?nonsense=1",
                ParseError::Parameter,
            ),
        ] {
            let error = url.parse::<PgOptions>().unwrap_err();
            assert_eq!(error, want, "{url}");
            assert!(!format!("{error} {error:?}").contains("secretpw"), "{url}");
        }
    }

    #[test]
    fn a_root_certificate_that_cannot_be_read_is_refused() {
        let options: PgOptions =
            "postgres://u@h/db?sslmode=verify-full&sslrootcert=/nonexistent.pem"
                .parse()
                .unwrap();
        assert!(matches!(
            options.tls_connector(),
            Err(ConnectError::RootCert)
        ));
        let plain: PgOptions = "postgres://u@h/db?sslmode=verify-full".parse().unwrap();
        assert!(plain.tls_connector().is_ok(), "the Mozilla roots alone");
    }
}
