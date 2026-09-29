//! Bearer auth towards the agent: the configured token reaches it; a wrong one is a delivery
//! failure, never a hang.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use common::*;
use orch_testsupport::FakeAgentOptions;

fn secured(token: &str) -> FakeAgentOptions {
    FakeAgentOptions {
        bearer: Some(token.to_owned()),
        ..FakeAgentOptions::default()
    }
}

#[tokio::test]
async fn the_bearer_token_from_the_agent_endpoint_reaches_the_agent() {
    let world = World::with(Setup {
        plain: secured("plain-secret"),
        plain_token: Some("plain-secret".to_owned()),
        ..Setup::default()
    })
    .await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let id = chat.create_thread("plain", "echo secure", None).await;
    chat.wait_state(&id, "done").await;
    assert_eq!(shape(&chat.events(&id).await), FIVE);
    let calls = world.plain.executions();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].authorization.as_deref(),
        Some("Bearer plain-secret")
    );
    assert_eq!(world.plain.unauthorized_requests(), 0);
}

#[tokio::test]
async fn cancel_and_polling_authenticate_too() {
    let world = World::with(Setup {
        plain: secured("plain-secret"),
        plain_token: Some("plain-secret".to_owned()),
        ..Setup::default()
    })
    .await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let id = chat.create_thread("plain", "slow secure", None).await;
    chat.wait_state(&id, "working").await;
    assert_eq!(chat.cancel(&id).await, 202);
    chat.wait_state(&id, "cancelled").await;
    assert_eq!(world.plain.unauthorized_requests(), 0);
    assert_eq!(world.plain.cancels().len(), 1);
    assert_eq!(
        world.plain.cancels()[0].authorization.as_deref(),
        Some("Bearer plain-secret")
    );
}

#[tokio::test]
async fn a_wrong_token_surfaces_as_a_delivery_failure_not_a_hang() {
    let world = World::with(Setup {
        plain: secured("right"),
        plain_token: Some("wrong".to_owned()),
        ..Setup::default()
    })
    .await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let id = chat.create_thread("plain", "echo denied", None).await;

    // Retried a bounded number of times, then dead-lettered: the thread blocks with an error.
    chat.wait_state(&id, "blocked").await;
    let events = chat.events(&id).await;
    assert_eq!(
        shape(&events),
        ["user_message", "error", "thread_state:blocked"]
    );
    assert_eq!(events[1]["data"]["retryable"], true);
    let message = events[1]["data"]["message"].as_str().unwrap();
    assert!(!message.is_empty(), "the failure is explained: {events:?}");
    assert!(
        world.plain.executions().is_empty(),
        "the agent never executed anything"
    );
    assert!(
        world.plain.unauthorized_requests() >= 2,
        "the delivery was retried: {}",
        world.plain.unauthorized_requests()
    );
    assert_contiguous(&events);
}

#[tokio::test]
async fn a_missing_token_is_a_delivery_failure_too() {
    let world = World::with(Setup {
        plain: secured("right"),
        plain_token: None,
        ..Setup::default()
    })
    .await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let id = chat.create_thread("plain", "echo anonymous", None).await;
    chat.wait_state(&id, "blocked").await;
    assert!(world.plain.executions().is_empty());
}

#[tokio::test]
async fn an_unreachable_agent_blocks_the_thread_after_bounded_retries() {
    let world = World::start().await;
    world.plain.stop();
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let id = chat.create_thread("plain", "echo nobody home", None).await;
    chat.wait_state(&id, "blocked").await;
    let events = chat.events(&id).await;
    assert_eq!(
        shape(&events),
        ["user_message", "error", "thread_state:blocked"]
    );
}
