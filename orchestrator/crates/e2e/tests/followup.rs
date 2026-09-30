//! A thread is a conversation (ADR 0020, ADR 0021), end to end on both stores, through the real
//! dispatcher and the A2A adapter against the fake agent: a message after `done`, `failed` or
//! `cancelled` starts the thread's next job as a new A2A task in the same context that names the
//! previous task, and the log says where the job began.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use common::*;

fn kinds(events: &[serde_json::Value]) -> Vec<String> {
    events
        .iter()
        .map(|e| e["kind"].as_str().unwrap().to_owned())
        .collect()
}

async fn a_follow_up_after_done_is_the_next_job_and_names_the_task_before_it(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let id = chat.seed_thread("plain", "echo first", None).await;
    chat.wait_state(&id, "done").await;
    chat.seed_message(&id, "echo second").await;
    chat.wait_state(&id, "done").await;
    // the second `done` is job 2's: wait for the second delivery to be complete
    let events = loop {
        let events = chat.events(&id).await;
        if kinds(&events)
            .iter()
            .filter(|k| *k == "thread_state")
            .count()
            == 2
        {
            break events;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    };
    assert_eq!(
        kinds(&events)
            .iter()
            .filter(|k| *k == "job_started")
            .count(),
        1
    );
    let job = chat.thread(&id).await;
    assert_eq!(job["state"], "done");

    let calls = world.plain.executions();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].context_id, calls[1].context_id, "one context");
    assert_ne!(
        calls[0].task_id, calls[1].task_id,
        "a new task, not a restart"
    );
    assert!(!calls[1].resuming);
    assert!(calls[0].reference_task_ids.is_empty());
    assert_eq!(
        calls[1].reference_task_ids,
        [calls[0].task_id.clone()],
        "the second task names the first"
    );
    assert_eq!(calls[1].text, "echo second");
}

async fn a_stopped_thread_takes_the_next_message_too(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let id = chat.seed_thread("plain", "slow work", None).await;
    chat.wait_state(&id, "working").await;
    assert_eq!(chat.cancel(&id).await, 202);
    chat.wait_state(&id, "cancelled").await;
    chat.seed_message(&id, "echo go on").await;
    chat.wait_state(&id, "done").await;
    let calls = world.plain.executions();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].reference_task_ids, [calls[0].task_id.clone()]);
    let events = chat.events(&id).await;
    assert_eq!(
        kinds(&events)
            .iter()
            .filter(|k| *k == "job_started")
            .count(),
        1
    );
}

backends!(
    a_follow_up_after_done_is_the_next_job_and_names_the_task_before_it,
    a_stopped_thread_takes_the_next_message_too,
);
