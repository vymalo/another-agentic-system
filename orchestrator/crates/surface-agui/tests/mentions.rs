//! Mentions on a run (ADR 0026, `mentions/v1`): `forwardedProps["vymalo.mentions"]` is read on
//! every run and refused before the stream when it is malformed (400) or does not hold (422), or
//! the registry cannot say (503); applied with the message otherwise. Nothing is written for a run
//! that is refused. The real router on a real port over the in-memory stack.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_core::{AgentId, AgentSource};
use orch_ports::memory::Call;
use orch_ports::{AgentEndpoint, RegistryEntry};
use serde_json::{Value, json};
use support::*;

const CODER_CARD: &str = "https://coder.example.com/.well-known/agent-card.json";

/// A person writes to `plain` and mentions `coder`; the emoji in front makes the offsets differ
/// in code points (`3`), UTF-16 code units (`3`..`9` after a pair) and UTF-8 bytes (`5`..`11`).
const TEXT: &str = "\u{1F604} @coder echo this";

fn coder(start: u32, end: u32) -> Value {
    json!({"agentId": "coder", "label": "@coder", "start": start, "end": end})
}

fn run_with(thread: &str, run: &str, text: &str, mentions: &Value) -> Value {
    input_with(
        thread,
        run,
        &[(&format!("m-{run}"), text)],
        json!({"forwardedProps": {"vymalo.mentions": mentions}}),
    )
}

/// Nothing of the run was written: no thread, no delegation.
async fn assert_nothing_written(h: &Harness, thread: &str) {
    assert!(
        h.app
            .thread_for_tools(thread.parse().unwrap())
            .await
            .unwrap()
            .is_none(),
        "the thread was created"
    );
    assert!(h.agent.sends().is_empty(), "the agent was sent something");
}

async fn refused(h: &Harness, agent: &str, body: &Value, status: u16) -> Value {
    h.refused(agent, Some(ALICE), body).await.problem(status)
}

#[tokio::test]
async fn a_run_with_mentions_records_them_as_sent_shows_them_on_the_message_and_tells_the_agent() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let mentions = json!([{"agentId": "coder", "label": "@coder", "start": 3, "end": 9,
        "cardUrl": CODER_CARD}]);
    let frames = h
        .run("plain", ALICE, &run_with(&thread, "run-1", TEXT, &mentions))
        .await
        .all()
        .await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");

    // the log: stored as sent, in the user message
    let events = h.events(ALICE, &thread).await;
    assert_eq!(events[0]["kind"], "user_message");
    assert_eq!(events[0]["data"]["text"], TEXT);
    assert_eq!(events[0]["data"]["mentions"], mentions);

    // the screen: the chips, on the START of the message, which a viewer connecting to the thread
    // is shown (the run that sent it does not hear its own message back)
    let viewed = h.connect_run(&thread, ALICE, None).await.all().await;
    let start = viewed
        .iter()
        .find(|f| f.kind() == "TEXT_MESSAGE_START" && f.event["role"] == "user")
        .expect("the user message is shown");
    assert_eq!(start.event["metadata"]["vymalo.mentions"], mentions);

    // the agent: named from the registry as it was when the message was sent
    let sends = h.agent.sends();
    let Call::Send {
        text,
        mentions: told,
        ..
    } = &sends[0]
    else {
        panic!("a send");
    };
    assert_eq!(text, TEXT, "the text is not rewritten");
    assert_eq!(told.len(), 1);
    assert_eq!(told[0].agent_id, AgentId::new("coder"));
    assert_eq!(told[0].name.as_deref(), Some("Coder"));
    assert_eq!(told[0].card_url.as_deref(), Some(CODER_CARD));
    assert_eq!((told[0].start, told[0].end), (3, 9));
    assert_eq!(told[0].label, "@coder");

    // the thread's job knows the agent it may ask
    let record = h
        .app
        .thread_for_tools(thread.parse().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.job.mentioned, [AgentId::new("coder")].into());
}

#[tokio::test]
async fn a_run_without_mentions_or_with_none_is_the_run_it_always_was() {
    let h = Harness::start().await;
    for (n, props) in [
        json!({}),
        json!({"forwardedProps": {"vymalo.mentions": []}}),
        json!({"forwardedProps": {"vymalo.mentions": null}}),
    ]
    .into_iter()
    .enumerate()
    {
        let thread = new_thread_id();
        let body = input_with(&thread, &format!("run-{n}"), &[("m", "echo hi")], props);
        let frames = h.run("plain", ALICE, &body).await.all().await;
        assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
        let events = h.events(ALICE, &thread).await;
        assert!(events[0]["data"].get("mentions").is_none(), "{events:?}");
        let viewed = h.connect_run(&thread, ALICE, None).await.all().await;
        let start = viewed
            .iter()
            .find(|f| f.kind() == "TEXT_MESSAGE_START" && f.event["role"] == "user")
            .unwrap();
        assert!(start.event["metadata"].get("vymalo.mentions").is_none());
    }
    for send in h.agent.sends() {
        let Call::Send { mentions, .. } = send else {
            unreachable!()
        };
        assert!(mentions.is_empty());
    }
}

#[tokio::test]
async fn a_follow_up_with_mentions_goes_with_its_message_and_starts_its_own_job_set() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let body = input(&thread, "run-1", &[("m-1", "echo hi")]);
    h.run("plain", ALICE, &body).await.all().await;
    h.wait_state(ALICE, &thread, "done").await;

    let follow_up = run_with(&thread, "run-2", TEXT, &json!([coder(3, 9)]));
    let frames = h.run("plain", ALICE, &follow_up).await.all().await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
    let events = h.events(ALICE, &thread).await;
    let messages: Vec<&Value> = events
        .iter()
        .filter(|e| e["kind"] == "user_message")
        .collect();
    assert_eq!(messages.len(), 2);
    assert!(messages[0]["data"].get("mentions").is_none());
    assert_eq!(messages[1]["data"]["mentions"], json!([coder(3, 9)]));
    let record = h
        .app
        .thread_for_tools(thread.parse().unwrap())
        .await
        .unwrap()
        .unwrap();
    // the second message started job 2, whose set is the second message's mentions
    assert_eq!(record.job.number, 2);
    assert_eq!(record.job.mentioned, [AgentId::new("coder")].into());
}

#[tokio::test]
async fn a_retry_of_the_same_run_does_not_write_the_message_twice() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let body = run_with(&thread, "run-1", TEXT, &json!([coder(3, 9)]));
    h.run("plain", ALICE, &body).await.all().await;
    h.run("plain", ALICE, &body).await.all().await;
    let events = h.events(ALICE, &thread).await;
    let messages = events
        .iter()
        .filter(|e| e["kind"] == "user_message")
        .count();
    assert_eq!(messages, 1);
}

#[tokio::test]
async fn a_member_that_is_not_an_array_of_references_is_400_and_nothing_is_written() {
    let h = Harness::start().await;
    let ok = coder(3, 9);
    let many: Vec<Value> = std::iter::repeat_n(ok.clone(), 17).collect();
    for (what, bad) in [
        ("a string", json!("@coder")),
        ("an object", json!({"agentId": "coder"})),
        ("a number", json!(3)),
        ("an element that is not an object", json!([1])),
        ("more than 16", json!(many)),
        (
            "a missing agentId",
            json!([{"label": "@coder", "start": 3, "end": 9}]),
        ),
        (
            "a missing label",
            json!([{"agentId": "coder", "start": 3, "end": 9}]),
        ),
        (
            "a missing start",
            json!([{"agentId": "coder", "label": "@coder", "end": 9}]),
        ),
        (
            "a missing end",
            json!([{"agentId": "coder", "label": "@coder", "start": 3}]),
        ),
        (
            "an agentId that is a number",
            json!([{"agentId": 1, "label": "@coder", "start": 3, "end": 9}]),
        ),
        (
            "a start that is a string",
            json!([{"agentId": "coder", "label": "@coder", "start": "3", "end": 9}]),
        ),
        (
            "a negative start",
            json!([{"agentId": "coder", "label": "@coder", "start": -3, "end": 9}]),
        ),
        (
            "a fractional end",
            json!([{"agentId": "coder", "label": "@coder", "start": 3, "end": 9.5}]),
        ),
        (
            "a cardUrl that is a number",
            json!([{"agentId": "coder", "label": "@coder", "start": 3, "end": 9, "cardUrl": 1}]),
        ),
        (
            "a member a reference does not have",
            json!([{"agentId": "coder", "label": "@coder", "start": 3, "end": 9, "role": "x"}]),
        ),
    ] {
        let thread = new_thread_id();
        let problem = refused(&h, "plain", &run_with(&thread, "run-1", TEXT, &bad), 400).await;
        assert!(
            problem["detail"].as_str().unwrap().contains("mention"),
            "{what}: {problem}"
        );
        assert_nothing_written(&h, &thread).await;
    }
    // sixteen are not too many
    let thread = new_thread_id();
    let text = "@coder ".repeat(16);
    let sixteen: Vec<Value> = (0..16u32).map(|i| coder(i * 7, i * 7 + 6)).collect();
    let frames = h
        .run(
            "plain",
            ALICE,
            &run_with(&thread, "run-16", &text, &json!(sixteen)),
        )
        .await
        .all()
        .await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
}

#[tokio::test]
async fn a_member_that_is_malformed_is_400_on_a_run_that_continues_a_thread_too() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    h.run(
        "plain",
        ALICE,
        &input(&thread, "run-1", &[("m-1", "echo hi")]),
    )
    .await
    .all()
    .await;
    h.wait_state(ALICE, &thread, "done").await;
    let before = h.events(ALICE, &thread).await.len();
    let sends = h.agent.sends().len();
    refused(
        &h,
        "plain",
        &run_with(&thread, "run-2", "echo more", &json!("@coder")),
        400,
    )
    .await;
    assert_eq!(h.events(ALICE, &thread).await.len(), before);
    assert_eq!(h.agent.sends().len(), sends);
}

#[tokio::test]
async fn a_reference_that_does_not_hold_against_the_text_is_422_and_nothing_is_written() {
    let h = Harness::start().await;
    for (what, mentions) in [
        // the offsets count code points (3..9 is right for this text; 2..8 is the code-point count
        // of an emoji before it) or bytes (5..11)
        ("offsets in code points", json!([coder(2, 8)])),
        ("offsets in bytes", json!([coder(5, 11)])),
        ("a start inside a surrogate pair", json!([coder(1, 7)])),
        ("an end past the text", json!([coder(3, 90)])),
        ("start not before end", json!([coder(9, 9)])),
        (
            "a label that is not the text",
            json!([{"agentId": "coder", "label": "@plain",
            "start": 3, "end": 9}]),
        ),
        (
            "a label without @",
            json!([{"agentId": "coder", "label": "coder", "start": 4, "end": 9}]),
        ),
        (
            "a label of one unit",
            json!([{"agentId": "coder", "label": "@", "start": 3, "end": 4}]),
        ),
        (
            "overlapping references",
            json!([coder(3, 9), {"agentId": "coder", "label": "@coder",
            "start": 3, "end": 9}]),
        ),
        (
            "references out of order",
            json!([
            {"agentId": "coder", "label": "echo", "start": 10, "end": 14},
            coder(3, 9)]),
        ),
    ] {
        let thread = new_thread_id();
        let problem = refused(
            &h,
            "plain",
            &run_with(&thread, "run-1", TEXT, &mentions),
            422,
        )
        .await;
        assert!(
            problem["detail"].as_str().unwrap().contains("mention"),
            "{what}: {problem}"
        );
        assert_nothing_written(&h, &thread).await;
    }
}

#[tokio::test]
async fn an_unknown_agent_a_moved_card_and_the_threads_own_agent_are_422() {
    let h = Harness::start().await;

    let unknown = json!([{"agentId": "ghost", "label": "@ghost", "start": 3, "end": 9}]);
    let thread = new_thread_id();
    let text = "\u{1F604} @ghost go";
    let problem = refused(
        &h,
        "plain",
        &run_with(&thread, "run-1", text, &unknown),
        422,
    )
    .await;
    assert_eq!(problem["detail"], "unknown agent 'ghost' in mentions");
    assert_nothing_written(&h, &thread).await;

    // an id that cannot be an agent's is as unknown as one nobody has
    let odd = json!([{"agentId": "../etc", "label": "@ghost", "start": 3, "end": 9}]);
    let thread = new_thread_id();
    let problem = refused(&h, "plain", &run_with(&thread, "run-2", text, &odd), 422).await;
    assert!(
        problem["detail"]
            .as_str()
            .unwrap()
            .starts_with("unknown agent")
    );
    assert_nothing_written(&h, &thread).await;

    // the card moved between the list and the send
    let moved = json!([{"agentId": "coder", "label": "@coder", "start": 3, "end": 9,
        "cardUrl": "https://coder.elsewhere.example.com/.well-known/agent-card.json"}]);
    let thread = new_thread_id();
    let problem = refused(&h, "plain", &run_with(&thread, "run-3", TEXT, &moved), 422).await;
    assert_eq!(
        problem["detail"],
        "the card of 'coder' moved; refresh the agent list"
    );
    assert_nothing_written(&h, &thread).await;

    // the thread's own agent cannot be mentioned in its own thread
    let thread = new_thread_id();
    let own = json!([{"agentId": "plain", "label": "@plain", "start": 3, "end": 9}]);
    let problem = refused(
        &h,
        "plain",
        &run_with(&thread, "run-4", "\u{1F604} @plain go", &own),
        422,
    )
    .await;
    assert_eq!(
        problem["detail"],
        "an agent cannot be mentioned in its own thread"
    );
    assert_nothing_written(&h, &thread).await;

    // and on a thread that exists: the same refusals, and the log is as it was
    let thread = new_thread_id();
    h.run(
        "plain",
        ALICE,
        &input(&thread, "run-5", &[("m-5", "echo hi")]),
    )
    .await
    .all()
    .await;
    h.wait_state(ALICE, &thread, "done").await;
    let (events, sends) = (h.events(ALICE, &thread).await.len(), h.agent.sends().len());
    let problem = refused(
        &h,
        "plain",
        &run_with(&thread, "run-6", text, &unknown),
        422,
    )
    .await;
    assert_eq!(problem["detail"], "unknown agent 'ghost' in mentions");
    assert_eq!(h.events(ALICE, &thread).await.len(), events);
    assert_eq!(h.agent.sends().len(), sends);
}

#[tokio::test]
async fn an_agent_that_the_registry_lists_can_be_mentioned_the_moment_it_is_listed() {
    let h = Harness::start().await;
    let text = "\u{1F604} @helper go";
    let helper = json!([{"agentId": "helper", "label": "@helper", "start": 3, "end": 10}]);
    let thread = new_thread_id();
    refused(&h, "plain", &run_with(&thread, "run-1", text, &helper), 422).await;

    h.registry.add(RegistryEntry {
        endpoint: AgentEndpoint::a2a(
            AgentId::new("helper"),
            "https://helper.agents.example.com/.well-known/agent-card.json",
            None,
        ),
        name: "Helper".to_owned(),
        tags: vec![],
        origin: AgentSource::Registry,
    });
    let frames = h
        .run("plain", ALICE, &run_with(&thread, "run-2", text, &helper))
        .await
        .all()
        .await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
    let sends = h.agent.sends();
    let Call::Send { mentions, .. } = &sends[0] else {
        panic!("a send")
    };
    assert_eq!(mentions[0].name.as_deref(), Some("Helper"));
}

#[tokio::test]
async fn a_registry_that_cannot_say_is_503_retryable_and_nothing_is_written() {
    let h = Harness::start().await;
    h.registry.add(RegistryEntry {
        endpoint: AgentEndpoint::a2a(
            AgentId::new("helper"),
            "https://helper.agents.example.com/.well-known/agent-card.json",
            None,
        ),
        name: "Helper".to_owned(),
        tags: vec![],
        origin: AgentSource::Registry,
    });
    h.registry.set_down(true);
    let text = "\u{1F604} @helper go";
    let helper = json!([{"agentId": "helper", "label": "@helper", "start": 3, "end": 10}]);
    let thread = new_thread_id();
    // the addressed agent is a static one, which the registry's trouble does not touch
    let r = h
        .refused(
            "plain",
            Some(ALICE),
            &run_with(&thread, "run-1", text, &helper),
        )
        .await;
    assert_eq!(r.status, 503, "{}", String::from_utf8_lossy(&r.body));
    assert!(r.headers.contains_key("retry-after"));
    assert_nothing_written(&h, &thread).await;

    // and it is retryable: once the registry answers, the same run goes through
    h.registry.set_down(false);
    let frames = h
        .run("plain", ALICE, &run_with(&thread, "run-1", text, &helper))
        .await
        .all()
        .await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
}

#[tokio::test]
async fn mentions_on_a_run_that_carries_no_message_are_ignored() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    h.run(
        "plain",
        ALICE,
        &input(&thread, "run-1", &[("m-1", "echo hi")]),
    )
    .await
    .all()
    .await;
    h.wait_state(ALICE, &thread, "done").await;
    // an attach to the run that exists
    let again = input_with(
        &thread,
        "run-1",
        &[("m-1", "echo hi")],
        json!({"forwardedProps": {"vymalo.mentions": [coder(3, 9)]}}),
    );
    let resp = h.post("plain", Some(ALICE), &again).await;
    assert_eq!(resp.status().as_u16(), 200);
}
