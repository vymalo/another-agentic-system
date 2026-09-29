//! Auth: fail-closed identity and per-user isolation.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use reqwest::Method;
use serde_json::json;
use support::*;

const RANDOM: &str = "0190aaaa-0000-7000-8000-000000000123";

fn every_route(id: &str) -> Vec<(Method, String)> {
    vec![
        (Method::GET, "/api/agents".into()),
        (Method::GET, "/api/threads".into()),
        (Method::POST, "/api/threads".into()),
        (Method::GET, format!("/api/threads/{id}")),
        (Method::GET, format!("/api/threads/{id}/events")),
        (Method::GET, format!("/api/threads/{id}/stream")),
        (Method::POST, format!("/api/threads/{id}/messages")),
        (Method::POST, format!("/api/threads/{id}/cancel")),
        // Unknown paths and wrong methods are refused the same way: nothing is reachable anonymously.
        (Method::GET, "/api/unknown".into()),
        (Method::GET, "/".into()),
        (Method::DELETE, format!("/api/threads/{id}")),
        (Method::PUT, "/api/threads".into()),
    ]
}

#[tokio::test]
async fn no_identity_is_401_everywhere_except_the_probes() {
    let h = Harness::start().await;
    let id = h.create(ALICE, "plain", "echo hi").await;
    for (method, path) in every_route(&id) {
        let r = h
            .send(
                method.clone(),
                &path,
                None,
                Some(json!({"text": "x", "target": {"agentId": "plain"}})),
            )
            .await;
        assert_eq!(r.status, 401, "{method} {path}");
        assert!(
            r.content_type.starts_with("application/problem+json"),
            "{method} {path}"
        );
        let p = r.json();
        assert_eq!(p["status"], 401);
        assert_eq!(p["title"], "Unauthorized");
    }
    for path in ["/healthz", "/readyz"] {
        let r = h.get(path, None).await;
        assert_eq!(r.status, 200, "{path}");
    }
    // Nothing was created or changed by the anonymous requests.
    assert_eq!(
        h.get("/api/threads", Some(ALICE))
            .await
            .json()
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn blank_or_malformed_identity_is_401() {
    let h = Harness::start().await;
    for value in ["", "   ", "not-an-email"] {
        let r = h
            .client
            .get(h.url("/api/agents"))
            .header("X-Auth-Request-Email", value)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status().as_u16(), 401, "{value:?}");
    }
}

#[tokio::test]
async fn a_user_cannot_see_another_users_thread() {
    let h = Harness::start().await;
    let id = h.create(ALICE, "plain", "echo private").await;
    h.wait_state(ALICE, &id, "done").await;
    for (method, path) in every_route(&id).into_iter().take(8) {
        if !path.contains(&id) {
            continue;
        }
        let r = h
            .send(method.clone(), &path, Some(BOB), Some(json!({"text": "x"})))
            .await;
        assert_eq!(r.status, 404, "{method} {path}");
        assert!(r.content_type.starts_with("application/problem+json"));
    }
    // Same as a thread that does not exist.
    let ghost = h.get(&format!("/api/threads/{RANDOM}"), Some(BOB)).await;
    let foreign = h.get(&format!("/api/threads/{id}"), Some(BOB)).await;
    assert_eq!(ghost.status, foreign.status);
    assert_eq!(ghost.json(), foreign.json());
    // Bob's list excludes it, and his own threads are his.
    assert!(
        h.get("/api/threads", Some(BOB))
            .await
            .json()
            .as_array()
            .unwrap()
            .is_empty()
    );
    let bobs = h.create(BOB, "plain", "echo mine").await;
    let list = h.get("/api/threads", Some(BOB)).await.json();
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["id"], bobs.as_str());
    // Alice still has hers untouched (Bob's cancel attempt did nothing).
    assert_eq!(h.state(ALICE, &id).await, "done");
}

#[tokio::test]
async fn identity_is_case_and_space_insensitive() {
    let h = Harness::start().await;
    let id = h.create("Alice@Example.COM", "plain", "echo case").await;
    let r = h
        .get(&format!("/api/threads/{id}"), Some("  alice@example.com "))
        .await;
    assert_eq!(r.status, 200);
    let ev = h.events("ALICE@example.com", &id).await;
    assert_eq!(ev[0]["actor"]["name"], "alice@example.com");
}

#[tokio::test]
async fn dev_user_applies_only_when_configured() {
    let with = Harness::start_with(dev_config(Some("dev@example.com"))).await;
    let r = with.get("/api/agents", None).await;
    assert_eq!(r.status, 200);
    let r = with
        .post(
            "/api/threads",
            None,
            json!({"target": {"agentId": "plain"}, "text": "echo dev"}),
        )
        .await;
    assert_eq!(r.status, 201);
    let id = r.json()["id"].as_str().unwrap().to_owned();
    let ev = with.events("dev@example.com", &id).await;
    assert_eq!(ev[0]["actor"]["name"], "dev@example.com");
    // A real identity still wins, and a malformed header is not papered over by the dev user.
    let r = with.get(&format!("/api/threads/{id}"), Some(ALICE)).await;
    assert_eq!(r.status, 404);
    let r = with
        .client
        .get(with.url("/api/agents"))
        .header("X-Auth-Request-Email", "garbage")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 401);

    let without = Harness::start_with(dev_config(None)).await;
    assert_eq!(without.get("/api/agents", None).await.status, 401);
}

#[tokio::test]
async fn probes_report_readiness_and_shutdown() {
    let h = Harness::start().await;
    assert_eq!(h.get("/readyz", None).await.status, 200);
    h.app.set_ready(false);
    assert_eq!(h.get("/readyz", None).await.status, 503);
    h.app.set_ready(true);
    assert_eq!(h.get("/readyz", None).await.status, 200);
    assert_eq!(h.get("/healthz", None).await.status, 200);
    h.app.set_shutting_down();
    assert_eq!(h.get("/healthz", None).await.status, 503);
    // The API keeps answering while draining.
    assert_eq!(h.get("/api/agents", Some(ALICE)).await.status, 200);
}
