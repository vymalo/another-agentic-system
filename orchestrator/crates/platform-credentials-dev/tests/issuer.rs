//! The adapter against an in-process stand-in of the development issuer's `/token`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::post;
use orch_platform_credentials_dev::{BuildError, DevConfig, DevCredentials};
use orch_ports::{CredentialsError, PlatformCredentials};
use secrecy::SecretString;

const SECRET: &str = "dev-client-secret-value";

type Seen = Arc<Mutex<Vec<(String, String)>>>;

async fn token(State(seen): State<Seen>, headers: HeaderMap, body: String) -> (StatusCode, String) {
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    seen.lock().unwrap().push((auth.clone(), body.clone()));
    // "dev-chat:dev-client-secret-value" in base64.
    if auth != "Basic ZGV2LWNoYXQ6ZGV2LWNsaWVudC1zZWNyZXQtdmFsdWU=" {
        return (StatusCode::UNAUTHORIZED, "{}".to_owned());
    }
    if body.contains("audience=broken") {
        return (StatusCode::OK, r#"{"token_type":"Bearer"}"#.to_owned());
    }
    (
        StatusCode::OK,
        r#"{"access_token":"issued-token","token_type":"Bearer"}"#.to_owned(),
    )
}

async fn serve(status: Option<StatusCode>) -> (String, Seen) {
    let seen = Seen::default();
    let app = match status {
        None => Router::new().route("/token", post(token)),
        Some(code) => Router::new().route("/token", post(move || async move { code })),
    }
    .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, seen)
}

fn config(url: &str, secret: &str, audience: Option<&str>) -> DevConfig {
    DevConfig {
        url: url.to_owned(),
        client_id: "dev-chat".to_owned(),
        client_secret: SecretString::from(secret.to_owned()),
        audience: audience.map(str::to_owned),
        timeout: Duration::from_secs(2),
    }
}

#[tokio::test]
async fn asks_for_a_client_credentials_token_and_gives_it() {
    let (url, seen) = serve(None).await;
    let creds = DevCredentials::new(config(&format!("{url}/"), SECRET, Some("platform"))).unwrap();
    let token = creds.token().await.unwrap();
    assert_eq!(token.expose(), "issued-token");
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].1.contains("grant_type=client_credentials"));
    assert!(seen[0].1.contains("audience=platform"));
}

#[tokio::test]
async fn a_refused_client_is_refused_and_a_failing_issuer_is_unavailable() {
    let (url, _) = serve(None).await;
    let wrong = DevCredentials::new(config(&url, "another-secret", None)).unwrap();
    assert!(matches!(
        wrong.token().await,
        Err(CredentialsError::Refused)
    ));

    let (url, _) = serve(Some(StatusCode::INTERNAL_SERVER_ERROR)).await;
    let down = DevCredentials::new(config(&url, SECRET, None)).unwrap();
    assert!(matches!(
        down.token().await,
        Err(CredentialsError::Unavailable { .. })
    ));

    let nobody = DevCredentials::new(config("http://127.0.0.1:1", SECRET, None)).unwrap();
    assert!(matches!(
        nobody.token().await,
        Err(CredentialsError::Unavailable { .. })
    ));
}

#[tokio::test]
async fn an_answer_with_no_token_is_invalid() {
    let (url, _) = serve(None).await;
    let creds = DevCredentials::new(config(&url, SECRET, Some("broken"))).unwrap();
    assert!(matches!(
        creds.token().await,
        Err(CredentialsError::Invalid)
    ));
}

#[tokio::test]
async fn no_error_and_no_debug_holds_the_secret() {
    let (url, _) = serve(None).await;
    let cfg = config(&url, "another-secret", None);
    assert!(!format!("{cfg:?}").contains("another-secret"));
    let creds = DevCredentials::new(cfg).unwrap();
    assert!(!format!("{creds:?}").contains("another-secret"));
    let error = creds.token().await.unwrap_err();
    assert!(!format!("{error:?} {error}").contains("another-secret"));
}

#[test]
fn a_url_that_is_not_plain_http_is_refused() {
    for url in ["ftp://issuer", "http://u:p@issuer", "issuer", ""] {
        assert!(
            matches!(
                DevCredentials::new(config(url, SECRET, None)),
                Err(BuildError::BadUrl)
            ),
            "{url}"
        );
    }
}
