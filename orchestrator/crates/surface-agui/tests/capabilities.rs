//! `GET /agui/agents/{agentId}/capabilities`: the `AgentCapabilities` document, read live.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_agui_projection::RELEASE_CHANNELS_URI;
use orch_agui_proto::testkit::assert_capabilities_json_conforms;
use serde_json::json;
use support::*;

#[tokio::test]
async fn the_document_conforms_and_describes_the_agent() {
    let h = Harness::start().await;
    let resp = h.get("/agui/agents/plain/capabilities", Some(ALICE)).await;
    assert_eq!(resp.status, 200);
    assert!(resp.content_type.starts_with("application/json"));
    assert_eq!(resp.headers["cache-control"], "no-store");
    let doc = resp.json();
    assert_capabilities_json_conforms(&doc);
    assert_eq!(doc["identity"]["name"], "Plain");
    assert_eq!(doc["identity"]["description"], "scripted agent");
    assert_eq!(doc["identity"]["version"], "1.0.0");
    assert_eq!(
        doc["transport"],
        json!({"streaming": true, "resumable": true})
    );
    assert_eq!(
        doc["humanInTheLoop"],
        json!({"supported": true, "interrupts": true})
    );
    assert_eq!(doc["multiAgent"]["subagents"][0]["name"], "plain");
    assert!(
        doc.get("custom").is_none(),
        "a card without releases declares none: {doc}"
    );
}

#[tokio::test]
async fn release_channels_are_declared_only_while_the_card_lists_them() {
    let h = Harness::start().await;
    let get = || async {
        let doc = h.get("/agui/agents/coder/capabilities", Some(ALICE)).await;
        assert_eq!(doc.status, 200);
        let doc = doc.json();
        assert_capabilities_json_conforms(&doc);
        doc
    };
    let doc = get().await;
    let releases = &doc["custom"][RELEASE_CHANNELS_URI];
    assert!(releases["defaultChannel"].is_string(), "{doc}");
    assert!(releases["channels"].is_object(), "{doc}");

    // Read live, never cached, fail closed: with the card unreadable the document is smaller,
    // and it is back when the card is.
    h.agent.set_card_down("coder", true);
    let down = get().await;
    assert!(down.get("custom").is_none(), "{down}");
    assert!(down["identity"].get("description").is_none(), "{down}");
    assert_eq!(down["identity"]["name"], "Coder");
    assert_eq!(down["transport"]["resumable"], true);
    h.agent.set_card_down("coder", false);
    assert_eq!(get().await, doc);
}

#[tokio::test]
async fn an_unknown_agent_is_a_404_and_no_identity_is_a_401() {
    let h = Harness::start().await;
    h.get("/agui/agents/nobody/capabilities", Some(ALICE))
        .await
        .problem(404);
    h.get("/agui/agents/plain/capabilities", None)
        .await
        .problem(401);
}
