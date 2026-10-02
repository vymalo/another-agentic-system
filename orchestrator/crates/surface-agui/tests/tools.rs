//! MCP servers attached on a run (ADR 0024): `forwardedProps["vymalo.tools"]` is read on every
//! run and refused before the stream when it is malformed (400) or names what cannot be attached
//! (422), applied only when the run creates the thread, and ignored on a run that continues one.
//! The real router on a real port over the in-memory stack.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_ports::memory::Call;
use serde_json::{Value, json};
use support::*;

fn with_tools(thread: &str, run: &str, message: &str, tools: &Value) -> Value {
    input_with(
        thread,
        run,
        &[(&format!("m-{run}"), message)],
        json!({"forwardedProps": {"vymalo.tools": tools}}),
    )
}

fn kinds_of(events: &[Value]) -> Vec<&str> {
    events.iter().map(|e| e["kind"].as_str().unwrap()).collect()
}

/// The `thread.tools` of every `STATE_SNAPSHOT` of a response (`Null` for one that says none).
fn snapshot_tools(frames: &[Frame]) -> Vec<Value> {
    frames
        .iter()
        .filter(|f| f.kind() == "STATE_SNAPSHOT")
        .map(|f| f.event["snapshot"]["thread"]["tools"].clone())
        .collect()
}

#[tokio::test]
async fn a_run_that_creates_a_thread_attaches_its_servers_after_the_message_and_says_so() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let frames = h
        .run(
            "plain",
            ALICE,
            &with_tools(&thread, "run-1", "echo hi", &json!(["websearch"])),
        )
        .await
        .all()
        .await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");

    // the log: the message, then the servers, in the creation commit
    let events = h.events(ALICE, &thread).await;
    assert_eq!(kinds_of(&events)[..2], ["user_message", "tools_attached"]);
    assert_eq!(events[1]["actor"], json!({"type": "user", "name": ALICE}));
    assert_eq!(events[1]["data"], json!({"servers": ["websearch"]}));

    // the thread has them, and so does every snapshot from the event on
    let record = h.thread(ALICE, &thread).await;
    assert_eq!(record["tools"], json!(["websearch"]));
    let tools = snapshot_tools(&frames);
    assert_eq!(
        tools.first(),
        Some(&Value::Null),
        "before the event it said none"
    );
    assert!(
        tools.last() == Some(&json!(["websearch"])),
        "after it every snapshot does: {tools:?}"
    );

    // a card says what changed, inside the run
    let card = frames
        .iter()
        .find(|f| f.event["activityType"] == "vymalo.tools")
        .expect("a vymalo.tools activity");
    assert_eq!(card.kind(), "ACTIVITY_SNAPSHOT");
    assert_eq!(card.event["content"]["attached"], json!(["websearch"]));
    assert!(card.event["content"].get("detached").is_none());
    assert_eq!(card.event["messageId"], "evt-2");

    // the agent was told what is attached, with no URL
    let sends = h.agent.sends();
    let Call::Send { thread_tools, .. } = &sends[0] else {
        panic!("{sends:?}");
    };
    let grant = thread_tools.as_ref().expect("a grant");
    assert_eq!(
        grant
            .attached
            .iter()
            .map(|s| (s.id.as_str(), s.name.as_str()))
            .collect::<Vec<_>>(),
        [("websearch", "Web search")]
    );
}

#[tokio::test]
async fn an_empty_or_null_or_absent_member_attaches_nothing() {
    let h = Harness::start().await;
    for (n, tools) in [json!([]), Value::Null].into_iter().enumerate() {
        let thread = new_thread_id();
        let frames = h
            .run(
                "plain",
                ALICE,
                &with_tools(&thread, &format!("run-{n}"), "echo hi", &tools),
            )
            .await
            .all()
            .await;
        assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
        assert!(!kinds_of(&h.events(ALICE, &thread).await).contains(&"tools_attached"));
        assert!(snapshot_tools(&frames).iter().all(Value::is_null));
    }
}

#[tokio::test]
async fn a_malformed_member_is_a_400_and_a_server_that_cannot_be_attached_a_422_with_nothing_written()
 {
    let h = Harness::start().await;
    for (why, tools, status) in [
        ("not an array", json!("websearch"), 400),
        ("an object", json!({"websearch": true}), 400),
        ("a number in it", json!(["websearch", 1]), 400),
        ("an id that is not one", json!(["Web Search"]), 400),
        ("an unknown server", json!(["nosuch"]), 422),
        ("a server for another agent", json!(["repos"]), 422),
        (
            "too many",
            Value::from((0..17).map(|n| format!("s{n}")).collect::<Vec<_>>()),
            422,
        ),
    ] {
        let thread = new_thread_id();
        let resp = h
            .refused(
                "plain",
                Some(ALICE),
                &with_tools(&thread, "run-1", "echo hi", &tools),
            )
            .await;
        let problem = resp.problem(status);
        let detail = problem["detail"].as_str().unwrap();
        assert!(
            !detail.contains("http"),
            "{why}: no URL in a refusal: {detail}"
        );
        // no thread was made, no message sent
        assert_eq!(
            h.get(&format!("/api/threads/{thread}"), Some(ALICE))
                .await
                .status,
            404,
            "{why}"
        );
    }
    assert!(h.agent.sends().is_empty());
    // the coder may have `repos`
    let thread = new_thread_id();
    let frames = h
        .run(
            "coder",
            ALICE,
            &with_tools(&thread, "run-1", "echo hi", &json!(["repos"])),
        )
        .await
        .all()
        .await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
}

#[tokio::test]
async fn a_run_that_continues_a_thread_ignores_the_member_and_a_malformed_one_is_still_refused() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    h.run(
        "plain",
        ALICE,
        &input(&thread, "run-1", &[("m-1", "echo one")]),
    )
    .await
    .all()
    .await;
    h.wait_state(ALICE, &thread, "done").await;
    // a follow-up that asks for servers: the message goes through, the servers do not
    h.run(
        "plain",
        ALICE,
        &input_with(
            &thread,
            "run-2",
            &[("m-1", "echo one"), ("m-2", "echo two")],
            json!({"forwardedProps": {"vymalo.tools": ["websearch"]}}),
        ),
    )
    .await
    .all()
    .await;
    h.wait_state(ALICE, &thread, "done").await;
    assert!(!kinds_of(&h.events(ALICE, &thread).await).contains(&"tools_attached"));
    assert!(h.thread(ALICE, &thread).await.get("tools").is_none());
    // read on every run: a malformed one is refused here too, before anything is written
    let before = h.events(ALICE, &thread).await;
    let resp = h
        .refused(
            "plain",
            Some(ALICE),
            &input_with(
                &thread,
                "run-3",
                &[
                    ("m-1", "echo one"),
                    ("m-2", "echo two"),
                    ("m-3", "echo three"),
                ],
                json!({"forwardedProps": {"vymalo.tools": 7}}),
            ),
        )
        .await;
    resp.problem(400);
    assert_eq!(h.events(ALICE, &thread).await, before);
}

#[tokio::test]
async fn a_retry_of_the_creating_run_attaches_and_records_nothing_twice() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let body = with_tools(&thread, "run-1", "echo hi", &json!(["websearch"]));
    h.run("plain", ALICE, &body).await.all().await;
    h.wait_state(ALICE, &thread, "done").await;
    let before = h.events(ALICE, &thread).await;
    let again = h.run("plain", ALICE, &body).await.all().await;
    assert_eq!(again[0].kind(), "RUN_STARTED");
    assert_eq!(h.events(ALICE, &thread).await, before);
    assert_eq!(h.agent.sends().len(), 1);
}

#[tokio::test]
async fn a_viewer_who_connects_later_is_told_the_servers_too() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    h.run(
        "plain",
        ALICE,
        &with_tools(&thread, "run-1", "echo hi", &json!(["websearch"])),
    )
    .await
    .all()
    .await;
    h.wait_state(ALICE, &thread, "done").await;
    let resp = h
        .connect_raw(&thread, Some(ALICE), None, Some("mode=run"))
        .await;
    let frames = Stream::new(resp).all().await;
    assert!(
        frames
            .iter()
            .any(|f| f.event["activityType"] == "vymalo.tools"),
        "{:?}",
        kinds(&frames)
    );
    assert!(snapshot_tools(&frames).contains(&json!(["websearch"])));
}
