//! Release channels (ADR 0008): discovery from the live card, and selection on the wire.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use common::*;
use orch_agent_a2a::RELEASE_CHANNELS_URI;
use serde_json::json;

#[tokio::test]
async fn agents_list_shows_releases_only_for_the_agent_whose_card_declares_the_extension() {
    let world = World::start().await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let (status, body) = chat.get("/api/agents").await;
    assert_eq!(status, 200);
    let agents = body.as_array().unwrap();
    assert_eq!(agents.len(), 2);
    let coder = agents.iter().find(|a| a["id"] == "coder").unwrap();
    let plain = agents.iter().find(|a| a["id"] == "plain").unwrap();

    assert_eq!(coder["releases"]["defaultChannel"], "production");
    assert_eq!(coder["releases"]["channels"]["staging"], "coder-r51");
    assert_eq!(
        coder["releases"]["revisions"],
        json!(["coder-r53", "coder-r51", "coder-r47"])
    );
    assert!(
        plain.get("releases").is_none(),
        "no extension in the card, no releases: {plain}"
    );
    assert_eq!(plain["description"], "in-process fake A2A agent");
}

#[tokio::test]
async fn the_selected_release_reaches_the_agent_and_its_revision_is_echoed_into_the_events() {
    let world = World::start().await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);

    let id = chat
        .create_thread("coder", "echo ship it", Some("staging"))
        .await;
    chat.wait_state(&id, "done").await;

    let calls = world.coder.executions();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].extensions_header,
        vec![RELEASE_CHANNELS_URI.to_owned()]
    );
    assert_eq!(calls[0].release.as_deref(), Some("staging"));

    let events = chat.events(&id).await;
    assert_eq!(shape(&events), FIVE);
    for e in events.iter().filter(|e| e["actor"]["type"] == "agent") {
        assert_eq!(e["actor"]["revision"], "coder-r51", "{e}");
    }
    let thread = chat.thread(&id).await;
    assert_eq!(thread["target"]["release"], "staging");
}

#[tokio::test]
async fn an_exact_revision_can_be_selected_too() {
    let world = World::start().await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let id = chat
        .create_thread("coder", "echo pin", Some("coder-r47"))
        .await;
    chat.wait_state(&id, "done").await;
    assert_eq!(
        world.coder.executions()[0].release.as_deref(),
        Some("coder-r47")
    );
    let events = chat.events(&id).await;
    assert_eq!(events[1]["actor"]["revision"], "coder-r47");
}

#[tokio::test]
async fn without_a_selection_no_extension_is_activated_and_the_default_runs() {
    let world = World::start().await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let id = chat.create_thread("coder", "echo default", None).await;
    chat.wait_state(&id, "done").await;
    let call = &world.coder.executions()[0];
    assert!(!call.activates_release_channels());
    assert_eq!(call.release, None);
    assert_eq!(chat.events(&id).await[1]["actor"]["revision"], "coder-r47");
}

#[tokio::test]
async fn unknown_releases_and_releases_on_plain_agents_are_refused_before_anything_is_sent() {
    let world = World::start().await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);

    let (status, body) = chat
        .try_create_thread("coder", "echo x", Some("nope"))
        .await;
    assert_eq!(status, 400, "{body}");
    let (status, body) = chat
        .try_create_thread("plain", "echo x", Some("staging"))
        .await;
    assert_eq!(status, 400, "{body}");
    assert!(world.coder.executions().is_empty());
    assert!(world.plain.executions().is_empty());
}

#[tokio::test]
async fn an_unreachable_card_makes_a_release_unselectable_but_plain_runs_still_work() {
    let world = World::start().await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    world.coder.stop();
    // Let the listener actually go away.
    eventually("the coder agent is down", || async {
        let (status, body) = chat.get("/api/agents").await;
        assert_eq!(status, 200);
        let coder = body
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["id"] == "coder")
            .unwrap()
            .clone();
        coder.get("releases").is_none().then_some(())
    })
    .await;
    let (status, _) = chat
        .try_create_thread("coder", "echo x", Some("staging"))
        .await;
    assert_eq!(status, 400, "release cannot be validated without the card");
}
