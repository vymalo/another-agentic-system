//! MCP servers attached to a thread, end to end (ADR 0024), on both stores: the real router, the
//! real dispatcher, the A2A adapter and two in-process fake agents, one whose card lists
//! `thread-tools/v1` and one whose card does not. A person attaches servers when a thread is
//! created and afterwards; the agent whose card lists the extension is told which are attached
//! (`attached` of its message), the other gets exactly the message it always got, and nothing an
//! agent is told, nothing the log keeps and nothing the API says holds a URL or a credential.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use common::*;
use orch_core::THREAD_TOOLS_EXTENSION;
use orch_testsupport::{FakeAgentOptions, eventually};
use serde_json::{Value, json};

/// `plain` lists `thread-tools/v1` and the adapter has the key to mint its grants; `coder` lists
/// nothing. The deployment offers three servers, one of them the coder's alone.
async fn world(backend: Backend) -> World {
    World::with(
        backend,
        Setup {
            thread_tools: true,
            plain: FakeAgentOptions {
                extensions: vec![THREAD_TOOLS_EXTENSION.to_owned()],
                ..FakeAgentOptions::default()
            },
            tool_servers: sample_tool_servers(),
            ..Setup::default()
        },
    )
    .await
}

async fn wait_for_jobs(chat: &orch_testsupport::Chat, thread: &str, jobs: usize) {
    eventually("the job to finish", || async {
        let events = chat.events(thread).await;
        let done = events
            .iter()
            .filter(|e| e["kind"] == "thread_state" && e["data"]["state"] == "done")
            .count();
        (done == jobs).then_some(())
    })
    .await;
}

fn kinds(events: &[Value]) -> Vec<&str> {
    events.iter().map(|e| e["kind"].as_str().unwrap()).collect()
}

/// Nothing the agent was given reaches what the system keeps or says about the thread: not the
/// endpoint's URL, not the token. (The only servers the deployment has here are named by id; a
/// real one would also have a URL and a credential, which `tests/config.rs` of `orch-config` and
/// `orch-app` keep out of every type the application holds.)
async fn nothing_the_agent_was_given_is_kept(
    chat: &orch_testsupport::Chat,
    thread: &str,
    given: &[Value],
) {
    let mut everything = String::new();
    everything.push_str(&chat.thread(thread).await.to_string());
    everything.push_str(&serde_json::to_string(&chat.events(thread).await).unwrap());
    let (_, export) = chat.get(&format!("/api/threads/{thread}/export")).await;
    everything.push_str(&export.to_string());
    assert!(!given.is_empty());
    for grant in given {
        for member in ["url", "token"] {
            let value = grant[member].as_str().unwrap();
            assert!(
                !everything.contains(value),
                "the grant's {member} is in what the system keeps"
            );
        }
        assert!(!everything.contains("/thread-tools/"));
    }
}

async fn the_servers_attached_when_the_thread_is_created_reach_an_agent_that_lists_the_extension(
    backend: Backend,
) {
    let world = world(backend).await;
    let orch = world.instance_with_thread_tools("orch-1", true).await;
    let chat = world.chat(&orch);

    let (status, created) = chat
        .try_create_thread_with_tools("plain", "echo hi", &["websearch", "docs"])
        .await;
    assert_eq!(status, 200, "{created}");
    let thread = created["threadId"].as_str().unwrap().to_owned();
    wait_for_jobs(&chat, &thread, 1).await;

    // the log: the message, then the servers, in the first commit
    let events = chat.events(&thread).await;
    assert_eq!(kinds(&events)[..2], ["user_message", "tools_attached"]);
    assert_eq!(events[1]["data"], json!({"servers": ["docs", "websearch"]}));
    assert_eq!(
        chat.thread(&thread).await["tools"],
        json!(["docs", "websearch"])
    );

    // the agent: the grant, and `attached` with ids, names and descriptions in the order of the ids
    let calls = world.plain.executions();
    assert_eq!(calls.len(), 1);
    let grant = calls[0].thread_tools.as_ref().expect("a grant");
    assert_eq!(
        grant["attached"],
        json!([
            {"server": "docs", "name": "Documentation"},
            {"server": "websearch", "name": "Web search", "description": "Search the web."},
        ])
    );
    // the endpoint is there; no other URL is: the agent reaches a server only through it.
    // The token is random and may hold the letters `http` itself, so it is left out of the count.
    let mut without_token = grant.clone();
    without_token
        .as_object_mut()
        .expect("a grant is an object")
        .remove("token");
    let text = without_token.to_string();
    assert!(grant["url"].as_str().unwrap().contains("/thread-tools/"));
    assert_eq!(
        text.matches("http").count(),
        1,
        "one URL, the endpoint's: {text}"
    );
    assert!(calls[0].activates(THREAD_TOOLS_EXTENSION));
    nothing_the_agent_was_given_is_kept(&chat, &thread, std::slice::from_ref(grant)).await;
}

async fn an_agent_whose_card_does_not_list_the_extension_gets_the_message_it_always_got(
    backend: Backend,
) {
    let world = world(backend).await;
    let orch = world.instance_with_thread_tools("orch-1", true).await;
    let chat = world.chat(&orch);

    // the same servers, on the agent that does not list `thread-tools/v1` (the coder may have
    // `repos` too)
    let (status, created) = chat
        .try_create_thread_with_tools("coder", "echo hi", &["repos", "docs"])
        .await;
    assert_eq!(status, 200, "{created}");
    let thread = created["threadId"].as_str().unwrap().to_owned();
    wait_for_jobs(&chat, &thread, 1).await;
    // the thread has them, and the person is told by the screen that this agent cannot use them
    assert_eq!(
        chat.thread(&thread).await["tools"],
        json!(["docs", "repos"])
    );
    let calls = world.coder.executions();
    assert_eq!(calls.len(), 1);
    assert!(
        calls[0].thread_tools.is_none(),
        "nothing about tools: {:?}",
        calls[0].thread_tools
    );
    assert!(!calls[0].activates(THREAD_TOOLS_EXTENSION));
    assert!(
        !calls[0]
            .message_extensions
            .iter()
            .any(|e| e == THREAD_TOOLS_EXTENSION)
    );
    assert_eq!(
        calls[0].text, "echo hi",
        "the text is the person's, untouched"
    );
}

async fn what_is_attached_at_each_send_is_what_the_agent_is_told(backend: Backend) {
    let world = world(backend).await;
    let orch = world.instance_with_thread_tools("orch-1", true).await;
    let chat = world.chat(&orch);

    // no servers: the grant has no `attached`
    let thread = chat.create_thread("plain", "echo one", None).await;
    wait_for_jobs(&chat, &thread, 1).await;
    let first = world.plain.executions();
    assert!(first[0].thread_tools.is_some());
    assert!(first[0].attached().is_empty());
    assert!(
        first[0]
            .thread_tools
            .as_ref()
            .unwrap()
            .get("attached")
            .is_none()
    );

    // attach one after the fact: the next message says it
    let (status, body) = chat.put_tools(&thread, &["websearch"]).await;
    assert_eq!((status, &body), (200, &json!({"servers": ["websearch"]})));
    chat.follow_up(&thread, "plain", "echo two").await;
    wait_for_jobs(&chat, &thread, 2).await;
    let calls = world.plain.executions();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].attached().len(), 1);
    assert_eq!(calls[1].attached()[0]["server"], "websearch");

    // a server that is not for this agent cannot be attached to its thread, and nothing changed
    let (status, body) = chat.put_tools(&thread, &["websearch", "repos"]).await;
    assert_eq!(status, 422, "{body}");
    assert_eq!(chat.thread(&thread).await["tools"], json!(["websearch"]));

    // detach it: the third message has no `attached` again
    let (status, _) = chat.put_tools(&thread, &[]).await;
    assert_eq!(status, 200);
    chat.follow_up(&thread, "plain", "echo three").await;
    wait_for_jobs(&chat, &thread, 3).await;
    let calls = world.plain.executions();
    assert_eq!(calls.len(), 3);
    assert!(calls[2].attached().is_empty());
    assert!(chat.thread(&thread).await.get("tools").is_none());

    let events = chat.events(&thread).await;
    let tool_events: Vec<(&str, &Value)> = events
        .iter()
        .filter(|e| e["kind"].as_str().unwrap().starts_with("tools_"))
        .map(|e| (e["kind"].as_str().unwrap(), &e["data"]["servers"]))
        .collect();
    assert_eq!(
        tool_events,
        [
            ("tools_attached", &json!(["websearch"])),
            ("tools_detached", &json!(["websearch"]))
        ]
    );
    let given: Vec<Value> = world
        .plain
        .executions()
        .iter()
        .filter_map(|c| c.thread_tools.clone())
        .collect();
    nothing_the_agent_was_given_is_kept(&chat, &thread, &given).await;
}

async fn the_servers_are_listed_without_a_url_and_a_refused_creation_makes_no_thread(
    backend: Backend,
) {
    let world = world(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let (status, list) = chat.tool_servers().await;
    assert_eq!(status, 200);
    assert_eq!(
        list,
        json!([
            {"id": "websearch", "name": "Web search", "description": "Search the web."},
            {"id": "docs", "name": "Documentation"},
            {"id": "repos", "name": "Repositories", "agents": ["coder"]},
        ])
    );
    for refused in [vec!["nosuch"], vec!["repos"]] {
        let (status, body) = chat
            .try_create_thread_with_tools("plain", "echo hi", &refused)
            .await;
        assert_eq!(status, 422, "{refused:?}: {body}");
    }
    assert!(world.plain.executions().is_empty());
    let (_, threads) = chat.get("/api/threads").await;
    assert_eq!(threads, json!([]));
}

backends!(
    the_servers_attached_when_the_thread_is_created_reach_an_agent_that_lists_the_extension,
    an_agent_whose_card_does_not_list_the_extension_gets_the_message_it_always_got,
    what_is_attached_at_each_send_is_what_the_agent_is_told,
    the_servers_are_listed_without_a_url_and_a_refused_creation_makes_no_thread,
);
