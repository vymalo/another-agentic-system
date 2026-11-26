//! The development credentials adapter against a stand-in token endpoint.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::{Router, extract::State, http::StatusCode, routing::post};
use orch_ports::{PlatformCredentials, PlatformCredentialsError};
use orch_provisioner_platform::{OidcClientConfig, OidcClientCredentials};
use secrecy::SecretString;
use tokio::net::TcpListener;

#[derive(Clone)]
struct Issuer {
    calls: Arc<AtomicUsize>,
    bodies: Arc<std::sync::Mutex<Vec<String>>>,
    status: StatusCode,
    expires_in: u64,
}

async fn token(State(i): State<Issuer>, body: String) -> (StatusCode, String) {
    let n = i.calls.fetch_add(1, Ordering::SeqCst) + 1;
    i.bodies.lock().unwrap().push(body);
    (
        i.status,
        format!(
            r#"{{"access_token":"tok-{n}","token_type":"Bearer","expires_in":{}}}"#,
            i.expires_in
        ),
    )
}

async fn issuer(status: StatusCode, expires_in: u64) -> (String, Issuer) {
    let state = Issuer {
        calls: Arc::default(),
        bodies: Arc::default(),
        status,
        expires_in,
    };
    let app = Router::new()
        .route("/token", post(token))
        .with_state(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/token", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, state)
}

fn config(token_url: String) -> OidcClientConfig {
    OidcClientConfig {
        token_url,
        client_id: "orchestrator".into(),
        client_secret: SecretString::from("s3cret".to_owned()),
        user: Some("owner@example.com".into()),
        audience: Some("platform".into()),
        roles: Some(vec!["platform:agents.own".into()]),
        timeout: Duration::from_secs(2),
    }
}

#[tokio::test]
async fn a_token_is_asked_for_once_while_it_is_good() {
    let (url, issuer) = issuer(StatusCode::OK, 3600).await;
    let credentials = OidcClientCredentials::new(config(url)).unwrap();
    assert_eq!(credentials.token().await.unwrap().expose(), "tok-1");
    assert_eq!(credentials.token().await.unwrap().expose(), "tok-1");
    assert_eq!(issuer.calls.load(Ordering::SeqCst), 1);
    let body = issuer.bodies.lock().unwrap()[0].clone();
    for part in [
        "grant_type=client_credentials",
        "client_id=orchestrator",
        "user=owner%40example.com",
        "audience=platform",
        "roles=platform%3Aagents.own",
    ] {
        assert!(body.contains(part), "{part} missing from {body}");
    }
}

#[tokio::test]
async fn a_token_about_to_expire_is_asked_for_again() {
    // Lives 10 s, and a token is renewed 30 s early: every call asks.
    let (url, issuer) = issuer(StatusCode::OK, 10).await;
    let credentials = OidcClientCredentials::new(config(url)).unwrap();
    assert_eq!(credentials.token().await.unwrap().expose(), "tok-1");
    assert_eq!(credentials.token().await.unwrap().expose(), "tok-2");
    assert_eq!(issuer.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_refusal_and_an_outage_are_told_apart_and_leak_nothing() {
    let (url, _) = issuer(StatusCode::UNAUTHORIZED, 3600).await;
    let refused = OidcClientCredentials::new(config(url))
        .unwrap()
        .token()
        .await;
    assert_eq!(refused.unwrap_err(), PlatformCredentialsError::Refused);

    let (url, _) = issuer(StatusCode::INTERNAL_SERVER_ERROR, 3600).await;
    let down = OidcClientCredentials::new(config(url))
        .unwrap()
        .token()
        .await;
    assert_eq!(down.unwrap_err(), PlatformCredentialsError::Unavailable);

    let unreachable = OidcClientCredentials::new(config("http://127.0.0.1:1/token".into()))
        .unwrap()
        .token()
        .await;
    assert_eq!(
        unreachable.unwrap_err(),
        PlatformCredentialsError::Unavailable
    );
}

#[test]
fn a_bad_endpoint_is_refused_at_build_and_the_secret_is_not_in_debug() {
    assert!(OidcClientCredentials::new(config("ftp://x/token".into())).is_err());
    assert!(OidcClientCredentials::new(config("http://u:p@x/token".into())).is_err());
    let text = format!("{:?}", config("http://x/token".into()));
    assert!(!text.contains("s3cret"));
}
