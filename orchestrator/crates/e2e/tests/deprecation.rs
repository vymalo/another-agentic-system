//! The composed process (AG-UI and the chat API mounted, as the default `ORCH_SURFACES` does):
//! the four legacy operations announce their deprecation (RFC 9745) and nothing else does.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use common::*;
use orch_testsupport::Chat;
use serde_json::json;

const DEPRECATION: &str = "@1790640000";

async fn only_the_legacy_operations_send_deprecation(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let url = |path: &str| format!("{}{path}", orch.base_url);
    let alice = "alice@example.com";
    let deprecation = |r: &reqwest::Response| {
        r.headers()
            .get("deprecation")
            .map(|v| v.to_str().unwrap().to_owned())
    };

    // The legacy four, over the wire.
    let created = http
        .post(url("/api/threads"))
        .header("X-Auth-Request-Email", alice)
        .json(&json!({"target": {"agentId": "plain"}, "text": "echo legacy"}))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status().as_u16(), 201);
    assert_eq!(deprecation(&created).as_deref(), Some(DEPRECATION));
    let id = created.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let chat_alice = chat.as_user(alice);
    chat_alice.wait_state(&id, "done").await;
    for (what, response) in [
        (
            "listEvents",
            http.get(url(&format!("/api/threads/{id}/events"))),
        ),
        (
            "streamEvents",
            http.get(url(&format!("/api/threads/{id}/stream"))),
        ),
        (
            "postMessage",
            http.post(url(&format!("/api/threads/{id}/messages")))
                .json(&json!({"text": "late"})),
        ),
    ] {
        let r = response
            .header("X-Auth-Request-Email", alice)
            .send()
            .await
            .unwrap();
        assert_eq!(deprecation(&r).as_deref(), Some(DEPRECATION), "{what}");
    }

    // The AG-UI operations and the resource API, on the same router, do not.
    let thread = "00000000-0000-7000-8000-00000000d0d0";
    let run = http
        .post(url("/agui/agents/plain"))
        .header("X-Auth-Request-Email", alice)
        .header("Accept", "text/event-stream")
        .json(&Chat::agui_input(
            thread,
            "run-1",
            &[("m1", "echo agui")],
            json!({}),
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(run.status().as_u16(), 200);
    assert_eq!(deprecation(&run), None, "runAgent");
    drop(run);
    for (what, path, want) in [
        (
            "connectThread",
            format!("/agui/threads/{thread}/connect?mode=run"),
            200,
        ),
        (
            "connectThread 404",
            "/agui/threads/00000000-0000-7000-8000-00000000d0d1/connect".to_owned(),
            404,
        ),
        (
            "getAgentCapabilities",
            "/agui/agents/plain/capabilities".to_owned(),
            200,
        ),
        ("listAgents", "/api/agents".to_owned(), 200),
        ("listThreads", "/api/threads".to_owned(), 200),
        ("getThread", format!("/api/threads/{id}"), 200),
        ("getHealth", "/healthz".to_owned(), 200),
    ] {
        let r = http
            .get(url(&path))
            .header("X-Auth-Request-Email", alice)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status().as_u16(), want, "{what}");
        assert_eq!(deprecation(&r), None, "{what}");
    }
}

backends!(only_the_legacy_operations_send_deprecation);
