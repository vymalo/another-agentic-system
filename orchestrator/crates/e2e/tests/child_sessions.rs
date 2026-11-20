//! `child-sessions/v1` (ADR 0072, `docs/api/child-sessions-v1.md`), on both stores: the live card is
//! read into the capabilities document, well formed or not at all, and never cached (S1); a helper an
//! agent runs as a session is a child thread with its own log, followed without sending the agent
//! anything, the parent's `child_session_*` events once each, usage counted once, and every restart
//! leaves no duplicate (S2d).
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use common::*;
use std::time::Duration;

use orch_core::CHILD_SESSIONS_EXTENSION;
use orch_testsupport::{Chat, FakeAgentOptions, eventually};
use serde_json::{Value, json};

fn params() -> Value {
    json!({
        "agents": [
            {"name": "planner", "description": "Breaks a request into steps."},
            {"name": "researcher", "description": "Looks things up."}
        ],
        "maxOpen": 16
    })
}

/// A world whose `plain` agent lists `child-sessions/v1` and whose `coder` does not.
async fn world(backend: Backend) -> World {
    World::with(
        backend,
        Setup {
            plain: FakeAgentOptions {
                child_sessions: Some(params()),
                ..FakeAgentOptions::default()
            },
            ..Setup::default()
        },
    )
    .await
}

async fn the_capabilities_document_carries_the_helpers_of_the_live_card(backend: Backend) {
    let world = world(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let (status, doc) = chat.agui_capabilities("plain").await;
    assert_eq!(status, 200);
    assert_eq!(doc["custom"][CHILD_SESSIONS_EXTENSION], params());
    // an agent that does not list it has no key
    let (status, doc) = chat.agui_capabilities("coder").await;
    assert_eq!(status, 200);
    assert!(
        doc.get("custom")
            .is_none_or(|c| c.get(CHILD_SESSIONS_EXTENSION).is_none())
    );
    // a card that lists it badly offers nothing (fail closed)
    world
        .plain
        .set_child_sessions(Some(json!({"agents": [], "maxOpen": 16})));
    let (_, doc) = chat.agui_capabilities("plain").await;
    assert!(
        doc.get("custom")
            .is_none_or(|c| c.get(CHILD_SESSIONS_EXTENSION).is_none())
    );
    // the card is read live: a change shows on the next read
    world.plain.set_child_sessions(Some(json!({
        "agents": [{"name": "writer", "description": ""}],
        "maxOpen": 2
    })));
    let (_, doc) = chat.agui_capabilities("plain").await;
    assert_eq!(
        doc["custom"][CHILD_SESSIONS_EXTENSION],
        json!({"agents": [{"name": "writer", "description": ""}], "maxOpen": 2})
    );
    world.plain.set_child_sessions(None);
    let (_, doc) = chat.agui_capabilities("plain").await;
    assert!(
        doc.get("custom")
            .is_none_or(|c| c.get(CHILD_SESSIONS_EXTENSION).is_none())
    );
}

backends!(the_capabilities_document_carries_the_helpers_of_the_live_card);

// ---- S2d: a session is a child thread ----------------------------------------------------------

async fn world_with_events(backend: Backend, events: bool) -> World {
    World::with(
        backend,
        Setup {
            plain: FakeAgentOptions {
                child_sessions: Some(params()),
                child_session_events: events,
                ..FakeAgentOptions::default()
            },
            // a process that dies leaves its claim to lapse
            lapsing_leases: true,
            ..Setup::default()
        },
    )
    .await
}

fn count(events: &[Value], kind: &str) -> usize {
    events.iter().filter(|e| e["kind"] == kind).count()
}

fn kinds(events: &[Value]) -> Vec<String> {
    shape(events)
}

/// How many events have this shape (`thread_state:done`, `agent_status:failed`).
fn count_shape(events: &[Value], one: &str) -> usize {
    kinds(events).iter().filter(|k| k.as_str() == one).count()
}

/// The child thread the parent's log names for its only session.
async fn child_of(chat: &Chat, parent: &str) -> String {
    eventually("the parent to name a child thread", || async {
        let events = chat.events(parent).await;
        data_of(&events, "child_session_opened")
            .first()
            .map(|d| d["thread"].as_str().unwrap().to_owned())
    })
    .await
}

/// The parent's `child_session_*` events, once the parent has all of `kinds`.
async fn parent_events_with(chat: &Chat, parent: &str, wanted: &[&str]) -> Vec<Value> {
    eventually("the parent's session events", || async {
        let events = chat.events(parent).await;
        wanted
            .iter()
            .all(|kind| count(&events, kind) >= 1)
            .then_some(events)
    })
    .await
}

async fn a_session_is_a_child_thread_with_its_own_log(backend: Backend, events: bool) {
    let world = world_with_events(backend, events).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    // the helper's round is held open until it is followed, so that its steps and words come on
    // the stream; the parent waits for it to end
    let id = chat
        .create_thread("plain", "child-sessions planner gate hold", None)
        .await;
    let child = child_of(&chat, &id).await;
    chat.wait_state(&child, "working").await;
    eventually("the child to follow its round", || async {
        (world.plain.rpc_count("subscribe_to_task") >= 1).then_some(())
    })
    .await;
    world.plain.release_gate();
    chat.wait_state(&id, "done").await;
    chat.wait_state(&child, "done").await;
    let parent_log = parent_events_with(
        &chat,
        &id,
        &[
            "child_session_opened",
            "child_session_reported",
            "child_session_closed",
        ],
    )
    .await;

    // the agent was asked to activate the extension, once, and nothing was sent for the child: its
    // round was read by task
    let sends = world.plain.executions();
    assert_eq!(sends.len(), 1, "{sends:?}");
    assert!(sends[0].activates_child_sessions());
    assert!(
        sends[0]
            .extensions_header
            .iter()
            .any(|h| h.contains(CHILD_SESSIONS_EXTENSION))
    );
    assert_eq!(world.plain.rpc_count("send_streaming_message"), 1);
    assert!(
        world.plain.rpc_count("subscribe_to_task") + world.plain.rpc_count("get_task") >= 1,
        "the round is followed by its task"
    );

    // the parent: each of the three events once, naming the child
    assert_eq!(count(&parent_log, "child_session_opened"), 1);
    assert_eq!(count(&parent_log, "child_session_reported"), 1);
    assert_eq!(count(&parent_log, "child_session_closed"), 1);
    let opened = data_of(&parent_log, "child_session_opened")[0];
    assert_eq!(opened["thread"], child.as_str());
    assert_eq!(opened["agent"], "planner");
    assert_eq!(opened["round"], 1);
    let reported = data_of(&parent_log, "child_session_reported")[0];
    assert_eq!(reported["thread"], child.as_str());
    assert_eq!(reported["round"], 1);
    assert_eq!(reported["report"], "done");
    assert_eq!(
        data_of(&parent_log, "child_session_closed")[0]["thread"],
        child.as_str()
    );

    // the child: one thread, nested under the parent, that says whose child it is
    let thread = chat.thread(&child).await;
    assert_eq!(thread["childOf"]["threadId"], id.as_str());
    assert_eq!(thread["childOf"]["agent"], "planner");
    assert_eq!(thread["childOf"]["state"], "closed");
    assert!(thread["childOf"]["session"].as_str().is_some());
    assert_eq!(thread["nestedUnder"], id.as_str());
    assert_eq!(thread["target"]["agentId"], "plain");

    // its log is the round: the job nobody sent, the helper's step and words, its end
    let child_log = chat.events(&child).await;
    assert_contiguous(&child_log);
    let first = &child_log[0];
    assert_eq!(first["kind"], "job_started", "{:?}", kinds(&child_log));
    assert_eq!(first["data"]["job"], 1);
    assert_eq!(first["data"]["round"]["number"], 1);
    assert_eq!(count(&child_log, "user_message"), 0);
    assert!(
        count(&child_log, "agent_step") >= 1,
        "{:?}",
        kinds(&child_log)
    );
    assert!(
        child_log
            .iter()
            .any(|e| e["kind"] == "agent_message" && e["data"]["text"] == "planner: done"),
        "{:?}",
        kinds(&child_log)
    );
    assert_eq!(
        kinds(&child_log).last().map(String::as_str),
        Some("thread_state:done")
    );

    // the person's list nests the child under the parent
    let (status, list) = chat.get("/api/threads?order=rail").await;
    assert_eq!(status, 200, "{list}");
    let items = list.as_array().unwrap();
    assert!(
        items
            .iter()
            .any(|t| t["id"] == child.as_str() && t["nestedUnder"] == id.as_str()),
        "{list}"
    );

    // the month counts the parent's task alone (its calls hold the helper's tokens): one task and
    // the 15 tokens the parent reported, not the helper's own round a second time
    let (status, month) = chat.get("/api/me/usage").await;
    assert_eq!(status, 200, "{month}");
    assert_eq!(month["tasks"], 1, "{month}");
    let models = month["models"].as_array().unwrap();
    assert_eq!(models.len(), 1, "{month}");
    assert_eq!(models[0]["totalTokens"], 15, "{month}");

    // the same thing said again by the agent (a poll) writes nothing twice
    let again = chat.events(&id).await;
    assert_eq!(count(&again, "child_session_opened"), 1);
    assert_eq!(count(&again, "child_session_reported"), 1);
    orch.shutdown().await;
}

async fn a_session_is_a_child_thread(backend: Backend) {
    a_session_is_a_child_thread_with_its_own_log(backend, false).await;
}

async fn the_same_result_with_the_agent_saying_events(backend: Backend) {
    a_session_is_a_child_thread_with_its_own_log(backend, true).await;
}

async fn a_kept_open_round_two_lands_in_the_same_child_as_job_two(backend: Backend) {
    let world = world_with_events(backend, false).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let id = chat
        .create_thread("plain", "child-sessions planner keep", None)
        .await;
    chat.wait_state(&id, "done").await;
    let child = child_of(&chat, &id).await;
    chat.wait_state(&child, "done").await;
    let opened = {
        let log = chat.events(&id).await;
        data_of(&log, "child_session_opened")[0].clone()
    };
    let session = opened["session"].as_str().unwrap().to_owned();
    // kept open: the session is idle, not closed
    assert_eq!(chat.thread(&child).await["childOf"]["state"], "idle");
    let log = chat.events(&id).await;
    assert_eq!(count(&log, "child_session_closed"), 0);

    // the parent's model sends the idle session another message: round 2, the same thread's job 2
    chat.seed_message(&id, &format!("child-sessions-send {session}"))
        .await;
    eventually("the child's second job", || async {
        let log = chat.events(&child).await;
        (count(&log, "job_started") == 2 && chat.state(&child).await == "done").then_some(())
    })
    .await;
    let log = chat.events(&child).await;
    assert_contiguous(&log);
    let starts = data_of(&log, "job_started");
    assert_eq!(starts[0]["job"], 1);
    assert_eq!(starts[0]["round"]["number"], 1);
    assert_eq!(starts[1]["job"], 2);
    assert_eq!(starts[1]["round"]["number"], 2);
    assert_eq!(
        count_shape(&log, "thread_state:done"),
        2,
        "{:?}",
        kinds(&log)
    );
    let parent_log = chat.events(&id).await;
    // two rounds reported to the parent, each once, and still one child and one opened
    eventually("round 2 reported", || async {
        let log = chat.events(&id).await;
        (count(&log, "child_session_reported") == 2).then_some(())
    })
    .await;
    let parent_log2 = chat.events(&id).await;
    assert_eq!(count(&parent_log2, "child_session_opened"), 1);
    assert_eq!(count(&parent_log, "child_session_opened"), 1);
    let rounds: Vec<_> = data_of(&parent_log2, "child_session_reported")
        .iter()
        .map(|d| d["round"].as_u64().unwrap())
        .collect();
    assert_eq!(rounds, [1, 2]);
    assert_eq!(
        world.plain.executions().len(),
        2,
        "two messages, none for the child"
    );
    orch.shutdown().await;
}

async fn news_that_breaks_the_contract_creates_nothing(backend: Backend) {
    let world = world_with_events(backend, false).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let id = chat
        .create_thread("plain", "child-sessions-bad", None)
        .await;
    chat.wait_state(&id, "done").await;
    // give a late child a moment to appear: none does
    tokio::time::sleep(Duration::from_millis(300)).await;
    let log = chat.events(&id).await;
    assert_eq!(count(&log, "child_session_opened"), 0, "{:?}", kinds(&log));
    let (status, list) = chat.get("/api/threads?order=rail").await;
    assert_eq!(status, 200);
    assert_eq!(list.as_array().unwrap().len(), 1, "{list}");
    // and the counter says so
    let metrics = reqwest::get(format!("{}/metrics", orch.base_url))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let rejected = metrics
        .lines()
        .find_map(|l| l.strip_prefix("child_sessions_dropped_total{reason=\"rejected\"} "))
        .unwrap_or_else(|| panic!("{metrics}"));
    assert!(rejected.parse::<u64>().unwrap() >= 1, "{metrics}");
    orch.shutdown().await;
}

async fn a_card_that_drops_the_uri_makes_the_follow_fail(backend: Backend) {
    let world = world_with_events(backend, false).await;
    let first = world.instance("orch-1").await;
    let chat = world.chat(&first);
    let id = chat
        .create_thread("plain", "child-sessions planner gate", None)
        .await;
    chat.wait_state(&id, "done").await;
    let child = child_of(&chat, &id).await;
    chat.wait_state(&child, "working").await;
    eventually("the child follows its round", || async {
        (world.plain.rpc_count("subscribe_to_task") >= 1).then_some(())
    })
    .await;
    // the process dies; the agent redeploys without the extension; another process follows on
    first.kill();
    world.plain.set_child_sessions(None);
    let second = world.instance("orch-2").await;
    let chat = world.chat(&second);
    // fail closed (ADR 0008): the round is asked for without the activation, the agent refuses,
    // and the child thread says its follow failed
    chat.wait_state(&child, "failed").await;
    let log = chat.events(&child).await;
    assert_eq!(count(&log, "error"), 1, "{:?}", kinds(&log));
    assert_eq!(count(&log, "job_started"), 1);
    second.shutdown().await;
}

async fn a_restart_while_a_round_runs_leaves_no_duplicates(backend: Backend) {
    let world = world_with_events(backend, false).await;
    let first = world.instance("orch-1").await;
    let chat = world.chat(&first);
    let id = chat
        .create_thread("plain", "child-sessions planner gate", None)
        .await;
    chat.wait_state(&id, "done").await;
    let child = child_of(&chat, &id).await;
    chat.wait_state(&child, "working").await;
    eventually("the child follows its round", || async {
        (world.plain.rpc_count("subscribe_to_task") >= 1).then_some(())
    })
    .await;
    first.kill();
    let second = world.instance("orch-2").await;
    let chat = world.chat(&second);
    // the new process takes the row over when the lease lapses, and follows the same task
    eventually("the new process to follow the round", || async {
        (world.plain.rpc_count("subscribe_to_task") >= 2).then_some(())
    })
    .await;
    world.plain.release_gate();
    chat.wait_state(&child, "done").await;
    let log = chat.events(&child).await;
    assert_contiguous(&log);
    assert_eq!(count(&log, "job_started"), 1, "{:?}", kinds(&log));
    assert_eq!(
        count_shape(&log, "thread_state:done"),
        1,
        "{:?}",
        kinds(&log)
    );
    let parent_log = parent_events_with(&chat, &id, &["child_session_reported"]).await;
    assert_eq!(count(&parent_log, "child_session_opened"), 1);
    assert_eq!(count(&parent_log, "child_session_reported"), 1);
    assert_eq!(world.plain.rpc_count("send_streaming_message"), 1);
    let (_, list) = chat.get("/api/threads?order=rail").await;
    assert_eq!(list.as_array().unwrap().len(), 2, "{list}");
    second.shutdown().await;
}

async fn usage_is_counted_once_when_the_round_is_read_after_it_ended(backend: Backend) {
    let world = world_with_events(backend, false).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    // the round is over by the time the parent says it: it is read by `GetTask`, which holds its totals
    let id = chat
        .create_thread("plain", "child-sessions planner", None)
        .await;
    chat.wait_state(&id, "done").await;
    let child = child_of(&chat, &id).await;
    chat.wait_state(&child, "done").await;
    let parent_log = parent_events_with(
        &chat,
        &id,
        &["child_session_reported", "child_session_closed"],
    )
    .await;
    assert_eq!(count(&parent_log, "child_session_opened"), 1);
    assert_eq!(count(&parent_log, "child_session_reported"), 1);
    assert_eq!(count(&parent_log, "child_session_closed"), 1);

    // the helper's tokens are in its own log, marked as the parent's to count
    let child_log = chat.events(&child).await;
    let totals = data_of(&child_log, "model_usage_total");
    assert_eq!(totals.len(), 1, "{:?}", kinds(&child_log));
    assert_eq!(totals[0]["counted"], false);
    // the month holds the parent's task alone: its 15 tokens, once
    let (status, month) = chat.get("/api/me/usage").await;
    assert_eq!(status, 200, "{month}");
    assert_eq!(month["tasks"], 1, "{month}");
    let models = month["models"].as_array().unwrap();
    assert_eq!(models.len(), 1, "{month}");
    assert_eq!(models[0]["totalTokens"], 15, "{month}");
    orch.shutdown().await;
}

async fn a_child_thread_takes_no_run_and_no_fork(backend: Backend) {
    let world = world_with_events(backend, false).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let id = chat
        .create_thread("plain", "child-sessions planner", None)
        .await;
    chat.wait_state(&id, "done").await;
    let child = child_of(&chat, &id).await;
    chat.wait_state(&child, "done").await;
    // an AG-UI run on the child's thread: well formed, and what it names cannot be done
    let body = Chat::agui_input(&child, "run-2", &[("msg-2", "hello helper")], json!({}));
    let response = chat.agui_post("plain", &body).await;
    assert_eq!(response.status().as_u16(), 422);
    let problem: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(problem["code"], "child_thread", "{problem}");
    // and nothing was written or sent
    let log = chat.events(&child).await;
    assert_eq!(count(&log, "user_message"), 0);
    assert_eq!(world.plain.executions().len(), 1);
    let (status, problem) = chat
        .post(
            &format!("/api/threads/{child}/fork"),
            Some(json!({"after": 1})),
        )
        .await;
    assert_eq!(status, 422, "{problem}");
    assert_eq!(problem["code"], "child_thread");
    orch.shutdown().await;
}

backends!(
    a_child_thread_takes_no_run_and_no_fork,
    a_session_is_a_child_thread,
    the_same_result_with_the_agent_saying_events,
    a_kept_open_round_two_lands_in_the_same_child_as_job_two,
    usage_is_counted_once_when_the_round_is_read_after_it_ended,
    news_that_breaks_the_contract_creates_nothing,
    a_card_that_drops_the_uri_makes_the_follow_fail,
    a_restart_while_a_round_runs_leaves_no_duplicates,
);
