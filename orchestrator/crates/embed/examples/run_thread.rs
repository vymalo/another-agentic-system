//! Runs one thread end to end on the in-process scripted agent and asserts the AG-UI events it
//! streams, with no HTTP server: the composition root of ADR 0076 decision 4.
//!
//! ```sh
//! cargo run -p orch-embed --example run_thread
//! ```

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use orch_agui_proto::{Event, RunFinishedOutcome, TextMessageRole};
use orch_app::{AgentDirectory, AgentEntry};
use orch_core::{AgentId, ThreadState, UserId};
use orch_embed::Embedded;
use orch_ports::AgentEndpoint;
use orch_ports::memory::{ScriptedAgent, stream_text};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let directory = std::env::temp_dir().join(format!("orch-embed-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&directory)?;
    let database = directory.join("orch.sqlite");

    let agent = AgentId::new("helper");
    let directory_of_agents = AgentDirectory::new(vec![AgentEntry {
        endpoint: AgentEndpoint::a2a(
            agent.clone(),
            "https://helper.invalid/.well-known/agent-card.json",
            None,
        ),
        name: "Helper".to_owned(),
    }]);
    let scripted = ScriptedAgent::new();
    let embed = Embedded::open(
        &database,
        UserId::new("local@example.invalid"),
        directory_of_agents,
        scripted.clone(),
    )
    .await?;

    // The first word scripts the fake agent: `stream` sends a reply word by word.
    let thread = embed.create_thread(&agent, "stream hello").await?;
    println!("thread {} created", thread.id);

    let events = tokio::time::timeout(Duration::from_secs(10), embed.collect_agui(thread.id))
        .await
        .map_err(|_| "the run did not finish within 10 s")??;

    for event in &events {
        println!("  {}", event.event_type().as_str());
    }

    // The events a client reads: one run, one streamed assistant message, one clean end.
    let started = events
        .iter()
        .filter(|event| matches!(event, Event::RunStarted(_)))
        .count();
    assert_eq!(started, 1, "one RUN_STARTED");

    // The viewer projection replays the whole log: the person's message and the reply.
    let starts = events
        .iter()
        .filter_map(|event| match event {
            Event::TextMessageStart(start) => Some(start),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(starts.len(), 2, "the person's message and the reply");
    let text_of = |id: &str| {
        events
            .iter()
            .filter_map(|event| match event {
                Event::TextMessageContent(content) if content.message_id.as_str() == id => {
                    Some(content.delta.as_str())
                }
                _ => None,
            })
            .collect::<String>()
    };
    let (user, assistant) = starts.iter().fold(
        (None, None),
        |(user, assistant): (Option<&str>, Option<&str>), start| match start.role {
            Some(TextMessageRole::User) => (Some(start.message_id.as_str()), assistant),
            Some(TextMessageRole::Assistant) | None => (user, Some(start.message_id.as_str())),
            _ => (user, assistant),
        },
    );
    let (Some(user), Some(assistant)) = (user, assistant) else {
        panic!("the log holds the person's message and the assistant's reply");
    };
    assert_eq!(text_of(user), "stream hello", "the message as it was sent");
    assert_eq!(
        text_of(assistant),
        stream_text(),
        "the reply as the agent sent it"
    );

    let ended = events
        .iter()
        .filter(|event| matches!(event, Event::TextMessageEnd(_)))
        .count();
    assert_eq!(ended, 2, "one TEXT_MESSAGE_END per message");

    let finished = events
        .iter()
        .filter_map(|event| match event {
            Event::RunFinished(finished) => Some(finished),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(finished.len(), 1, "one RUN_FINISHED");
    assert!(
        matches!(
            finished[0].outcome,
            None | Some(RunFinishedOutcome::Success { .. })
        ),
        "the run succeeded"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::RunError(_))),
        "no RUN_ERROR"
    );

    let record = embed.read_thread(thread.id).await?;
    assert_eq!(record.state, ThreadState::Done);

    println!("reply: {}", text_of(assistant));
    println!("{} AG-UI events, thread {:?}", events.len(), record.state);

    embed.shutdown().await;
    let _ = std::fs::remove_dir_all(&directory);
    Ok(())
}
