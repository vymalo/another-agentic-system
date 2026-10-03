//! Sending while an agent works (ADR 0036), end to end on both stores, through the real dispatcher
//! and the A2A adapter against the fake agent: an agent that lists `steer/v1` has the message read
//! by its running task in the same job, and one that does not has it after its turn, as the next
//! job. Neither loses the message.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use common::*;
use orch_testsupport::CallKind;
use serde_json::Value;

fn shape(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .map(|e| e["kind"].as_str().unwrap().to_owned())
        .collect()
}

async fn the_running_task_of_an_agent_that_lists_the_extension_reads_the_message(backend: Backend) {
    let world = world_with_steer_on(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let id = chat
        .seed_thread("plain", "steerable refactor the parser", None)
        .await;
    chat.wait_state(&id, "working").await;
    chat.seed_message(&id, "you were wrong since line 1").await;
    eventually(&format!("the task of {id} to read the message"), || async {
        chat.events(&id)
            .await
            .iter()
            .any(|e| {
                e["kind"] == "agent_message"
                    && e["data"]["text"] == "steered: you were wrong since line 1"
            })
            .then_some(())
    })
    .await;
    world.plain.release_gate();
    chat.wait_state(&id, "done").await;

    let events = chat.events(&id).await;
    assert_eq!(
        shape(&events),
        [
            "user_message",
            "agent_status",
            "user_message",
            "agent_message",
            "artifact",
            "agent_status",
            "thread_state",
        ],
        "one job, one end"
    );
    assert_eq!(events[2]["data"]["delivery"], "steer");
    // one task, one steer into it, in its context, and no second message delivered after the turn
    assert_eq!(world.plain.executions().len(), 1);
    let steers: Vec<_> = world
        .plain
        .calls()
        .into_iter()
        .filter(|c| c.kind == CallKind::Steer)
        .collect();
    assert_eq!(steers.len(), 1);
    assert_eq!(steers[0].task_id, world.plain.executions()[0].task_id);
    assert_eq!(steers[0].text, "you were wrong since line 1");
    assert!(steers[0].reference_task_ids.is_empty());
}

async fn an_agent_that_does_not_list_the_extension_has_the_message_after_its_turn(
    backend: Backend,
) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let id = chat
        .seed_thread("plain", "gate refactor the parser", None)
        .await;
    chat.wait_state(&id, "working").await;
    chat.seed_message(&id, "echo you were wrong since line 1")
        .await;
    // nothing is sent into the task: the agent did not say it reads one
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        world
            .plain
            .calls()
            .iter()
            .all(|c| c.kind != CallKind::Steer),
        "a message for a running task is never sent to an agent that does not list steer/v1"
    );
    world.plain.release_gate();
    eventually(&format!("job 2 of {id} to be done"), || async {
        let events = chat.events(&id).await;
        (events
            .iter()
            .filter(|e| e["kind"] == "thread_state")
            .count()
            == 2)
            .then_some(())
    })
    .await;
    let executions = world.plain.executions();
    assert_eq!(executions.len(), 2, "the message is the next job's task");
    assert_eq!(executions[1].text, "echo you were wrong since line 1");
    assert_eq!(
        executions[1].reference_task_ids,
        [executions[0].task_id.clone()]
    );
}

backends!(
    the_running_task_of_an_agent_that_lists_the_extension_reads_the_message,
    an_agent_that_does_not_list_the_extension_has_the_message_after_its_turn,
);
