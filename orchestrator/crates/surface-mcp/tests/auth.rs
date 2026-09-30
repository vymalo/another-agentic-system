//! The bearer guard of the machine route: a request without a known token is refused before any
//! tool runs and nothing is written (ADR 0019).
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use reqwest::StatusCode;
use serde_json::json;
use support::*;

fn start_job_body() -> serde_json::Value {
    json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "start_job", "arguments": {"text": "echo do it"}}
    })
}

async fn assert_unauthorized(h: &Harness, resp: reqwest::Response) {
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        resp.headers()
            .get("www-authenticate")
            .and_then(|v| v.to_str().ok()),
        Some("Bearer")
    );
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("application/problem+json")
    );
    let problem: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(problem["status"], 401);
    // Nothing ran: no job for anyone, no call to any agent.
    h.assert_nothing_written().await;
}

#[tokio::test]
async fn no_authorization_header_is_401_with_a_bearer_challenge_and_writes_nothing() {
    let h = Harness::start().await;
    let resp = h.post(&[], &start_job_body()).await;
    assert_unauthorized(&h, resp).await;
}

#[tokio::test]
async fn another_scheme_is_401() {
    let h = Harness::start().await;
    for value in [
        "Basic YWxpY2U6cGFzcw==",
        "Token alice-token",
        "alice-token",
        "Bearer",
        "Bearer ",
    ] {
        let resp = h.post(&[("Authorization", value)], &start_job_body()).await;
        assert_unauthorized(&h, resp).await;
    }
}

#[tokio::test]
async fn an_unknown_token_is_401_however_close_it_is() {
    let h = Harness::start().await;
    for token in ["nope", "alice-toke", "alice-token-", "ALICE-TOKEN", "bob"] {
        let resp = h
            .post(
                &[("Authorization", &format!("Bearer {token}"))],
                &start_job_body(),
            )
            .await;
        assert_unauthorized(&h, resp).await;
    }
}

#[tokio::test]
async fn the_challenge_covers_every_method_and_path_of_the_route() {
    let h = Harness::start().await;
    for (method, path) in [
        (reqwest::Method::GET, "/mcp"),
        (reqwest::Method::DELETE, "/mcp"),
        (reqwest::Method::POST, "/mcp/anything"),
    ] {
        let resp = h
            .http
            .request(method.clone(), format!("{}{path}", h.base))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{method} {path}");
    }
}

#[tokio::test]
async fn the_identity_header_is_never_an_identity_here() {
    // Even with AUTH_DEV_USER configured, and with the header the edge sets on other routes.
    let h = Harness::start_with(Options {
        dev_user: Some("dev@example.com"),
        ..Options::default()
    })
    .await;
    let resp = h
        .post(&[("X-Auth-Request-Email", ALICE)], &start_job_body())
        .await;
    assert_unauthorized(&h, resp).await;

    // With a token, the job is the token's user's, whatever the header says.
    let client = h.client(BOB_TOKEN).await;
    let out = call(&client, "start_job", json!({"text": "echo mine"})).await;
    assert!(!out.is_error, "{out:?}");
    assert_eq!(h.threads_of(BOB).await.len(), 1);
    assert!(h.threads_of(ALICE).await.is_empty());
    assert!(h.threads_of("dev@example.com").await.is_empty());
}

#[tokio::test]
async fn the_host_header_is_checked_and_a_stranger_writes_nothing() {
    let h = Harness::start().await;
    let resp = h
        .post(
            &[
                ("Authorization", &format!("Bearer {ALICE_TOKEN}")),
                ("Host", "evil.example.com"),
            ],
            &start_job_body(),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    h.assert_nothing_written().await;

    // A listed host passes: the tool ran.
    let resp = h
        .post(
            &[
                ("Authorization", &format!("Bearer {ALICE_TOKEN}")),
                ("Host", "localhost:8080"),
            ],
            &start_job_body(),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(h.threads_of(ALICE).await.len(), 1);
}

#[tokio::test]
async fn the_rest_of_the_service_still_needs_the_identity_header() {
    let h = Harness::start().await;
    // The machine route did not open anything else.
    for path in ["/api/agents", "/api/threads", "/mcp-not-really", "/nowhere"] {
        let resp = h
            .http
            .get(format!("{}{path}", h.base))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{path}");
    }
    // Health needs nothing, as before.
    let resp = h
        .http
        .get(format!("{}/healthz", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    // A bearer token is not an identity for the resource API.
    let resp = h
        .http
        .get(format!("{}/api/threads", h.base))
        .header("Authorization", format!("Bearer {ALICE_TOKEN}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_known_token_reaches_the_tools_over_a_stateless_server() {
    let h = Harness::start().await;
    let client = h.client(ALICE_TOKEN).await;
    let tools = client.list_all_tools().await.unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert_eq!(
        names,
        [
            "list_agents",
            "start_job",
            "get_job",
            "wait_for_job",
            "answer",
            "cancel_job"
        ]
    );
    // No session: the server never hands out an id, and a GET (the standalone stream of
    // sessions) is not offered.
    let resp = h
        .http
        .get(&h.mcp_url)
        .header("Authorization", format!("Bearer {ALICE_TOKEN}"))
        .header("Accept", "text/event-stream")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
    let resp = h
        .post(
            &[("Authorization", &format!("Bearer {ALICE_TOKEN}"))],
            &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "t", "version": "1"}}}),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers().get("mcp-session-id").is_none());
}
