//! What the authenticator writes to the log: a warning, once, for an issuer or a key set served over plain http,
//! and no URL in any line, whatever fails (a `jwksUrl` is the operator's to write and may hold a secret in its
//! query).
//!
//! In a test binary of its own: a callsite that another test of the same process hit first, while no subscriber
//! was installed, is cached as uninteresting and logs nothing here.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use std::io::Write;
use std::sync::{Arc, Mutex};

use common::{authenticator, check, good_claims, serve_keys, token};
use orch_auth_jwt::{JwtAuth, JwtConfig};
use orch_ports::Authenticator;
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Capture {
    type Writer = Capture;
    fn make_writer(&'a self) -> Capture {
        self.clone()
    }
}

#[tokio::test]
async fn plain_http_is_warned_about_once_and_no_url_is_logged() {
    let capture = Capture::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(capture.clone())
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    // An issuer over https, and a key set over https, are not warned about.
    let https = JwtConfig::new("https://idp.example/realms/x", ["api"]);
    JwtAuth::new(https).unwrap();
    let https_keys = JwtConfig::new("https://idp.example/realms/x", ["api"])
        .with_jwks_url("https://keys.example/jwks");
    JwtAuth::new(https_keys).unwrap();
    assert!(!capture.text().contains("plain http"), "{}", capture.text());

    // An issuer over plain http is warned about once, when the authenticator is built, and the line does not name
    // the issuer.
    let plain = JwtConfig::new("http://keycloak.keycloak.svc:8080/realms/x", ["api"]);
    JwtAuth::new(plain).unwrap();
    let log = capture.text();
    assert_eq!(
        log.matches("plain http").count(),
        1,
        "one warning for an http issuer: {log}"
    );
    assert!(log.contains("WARN"), "{log}");
    assert!(
        !log.contains("keycloak"),
        "the warning names the issuer: {log}"
    );

    // So is an https issuer whose keys are served over plain http.
    let before = capture.text().len();
    let plain_keys = JwtConfig::new("https://idp.example/realms/x", ["api"])
        .with_jwks_url("http://keys.internal:8080/jwks?key=url-secret-5d2e");
    JwtAuth::new(plain_keys).unwrap();
    let log = capture.text()[before..].to_owned();
    assert_eq!(log.matches("plain http").count(), 1, "{log}");
    assert!(
        !log.contains("keys.internal") && !log.contains("url-secret-5d2e"),
        "the warning names the URL: {log}"
    );

    // A secret in the query of a configured URL is in no log line, whatever fails: here the connection, then the
    // answer.
    const QUERY_SECRET: &str = "query-secret-3f9a1c";
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = closed.local_addr().unwrap();
    drop(closed);
    let config = JwtConfig::new(format!("http://{address}"), ["api"])
        .with_jwks_url(format!("http://{address}/jwks?token={QUERY_SECRET}"))
        .without_system_proxy();
    let auth = JwtAuth::new(config).unwrap();
    assert!(auth.ready().await.is_err());

    let base = serve_keys(Vec::new()).await;
    let config =
        common::config(&base).with_jwks_url(format!("{base}/missing?token={QUERY_SECRET}"));
    assert!(JwtAuth::new(config).unwrap().ready().await.is_err());
    let config = common::config(&base).with_jwks_url(format!("{base}/jwks?token={QUERY_SECRET}"));
    assert!(JwtAuth::new(config).unwrap().ready().await.is_err());

    // A token that is refused is in no line either, nor are its claims.
    let auth = authenticator(&base);
    let mut claims = good_claims(&base);
    claims.insert("email".into(), "someone-secret@example.com".into());
    let refused = token("EdDSA", "unknown-kid-7c1e9d", &claims, |_| vec![0; 64]);
    assert!(check(&auth, &refused).await.is_err());

    let log = capture.text();
    assert!(
        log.contains("cannot be fetched"),
        "the paths that log were taken: {log}"
    );
    for secret in [QUERY_SECRET, "someone-secret", "unknown-kid-7c1e9d"] {
        assert!(!log.contains(secret), "the log shows `{secret}`: {log}");
    }
}
