//! The JWT authenticator's use of the issuer: where the keys come from, when they are fetched
//! again, what happens when the issuer is odd or down, and what is refused at build time.
#![cfg(feature = "testkit")]
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use orch_auth_jwt::testkit::TestIdp;
use orch_auth_jwt::{BuildError, JwtAuth, JwtConfig, MAX_TOKEN_BYTES};
use orch_ports::testkit::bearer::{Alg, Signing};
use orch_ports::{AuthError, Authenticator, Credentials};
use serde_json::{Value, json};

const AUDIENCE: &str = "orchestrator-web";
const ALICE: &str = "alice@example.com";

fn config(idp: &TestIdp) -> JwtConfig {
    JwtConfig::new(idp.issuer(), [AUDIENCE]).without_system_proxy()
}

async fn check(auth: &JwtAuth, token: &str) -> Result<orch_ports::Principal, AuthError> {
    auth.authenticate(&Credentials {
        bearer: Some(token),
        identity_header: None,
    })
    .await
}

fn is_unavailable<T: std::fmt::Debug>(result: &Result<T, AuthError>) -> bool {
    matches!(result, Err(AuthError::Unavailable { .. }))
}

#[tokio::test]
async fn the_keys_come_from_the_discovery_document_and_are_fetched_on_first_use() {
    let idp = TestIdp::start().await;
    let auth = JwtAuth::new(config(&idp)).unwrap();
    assert_eq!(idp.jwks_fetches(), 0, "building reads nothing");
    let token = idp.token(AUDIENCE, ALICE);
    assert_eq!(check(&auth, &token).await.unwrap().user.as_str(), ALICE);
    assert_eq!((idp.discovery_fetches(), idp.jwks_fetches()), (1, 1));
    // The copy is fresh: no more requests.
    for _ in 0..3 {
        check(&auth, &token).await.unwrap();
    }
    assert_eq!((idp.discovery_fetches(), idp.jwks_fetches()), (1, 1));
}

#[tokio::test]
async fn a_configured_jwks_url_needs_no_discovery() {
    let idp = TestIdp::start().await;
    idp.set_discovery_down(true);
    let auth = JwtAuth::new(config(&idp).with_jwks_url(idp.jwks_url())).unwrap();
    check(&auth, &idp.token(AUDIENCE, ALICE)).await.unwrap();
    assert_eq!(idp.discovery_fetches(), 0);
}

#[tokio::test]
async fn the_keys_are_fetched_again_when_they_are_old() {
    let idp = TestIdp::start().await;
    let mut cfg = config(&idp);
    cfg.refresh_interval = Duration::from_millis(200);
    let auth = JwtAuth::new(cfg).unwrap();
    let token = idp.token(AUDIENCE, ALICE);
    check(&auth, &token).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    check(&auth, &token).await.unwrap();
    assert_eq!(idp.jwks_fetches(), 2);
}

#[tokio::test]
async fn concurrent_first_requests_make_one_fetch() {
    let idp = TestIdp::start().await;
    let auth = JwtAuth::new(config(&idp)).unwrap();
    let token = Arc::new(idp.token(AUDIENCE, ALICE));
    let mut tasks = Vec::new();
    for _ in 0..24 {
        let (auth, token) = (auth.clone(), Arc::clone(&token));
        tasks.push(tokio::spawn(
            async move { check(&auth, &token).await.is_ok() },
        ));
    }
    for task in tasks {
        assert!(task.await.unwrap());
    }
    assert_eq!(idp.jwks_fetches(), 1);
}

#[tokio::test]
async fn a_failed_fetch_is_not_repeated_within_the_retry_interval() {
    let idp = TestIdp::start().await;
    idp.set_jwks_down(true);
    let mut cfg = config(&idp);
    cfg.retry_interval = Duration::from_millis(400);
    let auth = JwtAuth::new(cfg).unwrap();
    let token = idp.token(AUDIENCE, ALICE);
    for _ in 0..6 {
        assert!(is_unavailable(&check(&auth, &token).await));
        assert!(auth.ready().await.is_err());
    }
    assert_eq!(idp.jwks_fetches(), 1, "the issuer is not hammered");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(is_unavailable(&check(&auth, &token).await));
    assert_eq!(idp.jwks_fetches(), 2);
    idp.set_jwks_down(false);
    tokio::time::sleep(Duration::from_millis(500)).await;
    check(&auth, &token).await.unwrap();
    auth.ready().await.unwrap();
}

#[tokio::test]
async fn keys_that_cannot_be_refreshed_go_on_verifying_for_the_grace_and_no_longer() {
    let idp = TestIdp::start().await;
    let mut cfg = config(&idp);
    cfg.refresh_interval = Duration::from_millis(300);
    cfg.stale_grace = Duration::from_millis(1000);
    cfg.retry_interval = Duration::from_millis(50);
    let auth = JwtAuth::new(cfg).unwrap();
    let token = idp.token(AUDIENCE, ALICE);
    check(&auth, &token).await.unwrap();

    idp.set_jwks_down(true);
    tokio::time::sleep(Duration::from_millis(400)).await;
    // Older than the refresh interval, the refresh fails, the copy is still inside the grace.
    check(&auth, &token).await.expect("the stale copy verifies");
    assert!(idp.jwks_fetches() >= 2, "a refresh was tried");
    auth.ready().await.expect("ready while the copy verifies");

    tokio::time::sleep(Duration::from_millis(1200)).await;
    // Past refresh + grace: fail closed.
    assert!(is_unavailable(&check(&auth, &token).await));
    assert!(auth.ready().await.is_err());

    idp.set_jwks_down(false);
    tokio::time::sleep(Duration::from_millis(100)).await;
    check(&auth, &token).await.unwrap();
}

/// An issuer that is not quite one: `routes` build what it serves at `/jwks` and `/discovery`.
async fn odd_issuer(router: impl FnOnce(String) -> Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = router(base.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

fn odd_config(issuer: &str) -> JwtConfig {
    JwtConfig::new(issuer, [AUDIENCE]).without_system_proxy()
}

async fn unavailable_with(auth: JwtAuth, token: &str) {
    let result = check(&auth, token).await;
    assert!(is_unavailable(&result), "{result:?}");
    assert!(auth.ready().await.is_err());
}

#[tokio::test]
async fn a_discovery_document_for_another_issuer_is_not_trusted() {
    let idp = TestIdp::start().await;
    let base = odd_issuer(|base| {
        Router::new().route(
            "/.well-known/openid-configuration",
            get(move || {
                let jwks = format!("{base}/jwks");
                async move {
                    axum::Json(json!({"issuer": "https://elsewhere.example", "jwks_uri": jwks}))
                }
            }),
        )
    })
    .await;
    unavailable_with(
        JwtAuth::new(odd_config(&base)).unwrap(),
        &idp.token(AUDIENCE, ALICE),
    )
    .await;
}

#[tokio::test]
async fn a_discovery_document_without_a_jwks_uri_is_unusable() {
    let idp = TestIdp::start().await;
    let base = odd_issuer(|base| {
        Router::new().route(
            "/.well-known/openid-configuration",
            get(move || {
                let issuer = base.clone();
                async move { axum::Json(json!({ "issuer": issuer })) }
            }),
        )
    })
    .await;
    unavailable_with(
        JwtAuth::new(odd_config(&base)).unwrap(),
        &idp.token(AUDIENCE, ALICE),
    )
    .await;
}

#[tokio::test]
async fn a_redirect_is_never_followed() {
    let idp = TestIdp::start().await;
    let target = idp.jwks_url();
    let base = odd_issuer(move |_| {
        Router::new().route(
            "/jwks",
            get(move || {
                let target = target.clone();
                async move { (StatusCode::FOUND, [(header::LOCATION, target)]).into_response() }
            }),
        )
    })
    .await;
    let cfg = odd_config(&base).with_jwks_url(format!("{base}/jwks"));
    unavailable_with(JwtAuth::new(cfg).unwrap(), &idp.token(AUDIENCE, ALICE)).await;
    assert_eq!(idp.jwks_fetches(), 0, "the redirect was not followed");
}

#[tokio::test]
async fn an_answer_over_a_mebibyte_is_refused() {
    let idp = TestIdp::start().await;
    let base = odd_issuer(|_| {
        Router::new().route(
            "/jwks",
            get(|| async { (StatusCode::OK, "x".repeat(1024 * 1024 + 1)) }),
        )
    })
    .await;
    let cfg = odd_config(&base).with_jwks_url(format!("{base}/jwks"));
    unavailable_with(JwtAuth::new(cfg).unwrap(), &idp.token(AUDIENCE, ALICE)).await;
}

#[tokio::test]
async fn an_answer_that_is_not_a_key_set_or_has_no_usable_key_is_unavailable() {
    let idp = TestIdp::start().await;
    for body in [
        json!("text"),
        json!({"keys": "no"}),
        json!({"keys": []}),
        // A symmetric key is no key for a token of the allowed algorithms.
        json!({"keys": [{"kty": "oct", "kid": "pub-rsa", "k": "c2VjcmV0c2VjcmV0c2VjcmV0"}]}),
    ] {
        let served = body.clone();
        let base = odd_issuer(move |_| {
            Router::new().route(
                "/jwks",
                get(move || {
                    let body = served.clone();
                    async move { axum::Json(body) }
                }),
            )
        })
        .await;
        let cfg = odd_config(&base).with_jwks_url(format!("{base}/jwks"));
        unavailable_with(JwtAuth::new(cfg).unwrap(), &idp.token(AUDIENCE, ALICE)).await;
    }
}

#[tokio::test]
async fn a_key_the_build_cannot_use_beside_a_good_one_does_not_spoil_the_set() {
    let idp = TestIdp::start().await;
    let real: Value = reqwest::Client::new()
        .get(idp.jwks_url())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mut keys = real["keys"].as_array().unwrap().clone();
    keys.insert(
        0,
        json!({"kty": "oct", "kid": "pub-rsa", "k": "c2VjcmV0c2VjcmV0c2VjcmV0"}),
    );
    keys.insert(0, json!({"kty": "RSA", "kid": "broken"}));
    let served = json!({ "keys": keys });
    let base = odd_issuer(move |_| {
        Router::new().route(
            "/jwks",
            get(move || {
                let body = served.clone();
                async move { axum::Json(body) }
            }),
        )
    })
    .await;
    let cfg = odd_config(&base).with_jwks_url(format!("{base}/jwks"));
    // The token's issuer is the test issuer's, so the claims are set to the odd one's.
    let auth = JwtAuth::new(JwtConfig {
        issuer: idp.issuer(),
        ..cfg
    })
    .unwrap();
    for alg in [Alg::Rs256, Alg::Es256, Alg::EdDsa] {
        let token = idp.mint(&idp.claims(AUDIENCE, ALICE), Signing::Published(alg));
        check(&auth, &token)
            .await
            .unwrap_or_else(|e| panic!("{alg:?}: {e:?}"));
    }
}

#[tokio::test]
async fn a_token_over_the_cap_is_refused_without_a_fetch() {
    let idp = TestIdp::start().await;
    let auth = JwtAuth::new(config(&idp)).unwrap();
    let huge = "a".repeat(MAX_TOKEN_BYTES + 1);
    assert!(matches!(
        check(&auth, &huge).await,
        Err(AuthError::Invalid { .. })
    ));
    assert_eq!(idp.jwks_fetches(), 0);
}

#[tokio::test]
async fn an_issuer_with_a_trailing_slash_is_discovered_without_a_double_slash() {
    let idp = TestIdp::start().await;
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    // The issuer is written with a slash; its discovery document must say exactly that.
    let base = odd_issuer(move |base| {
        let issuer = format!("{base}/");
        Router::new().route(
            "/.well-known/openid-configuration",
            get(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                let issuer = issuer.clone();
                let jwks = format!("{}jwks", issuer);
                async move { axum::Json(json!({"issuer": issuer, "jwks_uri": jwks})) }
            }),
        )
    })
    .await;
    let auth = JwtAuth::new(odd_config(&format!("{base}/"))).unwrap();
    let _ = check(&auth, &idp.token(AUDIENCE, ALICE)).await;
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "the document was found at one slash"
    );
}

#[test]
fn a_configuration_that_cannot_work_is_refused_at_build_time() {
    let ok = || JwtConfig::new("https://idp.example/realms/x", [AUDIENCE]);
    assert!(JwtAuth::new(ok()).is_ok());
    for (issuer, why) in [
        ("ftp://idp.example", "scheme"),
        ("idp.example", "no scheme"),
        ("https://user:pw@idp.example", "credentials"),
        ("https://idp.example/?a=b", "query"),
        ("https://idp.example/#f", "fragment"),
        ("", "empty"),
    ] {
        let err = JwtAuth::new(JwtConfig::new(issuer, [AUDIENCE])).unwrap_err();
        assert!(matches!(err, BuildError::BadIssuer), "{why}: {err:?}");
    }
    let none: [&str; 0] = [];
    assert!(matches!(
        JwtAuth::new(JwtConfig::new("https://idp.example", none)).unwrap_err(),
        BuildError::NoAudience
    ));
    assert!(matches!(
        JwtAuth::new(JwtConfig::new("https://idp.example", [" "])).unwrap_err(),
        BuildError::NoAudience
    ));
    assert!(matches!(
        JwtAuth::new(ok().with_user_claim(" ")).unwrap_err(),
        BuildError::EmptyClaim
    ));
    assert!(matches!(
        JwtAuth::new(ok().with_roles_claim("")).unwrap_err(),
        BuildError::EmptyClaim
    ));
    assert!(matches!(
        JwtAuth::new(ok().with_jwks_url("file:///etc/passwd")).unwrap_err(),
        BuildError::BadJwksUrl
    ));
}
