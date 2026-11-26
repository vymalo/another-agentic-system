//! The embedded path and the remote path look the same to the orchestrator (ADR 0076, ADR 0015).
//!
//! One scripted run, the `echo` agent, is driven twice over **one** runtime on the SQLite store: through the
//! in-process client, and through the A2A HTTP adapter against that runtime served as an A2A 1.0 agent. The
//! envelopes each path gives the dispatcher must be equal once what is per run (the task and context ids and the
//! idempotency keys built on them) is named by its place in the run.
#![cfg(feature = "embedded")]
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use adam_a2a::{A2aServer, AgentCardConfig, AuthConfig};
use futures::StreamExt as _;
use orch_agent_a2a::{A2aAgentClient, A2aConfig};
use orch_agent_adam::{LocalAgents, LocalKind, LocalOptions};
use orch_core::AgentId;
use orch_ports::{AgentClient, AgentEndpoint, AgentEnvelope, SendContent, SendRequest};
use tokio_util::sync::CancellationToken;

fn request(endpoint: &AgentEndpoint, context: &str, text: &str) -> SendRequest {
    SendRequest {
        endpoint: endpoint.clone(),
        message_id: format!("msg-{context}"),
        context_id: Some(context.to_owned()),
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

async fn drain(client: &impl AgentClient, req: SendRequest) -> Vec<AgentEnvelope> {
    let mut stream = client.send_stream(req).await.unwrap();
    let mut out = Vec::new();
    tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(item) = stream.next().await {
            out.push(item.unwrap());
        }
    })
    .await
    .expect("the run finished in time");
    out
}

#[tokio::test]
async fn the_local_and_the_remote_path_give_the_same_envelopes() {
    let path = std::env::temp_dir().join(format!("orch-parity-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let agents = Arc::new(
        LocalAgents::sqlite(&path, LocalOptions::new("parity"), &[LocalKind::Echo]).unwrap(),
    );
    agents.migrate().await.unwrap();
    let stop = CancellationToken::new();
    let worker = tokio::spawn({
        let (agents, stop) = (agents.clone(), stop.clone());
        async move { agents.run(stop).await }
    });

    // The same runtime, served as an A2A 1.0 agent behind a bearer.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let card = AgentCardConfig::new(
        "echo",
        "repeats the message back",
        url::Url::parse(&format!("{base}/")).unwrap(),
        "1",
    );
    let auth = AuthConfig::BearerTokens(vec!["parity-token".to_owned().into()]);
    let router = A2aServer::router(card, agents.task_backend("echo").unwrap(), auth);
    let server = tokio::spawn(async move { axum::serve(listener, router).await });

    let local = agents.client();
    let remote = A2aAgentClient::new(A2aConfig {
        use_system_proxy: false,
        ..A2aConfig::default()
    })
    .unwrap();

    let local_ep = AgentEndpoint::local(AgentId::new("echo"), LocalKind::Echo.name());
    let remote_ep = AgentEndpoint::a2a(
        AgentId::new("echo"),
        format!("{base}/.well-known/agent-card.json"),
        Some("parity-token".to_owned()),
    );
    let a = drain(&local, request(&local_ep, "ctx-local", "hello parity")).await;
    let b = drain(&remote, request(&remote_ep, "ctx-remote", "hello parity")).await;

    stop.cancel();
    let _ = worker.await;
    server.abort();
    let _ = std::fs::remove_file(&path);

    assert_eq!(a.len(), 1, "the echo run is one envelope: {a:#?}");
    assert_eq!(
        normalised(&a, "ctx-local"),
        normalised(&b, "ctx-remote"),
        "the in-process path and the A2A path must give the dispatcher the same envelopes"
    );
}

/// The envelopes as text with what is per run named by its place: the task id, the context id and the id of the
/// status message the idempotency key ends with. Everything else (state, update, revision, the key's shape) is compared as
/// it is.
fn normalised(envelopes: &[AgentEnvelope], context: &str) -> Vec<String> {
    envelopes
        .iter()
        .map(|e| {
            let mut text = format!("{e:?}");
            text = text
                .replace(&e.task_id, "<task>")
                .replace(context, "<context>");
            // `a2a:<task>:status-msg:<uuid>`: the uuid is the message the agent made for this run.
            if let Some(at) = text.find("status-msg:") {
                let start = at + "status-msg:".len();
                let end = text[start..].find('"').map_or(text.len(), |n| start + n);
                text.replace_range(start..end, "<message>");
            }
            text
        })
        .collect()
}
