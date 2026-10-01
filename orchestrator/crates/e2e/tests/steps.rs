//! Nested steps through the whole stack (ADR 0025, MVP slice 5): an agent whose card lists
//! `steps/v1` reports its work as steps, the orchestrator activates the extension, the A2A adapter
//! reads them, the core logs them with their path and keeps the log bounded, and a viewer's AG-UI
//! stream shows the tree. On the in-memory store and on Postgres, where the ledger of open steps
//! lives in the job and survives a change of replica.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use std::collections::BTreeMap;

use common::*;
use orch_core::{MAX_STEP_UPDATES, STEPS_EXTENSION};
use orch_testsupport::Frame;
use serde_json::Value;

fn steps(events: &[Value]) -> Vec<&Value> {
    events
        .iter()
        .filter(|e| e["kind"] == "agent_step")
        .collect()
}

/// (id without the task, path without the task, state, phase) of each step event.
fn story(events: &[Value]) -> Vec<(String, Vec<String>, String, String)> {
    let bare = |id: &Value| {
        id.as_str()
            .unwrap()
            .split_once('/')
            .map(|(_, rest)| rest.to_owned())
            .unwrap()
    };
    steps(events)
        .into_iter()
        .map(|e| {
            let d = &e["data"];
            (
                bare(&d["id"]),
                d["path"].as_array().unwrap().iter().map(bare).collect(),
                d["state"].as_str().unwrap().to_owned(),
                d["phase"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn s(id: &str, path: &[&str], state: &str, phase: &str) -> (String, Vec<String>, String, String) {
    (
        id.to_owned(),
        path.iter().map(|p| (*p).to_owned()).collect(),
        state.to_owned(),
        phase.to_owned(),
    )
}

async fn nested_steps_reach_the_log_with_their_paths_and_the_viewer_sees_the_tree(
    backend: Backend,
) {
    let world = world_with_steps_on(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let id = chat
        .create_thread("plain", "steps run the tests", None)
        .await;
    chat.wait_state(&id, "done").await;

    // the orchestrator asked for them: the header and the message's own extensions
    let call = world.plain.executions().pop().unwrap();
    assert!(call.activates_steps(), "{call:?}");

    let events = chat.events(&id).await;
    assert_eq!(
        shape(&events),
        [
            "user_message",
            "agent_status:working",
            "agent_step",
            "agent_step",
            "agent_step",
            "agent_step",
            "agent_message",
            "agent_status:completed",
            "thread_state:done"
        ]
    );
    assert_contiguous(&events);
    assert_eq!(
        story(&events),
        [
            s("tool:c2", &[], "running", "start"),
            s("acp:c2:1", &["tool:c2"], "running", "start"),
            s("acp:c2:1", &["tool:c2"], "failed", "end"),
            s("tool:c2", &[], "completed", "end"),
        ]
    );
    // the ids are `<task>/<the agent's id>`: unique within the thread
    let task = &call.task_id;
    assert!(steps(&events).iter().all(|e| {
        e["data"]["id"]
            .as_str()
            .unwrap()
            .starts_with(&format!("{task}/"))
    }));
    assert_eq!(steps(&events)[2]["data"]["detail"], "1 failed");
    assert_eq!(steps(&events)[0]["data"]["label"], "OpenCode");
    assert_eq!(steps(&events)[0]["actor"]["name"], "plain");

    // a viewer sees the sub-agent as a subagent of the run and its command in it
    let frames = chat
        .agui_connect(&id, None, true)
        .await
        .collect_frames(std::time::Duration::from_secs(20))
        .await;
    let started: Vec<&Frame> = frames
        .iter()
        .filter(|f| f.event["type"] == "SUBAGENT_STARTED" && f.event["name"] == "OpenCode")
        .collect();
    assert_eq!(started.len(), 1);
    let open_code = started[0].event["subagentRunId"].as_str().unwrap();
    assert!(open_code.starts_with("sub-step-"), "{open_code}");
    assert!(
        started[0].event["parentSubagentRunId"]
            .as_str()
            .is_some_and(|p| p.starts_with("sub-") && !p.starts_with("sub-step-")),
        "it runs in the agent's invocation"
    );
    let command: Vec<&Frame> = frames
        .iter()
        .filter(|f| {
            f.event["type"] == "ACTIVITY_SNAPSHOT"
                && f.event["activityType"] == "vymalo.step"
                && f.event["content"]["kind"] == "command"
        })
        .collect();
    assert_eq!(command.len(), 2, "the command's start and its end");
    assert!(
        command
            .iter()
            .all(|f| f.event["subagentRunId"] == open_code)
    );
    assert_eq!(command[1].event["content"]["state"], "failed");
    assert_eq!(command[1].event["content"]["detail"], "1 failed");
    assert!(
        frames.iter().any(
            |f| f.event["type"] == "SUBAGENT_FINISHED" && f.event["subagentRunId"] == open_code
        )
    );
}

async fn a_chatty_agent_cannot_fill_the_log(backend: Backend) {
    let world = world_with_steps_on(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let id = chat.create_thread("plain", "steps-chatty go", None).await;
    chat.wait_state(&id, "done").await;
    let events = chat.events(&id).await;

    // 22 reports of one step: its start, its end and at most `MAX_STEP_UPDATES` updates
    let mut per_step: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for e in steps(&events) {
        per_step
            .entry(e["data"]["id"].as_str().unwrap().to_owned())
            .or_default()
            .push(e["data"]["phase"].as_str().unwrap());
    }
    assert_eq!(per_step.len(), 1);
    let phases = per_step.values().next().unwrap();
    assert_eq!(
        phases.len(),
        2 + usize::from(MAX_STEP_UPDATES),
        "{phases:?}"
    );
    assert_eq!(phases.first(), Some(&"start"));
    assert_eq!(phases.last(), Some(&"end"));
    assert_eq!(
        phases.iter().filter(|p| **p == "update").count(),
        usize::from(MAX_STEP_UPDATES)
    );
    assert_contiguous(&events);
}

/// The ledger of open steps is part of the job, so a replica that did not see a step start
/// still ends it with the path it started with.
async fn a_step_open_when_the_agent_asks_ends_in_the_next_run_with_its_path(backend: Backend) {
    let world = world_with_steps_on(backend).await;
    let first = world.instance("orch-1").await;
    let chat = world.chat(&first);
    let id = chat
        .create_thread("plain", "steps-ask clean the build", None)
        .await;
    chat.wait_state(&id, "blocked").await;
    let events = chat.events(&id).await;
    assert_eq!(
        shape(&events),
        [
            "user_message",
            "agent_status:working",
            "agent_step",
            "agent_step",
            "agent_status:input_required",
            "thread_state:blocked"
        ]
    );

    // the answer goes through another replica, which dispatches it
    let second = world.instance("orch-2").await;
    let chat2 = world.chat(&second);
    let run = chat2.follow_up(&id, "plain", "yes").await;
    assert_eq!(run.status, 200);
    chat2.wait_state(&id, "done").await;
    let events = chat2.events(&id).await;
    assert_contiguous(&events);
    assert_eq!(
        story(&events),
        [
            s("tool:c2", &[], "running", "start"),
            s("acp:c2:1", &["tool:c2"], "waiting", "start"),
            s("acp:c2:1", &["tool:c2"], "completed", "end"),
            s("tool:c2", &[], "completed", "end"),
        ]
    );
    // the second task of the thread reported the ends, and the ids are of the same task: the
    // thread's task continued
    let ids: Vec<&str> = steps(&events)
        .iter()
        .map(|e| e["data"]["id"].as_str().unwrap())
        .collect();
    let task = ids[0].split_once('/').unwrap().0;
    assert!(ids.iter().all(|i| i.starts_with(&format!("{task}/"))));
}

/// An agent that does not list the extension is not asked for steps, and is one level as before.
async fn an_agent_without_the_extension_is_not_asked(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let id = chat.create_thread("plain", "echo hi", None).await;
    chat.wait_state(&id, "done").await;
    let call = world.plain.executions().pop().unwrap();
    assert!(!call.activates(STEPS_EXTENSION));
    assert!(call.message_extensions.is_empty());
    assert!(steps(&chat.events(&id).await).is_empty());
}

/// A world on `backend` whose `plain` agent lists `steps/v1`.
async fn world_with_steps_on(backend: Backend) -> World {
    World::with(
        backend,
        Setup {
            plain: orch_testsupport::FakeAgentOptions {
                extensions: vec![STEPS_EXTENSION.to_owned()],
                ..orch_testsupport::FakeAgentOptions::default()
            },
            ..Setup::default()
        },
    )
    .await
}

backends!(
    nested_steps_reach_the_log_with_their_paths_and_the_viewer_sees_the_tree,
    a_chatty_agent_cannot_fill_the_log,
    a_step_open_when_the_agent_asks_ends_in_the_next_run_with_its_path,
    an_agent_without_the_extension_is_not_asked,
);
