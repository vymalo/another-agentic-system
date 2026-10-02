//! `turn_output`: an agent announces its answer (ADR 0031). Over the real endpoint on a real
//! port, with the in-memory stack and tokens minted the way the A2A adapter mints them.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_core::{
    AnswerVia, EventBody, MAX_ANSWER_BYTES, MessagePurpose, ThreadId, ThreadState, UserId,
};
use serde_json::json;
use support::*;

/// A client whose token is for job `job`, under the message id `jti`.
async fn client_as(h: &Harness, thread: ThreadId, job: u32, jti: &str) -> Client {
    let mut claims = claims(thread, "plain");
    claims.job = job;
    claims.message_id = jti.to_owned();
    connect(&h.url(thread), &token(&keys(), &claims)).await
}

/// The agent's messages of the thread, as the log has them.
async fn messages(
    h: &Harness,
    thread: ThreadId,
) -> Vec<(orch_core::Actor, orch_core::AgentMessageData)> {
    h.events(thread)
        .await
        .into_iter()
        .filter_map(|e| match e.body {
            EventBody::AgentMessage(m) => Some((e.actor, m)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn turn_output_is_listed_after_get_ui_catalog_with_its_schemas() {
    let h = Harness::start().await;
    let thread = h.working_thread("plain").await;
    let client = client_as(&h, thread, 1, "m-1").await;
    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(
        tools.iter().map(|t| t.name.to_string()).collect::<Vec<_>>(),
        ["get_ui_catalog", "turn_output"]
    );
    let tool = &tools[1];
    assert_eq!(tool.input_schema["required"], json!(["text"]));
    assert_eq!(tool.input_schema["additionalProperties"], json!(false));
    assert_eq!(tool.input_schema["properties"]["text"]["type"], "string");
    assert_eq!(
        tool.output_schema.as_ref().expect("an output schema")["required"],
        json!(["delivered"])
    );
    let description = tool.description.as_deref().unwrap();
    for phrase in ["once the answer is ready", "one short line", "replaces"] {
        assert!(description.contains(phrase), "{phrase}: {description}");
    }
    let hints = tool.annotations.as_ref().unwrap();
    assert_eq!(hints.read_only_hint, Some(false));
    assert_eq!(hints.idempotent_hint, Some(false));
}

#[tokio::test]
async fn an_announced_answer_is_recorded_as_the_agents_message_and_returns_delivered() {
    let h = Harness::start().await;
    let thread = h.working_thread("plain").await;
    let client = client_as(&h, thread, 1, "m-1").await;

    let out = call(
        &client,
        "turn_output",
        json!({"text": "## Result\n\nIt works."}),
    )
    .await;
    assert!(!out.is_error, "{out:?}");
    assert_eq!(out.value, json!({"delivered": true}));

    let said = messages(&h, thread).await;
    assert_eq!(said.len(), 1, "{said:?}");
    let (actor, message) = &said[0];
    assert_eq!(actor.name, "plain");
    assert_eq!(message.text, "## Result\n\nIt works.");
    assert_eq!(message.message_id, "out-m-1-1");
    assert!(message.is_final);
    assert_eq!(message.purpose, Some(MessagePurpose::Answer));
    assert_eq!(message.via, Some(AnswerVia::TurnOutput));
    // announcing does not end the turn
    let state = h
        .app
        .get_thread(&UserId::new(ALICE), thread)
        .await
        .unwrap()
        .state;
    assert_eq!(state, ThreadState::Working);
}

#[tokio::test]
async fn a_second_call_in_the_turn_is_another_message_the_later_one_replaces_the_shown_answer() {
    let h = Harness::start().await;
    let thread = h.working_thread("plain").await;
    let client = client_as(&h, thread, 1, "m-1").await;

    for text in ["A first try.", "The answer."] {
        let out = call(&client, "turn_output", json!({"text": text})).await;
        assert_eq!(out.value, json!({"delivered": true}), "{out:?}");
    }
    let said = messages(&h, thread).await;
    let ids: Vec<_> = said.iter().map(|(_, m)| m.message_id.as_str()).collect();
    assert_eq!(ids, ["out-m-1-1", "out-m-1-2"]);
    // the log is append-only: both are marked as an answer announced by the tool, and the rule
    // (the last message marked answer in a turn is the answer) makes the first working text
    assert!(said.iter().all(|(_, m)| {
        m.purpose == Some(MessagePurpose::Answer) && m.via == Some(AnswerVia::TurnOutput)
    }));
    assert_eq!(said[1].1.text, "The answer.");
}

#[tokio::test]
async fn an_empty_text_and_an_oversize_one_are_errors_to_read_and_nothing_is_recorded() {
    let h = Harness::start().await;
    let thread = h.working_thread("plain").await;
    let client = client_as(&h, thread, 1, "m-1").await;

    for text in ["", "   \n  "] {
        let out = call(&client, "turn_output", json!({"text": text})).await;
        assert!(out.is_error, "{out:?}");
        assert_eq!(out.text, "text must not be empty");
    }
    let out = call(
        &client,
        "turn_output",
        json!({"text": "x".repeat(MAX_ANSWER_BYTES + 1)}),
    )
    .await;
    assert!(out.is_error, "{out:?}");
    assert_eq!(
        out.text,
        format!("text must be at most {MAX_ANSWER_BYTES} bytes")
    );
    // the largest answer is fine
    let out = call(
        &client,
        "turn_output",
        json!({"text": "y".repeat(MAX_ANSWER_BYTES)}),
    )
    .await;
    assert!(!out.is_error, "{out:?}");
    assert_eq!(messages(&h, thread).await.len(), 1);
}

#[tokio::test]
async fn arguments_that_are_not_text_are_a_protocol_error() {
    let h = Harness::start().await;
    let thread = h.working_thread("plain").await;
    let client = client_as(&h, thread, 1, "m-1").await;
    for bad in [
        json!({}),
        json!({"text": 7}),
        json!({"text": "ok", "extra": true}),
    ] {
        let err = try_call(&client, "turn_output", bad.clone())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("invalid arguments"),
            "{bad}: {err}"
        );
    }
    assert!(messages(&h, thread).await.is_empty());
}

#[tokio::test]
async fn once_the_turn_is_over_the_answer_is_refused() {
    let h = Harness::start().await;
    // a thread that has run to the end
    let done = h.thread("plain").await;
    let client = client_as(&h, done, 1, "m-1").await;
    let out = call(&client, "turn_output", json!({"text": "too late"})).await;
    assert!(out.is_error, "{out:?}");
    assert_eq!(out.text, "this turn is over");
    assert!(messages(&h, done).await.is_empty());

    // a thread that was cancelled while the agent worked
    let thread = h.working_thread("plain").await;
    let client = client_as(&h, thread, 1, "m-1").await;
    assert!(
        !call(&client, "turn_output", json!({"text": "in time"}))
            .await
            .is_error
    );
    h.app.cancel(&UserId::new(ALICE), thread).await.unwrap();
    orch_testsupport::eventually("the thread is cancelled", || async {
        let t = h.app.get_thread(&UserId::new(ALICE), thread).await.unwrap();
        (t.state == ThreadState::Cancelled).then_some(())
    })
    .await;
    let out = call(&client, "turn_output", json!({"text": "after the cancel"})).await;
    assert!(out.is_error, "{out:?}");
    assert_eq!(out.text, "this turn is over");
    assert_eq!(messages(&h, thread).await.len(), 1);
}

#[tokio::test]
async fn a_token_of_another_job_and_a_token_of_another_turn_announce_nothing() {
    let h = Harness::start().await;
    let thread = h.working_thread("plain").await;

    // the token of an earlier job (the thread is on job 1)
    let stale = client_as(&h, thread, 2, "m-9").await;
    let out = call(&stale, "turn_output", json!({"text": "from the future"})).await;
    assert!(out.is_error, "{out:?}");
    assert_eq!(out.text, "this turn is over");

    // once one token has announced, a token minted for another message is a turn that is over
    let current = client_as(&h, thread, 1, "m-1").await;
    assert!(
        !call(&current, "turn_output", json!({"text": "mine"}))
            .await
            .is_error
    );
    let other = client_as(&h, thread, 1, "m-0").await;
    let out = call(&other, "turn_output", json!({"text": "not mine"})).await;
    assert!(out.is_error, "{out:?}");
    assert_eq!(out.text, "this turn is over");
    let said = messages(&h, thread).await;
    assert_eq!(said.len(), 1);
    assert_eq!(said[0].1.text, "mine");
}
