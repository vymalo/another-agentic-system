//! Connects to Postgres on `tokio-postgres` as the orchestrator's crates connected through sqlx 0.9
//! (ADR 0076, sys#382 E1): the same connection string means the same server, the same TLS and the
//! same checks, so moving a crate off sqlx changes no deployment's configuration.
//!
//! * **The connection string** is read as sqlx read it: the `PG*` environment variables first
//!   (`PGHOST`, `PGHOSTADDR`, `PGPORT`, `PGUSER`, `PGPASSWORD`, `PGDATABASE`, `PGAPPNAME`,
//!   `PGOPTIONS`, `PGSSLMODE`, `PGSSLROOTCERT`), a `postgres://` or `postgresql://` URL over them
//!   (its query key by key, sqlx's aliases included, the last one winning, `options` growing), then
//!   the password file (`PGPASSFILE`, `~/.pgpass`) for a password nothing gave. A key sqlx ignored
//!   is ignored. A client certificate (`sslcert`, `sslkey`, `sslpassword`, `PGSSLCERT`,
//!   `PGSSLKEY`) is refused: no deployment uses one, and sqlx would have sent it.
//! * **TLS is sqlx's**, with rustls on aws-lc-rs and the Mozilla roots (`webpki-roots`) plus the
//!   `sslrootcert` (a PEM file, or the PEM itself):
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

mod parse;

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

impl TlsMode {
    /// The `sslmode` `tokio-postgres` is given: it negotiates, rustls checks.
    fn wire(self) -> SslMode {
        match self {
            TlsMode::Disable => SslMode::Disable,
            TlsMode::Prefer => SslMode::Prefer,
            TlsMode::Require | TlsMode::VerifyCa | TlsMode::VerifyFull => SslMode::Require,
        }
    }
}

/// `sslrootcert`: a file, or the PEM itself, as sqlx's `CertificateInput` told them apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RootCert {
    Path(PathBuf),
    Pem(String),
}

impl RootCert {
    pub(crate) fn from_text(text: &str) -> Self {
        let trimmed = text.trim();
        if trimmed.starts_with("-----BEGIN") && trimmed.ends_with("-----") {
            Self::Pem(text.to_owned())
        } else {
            Self::Path(PathBuf::from(text))
        }
    }
}

/// A connection string, read: the `tokio-postgres` configuration and the TLS it asks for.
#[derive(Clone, Debug)]
pub struct PgOptions {
    config: tokio_postgres::Config,
    tls: TlsMode,
    root_cert: Option<RootCert>,
}

impl FromStr for PgOptions {
    type Err = ParseError;

    /// Reads `url` with the process's environment and password file, as sqlx did.
    fn from_str(url: &str) -> Result<Self, ParseError> {
        Ok(Self::from_parsed(parse::parse(url, &parse::Process)?))
    }
}

impl PgOptions {
    fn from_parsed(parsed: parse::Parsed) -> Self {
        let mut config = tokio_postgres::Config::new();
        match parsed.target() {
            parse::Host::Tcp(host) => config.host(&host),
            #[cfg(unix)]
            parse::Host::Socket(dir) => config.host_path(&dir),
            #[cfg(not(unix))]
            parse::Host::Socket(dir) => config.host(&dir.to_string_lossy()),
        };
        config.port(parsed.port).user(&parsed.user);
        if let Some(password) = &parsed.password {
            config.password(password);
        }
        if let Some(dbname) = &parsed.dbname {
            config.dbname(dbname);
        }
        if let Some(name) = &parsed.application_name {
            config.application_name(name);
        }
        if let Some(options) = &parsed.options {
            config.options(options);
        }
        config.ssl_mode(parsed.tls.wire());
        Self {
            config,
            tls: parsed.tls,
            root_cert: parsed.root_cert,
        }
    }

    /// The configuration a connection is made with: the one held, its `sslmode` the one
    /// [`tls`](Self::tls) needs whatever [`config_mut`](Self::config_mut) did to it.
    fn wire_config(&self) -> tokio_postgres::Config {
        let mut config = self.config.clone();
        config.ssl_mode(self.tls.wire());
        config
    }

    /// The `tokio-postgres` configuration (its `sslmode` already the one TLS needs).
    pub fn config(&self) -> &tokio_postgres::Config {
        &self.config
    }

    /// The configuration, to change (a test pins a database of its own). Its `sslmode` is not
    /// for changing: every connection is made with the one [`tls`](Self::tls) needs.
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

    /// The Mozilla roots, and the certificates of `sslrootcert` (its file read now: a pool reads
    /// it once, where sqlx read it for every connection).
    fn roots(&self) -> Result<RootCertStore, ConnectError> {
        let mut roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        if let Some(root_cert) = &self.root_cert {
            let certs: Vec<_> = match root_cert {
                RootCert::Path(path) => CertificateDer::pem_file_iter(path)
                    .map_err(|_| ConnectError::RootCert)?
                    .collect(),
                RootCert::Pem(pem) => CertificateDer::pem_slice_iter(pem.as_bytes()).collect(),
            };
            if certs.is_empty() {
                return Err(ConnectError::RootCert);
            }
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
        let (client, connection) = self.wire_config().connect(self.tls_connector()?).await?;
        tokio::spawn(async move {
            // The client sees a closed connection on its next call; the cause is only here.
            if let Err(error) = connection.await {
                tracing::warn!(%error, "a postgres connection ended with an error");
            }
        });
        Ok(client)
    }

    /// A pool of at most `max_size` connections (at least one), each made on demand and checked
    /// with an empty query before it is handed out again, as sqlx's `test_before_acquire` pinged
    /// it. `wait` bounds a wait for a free connection, the making of a new one and that check, as
    /// sqlx's `acquire_timeout` did. Unlike sqlx's, a connection is not closed after ten idle minutes
    /// or thirty in all.
    ///
    /// # Errors
    ///
    /// The TLS errors of [`tls_connector`](Self::tls_connector), and [`ConnectError::Pool`].
    pub fn pool(&self, max_size: usize, wait: Duration) -> Result<Pool, ConnectError> {
        let manager = Manager::from_config(
            self.wire_config(),
            self.tls_connector()?,
            ManagerConfig {
                recycling_method: RecyclingMethod::Verified,
            },
        );
        Pool::builder(manager)
            .max_size(max_size.max(1))
            .wait_timeout(Some(wait))
            .create_timeout(Some(wait))
            // the ping of `Verified` on a socket that is half open must not hang a call
            .recycle_timeout(Some(wait))
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
    /// A value sqlx refused: a `port` or `statement-cache-capacity` that is not a number, a
    /// `hostaddr` that is not an address, a part of the URL that is not UTF-8 once decoded.
    #[error("the parameter {0} of the connection string is not valid")]
    Parameter(&'static str),
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
    /// The server could not be reached, or it refused the connection (its authentication, a
    /// database that does not exist, its TLS).
    #[error("the database could not be connected to")]
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
                "postgres://u:secretpw@h/db?port=secretpw",
                ParseError::Parameter("port"),
            ),
        ] {
            let error = url.parse::<PgOptions>().unwrap_err();
            assert_eq!(error, want, "{url}");
            assert!(!format!("{error} {error:?}").contains("secretpw"), "{url}");
        }
    }

    #[test]
    fn a_change_to_the_configuration_cannot_undo_tls() {
        let mut options: PgOptions = "postgres://u@h/db?sslmode=verify-full".parse().unwrap();
        options
            .config_mut()
            .ssl_mode(SslMode::Disable)
            .dbname("other");
        let wire = options.wire_config();
        assert_eq!(wire.get_ssl_mode(), SslMode::Require);
        assert_eq!(wire.get_dbname(), Some("other"));
    }

    #[test]
    fn the_configuration_is_built_from_the_parts() {
        let options: PgOptions =
            "postgres://u:p@h:5433/db?application_name=app&options=-c%20a%3D1&sslmode=disable"
                .parse()
                .unwrap();
        let config = options.config();
        assert_eq!(
            config.get_hosts(),
            [tokio_postgres::config::Host::Tcp("h".into())]
        );
        assert_eq!(config.get_ports(), [5433]);
        assert_eq!(config.get_user(), Some("u"));
        assert_eq!(config.get_password(), Some(&b"p"[..]));
        assert_eq!(config.get_dbname(), Some("db"));
        assert_eq!(config.get_application_name(), Some("app"));
        assert_eq!(config.get_options(), Some("-c a=1"));
        assert_eq!(config.get_ssl_mode(), SslMode::Disable);
        let socket: PgOptions = "postgres://u@%2Frun%2Fpg/db".parse().unwrap();
        assert_eq!(
            socket.config().get_hosts(),
            [tokio_postgres::config::Host::Unix("/run/pg".into())]
        );
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
        let not_pem: PgOptions = "postgres://u@h/db?sslmode=verify-full\
             &sslrootcert=-----BEGIN%20CERTIFICATE-----%0Anot%20base64%0A-----END%20CERTIFICATE-----"
            .parse()
            .unwrap();
        assert!(matches!(
            not_pem.tls_connector(),
            Err(ConnectError::RootCert)
        ));
        let no_certificate: PgOptions = "postgres://u@h/db?sslmode=verify-ca\
             &sslrootcert=-----BEGIN%20PRIVATE%20KEY-----%0AAAAA%0A-----END%20PRIVATE%20KEY-----"
            .parse()
            .unwrap();
        assert!(
            matches!(no_certificate.tls_connector(), Err(ConnectError::RootCert)),
            "an sslrootcert that holds no certificate"
        );
    }
}
