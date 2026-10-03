//! `ask_agent` end to end (ADR 0026), on both stores: an agent that coordinates (the fake's
//! `coordinate` script, which makes the calls an adam agent makes) asks the agents the person
//! mentioned through the orchestrator's thread-tools endpoint, each runs as a child task of the
//! thread in a context of its own, and its answer goes back to the call. The real A2A adapter, the
//! real dispatcher, the real endpoint and the AG-UI run route are in the chain; nothing is
//! scripted but the agents.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use std::time::Duration;

use common::*;
use orch_core::{AskLimits, MENTIONS_EXTENSION, THREAD_TOOLS_EXTENSION};
use orch_testsupport::{Chat, FakeAgentOptions, ask_agent, eventually};
use serde_json::{Value, json};
use uuid::Uuid;

const WAIT: Duration = Duration::from_secs(20);

fn listing_thread_tools() -> FakeAgentOptions {
    FakeAgentOptions {
        extensions: vec![THREAD_TOOLS_EXTENSION.to_owned()],
        ..FakeAgentOptions::default()
    }
}

/// `plain` (the thread's agent) lists `mentions/v1` and `thread-tools/v1`; `coder`, `researcher`
/// and `browser` list `thread-tools/v1`, so each is given the grant of an asked agent.
async fn world(backend: Backend, asks: Option<AskLimits>) -> World {
    World::with(
        backend,
        Setup {
            coder: listing_thread_tools(),
            plain: FakeAgentOptions {
                extensions: vec![
                    THREAD_TOOLS_EXTENSION.to_owned(),
                    MENTIONS_EXTENSION.to_owned(),
                ],
                ..FakeAgentOptions::default()
            },
            extra: vec![
                ("researcher", listing_thread_tools()),
                ("browser", listing_thread_tools()),
            ],
            thread_tools: true,
            asks,
            ..Setup::default()
        },
    )
    .await
}

/// Where `@name` stands in the JavaScript string `text`: its start and end in UTF-16 code units.
fn mention(text: &str, name: &str) -> Value {
    let label = format!("@{name}");
    let start = text.find(&label).expect("the label is in the text");
    let start = u32::try_from(text[..start].encode_utf16().count()).unwrap();
    let end = start + u32::try_from(label.encode_utf16().count()).unwrap();
    json!({"agentId": name, "label": label, "start": start, "end": end})
}

/// Sends `text` to `plain` over the AG-UI run route, mentioning `names`, and returns the thread
/// and the stream, which stays open until the run ends.
async fn start(chat: &Chat, text: &str, names: &[&str]) -> (String, orch_testsupport::SseClient) {
    let thread = Uuid::now_v7().to_string();
    let mentions: Vec<Value> = names.iter().map(|n| mention(text, n)).collect();
    let body = Chat::agui_input(
        &thread,
        "run-1",
        &[("m-1", text)],
        json!({"forwardedProps": {"vymalo.mentions": mentions}}),
    );
    let sse = chat.agui_run("plain", &body).await;
    assert_eq!(sse.status.as_u16(), 200);
    (thread, sse)
}

fn kinds_of<'a>(events: &'a [Value], kind: &str) -> Vec<&'a Value> {
    events.iter().filter(|e| e["kind"] == kind).collect()
}

/// The frames of `type_` in a run's stream, as `(subagentRunId, parentSubagentRunId?)`.
fn subagents(frames: &[orch_testsupport::Frame], type_: &str) -> Vec<(String, Option<String>)> {
    frames
        .iter()
        .filter(|f| f.event["type"] == type_)
        .map(|f| {
            (
                f.event["subagentRunId"].as_str().unwrap().to_owned(),
                f.event["parentSubagentRunId"].as_str().map(str::to_owned),
            )
        })
        .collect()
}

async fn the_coordinator_asks_the_agents_the_person_mentioned_in_order_and_uses_their_answers(
    backend: Backend,
) {
    let world = world(backend, None).await;
    let orch = world.instance_with_thread_tools("orch-1", true).await;
    let chat = world.chat(&orch);
    let (thread, mut sse) = start(
        &chat,
        "coordinate @coder @researcher -- go",
        &["coder", "researcher"],
    )
    .await;
    let frames = sse.collect_frames(WAIT).await;
    chat.wait_state(&thread, "done").await;

    // the agent was told it may coordinate: the card lists both extensions and the endpoint
    // offers the tool
    let calls = world.plain.executions();
    assert_eq!(calls.len(), 1);
    let told = calls[0].mentions.clone().expect("told the mentions");
    assert_eq!(told["coordinate"], json!({"tool": "ask_agent"}));
    assert!(calls[0].thread_tools.is_some());

    // each agent was asked once, as its own message, in a context of its own, nothing of the
    // thread's
    for (agent, fake) in [
        ("coder", &world.coder),
        ("researcher", world.agent("researcher")),
    ] {
        let asked = fake.executions();
        assert_eq!(asked.len(), 1, "{agent}");
        assert_eq!(asked[0].text, format!("echo {agent}"));
        assert_eq!(asked[0].context_id, format!("{thread}-ask-{agent}"));
    }

    // the log: the asks in the order the agent made them, each answered before the next
    let events = chat.events(&thread).await;
    let story: Vec<String> = events
        .iter()
        .filter(|e| e["kind"] == "ask_started" || e["kind"] == "ask_finished")
        .map(|e| {
            format!(
                "{} {} {}",
                e["kind"].as_str().unwrap(),
                e["data"]["ask"],
                e["data"]["agent"]
                    .as_str()
                    .or_else(|| e["data"]["state"].as_str())
                    .unwrap()
            )
        })
        .collect();
    assert_eq!(
        story,
        [
            "ask_started 1 coder",
            "ask_finished 1 completed",
            "ask_started 2 researcher",
            "ask_finished 2 completed"
        ]
    );
    for started in kinds_of(&events, "ask_started") {
        assert_eq!(started["data"]["by"], "main");
        assert_eq!(started["data"]["depth"], 1);
        assert_eq!(started["actor"]["name"], "plain", "the asker's words");
    }
    let answers: Vec<&str> = kinds_of(&events, "ask_finished")
        .iter()
        .map(|e| e["data"]["text"].as_str().unwrap())
        .collect();
    assert_eq!(answers, ["echo: echo coder", "echo: echo researcher"]);
    // and the agent used them: its artifact is what the calls gave it
    let artifact = kinds_of(&events, "artifact")[0];
    assert_eq!(
        artifact["data"]["text"],
        "coordinate: coder: completed echo: echo coder | researcher: completed echo: echo researcher"
    );
    // nothing of the asked agents' work is the thread's own
    assert_eq!(kinds_of(&events, "artifact").len(), 1);

    // the viewer was shown two subagents under the agent's invocation, each ended
    let started = subagents(&frames, "SUBAGENT_STARTED");
    let asked: Vec<&(String, Option<String>)> = started
        .iter()
        .filter(|(id, _)| id.starts_with("sub-ask-"))
        .collect();
    assert_eq!(asked.len(), 2, "{started:?}");
    let invocation = started
        .iter()
        .find(|(id, _)| !id.starts_with("sub-ask-"))
        .expect("the agent's invocation");
    assert!(
        asked
            .iter()
            .all(|(_, parent)| parent.as_deref() == Some(invocation.0.as_str())),
        "{started:?}"
    );
    let ended = subagents(&frames, "SUBAGENT_FINISHED");
    for id in ["sub-ask-1", "sub-ask-2"] {
        assert!(
            ended.iter().any(|(ended, _)| ended == id),
            "{id}: {ended:?}"
        );
    }
}

async fn an_asked_agent_may_ask_and_the_third_level_is_refused_and_the_persons_cancel_ends_every_ask(
    backend: Backend,
) {
    let world = world(backend, None).await;
    let orch = world.instance_with_thread_tools("orch-1", true).await;
    let chat = world.chat(&orch);
    // plain asks coder, which asks researcher, which is held (`slow`) so the chain stays open
    let (thread, mut sse) = start(
        &chat,
        "coordinate @coder>researcher!slow -- go @researcher @browser",
        &["coder", "researcher", "browser"],
    )
    .await;
    let researcher = world.agent("researcher");
    eventually("the second level is asked", || async {
        (researcher.executions().len() == 1).then_some(())
    })
    .await;

    // the asked agents' own tokens, as they were given them: ask 1 is coder at depth 1, ask 2 is
    // researcher at depth 2
    let events = chat.events(&thread).await;
    let started = kinds_of(&events, "ask_started");
    assert_eq!(started.len(), 2);
    assert_eq!(
        (
            started[0]["data"]["by"].clone(),
            started[0]["data"]["depth"].clone()
        ),
        (json!("main"), json!(1))
    );
    assert_eq!(
        (
            started[1]["data"]["by"].clone(),
            started[1]["data"]["depth"].clone()
        ),
        (json!("ask:1"), json!(2))
    );
    assert_eq!(started[1]["actor"]["name"], "coder");
    assert_eq!(researcher.executions()[0].text, "slow work");
    assert_eq!(
        world.coder.executions()[0].text,
        "coordinate researcher!slow"
    );

    // the third level is refused: researcher is at the depth limit, and the answer says so
    let grant = researcher.executions()[0].thread_tools.clone().unwrap();
    let refused = ask_agent(&grant, "browser", "echo hi", Some("deeper")).await;
    assert!(refused.is_error, "{refused:?}");
    assert_eq!(refused.text, "asks are nested at most 2 deep");
    // and so is an agent asking the one that asked it
    let cycle = ask_agent(&grant, "coder", "echo hi", Some("circle")).await;
    assert_eq!(
        cycle.text,
        "an agent cannot ask itself or an agent already in its chain"
    );
    assert_eq!(
        kinds_of(&chat.events(&thread).await, "ask_started").len(),
        2
    );

    // the person cancels: both asks end, both asked agents are told, the thread is cancelled
    assert_eq!(chat.cancel(&thread).await, 202);
    chat.wait_state(&thread, "cancelled").await;
    let frames = sse.collect_frames(WAIT).await;
    let events = chat.events(&thread).await;
    let finished = kinds_of(&events, "ask_finished");
    assert_eq!(finished.len(), 2, "{finished:?}");
    for end in &finished {
        assert_eq!(end["data"]["state"], "canceled", "{end}");
    }
    eventually("both asked agents are told to stop", || async {
        (world.coder.cancels().len() == 1 && researcher.cancels().len() == 1).then_some(())
    })
    .await;
    // the stream nested the second under the first, and ended the child before its parent
    let started = subagents(&frames, "SUBAGENT_STARTED");
    assert!(
        started.contains(&("sub-ask-2".to_owned(), Some("sub-ask-1".to_owned()))),
        "{started:?}"
    );
    let ended = subagents(&frames, "SUBAGENT_FINISHED");
    let position = |id: &str| ended.iter().position(|(e, _)| e == id).unwrap();
    assert!(position("sub-ask-2") < position("sub-ask-1"), "{ended:?}");
}

async fn a_mention_is_the_only_way_to_be_asked_and_the_limits_are_the_deployments(
    backend: Backend,
) {
    let world = world(
        backend,
        Some(AskLimits {
            per_job: 1,
            ..AskLimits::default()
        }),
    )
    .await;
    let orch = world.instance_with_thread_tools("orch-1", true).await;
    let chat = world.chat(&orch);
    // `browser` is not mentioned; `coder` is; the job may make one ask
    let (thread, mut sse) =
        start(&chat, "coordinate @coder @browser @coder -- go", &["coder"]).await;
    sse.collect_frames(WAIT).await;
    chat.wait_state(&thread, "done").await;
    let events = chat.events(&thread).await;
    let artifact = kinds_of(&events, "artifact")[0];
    assert_eq!(
        artifact["data"]["text"],
        "coordinate: coder: completed echo: echo coder \
         | browser: refused you can ask only the agents the person mentioned: coder \
         | coder: refused this job has used its 1 asks"
    );
    assert_eq!(kinds_of(&events, "ask_started").len(), 1);
    assert!(
        world.agent("browser").executions().is_empty(),
        "never asked"
    );
}

backends!(
    the_coordinator_asks_the_agents_the_person_mentioned_in_order_and_uses_their_answers,
    an_asked_agent_may_ask_and_the_third_level_is_refused_and_the_persons_cancel_ends_every_ask,
    a_mention_is_the_only_way_to_be_asked_and_the_limits_are_the_deployments,
);
