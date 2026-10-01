//! The acceptance sequence over real HTTP: AG-UI run route -> dispatcher -> A2A adapter -> agent.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use std::time::Duration;

use common::*;
use orch_testsupport::fake::PR_URL;
use serde_json::json;

const WAIT: Duration = Duration::from_secs(20);

async fn a_run_streams_the_expected_sequence_and_the_log_agrees(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);

    let id = chat.create_thread("plain", "echo hi", None).await;
    // The connect stream is the log, projected: the run, ending with the thread's state.
    let mut sse = chat.agui_connect(&id, None, true).await;
    assert_eq!(sse.status, 200);
    let frames = sse.collect_frames(WAIT).await;
    let types: Vec<&str> = frames
        .iter()
        .map(|f| f.event["type"].as_str().unwrap())
        .collect();
    assert_eq!(types.first(), Some(&"RUN_STARTED"), "{types:?}");
    assert_eq!(types.last(), Some(&"RUN_FINISHED"), "{types:?}");
    assert_eq!(
        frames.last().unwrap().event["outcome"],
        json!({"type": "success"})
    );
    // Each log event ends in exactly one resume point, its `seq`.
    let ids: Vec<i64> = frames.iter().filter_map(|f| f.id).collect();
    assert_eq!(ids, [1, 2, 3, 4, 5]);

    // The log holds the same five events, in order.
    let listed = chat.events(&id).await;
    assert_eq!(shape(&listed), FIVE);
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

/// The legacy interaction routes (`createThread`, `postMessage`, `listEvents`, `streamEvents`)
/// were removed on 2026-09-30 (ADR 0012): on the composed router, with the AG-UI surface mounted,
/// they match nothing, while the resource API beside them answers.
async fn the_legacy_interaction_routes_are_gone(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let id = chat.create_thread("plain", "echo hi", None).await;
    chat.wait_state(&id, "done").await;

    for path in ["events", "stream"] {
        let (status, body) = chat.get(&format!("/api/threads/{id}/{path}")).await;
        assert_eq!(status, 404, "GET {path}");
        assert_eq!(body["detail"], "no such route", "GET {path}");
    }
    let (status, body) = chat
        .post(
            &format!("/api/threads/{id}/messages"),
            Some(json!({"text": "hello"})),
        )
        .await;
    assert_eq!((status, &body["detail"]), (404, &json!("no such route")));
    // `POST /api/threads` shares its path with the served `GET /api/threads`: the router can only
    // say the method is not allowed.
    let (status, _) = chat
        .post(
            "/api/threads",
            Some(json!({"target": {"agentId": "plain"}, "text": "hi"})),
        )
        .await;
    assert_eq!(status, 405);
    // Nothing was created or sent by any of it, and the resource API is where it was.
    assert_eq!(shape(&chat.events(&id).await), FIVE);
    let (status, threads) = chat.get("/api/threads").await;
    assert_eq!(status, 200);
    assert_eq!(threads.as_array().unwrap().len(), 1);
    assert_eq!(world.plain.executions().len(), 1);
}

async fn concurrent_threads_of_two_users_do_not_interfere(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let alice = world.chat(&orch);
    let bob = alice.as_user(BOB);
    let mut ids = Vec::new();
    for i in 0..4 {
        ids.push((
            &alice,
            alice
                .create_thread("plain", &format!("echo a{i}"), None)
                .await,
        ));
        ids.push((
            &bob,
            bob.create_thread("plain", &format!("echo b{i}"), None)
                .await,
        ));
    }
    for (chat, id) in &ids {
        chat.wait_state(id, "done").await;
        assert_eq!(shape(&chat.events(id).await), FIVE);
    }
    assert_eq!(world.plain.executions().len(), 8);
    // Each user lists their own four, and only theirs.
    for chat in [&alice, &bob] {
        let (status, threads) = chat.get("/api/threads").await;
        assert_eq!(status, 200);
        let listed: Vec<&str> = threads
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap())
            .collect();
        assert_eq!(listed.len(), 4, "{listed:?}");
        for (owner, id) in &ids {
            assert_eq!(
                listed.contains(&id.as_str()),
                std::ptr::eq(*owner, chat),
                "{id}"
            );
        }
    }
}

/// The orchestrator's own model call (ADR 0005): after the agent's first reply the thread is
/// titled, the sidebar's list says the title, the log says who wrote it, and a person's rename
/// afterwards is final.
async fn a_reply_gets_a_title_and_a_persons_rename_is_final(backend: Backend) {
    let world = World::with(
        backend,
        Setup {
            titles: true,
            ..Setup::default()
        },
    )
    .await;
    // the first reply has no topic yet; the second has
    world
        .model
        .then_answer("NONE")
        .then_answer("\"Fix the build\"\nA longer explanation that is not the title");
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);

    let id = chat.seed_thread("plain", "stream hello", None).await;
    chat.wait_state(&id, "done").await;
    eventually("the model's answer to the first ask", || async {
        (world.model.calls().len() == 1).then_some(())
    })
    .await;
    // "no topic yet": the thread keeps the first words
    let (_, thread) = chat.get(&format!("/api/threads/{id}")).await;
    assert_eq!(thread["title"], "stream hello");

    // the next job's reply asks again, and the model has a title
    chat.seed_message(&id, "stream write a fibonacci function")
        .await;
    eventually("the model's title", || async {
        let (_, t) = chat.get(&format!("/api/threads/{id}")).await;
        (t["title"] == "Fix the build").then_some(())
    })
    .await;
    chat.wait_state(&id, "done").await;
    let (_, listed) = chat.get("/api/threads").await;
    let titles: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["Fix the build"], "the sidebar's list says it");
    let titled: Vec<serde_json::Value> = chat
        .events(&id)
        .await
        .into_iter()
        .filter(|e| e["kind"] == "thread_titled")
        .collect();
    assert_eq!(titled.len(), 1);
    assert_eq!(
        titled[0]["data"],
        json!({"title": "Fix the build", "source": "model"})
    );
    assert_eq!(
        titled[0]["actor"],
        json!({"type": "system", "name": "orchestrator"})
    );
    // the model was asked about the conversation, as data
    let calls = world.model.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].model, "mock-title");
    assert!(
        calls[1].user.contains("stream write a fibonacci function"),
        "{}",
        calls[1].user
    );

    // a person renames it: that title is final, and the model is not asked again
    let (status, renamed) = chat.rename(&id, "My own title").await;
    assert_eq!(status, 200, "{renamed}");
    chat.seed_message(&id, "stream one more thing").await;
    chat.wait_state(&id, "done").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (_, thread) = chat.get(&format!("/api/threads/{id}")).await;
    assert_eq!(thread["title"], "My own title");
    assert_eq!(world.model.calls().len(), 2);
}

backends!(
    a_reply_gets_a_title_and_a_persons_rename_is_final,
    a_run_streams_the_expected_sequence_and_the_log_agrees,
    concurrent_threads_of_two_users_do_not_interfere,
    the_legacy_interaction_routes_are_gone,
    an_agent_that_fails_the_task_fails_the_thread_with_its_message,
    a_chunked_artifact_becomes_one_artifact_event,
    two_threads_run_side_by_side_without_mixing_events,
);
