//! Asked agents end to end (ADR 0026), on both stores: the agent a thread runs on asks one of the
//! agents the person mentioned, and the real dispatcher sends the question through the real A2A
//! adapter to a second fake agent, in a conversation of its own, and puts what it answers in the
//! log. Nothing here is the thread's own task: the first agent waits at its gate the whole time.
//!
//! The ask is made through the core's input, as the `ask_agent` tool will make it (the tool is the
//! next change): the test plays the asking agent with a [`Node`] on the shared database.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use std::time::Duration;

use common::*;
use jiff::Timestamp;
use orch_core::{Actor, AgentId, AskLimits, Caller, Input, THREAD_TOOLS_EXTENSION, ThreadId};
use orch_testsupport::{Chat, FakeAgentOptions, FakeReleases, eventually};
use serde_json::{Value, json};
use uuid::Uuid;

/// `@coder` is at 5..11.
const TEXT: &str = "gate @coder look into it";

/// `plain` is the agent the thread runs on; `coder` is mentioned, and lists `thread-tools/v1` so
/// that it is given the grant of an asked agent.
async fn world(backend: Backend) -> World {
    World::with(
        backend,
        Setup {
            coder: FakeAgentOptions {
                releases: Some(FakeReleases::sample()),
                extensions: vec![THREAD_TOOLS_EXTENSION.to_owned()],
                ..FakeAgentOptions::default()
            },
            thread_tools: true,
            ..Setup::default()
        },
    )
    .await
}

/// A thread of `plain` that mentions `coder`, working (its task waits at the fake's gate).
async fn working_thread(chat: &Chat) -> (String, orch_testsupport::SseClient) {
    let thread = Uuid::now_v7().to_string();
    let body = Chat::agui_input(
        &thread,
        "run-1",
        &[("m-1", TEXT)],
        json!({"forwardedProps": {"vymalo.mentions": [
            {"agentId": "coder", "label": "@coder", "start": 5, "end": 11}]}}),
    );
    let sse = chat.agui_run("plain", &body).await;
    chat.wait_state(&thread, "working").await;
    (thread, sse)
}

fn ask(text: &str, key: &str, limits: AskLimits) -> Input {
    Input::Ask {
        actor: Actor::agent(&AgentId::new("plain"), None),
        caller: Caller::Main,
        agent: AgentId::new("coder"),
        text: text.to_owned(),
        call_key: Some(key.to_owned()),
        parent_step: None,
        limits,
    }
}

fn id(thread: &str) -> ThreadId {
    ThreadId(thread.parse().unwrap())
}

async fn ask_finished(chat: &Chat, thread: &str, n: u64) -> Value {
    eventually(&format!("ask {n} to finish"), || async {
        chat.events(thread)
            .await
            .into_iter()
            .find(|e| e["kind"] == "ask_finished" && e["data"]["ask"] == n)
    })
    .await
}

fn kinds_of(events: &[Value], kind: &str) -> usize {
    events.iter().filter(|e| e["kind"] == kind).count()
}

async fn the_mentioned_agent_is_asked_in_a_context_of_its_own_and_its_answer_is_in_the_log(
    backend: Backend,
) {
    let world = world(backend).await;
    let orch = world.instance_with_thread_tools("orch-1", true).await;
    let chat = world.chat(&orch);
    let (thread, _sse) = working_thread(&chat).await;
    let node = world.node("asker").await;

    node.apply(
        id(&thread),
        ask("echo what is the plan", "call-1", AskLimits::default()),
    )
    .await;

    let done = ask_finished(&chat, &thread, 1).await;
    assert_eq!(done["data"]["state"], "completed", "{done}");
    assert_eq!(done["data"]["text"], "echo: echo what is the plan");
    assert_eq!(done["data"]["artifacts"][0]["name"], "result");
    assert_eq!(
        done["actor"]["name"], "coder",
        "the asked agent's own words"
    );

    // the log tells the story and nothing of the asked agent's work is the thread's
    let events = chat.events(&thread).await;
    assert_eq!(kinds_of(&events, "ask_started"), 1);
    assert_eq!(kinds_of(&events, "ask_finished"), 1);
    assert_eq!(kinds_of(&events, "artifact"), 0);
    assert_eq!(events[0]["kind"], "user_message");
    let started = events.iter().find(|e| e["kind"] == "ask_started").unwrap();
    assert_eq!(started["data"]["agent"], "coder");
    assert_eq!(started["data"]["by"], "main");
    assert_eq!(started["data"]["depth"], 1);
    assert_eq!(started["data"]["text"], "echo what is the plan");
    assert_eq!(chat.thread(&thread).await["state"], "working");

    // the asked agent got one message, in its own context and as `ask:1`
    let calls = world.coder.executions();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].context_id, format!("{thread}-ask-coder"));
    assert_eq!(calls[0].text, "echo what is the plan");
    assert!(calls[0].reference_task_ids.is_empty());
    assert!(!calls[0].resuming);
    let grant = calls[0].thread_tools.as_ref().expect("a grant");
    let claims = orch_thread_token::verify(
        &thread_tools_keys(),
        grant["token"].as_str().unwrap(),
        Timestamp::now(),
    )
    .unwrap();
    assert_eq!(
        (claims.thread, claims.job, claims.agent.as_str()),
        (id(&thread), 1, "coder")
    );
    assert_eq!((claims.caller, claims.depth), (Caller::Ask(1), 1));
    // and no mention of it: it was not the one the person addressed
    assert_eq!(calls[0].mentions, None);

    // the thread's own agent is told nothing of it: one message, its own, still at its gate
    assert_eq!(world.plain.executions().len(), 1);
    assert!(
        node.open_outbox(id(&thread))
            .await
            .iter()
            .all(|r| r.kind != orch_ports::OutboxKind::Ask),
        "the row is over"
    );
    world.plain.release_gate();
    chat.wait_state(&thread, "done").await;
    assert_eq!(kinds_of(&chat.events(&thread).await, "ask_finished"), 1);
}

async fn a_question_back_ends_the_ask_and_the_next_ask_continues_the_task(backend: Backend) {
    let world = world(backend).await;
    let orch = world.instance_with_thread_tools("orch-1", true).await;
    let chat = world.chat(&orch);
    let (thread, _sse) = working_thread(&chat).await;
    let node = world.node("asker").await;

    node.apply(
        id(&thread),
        ask("ask which branch", "call-1", AskLimits::default()),
    )
    .await;
    let first = ask_finished(&chat, &thread, 1).await;
    assert_eq!(first["data"]["state"], "input_required", "{first}");
    assert_eq!(first["data"]["question"], "Which branch?");

    // the answer is the next ask of the same agent: the same task, in the same context
    node.apply(id(&thread), ask("ask main", "call-2", AskLimits::default()))
        .await;
    let second = ask_finished(&chat, &thread, 2).await;
    assert_eq!(second["data"]["state"], "completed", "{second}");
    assert_eq!(second["data"]["text"], "answered: ask main");

    // and the one after is a new task that says which one it follows
    node.apply(
        id(&thread),
        ask("echo again", "call-3", AskLimits::default()),
    )
    .await;
    let third = ask_finished(&chat, &thread, 3).await;
    assert_eq!(third["data"]["state"], "completed", "{third}");

    let calls = world.coder.executions();
    assert_eq!(calls.len(), 3);
    let context = format!("{thread}-ask-coder");
    assert!(calls.iter().all(|c| c.context_id == context));
    assert_eq!(calls[1].task_id, calls[0].task_id, "the same task");
    assert!(calls[1].resuming && !calls[0].resuming);
    assert_ne!(calls[2].task_id, calls[0].task_id, "a task of its own");
    assert_eq!(calls[2].reference_task_ids, [calls[0].task_id.clone()]);
    world.plain.release_gate();
    chat.wait_state(&thread, "done").await;
}

async fn a_task_that_fails_ends_the_ask_failed_and_the_thread_goes_on(backend: Backend) {
    let world = world(backend).await;
    let orch = world.instance_with_thread_tools("orch-1", true).await;
    let chat = world.chat(&orch);
    let (thread, _sse) = working_thread(&chat).await;
    let node = world.node("asker").await;

    node.apply(id(&thread), ask("fail now", "call-1", AskLimits::default()))
        .await;
    let done = ask_finished(&chat, &thread, 1).await;
    assert_eq!(done["data"]["state"], "failed", "{done}");
    assert_eq!(done["data"]["error"], "scripted failure");
    assert_eq!(chat.thread(&thread).await["state"], "working");
    world.plain.release_gate();
    chat.wait_state(&thread, "done").await;
}

async fn an_agent_that_is_down_fails_the_ask_after_the_retries(backend: Backend) {
    let world = world(backend).await;
    let orch = world.instance_with_thread_tools("orch-1", true).await;
    let chat = world.chat(&orch);
    let (thread, _sse) = working_thread(&chat).await;
    let node = world.node("asker").await;

    world.coder.stop();
    node.apply(
        id(&thread),
        ask("echo anyone?", "call-1", AskLimits::default()),
    )
    .await;
    let done = ask_finished(&chat, &thread, 1).await;
    assert_eq!(done["data"]["state"], "failed", "{done}");
    assert_eq!(done["actor"]["type"], "system", "no agent said it");
    assert_eq!(chat.thread(&thread).await["state"], "working");
    world.plain.release_gate();
    chat.wait_state(&thread, "done").await;
}

async fn the_persons_stop_cancels_the_asked_agents_task(backend: Backend) {
    let world = world(backend).await;
    let orch = world.instance_with_thread_tools("orch-1", true).await;
    let chat = world.chat(&orch);
    let (thread, _sse) = working_thread(&chat).await;
    let node = world.node("asker").await;

    node.apply(
        id(&thread),
        ask("slow work", "call-1", AskLimits::default()),
    )
    .await;
    eventually("the asked agent runs the task", || async {
        (world.coder.executions().len() == 1).then_some(())
    })
    .await;
    assert!(world.coder.cancels().is_empty());

    assert_eq!(chat.cancel(&thread).await, 202);

    let done = ask_finished(&chat, &thread, 1).await;
    assert_eq!(done["data"]["state"], "canceled", "{done}");
    assert_eq!(done["data"]["error"], "the person stopped the job");
    eventually("the asked agent is told to stop", || async {
        (world.coder.cancels().len() == 1).then_some(())
    })
    .await;
    assert_eq!(
        world.coder.cancels()[0].task_id,
        world.coder.executions()[0].task_id
    );
    chat.wait_state(&thread, "cancelled").await;
    eventually("the row is over", || async {
        node.open_outbox(id(&thread))
            .await
            .iter()
            .all(|r| r.kind != orch_ports::OutboxKind::Ask)
            .then_some(())
    })
    .await;
    assert_eq!(kinds_of(&chat.events(&thread).await, "ask_finished"), 1);
}

async fn the_deadline_ends_the_ask_and_the_asked_agent_is_told_to_stop(backend: Backend) {
    let world = world(backend).await;
    let orch = world.instance_with_thread_tools("orch-1", true).await;
    let chat = world.chat(&orch);
    let (thread, _sse) = working_thread(&chat).await;
    let node = world.node("asker").await;

    // the timer is the inbox worker's
    let inbox = node.spawn_inbox(fast_inbox(), "inbox-1");
    let short = AskLimits::default().with_timeout(Duration::from_secs(1));
    node.apply(id(&thread), ask("slow work", "call-1", short))
        .await;

    let done = ask_finished(&chat, &thread, 1).await;
    assert_eq!(done["data"]["state"], "timed_out", "{done}");
    assert_eq!(done["actor"]["type"], "system");
    eventually("the asked agent is told to stop", || async {
        (world.coder.cancels().len() == 1).then_some(())
    })
    .await;
    assert_eq!(
        chat.thread(&thread).await["state"],
        "working",
        "the job is not the ask's to end"
    );
    world.plain.release_gate();
    chat.wait_state(&thread, "done").await;
    assert_eq!(kinds_of(&chat.events(&thread).await, "ask_finished"), 1);
    inbox.shutdown().await;
}

backends!(
    the_mentioned_agent_is_asked_in_a_context_of_its_own_and_its_answer_is_in_the_log,
    a_question_back_ends_the_ask_and_the_next_ask_continues_the_task,
    a_task_that_fails_ends_the_ask_failed_and_the_thread_goes_on,
    an_agent_that_is_down_fails_the_ask_after_the_retries,
    the_persons_stop_cancels_the_asked_agents_task,
    the_deadline_ends_the_ask_and_the_asked_agent_is_told_to_stop,
);
