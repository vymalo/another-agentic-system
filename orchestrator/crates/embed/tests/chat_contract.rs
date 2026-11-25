//! The chat contract the cloud adapter shares with the HTTP API: replay from a sequence number,
//! the client message id, the limit event (402) and the version gate (426).

#![allow(clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use orch_agui_proto as agui;
use orch_app::{AgentDirectory, AgentEntry};
use orch_core::{AgentId, EventKind, UserId};
use orch_embed::{Admission, CUSTOM_LIMIT_REACHED, EmbedError, EmbedPorts, Embedded, Limit};
use orch_ports::AgentEndpoint;
use orch_ports::memory::ScriptedAgent;

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
    let dir = std::env::temp_dir().join(format!("orch-embed-c-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("orch.sqlite");
    let _ = std::fs::remove_file(&file);
    file
}

async fn open(name: &str, parts: EmbedPorts) -> (Embedded<ScriptedAgent>, AgentId) {
    let agent = AgentId::new("helper");
    let embed = Embedded::open_with(
        database(name),
        UserId::new("local@example.invalid"),
        directory(&agent),
        ScriptedAgent::new(),
        parts,
    )
    .await
    .expect("open");
    (embed, agent)
}

#[tokio::test]
async fn replay_from_a_seq_says_only_what_follows_and_reopens_the_run() {
    let (embed, agent) = open("replay", EmbedPorts::default()).await;
    let thread = embed
        .create_thread(&agent, "stream hello")
        .await
        .expect("create");
    let all: Vec<_> = tokio::time::timeout(
        Duration::from_secs(10),
        embed
            .stream_agui_from(thread.id, 0)
            .await
            .expect("stream")
            .collect::<Vec<_>>(),
    )
    .await
    .expect("run finishes");
    let points: Vec<i64> = all.iter().filter_map(|f| f.resume_id).collect();
    assert!(points.len() >= 2, "several resume points: {points:?}");
    let mid = points[points.len() / 2 - 1];
    let rest: Vec<_> = embed
        .stream_agui_from(thread.id, mid)
        .await
        .expect("stream")
        .collect()
        .await;
    assert!(matches!(
        rest.first().map(|f| &f.event),
        Some(agui::Event::RunStarted(_))
    ));
    assert!(matches!(
        rest.last().map(|f| &f.event),
        Some(agui::Event::RunFinished(_))
    ));
    let tail_ids: Vec<i64> = rest.iter().filter_map(|f| f.resume_id).collect();
    assert!(
        tail_ids.iter().all(|id| *id > mid),
        "nothing at or before the cursor: {tail_ids:?}"
    );
    assert!(rest.len() < all.len());
    embed.shutdown().await;
}

#[tokio::test]
async fn a_client_message_id_makes_a_retry_one_message() {
    let (embed, agent) = open("msgid", EmbedPorts::default()).await;
    let thread = embed
        .create_thread(&agent, "stream hello")
        .await
        .expect("create");
    embed.collect_agui(thread.id).await.expect("first run");
    let first = embed
        .post_message_with_id(thread.id, "client-1", "stream again")
        .await
        .expect("post");
    let retry = embed
        .post_message_with_id(thread.id, "client-1", "stream again")
        .await
        .expect("retry");
    assert_eq!(
        first.seq, retry.seq,
        "the retry answers with the logged message"
    );
    let events = embed.events(thread.id).await.expect("events");
    let messages = events
        .iter()
        .filter(|e| e.kind() == EventKind::UserMessage)
        .count();
    assert_eq!(
        messages, 2,
        "the thread's first message and one more, not three"
    );
    embed.shutdown().await;
}

#[derive(Debug)]
struct Full;
impl Admission for Full {
    fn admit(&self, _: &UserId) -> Result<(), Limit> {
        Err(Limit {
            code: "limit_reached".to_owned(),
            message: "no more for now".to_owned(),
        })
    }
}

#[tokio::test]
async fn a_limit_stops_new_work_and_is_one_custom_event() {
    let parts = EmbedPorts {
        admission: Some(Arc::new(Full)),
        ..EmbedPorts::default()
    };
    let (embed, agent) = open("limit", parts).await;
    let Err(EmbedError::LimitReached(limit)) = embed.create_thread(&agent, "hi").await else {
        panic!("a limit was expected");
    };
    let agui::Event::Custom(event) = limit.event() else {
        panic!("a CUSTOM event was expected");
    };
    assert_eq!(event.name, CUSTOM_LIMIT_REACHED);
    assert_eq!(event.value["status"], 402);
    assert_eq!(event.value["code"], "limit_reached");
    embed.shutdown().await;
}

#[tokio::test]
async fn an_old_or_unstated_client_is_refused_before_anything_is_read() {
    let parts = || EmbedPorts {
        min_client_version: Some("2.1.0".parse().expect("version")),
        ..EmbedPorts::default()
    };
    let (none, agent) = open("gate-none", parts()).await;
    assert!(matches!(
        none.create_thread(&agent, "hi").await,
        Err(EmbedError::ClientUpdateRequired { .. })
    ));
    none.shutdown().await;
    let (old, agent) = open("gate-old", parts()).await;
    let old = old.with_client_version("2.0.9");
    assert!(matches!(
        old.create_thread(&agent, "hi").await,
        Err(EmbedError::ClientUpdateRequired { .. })
    ));
    old.shutdown().await;
    let (ok, agent) = open("gate-ok", parts()).await;
    let ok = ok.with_client_version("2.1");
    ok.create_thread(&agent, "hi").await.expect("allowed");
    ok.shutdown().await;
}
