//! The mode of a run (ADR 0109, `agent-mode/v1`): `forwardedProps["vymalo.mode"]` is read on every
//! run and refused before the stream when it is not a mode name (400) or not offered by the agent's
//! live card (422); applied with the message otherwise, written as sent and sent to the agent.
//! Nothing is written for a run that is refused. The real router on a real port over the in-memory
//! stack.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_core::{ModeName, ModeOffer};
use orch_ports::memory::Call;
use serde_json::{Value, json};

use support::*;

fn offer() -> Option<ModeOffer> {
    ModeOffer::from_params(&json!({
        "modes": [
            {"name": "build", "title": "Build", "description": "Make the change."},
            {"name": "plan", "title": "Plan", "description": "Read and plan."}
        ],
        "default": "build"
    }))
}

/// A harness whose agent `plain` lists `agent-mode/v1` with `build` (default) and `plan`.
async fn moded() -> Harness {
    let h = Harness::start().await;
    h.agent.set_mode("plain", offer());
    h
}

fn plan() -> ModeName {
    ModeName::parse("plan").unwrap()
}

fn run_with(thread: &str, run: &str, text: &str, props: Value) -> Value {
    input_with(
        thread,
        run,
        &[(run, text)],
        json!({"forwardedProps": props}),
    )
}

/// The mode each `Send` to the agent carried, in order.
fn sent(h: &Harness) -> Vec<Option<ModeName>> {
    h.agent
        .sends()
        .into_iter()
        .map(|call| match call {
            Call::Send(call) => call.mode,
            other => panic!("{other:?}"),
        })
        .collect()
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

#[tokio::test]
async fn a_run_with_a_mode_sends_it_and_records_it() {
    let h = moded().await;
    let thread = new_thread_id();
    let frames = h
        .run(
            "plain",
            ALICE,
            &run_with(&thread, "run-1", "echo go", json!({"vymalo.mode": "plan"})),
        )
        .await
        .all()
        .await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
    let events = h.events(ALICE, &thread).await;
    assert_eq!(events[0]["kind"], "user_message");
    assert_eq!(events[0]["data"]["mode"], "plan");
    assert_eq!(sent(&h), [Some(plan())], "the agent is told it");
}

#[tokio::test]
async fn a_run_with_no_mode_is_the_run_it_always_was() {
    let h = moded().await;
    for (n, props) in [json!({}), json!({"vymalo.mode": null})]
        .into_iter()
        .enumerate()
    {
        let thread = new_thread_id();
        let frames = h
            .run(
                "plain",
                ALICE,
                &run_with(&thread, &format!("run-{n}"), "echo hi", props.clone()),
            )
            .await
            .all()
            .await;
        assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED", "{props}");
        let events = h.events(ALICE, &thread).await;
        assert!(events[0]["data"].get("mode").is_none(), "{props}");
    }
    assert_eq!(sent(&h), [None, None]);
}

#[tokio::test]
async fn a_mode_the_card_does_not_offer_is_422_and_nothing_is_written() {
    let h = moded().await;
    let thread = new_thread_id();
    let problem = h
        .refused(
            "plain",
            Some(ALICE),
            &run_with(
                &thread,
                "run-1",
                "echo go",
                json!({"vymalo.mode": "review"}),
            ),
        )
        .await
        .problem(422);
    assert_eq!(problem["code"], "mode_not_offered");
    assert_eq!(problem["offered"], json!(["build", "plan"]));
    assert_nothing_written(&h, &thread).await;
}

#[tokio::test]
async fn an_agent_that_lists_nothing_refuses_a_mode_with_an_empty_offer() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let problem = h
        .refused(
            "plain",
            Some(ALICE),
            &run_with(&thread, "run-1", "echo go", json!({"vymalo.mode": "plan"})),
        )
        .await
        .problem(422);
    assert_eq!(problem["code"], "mode_not_offered");
    assert_eq!(problem["offered"], json!([]));
    assert_nothing_written(&h, &thread).await;
}

#[tokio::test]
async fn a_member_that_is_not_a_mode_name_is_400_on_every_run() {
    let h = moded().await;
    for props in [
        json!({"vymalo.mode": 3}),
        json!({"vymalo.mode": true}),
        json!({"vymalo.mode": {"name": "plan"}}),
        json!({"vymalo.mode": "   "}),
        json!({"vymalo.mode": "a\nb"}),
    ] {
        let thread = new_thread_id();
        let problem = h
            .refused(
                "plain",
                Some(ALICE),
                &run_with(&thread, "run-1", "echo go", props.clone()),
            )
            .await
            .problem(400);
        assert!(
            problem["detail"].as_str().unwrap().contains("vymalo.mode"),
            "{props}: {problem}"
        );
        assert_nothing_written(&h, &thread).await;
    }
}
