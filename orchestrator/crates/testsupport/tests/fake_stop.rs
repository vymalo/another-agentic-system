//! A stopped fake agent is unreachable, also over a connection a client opened before the stop.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_testsupport::fake::{FakeAgent, FakeAgentOptions};

#[tokio::test]
async fn a_stopped_agent_does_not_answer_on_a_pooled_connection() {
    let agent = FakeAgent::spawn(FakeAgentOptions::default()).await;
    // one client, so the second request reuses the keep-alive connection of the first
    let client = reqwest::Client::new();
    let card = client.get(agent.card_url()).send().await.unwrap();
    assert!(card.status().is_success());
    card.bytes().await.unwrap();

    agent.stop();

    let after = client.get(agent.card_url()).send().await;
    assert!(after.is_err(), "the stopped agent answered: {after:?}");
}
