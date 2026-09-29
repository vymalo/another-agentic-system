//! What the agent SAYS (A2A `Message` frames) reaches the chat: as `agent_message` events,
//! each once and final.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use std::collections::HashSet;

use common::*;

async fn agent_messages_arrive_once_with_final_true(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);

    let id = chat.create_thread("plain", "messages about it", None).await;
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

    let messages = data_of(&events, "agent_message");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["text"], "message one");
    assert_eq!(messages[1]["text"], "message two");
    let mut ids = HashSet::new();
    for m in &messages {
        assert_eq!(m["final"], true, "the adapter never streams partial text");
        let message_id = m["messageId"].as_str().expect("a messageId");
        assert!(!message_id.is_empty());
        assert!(ids.insert(message_id), "message ids are unique");
    }
    for e in events.iter().filter(|e| e["kind"] == "agent_message") {
        assert_eq!(e["actor"]["type"], "agent");
        assert_eq!(e["actor"]["name"], "plain");
    }
    assert_eq!(
        world.plain.executions().len(),
        1,
        "one message reached the agent"
    );
}

backends!(agent_messages_arrive_once_with_final_true);
