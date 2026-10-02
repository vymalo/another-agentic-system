//! The guard of the route: a request without a good token is refused before any tool runs, with
//! the same `401` whatever is wrong, and nothing is written or called.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use jiff::Timestamp;
use orch_core::{AgentId, Caller, ThreadId};
use orch_thread_token::{Claims, ThreadToolsKeys};
use reqwest::StatusCode;
use serde_json::{Value, json};
use support::*;

fn list_body() -> Value {
    json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})
}

fn at(seconds: i64) -> Timestamp {
    Timestamp::from_second(seconds).unwrap()
}

/// The refusal every failure gets: `401`, `WWW-Authenticate: Bearer error="invalid_token"`, a
/// problem body with no detail.
async fn assert_invalid_token(resp: reqwest::Response) {
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        resp.headers()
            .get("www-authenticate")
            .and_then(|v| v.to_str().ok()),
        Some(r#"Bearer error="invalid_token""#)
    );
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("application/problem+json")
    );
    let problem: Value = resp.json().await.unwrap();
    assert_eq!(problem["status"], 401);
    assert_eq!(problem["detail"], "a valid bearer token is required");
}

/// Every request below would list the probe's tools if it got through.
async fn harness_with_probe() -> (Harness, Probe, ThreadId) {
    let probe = Probe::new("probe");
    let h = Harness::start_with(keys(), {
        let probe = probe.clone();
        |config| config.with_provider(probe)
    })
    .await;
    let thread = h.thread("plain").await;
    (h, probe, thread)
}

#[tokio::test]
async fn a_request_without_a_bearer_token_is_401_with_no_error_code() {
    let (h, probe, thread) = harness_with_probe().await;
    for headers in [
        vec![],
        vec![("Authorization", "Basic YWxpY2U6cGFzcw==")],
        vec![("Authorization", "Bearer")],
        vec![("Authorization", "Bearer ")],
        vec![("Authorization", "abc")],
    ] {
        let resp = h.post(&h.url(thread), &headers, &list_body()).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{headers:?}");
        assert_eq!(
            resp.headers()
                .get("www-authenticate")
                .and_then(|v| v.to_str().ok()),
            Some("Bearer"),
            "{headers:?}"
        );
    }
    assert!(probe.seen().is_empty());
}

#[tokio::test]
async fn two_authorization_headers_are_no_credentials() {
    let (h, probe, thread) = harness_with_probe().await;
    let good = format!("Bearer {}", grant_token(thread, "plain"));
    let resp = h
        .post(
            &h.url(thread),
            &[("Authorization", &good), ("Authorization", &good)],
            &list_body(),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(probe.seen().is_empty());
}

#[tokio::test]
async fn a_token_that_fails_any_check_is_the_same_401_and_nothing_is_called() {
    let (h, probe, thread) = harness_with_probe().await;
    let other_thread = h.thread("plain").await;
    let good = claims(thread, "plain");
    let now = Timestamp::now().as_second();

    let other_keys = ThreadToolsKeys::new(secret(OTHER_KEY), None).unwrap();
    let mut tokens: Vec<(&str, String)> = vec![
        ("garbage", "not.a.token".to_owned()),
        ("empty", "x".to_owned()),
        // signed with a key nobody here holds
        ("another key", token(&other_keys, &good)),
        // the right key, a claim the endpoint does not accept
        (
            "expired",
            token(
                &keys(),
                &Claims {
                    issued_at: at(now - 7300),
                    expires_at: at(now - 100),
                    ..good.clone()
                },
            ),
        ),
        (
            "issued in the future",
            token(
                &keys(),
                &Claims {
                    issued_at: at(now + 3600),
                    expires_at: at(now + 7200),
                    ..good.clone()
                },
            ),
        ),
        // another thread's token on this thread's URL
        ("another thread's token", grant_token(other_thread, "plain")),
        // a token minted for another agent than the thread's
        ("another agent", grant_token(thread, "coder")),
        // an ask that no ledger holds
        (
            "an ask",
            token(
                &keys(),
                &Claims {
                    caller: Caller::Ask(1),
                    depth: 1,
                    ..good.clone()
                },
            ),
        ),
        // a thread nobody created
        (
            "no such thread",
            token(
                &keys(),
                &Claims {
                    thread: ThreadId(uuid_of(0xdead_beef)),
                    agent: AgentId::new("plain"),
                    ..good.clone()
                },
            ),
        ),
    ];
    // a good token with its signature changed
    let mut forged = grant_token(thread, "plain");
    let last = forged.pop().unwrap();
    forged.push(if last == 'A' { 'B' } else { 'A' });
    tokens.push(("a changed signature", forged));

    for (what, token) in &tokens {
        let resp = h
            .post(
                &h.url(thread),
                &[("Authorization", &format!("Bearer {token}"))],
                &list_body(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{what}");
        assert_invalid_token(resp).await;
    }
    // a token for the thread named in the path of a thread that does not exist
    let nobody = ThreadId(uuid_of(0xdead_beef));
    let resp = h
        .post(
            &h.url(nobody),
            &[(
                "Authorization",
                &format!(
                    "Bearer {}",
                    token(
                        &keys(),
                        &Claims {
                            thread: nobody,
                            ..good.clone()
                        }
                    )
                ),
            )],
            &list_body(),
        )
        .await;
    assert_invalid_token(resp).await;
    // a path that does not name a thread
    let resp = h
        .post(
            &format!("{}/thread-tools/nope/mcp", h.base),
            &[(
                "Authorization",
                &format!("Bearer {}", grant_token(thread, "plain")),
            )],
            &list_body(),
        )
        .await;
    assert_invalid_token(resp).await;

    assert!(probe.seen().is_empty(), "{:?}", probe.seen());
}

fn uuid_of(bits: u128) -> uuid::Uuid {
    uuid::Uuid::from_u128(bits)
}

#[tokio::test]
async fn the_previous_key_still_opens_the_endpoint_until_it_is_removed() {
    let old = ThreadToolsKeys::new(secret(OLD_KEY), None).unwrap();
    // A replica in the middle of a rotation: the new key signs and verifies, the old verifies.
    let rotating = ThreadToolsKeys::new(secret(KEY), Some(secret(OLD_KEY))).unwrap();
    let h = Harness::start_with(rotating, |config| config).await;
    let thread = h.thread("plain").await;
    let good = claims(thread, "plain");

    // a token minted with the old key, and one with the new
    for signer in [&old, &keys()] {
        let client = connect(&h.url(thread), &token(signer, &good)).await;
        assert_eq!(tool_names(&client).await, ["get_ui_catalog", "turn_output"]);
    }

    // the rotation is over: a replica that no longer holds the old key refuses its tokens
    let h = Harness::start().await;
    let thread = h.thread("plain").await;
    let resp = h
        .post(
            &h.url(thread),
            &[(
                "Authorization",
                &format!("Bearer {}", token(&old, &claims(thread, "plain"))),
            )],
            &list_body(),
        )
        .await;
    assert_invalid_token(resp).await;
}

#[tokio::test]
async fn the_host_header_is_checked_after_the_token() {
    let (h, probe, thread) = harness_with_probe().await;
    let bearer = format!("Bearer {}", grant_token(thread, "plain"));
    // A listed host with a good token passes the guard and the host check.
    let ok = h
        .post(
            &h.url(thread),
            &[("Authorization", &bearer), ("Host", "localhost:8080")],
            &list_body(),
        )
        .await;
    assert_eq!(ok.status(), StatusCode::OK);
    // An unlisted host is refused with a good token (DNS rebinding) ...
    let bad = h
        .post(
            &h.url(thread),
            &[("Authorization", &bearer), ("Host", "evil.example.com")],
            &list_body(),
        )
        .await;
    assert_eq!(bad.status(), StatusCode::FORBIDDEN);
    // ... and a request that carries an Origin is refused (an agent is not a browser).
    let origin = h
        .post(
            &h.url(thread),
            &[
                ("Authorization", &bearer),
                ("Origin", "https://evil.example.com"),
            ],
            &list_body(),
        )
        .await;
    assert_eq!(origin.status(), StatusCode::FORBIDDEN);
    // Only the listed host's request listed the tools.
    assert_eq!(probe.seen().iter().filter(|s| s.what == "list").count(), 1);
}

#[tokio::test]
async fn the_route_is_a_machine_route_the_edge_identity_does_not_guard() {
    // No X-Auth-Request-Email, and a good token: served. A forged identity header changes nothing
    // (it is never read).
    let (h, _probe, thread) = harness_with_probe().await;
    let bearer = format!("Bearer {}", grant_token(thread, "plain"));
    for headers in [
        vec![("Authorization", bearer.as_str())],
        vec![
            ("Authorization", bearer.as_str()),
            ("X-Auth-Request-Email", "mallory@example.com"),
        ],
    ] {
        let resp = h.post(&h.url(thread), &headers, &list_body()).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }
    // And the route is not under /mcp: the MCP surface's path is not this surface's.
    let resp = h
        .post(
            &format!("{}/mcp", h.base),
            &[("Authorization", &bearer)],
            &list_body(),
        )
        .await;
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "the identity layer answers"
    );
}

#[tokio::test]
async fn config_is_checked() {
    use orch_surface_thread_tools::{ConfigError, ThreadToolsConfig};
    let no_hosts: [&str; 0] = [];
    assert_eq!(
        ThreadToolsConfig::new(keys(), no_hosts).unwrap_err(),
        ConfigError::NoAllowedHosts
    );
    for bad in [
        "*",
        "https://orch.example.com",
        "orch.example.com/",
        "user@orch",
        "",
    ] {
        assert_eq!(
            ThreadToolsConfig::new(keys(), [bad]).unwrap_err(),
            ConfigError::BadHost(bad.to_owned()),
            "{bad:?}"
        );
    }
    assert!(ThreadToolsConfig::new(keys(), ["orchestrator:8080", "localhost"]).is_ok());
    assert_eq!(
        ThreadToolsConfig::new(keys(), ["localhost"])
            .unwrap()
            .with_tool_timeout(std::time::Duration::ZERO)
            .unwrap_err(),
        ConfigError::ZeroTimeout
    );
    // `Debug` shows the key ids, never a key.
    let shown = format!(
        "{:?}",
        ThreadToolsConfig::new(keys(), ["localhost"]).unwrap()
    );
    assert!(!shown.contains(KEY), "{shown}");
}
