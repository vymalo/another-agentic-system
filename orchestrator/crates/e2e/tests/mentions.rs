//! Mentions end to end (ADR 0026, `mentions/v1`), on both stores: the references on an AG-UI run
//! are recorded as sent, ride the outbox to the dispatcher, which names the agents from the
//! registry, and reach the agent in the message metadata through the real A2A adapter, only when
//! the agent's live card lists the extension.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use std::time::Duration;

use common::*;
use orch_core::{MENTIONS_EXTENSION, STEER_EXTENSION};
use orch_testsupport::{Call, CallKind, Chat, FakeAgentOptions};
use serde_json::{Value, json};
use uuid::Uuid;

const WAIT: Duration = Duration::from_secs(20);

/// The emoji is two UTF-16 code units, so `@coder` stands at 3..9 in the JavaScript string.
const TEXT: &str = "\u{1F604} @coder look at the build, echo it";

/// A world whose `plain` agent lists `mentions/v1` and whose `coder` does not.
async fn world(backend: Backend) -> World {
    World::with(
        backend,
        Setup {
            plain: FakeAgentOptions {
                extensions: vec![MENTIONS_EXTENSION.to_owned()],
                ..FakeAgentOptions::default()
            },
            ..Setup::default()
        },
    )
    .await
}

async fn run(chat: &Chat, agent: &str, thread: &str, text: &str, mentions: Value) -> u16 {
    let body = Chat::agui_input(
        thread,
        "run-1",
        &[("m-1", text)],
        json!({"forwardedProps": {"vymalo.mentions": mentions}}),
    );
    let mut sse = chat.agui_run(agent, &body).await;
    if sse.status.as_u16() == 200 {
        sse.collect_frames(WAIT).await;
    }
    sse.status.as_u16()
}

fn mentions_of(call: &Call) -> Value {
    call.mentions
        .clone()
        .expect("the agent was told the mentions")
}

async fn the_metadata_reaches_an_agent_that_lists_the_extension(backend: Backend) {
    let world = world(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let thread = Uuid::now_v7().to_string();
    let sent = json!([{"agentId": "coder", "label": "@coder", "start": 3, "end": 9,
        "cardUrl": world.coder.card_url()}]);
    assert_eq!(run(&chat, "plain", &thread, TEXT, sent.clone()).await, 200);
    chat.wait_state(&thread, "done").await;

    // the agent was told, in the message metadata, with what the registry says of the agent
    let calls = world.plain.executions();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        mentions_of(&calls[0]),
        json!({"mentions": [{
            "agentId": "coder",
            "name": "Coder",
            "label": "@coder",
            "start": 3,
            "end": 9,
            "cardUrl": world.coder.card_url()
        }]})
    );
    // the extension is activated, header and message
    assert!(
        calls[0].activates(MENTIONS_EXTENSION),
        "{:?}",
        calls[0].extensions_header
    );
    assert!(
        calls[0]
            .message_extensions
            .iter()
            .any(|u| u == MENTIONS_EXTENSION)
    );
    // the text is the person's, labels and all
    assert_eq!(calls[0].text, TEXT);
    // no thread-tools in this card, so nothing to coordinate with
    assert!(mentions_of(&calls[0]).get("coordinate").is_none());

    // the log holds the references as sent
    let events = chat.events(&thread).await;
    assert_eq!(events[0]["kind"], "user_message");
    assert_eq!(events[0]["data"]["mentions"], sent);
    assert_eq!(events[0]["data"]["text"], TEXT);
}

async fn an_agent_whose_card_does_not_list_it_is_sent_the_text_and_nothing_else(backend: Backend) {
    let world = world(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let thread = Uuid::now_v7().to_string();
    // the thread is `coder`'s, which does not list the extension; `plain` is mentioned
    let sent = json!([{"agentId": "plain", "label": "@plain", "start": 3, "end": 9}]);
    let text = "\u{1F604} @plain look at the build, echo it";
    assert_eq!(run(&chat, "coder", &thread, text, sent.clone()).await, 200);
    chat.wait_state(&thread, "done").await;

    let calls = world.coder.executions();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].mentions, None, "{:?}", calls[0].mentions);
    assert!(!calls[0].activates(MENTIONS_EXTENSION));
    assert!(
        !calls[0]
            .message_extensions
            .iter()
            .any(|u| u == MENTIONS_EXTENSION)
    );
    assert_eq!(calls[0].text, text, "the label stays in the text");
    // the person's references are in the log all the same: the chat shows them as chips
    let events = chat.events(&thread).await;
    assert_eq!(events[0]["data"]["mentions"], sent);
    // and the agent that was mentioned was sent nothing
    assert!(world.plain.executions().is_empty());
}

async fn a_message_that_mentions_nobody_is_the_message_it_always_was(backend: Backend) {
    let world = world(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let thread = Uuid::now_v7().to_string();
    assert_eq!(
        run(&chat, "plain", &thread, "echo hi", json!([])).await,
        200
    );
    chat.wait_state(&thread, "done").await;
    let calls = world.plain.executions();
    assert_eq!(calls[0].mentions, None);
    assert!(!calls[0].activates(MENTIONS_EXTENSION));
    let events = chat.events(&thread).await;
    assert!(events[0]["data"].get("mentions").is_none());
}

async fn a_follow_up_is_told_its_own_mentions_and_a_refused_run_writes_nothing(backend: Backend) {
    let world = world(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let thread = Uuid::now_v7().to_string();
    assert_eq!(
        run(&chat, "plain", &thread, "echo hi", json!([])).await,
        200
    );
    chat.wait_state(&thread, "done").await;

    // a mention of nobody that exists is refused before anything is written
    let before = chat.events(&thread).await.len();
    let ghost = json!([{"agentId": "ghost", "label": "@ghost", "start": 3, "end": 9}]);
    let body = Chat::agui_input(
        &thread,
        "run-2",
        &[("m-2", "\u{1F604} @ghost echo")],
        json!({"forwardedProps": {"vymalo.mentions": ghost}}),
    );
    let refused = chat.agui_post("plain", &body).await;
    assert_eq!(refused.status().as_u16(), 422);
    assert_eq!(chat.events(&thread).await.len(), before);
    assert_eq!(world.plain.executions().len(), 1);

    // the follow-up that names a real agent is told, and only it
    let body = Chat::agui_input(
        &thread,
        "run-3",
        &[("m-3", TEXT)],
        json!({"forwardedProps": {"vymalo.mentions": [
            {"agentId": "coder", "label": "@coder", "start": 3, "end": 9}]}}),
    );
    let mut sse = chat.agui_run("plain", &body).await;
    assert_eq!(sse.status.as_u16(), 200);
    sse.collect_frames(WAIT).await;
    eventually("the follow-up reaches the agent", || async {
        (world.plain.executions().len() == 2).then_some(())
    })
    .await;
    let calls = world.plain.executions();
    assert_eq!(calls[0].mentions, None);
    assert_eq!(mentions_of(&calls[1])["mentions"][0]["agentId"], "coder");
    assert_eq!(
        mentions_of(&calls[1])["mentions"].as_array().unwrap().len(),
        1
    );
}

/// A world whose `plain` agent lists `steer/v1` as well as `mentions/v1` (when `steer`), or only
/// `mentions/v1`.
async fn steering_world(backend: Backend, steer: bool) -> World {
    let mut extensions = vec![MENTIONS_EXTENSION.to_owned()];
    if steer {
        extensions.push(STEER_EXTENSION.to_owned());
    }
    World::with(
        backend,
        Setup {
            plain: FakeAgentOptions {
                extensions,
                ..FakeAgentOptions::default()
            },
            ..Setup::default()
        },
    )
    .await
}

/// A message sent while the agent works, steered, that mentions `coder` (ADR 0036, ADR 0026).
async fn steer_mentioning(chat: &Chat, thread: &str) {
    let body = Chat::agui_input(
        thread,
        "run-2",
        &[("m-2", TEXT)],
        json!({"forwardedProps": {
            "vymalo.send": "steer",
            "vymalo.mentions": [{"agentId": "coder", "label": "@coder", "start": 3, "end": 9}]
        }}),
    );
    let response = chat.agui_run("plain", &body).await;
    assert_eq!(response.status.as_u16(), 200);
}

async fn a_steered_message_tells_the_running_task_who_was_mentioned(backend: Backend) {
    let world = steering_world(backend, true).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let thread = chat
        .seed_thread("plain", "steerable refactor the parser", None)
        .await;
    chat.wait_state(&thread, "working").await;
    steer_mentioning(&chat, &thread).await;
    eventually("the task reads the message", || async {
        let events = chat.events(&thread).await;
        events
            .iter()
            .any(|e| {
                e["kind"] == "agent_message" && e["data"]["text"] == format!("steered: {TEXT}")
            })
            .then_some(())
    })
    .await;

    // into the running task, told who was mentioned as a delegation would be
    let steers: Vec<_> = world
        .plain
        .calls()
        .into_iter()
        .filter(|c| c.kind == CallKind::Steer)
        .collect();
    assert_eq!(steers.len(), 1);
    assert_eq!(steers[0].text, TEXT);
    assert_eq!(
        mentions_of(&steers[0]),
        json!({"mentions": [{
            "agentId": "coder",
            "name": "Coder",
            "label": "@coder",
            "start": 3,
            "end": 9,
            "cardUrl": world.coder.card_url()
        }]})
    );
    assert!(steers[0].activates(MENTIONS_EXTENSION));
    assert!(steers[0].activates(STEER_EXTENSION));

    // the log: one message, steered, with the reference as sent
    let events = chat.events(&thread).await;
    let said: Vec<_> = events
        .iter()
        .filter(|e| e["kind"] == "user_message")
        .collect();
    assert_eq!(said.len(), 2);
    assert_eq!(said[1]["data"]["delivery"], "steer");
    assert_eq!(said[1]["data"]["mentions"][0]["agentId"], "coder");

    // it is delivered once: the job ends as one job
    world.plain.release_gate();
    chat.wait_state(&thread, "done").await;
    assert_eq!(world.plain.executions().len(), 1, "no second task");
}

async fn a_steer_the_agent_cannot_take_reaches_it_after_the_turn_with_the_same_mentions(
    backend: Backend,
) {
    // `plain` lists `mentions/v1` and not `steer/v1`: the steer falls back to its delegation
    let world = steering_world(backend, false).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let thread = chat
        .seed_thread("plain", "gate refactor the parser", None)
        .await;
    chat.wait_state(&thread, "working").await;
    steer_mentioning(&chat, &thread).await;
    // nothing was steered into the task: the card does not list the extension
    eventually("the steer was refused and requeued", || async {
        let events = chat.events(&thread).await;
        events
            .iter()
            .any(|e| e["kind"] == "user_message" && e["data"]["delivery"] == "steer")
            .then_some(())
    })
    .await;
    assert!(
        world
            .plain
            .calls()
            .iter()
            .all(|c| c.kind != CallKind::Steer),
        "nothing reached the agent as a steer"
    );

    // after the turn the message reaches the agent as a message of its own, told the references
    world.plain.release_gate();
    eventually("the delegation reaches the agent", || async {
        (world.plain.executions().len() == 2).then_some(())
    })
    .await;
    let calls = world.plain.executions();
    assert_eq!(calls[0].mentions, None);
    assert_eq!(calls[1].text, TEXT);
    assert_eq!(
        mentions_of(&calls[1]),
        json!({"mentions": [{
            "agentId": "coder",
            "name": "Coder",
            "label": "@coder",
            "start": 3,
            "end": 9,
            "cardUrl": world.coder.card_url()
        }]})
    );
    chat.wait_state(&thread, "done").await;
}

backends!(
    the_metadata_reaches_an_agent_that_lists_the_extension,
    an_agent_whose_card_does_not_list_it_is_sent_the_text_and_nothing_else,
    a_message_that_mentions_nobody_is_the_message_it_always_was,
    a_follow_up_is_told_its_own_mentions_and_a_refused_run_writes_nothing,
    a_steered_message_tells_the_running_task_who_was_mentioned,
    a_steer_the_agent_cannot_take_reaches_it_after_the_turn_with_the_same_mentions,
);
