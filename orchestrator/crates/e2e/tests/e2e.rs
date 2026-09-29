//! The acceptance sequence over real HTTP: chat API -> dispatcher -> A2A adapter -> agent.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use std::time::Duration;

use common::*;
use orch_testsupport::fake::PR_URL;

const WAIT: Duration = Duration::from_secs(20);

async fn create_thread_streams_expected_sequence_and_events_endpoint_agrees(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);

    let id = chat.create_thread("plain", "echo hi", None).await;
    let mut sse = chat.stream(&id, None).await;
    assert_eq!(sse.status, 200);
    let frames = sse
        .collect_until(WAIT, |kind, data| {
            kind == "thread_state" && data["data"]["state"] == "done"
        })
        .await;

    let kinds: Vec<&str> = frames.iter().map(|(_, k, _)| k.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "user_message",
            "agent_status",
            "artifact",
            "agent_status",
            "thread_state"
        ]
    );
    let seqs: Vec<i64> = frames.iter().map(|(s, _, _)| *s).collect();
    assert_eq!(seqs, [1, 2, 3, 4, 5]);
    let over_sse: Vec<_> = frames.iter().map(|(_, _, d)| d.clone()).collect();
    assert_eq!(shape(&over_sse), FIVE);

    // The events endpoint returns the very same list.
    let listed = chat.events(&id).await;
    assert_eq!(listed, over_sse);
    assert_contiguous(&listed);

    // Content: who said what.
    assert_eq!(listed[0]["actor"]["type"], "user");
    assert_eq!(listed[0]["data"]["text"], "echo hi");
    assert_eq!(listed[1]["actor"]["type"], "agent");
    assert_eq!(listed[1]["actor"]["name"], "plain");
    let artifact = &listed[2]["data"];
    assert_eq!(artifact["name"], "result");
    assert_eq!(artifact["text"], "echo: echo hi");
    assert_eq!(artifact["uri"], PR_URL);
    assert_eq!(listed[4]["actor"]["type"], "system");

    let thread = chat.thread(&id).await;
    assert_eq!(thread["state"], "done");
    assert_eq!(
        world.plain.executions().len(),
        1,
        "one message reached the agent"
    );
    assert_eq!(world.coder.executions().len(), 0);
}

async fn an_agent_that_fails_the_task_fails_the_thread_with_its_message(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let id = chat.create_thread("plain", "fail please", None).await;
    chat.wait_state(&id, "failed").await;
    let events = chat.events(&id).await;
    assert_eq!(
        shape(&events),
        [
            "user_message",
            "agent_status:working",
            "agent_status:failed",
            "thread_state:failed"
        ]
    );
    assert_eq!(events[2]["data"]["detail"], "scripted failure");
    assert_contiguous(&events);
}

async fn a_chunked_artifact_becomes_one_artifact_event(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let id = chat.create_thread("plain", "chunks", None).await;
    chat.wait_state(&id, "done").await;
    let events = chat.events(&id).await;
    assert_eq!(shape(&events), FIVE);
    assert_eq!(events[2]["data"]["name"], "log");
    assert_eq!(events[2]["data"]["text"], "one\ntwo\nthree");
}

async fn two_threads_run_side_by_side_without_mixing_events(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let a = chat.create_thread("plain", "echo first", None).await;
    let b = chat.create_thread("coder", "echo second", None).await;
    chat.wait_state(&a, "done").await;
    chat.wait_state(&b, "done").await;
    let (ea, eb) = (chat.events(&a).await, chat.events(&b).await);
    assert_eq!(shape(&ea), FIVE);
    assert_eq!(shape(&eb), FIVE);
    assert_eq!(ea[2]["data"]["text"], "echo: echo first");
    assert_eq!(eb[2]["data"]["text"], "echo: echo second");
    assert_eq!(world.plain.executions().len(), 1);
    assert_eq!(world.coder.executions().len(), 1);
}

backends!(
    create_thread_streams_expected_sequence_and_events_endpoint_agrees,
    an_agent_that_fails_the_task_fails_the_thread_with_its_message,
    a_chunked_artifact_becomes_one_artifact_event,
    two_threads_run_side_by_side_without_mixing_events,
);
