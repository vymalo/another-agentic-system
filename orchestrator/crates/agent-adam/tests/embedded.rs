//! The embedded build (ADR 0076): the local agents on the SQLite store, no Postgres.
#![cfg(feature = "embedded")]
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt as _;
use orch_agent_adam::{LocalAgents, LocalKind, LocalOptions};
use orch_core::{AgentId, AgentTaskState, AgentUpdate};
use orch_ports::{AgentClient as _, AgentEndpoint, SendContent, SendRequest};
use tokio_util::sync::CancellationToken;

fn echo_request(text: &str) -> SendRequest {
    SendRequest {
        endpoint: AgentEndpoint::local(AgentId::new("echo"), LocalKind::Echo.name()),
        message_id: "m-1".to_owned(),
        context_id: Some("ctx-1".to_owned()),
        task_id: None,
        reference_task_ids: Vec::new(),
        content: SendContent::Text(text.to_owned()),
        release: None,
        ui_catalog: None,
        thread_tools: None,
        history: None,
        steer: false,
        mentions: Vec::new(),
        tier: None,
        tier_cap: None,
        locale: None,
        pick: None,
    }
}

#[tokio::test]
async fn a_scripted_run_completes_on_the_sqlite_store() {
    let path = std::env::temp_dir().join(format!("orch-embedded-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let agents = Arc::new(
        LocalAgents::sqlite(
            &path,
            LocalOptions::new("embedded-test"),
            &[LocalKind::Echo],
        )
        .unwrap(),
    );
    agents.migrate().await.expect("migrate the SQLite journal");
    assert!(path.exists(), "the journal is the file");

    let stop = CancellationToken::new();
    let worker = tokio::spawn({
        let (agents, stop) = (agents.clone(), stop.clone());
        async move { agents.run(stop).await }
    });
    let mut stream = agents
        .client()
        .send_stream(echo_request("hello embedded"))
        .await
        .unwrap();
    let said = tokio::time::timeout(Duration::from_secs(30), async {
        let (mut said, mut done) = (None, false);
        while let Some(envelope) = stream.next().await {
            let envelope = envelope.expect("the stream failed");
            match envelope.update {
                Some(AgentUpdate::Artifact { text, .. }) => said = said.or(text),
                Some(AgentUpdate::Status { detail, .. }) => said = said.or(detail),
                _ => {}
            }
            done |= envelope.task_state == Some(AgentTaskState::Completed);
        }
        (said, done)
    })
    .await
    .expect("the run finished in time");
    assert!(said.1, "the run completed");
    assert!(
        said.0.is_some_and(|t| t.contains("hello embedded")),
        "the answer"
    );

    stop.cancel();
    worker.await.unwrap().unwrap();
    let _ = std::fs::remove_file(&path);
}
