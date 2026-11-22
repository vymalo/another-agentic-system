//! A receipt for a trusted card (ADR 0083, `docs/api/trusted-components-v1.md`): the client's
//! action names the card's `messageId` (`trusted-<seq>`), not a surface, and is logged with the
//! receipt as its context and forwarded to the agent as an A2UI action; an id that is neither a
//! surface nor a card of the thread is still refused before anything is written or sent.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_core::UiVersion;
use orch_ports::memory::{Call, SendCall};
use serde_json::{Value, json};
use support::*;

fn action_run(thread: &str, run: &str, user_action: Value) -> Value {
    input_with(
        thread,
        run,
        &[],
        json!({"forwardedProps": {"a2uiAction": {"userAction": user_action}}}),
    )
}

#[tokio::test]
async fn a_receipt_naming_a_trusted_card_is_accepted_and_forwarded_and_an_unknown_id_is_refused() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let frames = h
        .run(
            "plain",
            ALICE,
            &input(&thread, "run-1", &[("m-1", "trusted add a server")]),
        )
        .await
        .all()
        .await;
    assert_eq!(
        frames.last().unwrap().event["outcome"]["type"],
        "interrupt",
        "the agent asked and the thread waits"
    );
    let card = frames
        .iter()
        .find(|f| f.event["activityType"] == "vymalo.trustedRequest")
        .expect("the card reached the client");
    let card_id = card.event["messageId"].as_str().unwrap().to_owned();
    assert!(card_id.starts_with("trusted-"), "{card_id}");
    assert_eq!(card.event["content"]["component"], "McpCredential");

    // The receipt, exactly as the web sends it: the action names the card it answers.
    let receipt = json!({
        "status": "created",
        "id": "u-abc1234567",
        "name": "My search",
        "scopes": ["search.read"],
    });
    let submit = json!({
        "name": "submit",
        "surfaceId": card_id.clone(),
        "sourceComponentId": "McpCredential",
        "context": receipt.clone(),
    });

    // An id that is neither a surface nor a card of the thread is still a 422, and nothing is
    // written or sent.
    let before = h.events(ALICE, &thread).await.len();
    let sent = h.agent.sends().len();
    let mut made_up = submit.clone();
    made_up["surfaceId"] = json!("trusted-99");
    let r = h
        .refused("plain", Some(ALICE), &action_run(&thread, "run-x", made_up))
        .await;
    let p = r.problem(422);
    assert!(p["detail"].as_str().unwrap().contains("trusted-99"), "{p}");
    assert_eq!(
        h.events(ALICE, &thread).await.len(),
        before,
        "nothing was written"
    );
    assert_eq!(h.agent.sends().len(), sent, "nothing was sent");

    // The card's own id is accepted, and the action goes out as it is.
    let frames = h
        .run(
            "plain",
            ALICE,
            &action_run(&thread, "run-act", submit.clone()),
        )
        .await
        .all()
        .await;
    assert_eq!(frames[0].kind(), "RUN_STARTED");
    assert_eq!(
        frames.last().unwrap().event["outcome"],
        json!({"type": "success"})
    );

    // What the requester is shown it did.
    let activity = frames
        .iter()
        .find(|f| f.event["activityType"] == "vymalo.action")
        .expect("the action is an activity");
    let mut content = activity.event["content"].clone();
    assert!(
        content
            .as_object_mut()
            .unwrap()
            .remove("at")
            .is_some_and(|at| at.is_string()),
        "every vymalo activity says when"
    );
    assert_eq!(
        content,
        json!({"surfaceId": card_id.clone(), "name": "submit", "sourceComponentId": "McpCredential",
               "context": receipt.clone()})
    );

    // The log: a ui_action by the user, with the receipt as its context.
    let events = h.events(ALICE, &thread).await;
    let logged = events
        .iter()
        .find(|e| e["kind"] == "ui_action")
        .expect("the ui_action is logged");
    assert_eq!(logged["actor"]["type"], "user");
    assert_eq!(logged["data"]["surfaceId"], card_id);
    assert_eq!(logged["data"]["name"], "submit");
    assert_eq!(logged["data"]["sourceComponentId"], "McpCredential");
    assert_eq!(logged["data"]["context"], receipt);
    assert_eq!(logged["data"]["version"], "1.0");
    assert_eq!(logged["data"]["runId"], "run-act");

    // And the agent got the action, with the five members of an A2UI v1.0 action.
    let sends = h.agent.sends();
    let Call::Send(call) = sends.last().unwrap() else {
        panic!("{sends:?}");
    };
    let SendCall {
        action,
        action_at,
        task_id,
        ..
    } = &**call;
    assert!(
        task_id.is_some(),
        "the action answers the task the agent was waiting on"
    );
    let action = action.as_ref().unwrap();
    assert_eq!(action.name, "submit");
    assert_eq!(action.surface_id, card_id);
    assert_eq!(action.source_component_id, "McpCredential");
    assert_eq!(Value::Object(action.context.clone()), receipt);
    assert_eq!(action.version, UiVersion::V1_0);
    assert_eq!(
        action_at.as_ref().unwrap().to_string(),
        logged["at"].as_str().unwrap(),
        "the fifth member, the timestamp, is the log's own time"
    );
}
