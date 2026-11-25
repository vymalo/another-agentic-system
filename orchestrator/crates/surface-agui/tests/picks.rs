//! The model and reasoning effort of a run (ADR 0086, `model-pick/v1` and `reasoning/v1`):
//! `forwardedProps["vymalo.model"]` and `["vymalo.reasoningEffort"]` are read on every run and
//! refused before the stream when they are not strings (400) or not offered by the agent's live
//! card (422); applied with the message otherwise, written as sent and sent to the agent. Nothing
//! is written for a run that is refused. The real router on a real port over the in-memory stack.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_core::{EffortOffer, ModelPick, ModelPickOffer};
use orch_ports::memory::Call;
use serde_json::{Value, json};

use support::*;

fn auto_offer() -> (Option<ModelPickOffer>, Option<EffortOffer>) {
    (
        Some(
            ModelPickOffer::from_params(&json!({
                "choices": [{"id": "fast"}, {"id": "deep", "label": "Deep"}],
                "default": "fast"
            }))
            .unwrap(),
        ),
        Some(
            EffortOffer::from_params(&json!({"efforts": ["low", "high"], "default": "low"}))
                .unwrap(),
        ),
    )
}

/// A harness whose agent `plain` lists `model-pick/v1` (`fast`, `deep`) and `reasoning/v1`
/// (`low`, `high`).
async fn picking() -> Harness {
    let h = Harness::start().await;
    let (models, efforts) = auto_offer();
    h.agent.set_pick("plain", models, efforts);
    h
}

fn run_with(thread: &str, run: &str, text: &str, props: Value) -> Value {
    input_with(
        thread,
        run,
        &[(run, text)],
        json!({"forwardedProps": props}),
    )
}

/// The pick each `Send` to the agent carried, in order.
fn sent(h: &Harness) -> Vec<Option<ModelPick>> {
    h.agent
        .sends()
        .into_iter()
        .map(|call| match call {
            Call::Send(call) => call.pick,
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
async fn a_run_with_a_model_and_an_effort_sends_them_and_records_them() {
    let h = picking().await;
    let thread = new_thread_id();
    let frames = h
        .run(
            "plain",
            ALICE,
            &run_with(
                &thread,
                "run-1",
                "echo go",
                json!({"vymalo.model": "deep", "vymalo.reasoningEffort": "high"}),
            ),
        )
        .await
        .all()
        .await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
    // the log: stored as sent, both axes in one member
    let events = h.events(ALICE, &thread).await;
    assert_eq!(events[0]["kind"], "user_message");
    assert_eq!(events[0]["data"]["pick"]["model"], "deep");
    assert_eq!(events[0]["data"]["pick"]["reasoningEffort"], "high");
    // the agent: told it with the message
    assert_eq!(
        sent(&h),
        [Some(ModelPick {
            model: Some(orch_core::ModelId::parse("deep").unwrap()),
            reasoning_effort: Some(orch_core::ReasoningEffort::High),
        })],
    );
}

#[tokio::test]
async fn one_axis_alone_is_sent_alone_and_a_run_with_none_is_the_run_it_always_was() {
    let h = picking().await;
    for (n, props) in [
        json!({"vymalo.model": "fast"}),
        json!({"vymalo.reasoningEffort": "low"}),
        json!({}),
        json!({"vymalo.model": null, "vymalo.reasoningEffort": null}),
    ]
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
    }
    assert_eq!(
        sent(&h),
        [
            Some(ModelPick {
                model: Some(orch_core::ModelId::parse("fast").unwrap()),
                reasoning_effort: None,
            }),
            Some(ModelPick {
                model: None,
                reasoning_effort: Some(orch_core::ReasoningEffort::Low),
            }),
            None,
            None,
        ],
    );
}

#[tokio::test]
async fn a_pick_the_card_does_not_offer_is_422_and_nothing_is_written() {
    let h = picking().await;
    // a model the card does not list
    let thread = new_thread_id();
    let problem = h
        .refused(
            "plain",
            Some(ALICE),
            &run_with(&thread, "run-1", "echo go", json!({"vymalo.model": "huge"})),
        )
        .await
        .problem(422);
    assert_eq!(problem["code"], "model_not_offered");
    assert_eq!(problem["offered"], json!(["fast", "deep"]));
    assert_nothing_written(&h, &thread).await;

    // an effort the card does not list
    let h = picking().await;
    let thread = new_thread_id();
    let problem = h
        .refused(
            "plain",
            Some(ALICE),
            &run_with(
                &thread,
                "run-1",
                "echo go",
                json!({"vymalo.reasoningEffort": "max"}),
            ),
        )
        .await
        .problem(422);
    assert_eq!(problem["code"], "reasoning_not_offered");
    assert_eq!(problem["offered"], json!(["low", "high"]));
    assert_nothing_written(&h, &thread).await;
}

#[tokio::test]
async fn an_agent_that_lists_nothing_refuses_a_pick_with_an_empty_offer() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let problem = h
        .refused(
            "plain",
            Some(ALICE),
            &run_with(&thread, "run-1", "echo go", json!({"vymalo.model": "fast"})),
        )
        .await
        .problem(422);
    assert_eq!(problem["code"], "model_not_offered");
    assert_eq!(problem["offered"], json!([]));
    assert_nothing_written(&h, &thread).await;
}

#[tokio::test]
async fn a_member_that_is_not_a_string_is_400_on_every_run() {
    let h = picking().await;
    for props in [
        json!({"vymalo.model": 3}),
        json!({"vymalo.reasoningEffort": true}),
        json!({"vymalo.model": {"id": "fast"}}),
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
            problem["detail"].as_str().unwrap().contains("vymalo."),
            "{props}: {problem}"
        );
        assert_nothing_written(&h, &thread).await;
    }
}
