//! What the key and claim tests share: a server that serves the key set a test builds, tokens signed as a
//! test says, and an authenticator for that issuer.
//!
//! This is not the crate's `testkit` (its issuer publishes keys of its own): these tests need the exact key
//! set and the exact claims, and they run without the feature.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::routing::get;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use orch_auth_jwt::{JwtAuth, JwtConfig};
use orch_ports::{AuthError, Authenticator, Credentials, Principal};
use serde_json::{Map, Value, json};

pub const AUDIENCE: &str = "orchestrator-web";
pub const USER: &str = "alice@example.com";

pub fn b64(bytes: impl AsRef<[u8]>) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// The address of a server that serves `keys` as its key set at `/jwks`. It is also the name of the issuer.
pub async fn serve_keys(keys: Vec<Value>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let served = json!({ "keys": keys });
    let router = Router::new().route(
        "/jwks",
        get(move || {
            let body = served.clone();
            async move { axum::Json(body) }
        }),
    );
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    base
}

/// The configuration for the issuer `base` (see [`serve_keys`]): its keys at `/jwks`, the proxy of the
/// environment ignored (the issuer is on the loopback).
pub fn config(base: &str) -> JwtConfig {
    JwtConfig::new(base, [AUDIENCE])
        .with_jwks_url(format!("{base}/jwks"))
        .without_system_proxy()
}

pub fn authenticator(base: &str) -> JwtAuth {
    JwtAuth::new(config(base)).unwrap()
}

/// The claims of a good token of the issuer `issuer`: valid for five minutes, for `AUDIENCE`, of `USER`.
pub fn good_claims(issuer: &str) -> Map<String, Value> {
    let now = now();
    json!({
        "iss": issuer,
        "aud": AUDIENCE,
        "email": USER,
        "iat": now,
        "exp": now + 300,
    })
    .as_object()
    .unwrap()
    .clone()
}

/// A token whose header names `alg` and `kid`, whose payload is `claims`, signed by `sign` over the
/// signing input.
pub fn token(
    alg: &str,
    kid: &str,
    claims: &Map<String, Value>,
    sign: impl FnOnce(&[u8]) -> Vec<u8>,
) -> String {
    let header = json!({"alg": alg, "typ": "JWT", "kid": kid});
    let input = format!(
        "{}.{}",
        b64(serde_json::to_vec(&header).unwrap()),
        b64(serde_json::to_vec(claims).unwrap())
    );
    let signature = sign(input.as_bytes());
    format!("{input}.{}", b64(signature))
}

pub async fn check(auth: &JwtAuth, token: &str) -> Result<Principal, AuthError> {
    auth.authenticate(&Credentials {
        bearer: Some(token),
        ..Credentials::default()
    })
    .await
}

#[track_caller]
pub fn assert_accepted(result: Result<Principal, AuthError>, what: &str) -> Principal {
    match result {
        Ok(principal) => principal,
        Err(error) => panic!("{what}: expected the token to be accepted, got {error:?}"),
    }
}

/// Why a token was refused as an invalid bearer; panics when it was accepted or the issuer was unavailable.
#[track_caller]
pub fn refusal(result: Result<Principal, AuthError>, what: &str) -> String {
    match result {
        Err(AuthError::Invalid { detail, .. }) => detail,
        other => panic!("{what}: expected a refusal, got {other:?}"),
    }
}

/// Polls `condition` every 10 ms until it holds, and fails after ten seconds. For what happens on a task of its
/// own (a refresh in the background), which a request that was answered at once does not wait for.
pub async fn eventually(what: &str, mut condition: impl FnMut() -> bool) {
    let give_up = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < give_up, "gave up waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// How long [`exactly`] usually lets a count settle: an extra, later request would have come by then.
pub const SETTLE: Duration = Duration::from_millis(300);

/// `count()` reaches `want` and stays there. [`eventually`] on `count() == want` is true the first time the
/// count is right and so cannot see a second, later request that a fault makes; this waits for `want`, lets
/// `settle` go by with the test asking for nothing, and asserts the count is still `want`.
pub async fn exactly(what: &str, count: impl Fn() -> usize, want: usize, settle: Duration) {
    eventually(what, || count() >= want).await;
    tokio::time::sleep(settle).await;
    assert_eq!(count(), want, "{what}: more than {want} after a settle");
}
