//! Asked agents end to end (ADR 0026), on both stores: the agent a thread runs on asks one of the
//! agents the person mentioned, and the real dispatcher sends the question through the real A2A
//! adapter to a second fake agent, in a conversation of its own, and puts what it answers in the
//! log. Nothing here is the thread's own task: the first agent waits at its gate the whole time.
//!
//! The ask is made through the thread tool `ask_agent` of the orchestrator's own endpoint, as the
//! agent does: the test plays the asking agent with the grant `plain` was given in its message and
//! rmcp's client (`orch_testsupport::ask_agent`), and a call returns when the asked agent has
//! answered. The coordinating agent itself, a fake that makes the calls, is `ask_agent.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use std::time::Duration;

use common::*;
use jiff::Timestamp;
use orch_core::{AskLimits, Caller, MENTIONS_EXTENSION, THREAD_TOOLS_EXTENSION, ThreadId};
use orch_testsupport::{AskReply, Chat, FakeAgentOptions, FakeReleases, ask_agent, eventually};
use serde_json::{Value, json};
use uuid::Uuid;

/// `@coder` is at 5..11.
const TEXT: &str = "gate @coder look into it";

/// `plain` is the agent the thread runs on and is given the thread tools; `coder` is mentioned,
/// and lists `thread-tools/v1` so that it is given the grant of an asked agent.
async fn world(backend: Backend) -> World {
    world_with(backend, None).await
}

async fn world_with(backend: Backend, asks: Option<AskLimits>) -> World {
    World::with(
        backend,
        Setup {
            coder: FakeAgentOptions {
                releases: Some(FakeReleases::sample()),
                extensions: vec![THREAD_TOOLS_EXTENSION.to_owned()],
                ..FakeAgentOptions::default()
            },
            plain: FakeAgentOptions {
                extensions: vec![
                    THREAD_TOOLS_EXTENSION.to_owned(),
                    MENTIONS_EXTENSION.to_owned(),
                ],
                ..FakeAgentOptions::default()
            },
            thread_tools: true,
            asks,
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

/// The grant `plain` was given with its message: what its agent calls the endpoint with.
fn grant(world: &World) -> Value {
    world
        .grants_of_plain()
        .into_iter()
        .next()
        .expect("plain was given a grant")
}

/// A call of `ask_agent` as `plain`'s agent makes it, which returns when the ask has ended.
async fn ask(world: &World, message: &str, call_id: &str) -> AskReply {
    ask_agent(&grant(world), "coder", message, Some(call_id)).await
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

    let reply = ask(&world, "echo what is the plan", "call-1").await;
    assert!(!reply.is_error, "{reply:?}");
    assert_eq!(reply.value["ask"], 1);
    assert_eq!(reply.value["state"], "completed");
    assert_eq!(reply.value["text"], "echo: echo what is the plan");

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
    assert_eq!(started["actor"]["name"], "plain", "the asker's words");
    assert_eq!(started["data"]["agent"], "coder");
    assert_eq!(started["data"]["by"], "main");
    assert_eq!(started["data"]["depth"], 1);
    assert_eq!(started["data"]["text"], "echo what is the plan");
    assert_eq!(chat.thread(&thread).await["state"], "working");

    // the asked agent got one message, in its own context and as `ask:1`
    let calls = world.coder.executions();
    assert_eq!(calls.len(), 1);
    // it names no context: the asked agent starts a conversation of its own, never the thread's (ADR 0055)
    assert_eq!(calls[0].requested_context, None);
    assert_ne!(calls[0].context_id, thread);
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

    let first = ask(&world, "ask which branch", "call-1").await;
    assert!(!first.is_error, "{first:?}");
    assert_eq!(first.value["state"], "input_required", "{first:?}");
    assert_eq!(first.value["question"], "Which branch?");
    let logged = ask_finished(&chat, &thread, 1).await;
    assert_eq!(logged["data"]["state"], "input_required", "{logged}");
    assert_eq!(logged["data"]["question"], "Which branch?");

    // the answer is the next ask of the same agent: the same task, in the same context
    let second = ask(&world, "ask main", "call-2").await;
    assert_eq!(second.value["ask"], 2);
    assert_eq!(second.value["state"], "completed", "{second:?}");
    assert_eq!(second.value["text"], "answered: ask main");

    // and the one after is a new task that says which one it follows
    let third = ask(&world, "echo again", "call-3").await;
    assert_eq!(third.value["state"], "completed", "{third:?}");

    let calls = world.coder.executions();
    assert_eq!(calls.len(), 3);
    // the first ask names no context, the agent assigns one, and the next asks of that agent go on in it
    let context = calls[0].context_id.clone();
    assert!(calls.iter().all(|c| c.context_id == context));
    assert_eq!(calls[0].requested_context, None);
    assert!(
        calls[1..]
            .iter()
            .all(|c| c.requested_context.as_deref() == Some(context.as_str())),
        "{calls:?}"
    );
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

    let reply = ask(&world, "fail now", "call-1").await;
    assert!(reply.is_error, "{reply:?}");
    assert_eq!(reply.value["state"], "failed");
    assert_eq!(reply.value["error"], "scripted failure");
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

    world.coder.stop();
    let reply = ask(&world, "echo anyone?", "call-1").await;
    assert!(reply.is_error, "{reply:?}");
    assert_eq!(reply.value["state"], "failed");
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

    let waiting = tokio::spawn({
        let grant = grant(&world);
        async move { ask_agent(&grant, "coder", "slow work", Some("call-1")).await }
    });
    eventually("the asked agent runs the task", || async {
        (world.coder.executions().len() == 1).then_some(())
    })
    .await;
    assert!(world.coder.cancels().is_empty());

    assert_eq!(chat.cancel(&thread).await, 202);

    let reply = waiting.await.unwrap();
    assert!(reply.is_error, "{reply:?}");
    assert_eq!(reply.value["state"], "canceled");
    assert_eq!(reply.value["error"], "the person stopped the job");
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
    // the deployment's deadline is a second here, as `asks.timeoutSecs` would say (10 s at least in
    // a file)
    let world = world_with(
        backend,
        Some(AskLimits::default().with_timeout(Duration::from_secs(1))),
    )
    .await;
    let orch = world.instance_with_thread_tools("orch-1", true).await;
    let chat = world.chat(&orch);
    let (thread, _sse) = working_thread(&chat).await;
    let node = world.node("asker").await;

    // the timer is the inbox worker's
    let inbox = node.spawn_inbox(fast_inbox(), "inbox-1");
    let reply = ask(&world, "slow work", "call-1").await;
    assert!(reply.is_error, "{reply:?}");
    assert_eq!(reply.value["state"], "timed_out");

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
