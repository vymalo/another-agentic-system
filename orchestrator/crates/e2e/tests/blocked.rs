//! `input-required` blocks the thread; a follow-up continues the SAME A2A task.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use common::*;

async fn input_required_blocks_and_the_follow_up_resumes_the_same_task(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);

    let id = chat
        .create_thread("plain", "ask about branches", None)
        .await;
    chat.wait_state(&id, "blocked").await;
    let events = chat.events(&id).await;
    assert_eq!(
        shape(&events),
        [
            "user_message",
            "agent_status:working",
            "agent_status:input_required",
            "thread_state:blocked"
        ]
    );
    assert_eq!(events[2]["data"]["detail"], "Which branch?");

    let (status, body) = chat.post_message(&id, "main").await;
    assert_eq!(status, 202, "{body}");
    assert_eq!(body["kind"], "user_message");
    assert_eq!(body["data"]["text"], "main");
    chat.wait_state(&id, "done").await;

    let events = chat.events(&id).await;
    assert_eq!(
        shape(&events),
        [
            "user_message",
            "agent_status:working",
            "agent_status:input_required",
            "thread_state:blocked",
            "user_message",
            "agent_status:working",
            "artifact",
            "agent_status:completed",
            "thread_state:done"
        ]
    );
    assert_contiguous(&events);
    assert_eq!(events[6]["data"]["text"], "answered: main");

    // The agent side: two executions of ONE task; the second continued the blocked task.
    let calls = world.plain.executions();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].task_id, calls[1].task_id, "same A2A task");
    assert_eq!(calls[0].context_id, calls[1].context_id, "same A2A context");
    assert!(!calls[0].resuming);
    assert!(calls[1].resuming);
    assert_eq!(calls[1].text, "main");
}

async fn a_follow_up_survives_an_orchestrator_restart_between_the_turns(backend: Backend) {
    let world = World::start(backend).await;
    let first = world.instance("orch-1").await;
    let chat = world.chat(&first);
    let id = chat.create_thread("plain", "ask again", None).await;
    chat.wait_state(&id, "blocked").await;
    first.shutdown().await;

    let second = world.instance("orch-2").await;
    let chat = world.chat(&second);
    let (status, _) = chat.post_message(&id, "develop").await;
    assert_eq!(status, 202);
    chat.wait_state(&id, "done").await;
    let calls = world.plain.executions();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].task_id, calls[1].task_id);
    assert!(calls[1].resuming);
    assert_contiguous(&chat.events(&id).await);
}

async fn auth_required_blocks_with_its_detail(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);

    let id = chat.create_thread("plain", "auth to github", None).await;
    chat.wait_state(&id, "blocked").await;
    let events = chat.events(&id).await;
    assert_eq!(
        shape(&events),
        [
            "user_message",
            "agent_status:working",
            "agent_status:input_required",
            "thread_state:blocked"
        ]
    );
    assert_eq!(
        events[2]["data"]["detail"], "authentication required: github",
        "the chat is told it is an authentication request, and for what"
    );

    // The user answers (say, "done, retry"): the same A2A task continues.
    let (status, body) = chat.post_message(&id, "signed in").await;
    assert_eq!(status, 202, "{body}");
    chat.wait_state(&id, "done").await;
    let events = chat.events(&id).await;
    assert_eq!(events[6]["data"]["text"], "answered: signed in");
    assert_contiguous(&events);
    let calls = world.plain.executions();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].task_id, calls[1].task_id, "same A2A task");
    assert!(calls[1].resuming);
}

backends!(
    auth_required_blocks_with_its_detail,
    input_required_blocks_and_the_follow_up_resumes_the_same_task,
    a_follow_up_survives_an_orchestrator_restart_between_the_turns,
);
