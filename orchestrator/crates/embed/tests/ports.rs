//! The application's own model client, file store and user-tool bundle reach the orchestrator
//! through `EmbedPorts` (ADR 0076), and the refusing ports stay the default.

#![allow(clippy::expect_used)]

use std::collections::BTreeMap;
use std::time::Duration;

use orch_app::{AgentDirectory, AgentEntry, TaskSettings};
use orch_core::{AgentId, TaskKind, UserId};
use orch_embed::{EmbedPorts, Embedded};
use orch_ports::memory::{MemoryArtifacts, ScriptedAgent, ScriptedModel};
use orch_ports::{AgentEndpoint, NoUserTools};

fn directory(agent: &AgentId) -> AgentDirectory {
    AgentDirectory::new(vec![AgentEntry {
        endpoint: AgentEndpoint::a2a(
            agent.clone(),
            "https://helper.invalid/.well-known/agent-card.json",
            None,
        ),
        name: "Helper".to_owned(),
    }])
}

fn database(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("orch-embed-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir.join("orch.sqlite")
}

#[tokio::test]
async fn the_supplied_model_titles_the_thread() {
    let agent = AgentId::new("helper");
    let model = ScriptedModel::default();
    model.then_answer("A supplied title");
    let parts = EmbedPorts {
        model: model.clone(),
        artifacts: MemoryArtifacts::new(),
        user_tools: NoUserTools::off(),
        tasks: BTreeMap::from([(
            TaskKind::Title,
            TaskSettings::new(TaskKind::Title, "app", "gateway-model"),
        )]),
        min_client_version: None,
        admission: None,
    };
    let embed = Embedded::open_with(
        database("model"),
        UserId::new("local@example.invalid"),
        directory(&agent),
        ScriptedAgent::new(),
        parts,
    )
    .await
    .expect("open");
    let thread = embed
        .create_thread(&agent, "stream hello")
        .await
        .expect("create");
    tokio::time::timeout(Duration::from_secs(10), embed.collect_agui(thread.id))
        .await
        .expect("run finishes")
        .expect("agui");
    let mut title = None;
    for _ in 0..200 {
        let record = embed.read_thread(thread.id).await.expect("read");
        if record.title == "A supplied title" {
            title = Some(record.title);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(title.as_deref(), Some("A supplied title"));
    let calls = model.calls();
    assert_eq!(calls.len(), 1, "the supplied model was asked once");
    assert_eq!(calls[0].endpoint, "app");
    embed.shutdown().await;
}

#[tokio::test]
async fn the_default_ports_ask_no_model() {
    let agent = AgentId::new("helper");
    let embed = Embedded::open(
        database("default"),
        UserId::new("local@example.invalid"),
        directory(&agent),
        ScriptedAgent::new(),
    )
    .await
    .expect("open");
    let thread = embed
        .create_thread(&agent, "stream hello")
        .await
        .expect("create");
    tokio::time::timeout(Duration::from_secs(10), embed.collect_agui(thread.id))
        .await
        .expect("run finishes")
        .expect("agui");
    embed.shutdown().await;
}
