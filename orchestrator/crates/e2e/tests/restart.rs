//! Restart safety: the orchestrator process dies mid-stream; a new one on the same database
//! finishes the thread with no gap and no duplicate.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use common::*;
use orch_testsupport::FakeAgentOptions;

/// Kills instance 1 while the agent works on `gate`, then finishes on instance 2.
///
/// `release_before_restart`: the agent completes while no orchestrator is alive, so the new
/// instance finds a finished task (`SubscribeToTask` says TASK_NOT_FOUND; it polls `GetTask`).
/// Otherwise the new instance re-attaches to the live task and the gate opens afterwards.
async fn crash_scenario(backend: Backend, release_before_restart: bool, resubscribe: bool) {
    let world = World::with(
        backend,
        Setup {
            plain: FakeAgentOptions {
                resubscribe,
                ..FakeAgentOptions::default()
            },
            ..Setup::default()
        },
    )
    .await;
    let first = world.instance("orch-1").await;
    let chat = world.chat(&first);
    let id = chat.create_thread("plain", "gate crash", None).await;
    chat.wait_state(&id, "working").await;
    // The A2A task is definitely running on the agent.
    eventually("the agent executes the task", || async {
        (world.plain.executions().len() == 1).then_some(())
    })
    .await;

    // The process dies mid-stream: nothing is released, its lease has to expire.
    first.kill();
    if release_before_restart {
        world.plain.release_gate();
    }

    let second = world.instance("orch-2").await;
    let chat = world.chat(&second);
    if !release_before_restart {
        // Wait until the new instance has re-attached (or started polling), then let the
        // agent finish, so the rest of the task really is observed by the new instance.
        let method = if resubscribe {
            "subscribe_to_task"
        } else {
            "get_task"
        };
        eventually(
            "the new instance re-attaches to the running task",
            || async { (world.plain.rpc_count(method) >= 1).then_some(()) },
        )
        .await;
        world.plain.release_gate();
    }
    chat.wait_state(&id, "done").await;

    // How the new instance found out matters: it must not have sent the message again.
    assert_eq!(world.plain.rpc_count("send_streaming_message"), 1);
    if release_before_restart {
        assert!(
            world.plain.rpc_count("get_task") >= 1,
            "a finished task can only be read back by polling"
        );
    }
    let events = chat.events(&id).await;
    assert_eq!(shape(&events), FIVE, "each A2A update exactly once");
    assert_contiguous(&events);
    let artifacts = data_of(&events, "artifact");
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0]["text"], "echo: gate crash");
    assert_eq!(
        world.plain.executions().len(),
        1,
        "the message must reach the agent exactly once"
    );
    second.shutdown().await;
}

async fn kill_mid_stream_and_finish_while_down_polls_the_result(backend: Backend) {
    crash_scenario(backend, true, true).await;
}

async fn kill_mid_stream_then_resubscribe_finishes_live(backend: Backend) {
    crash_scenario(backend, false, true).await;
}

async fn kill_mid_stream_without_resubscribe_falls_back_to_polling(backend: Backend) {
    crash_scenario(backend, false, false).await;
}

async fn kill_mid_stream_and_finish_while_down_without_resubscribe(backend: Backend) {
    crash_scenario(backend, true, false).await;
}

async fn a_graceful_shutdown_hands_the_thread_over_immediately(backend: Backend) {
    let world = World::start(backend).await;
    let first = world.instance("orch-1").await;
    let chat = world.chat(&first);
    let id = chat.create_thread("plain", "gate handover", None).await;
    chat.wait_state(&id, "working").await;
    first.shutdown().await; // releases the lease

    let second = world.instance("orch-2").await;
    let chat = world.chat(&second);
    world.plain.release_gate();
    chat.wait_state(&id, "done").await;
    let events = chat.events(&id).await;
    assert_eq!(shape(&events), FIVE);
    assert_contiguous(&events);
    assert_eq!(world.plain.executions().len(), 1);
}

/// The agent sends messages, the process dies while the task waits at its gate, and the new process
/// finishes it: what the agent already said is not said again.
async fn agent_messages_are_not_duplicated_across_a_crash(backend: Backend) {
    let world = World::start(backend).await;
    let first = world.instance("orch-1").await;
    let chat = world.chat(&first);
    let id = chat
        .create_thread("plain", "messages gate crash", None)
        .await;
    // Both messages are in the log before the crash; the task is parked at the gate.
    let before = chat.wait_events(&id, 4).await;
    assert_eq!(
        shape(&before),
        [
            "user_message",
            "agent_status:working",
            "agent_message",
            "agent_message"
        ]
    );

    first.kill();
    let second = world.instance("orch-2").await;
    let chat = world.chat(&second);
    // The new instance re-attaches to the still-running task before the gate opens.
    eventually(
        "the new instance re-attaches to the running task",
        || async { (world.plain.rpc_count("subscribe_to_task") >= 1).then_some(()) },
    )
    .await;
    world.plain.release_gate();
    chat.wait_state(&id, "done").await;

    let events = chat.events(&id).await;
    assert_eq!(
        shape(&events),
        [
            "user_message",
            "agent_status:working",
            "agent_message",
            "agent_message",
            "artifact",
            "agent_status:completed",
            "thread_state:done"
        ]
    );
    assert_contiguous(&events);
    let ids: Vec<&str> = data_of(&events, "agent_message")
        .iter()
        .map(|m| m["messageId"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
    assert_eq!(
        &events[..4],
        &before[..],
        "what was said before the crash is untouched"
    );
    assert_eq!(world.plain.rpc_count("send_streaming_message"), 1);
    assert_eq!(world.plain.executions().len(), 1);
    second.shutdown().await;
}

backends!(
    agent_messages_are_not_duplicated_across_a_crash,
    kill_mid_stream_and_finish_while_down_polls_the_result,
    kill_mid_stream_then_resubscribe_finishes_live,
    kill_mid_stream_without_resubscribe_falls_back_to_polling,
    kill_mid_stream_and_finish_while_down_without_resubscribe,
    a_graceful_shutdown_hands_the_thread_over_immediately,
);
