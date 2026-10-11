//! The access tier on a run (ADR 0061, `access-tier/v1`): `forwardedProps["vymalo.tier"]` is read on
//! every run and refused before the stream when it is malformed (400), above what the person's role
//! allows or not honoured by the agent's live card (422), or the card cannot be read (503); applied
//! with the message otherwise, written as sent, shown on the message and sent to the agent. Nothing
//! is written for a run that is refused. The real router on a real port over the in-memory stack.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::BTreeSet;

use orch_app::{AgentScope, Permission, Policy, RoleGrant};
use orch_core::{AccessTier, TierOffer};
use orch_ports::Role;
use orch_ports::memory::Call;
use serde_json::{Value, json};
use support::*;

fn offer(tiers: &[&str]) -> TierOffer {
    TierOffer::from_params(&json!({"tiers": tiers, "default": tiers[0]})).unwrap()
}

/// A harness whose agent `plain` lists `access-tier/v1` with all four tiers.
async fn tiered() -> Harness {
    let h = Harness::start().await;
    h.agent
        .set_access_tier("plain", Some(offer(&["plan", "edits", "auto", "bypass"])));
    h
}

fn run_with(thread: &str, run: &str, text: &str, tier: &Value) -> Value {
    input_with(
        thread,
        run,
        &[(&format!("m-{run}"), text)],
        json!({"forwardedProps": {"vymalo.tier": tier}}),
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

fn tier_sent(h: &Harness) -> Vec<Option<AccessTier>> {
    h.agent
        .sends()
        .into_iter()
        .map(|call| match call {
            Call::Send { tier, .. } => tier,
            other => panic!("{other:?}"),
        })
        .collect()
}

#[tokio::test]
async fn a_run_with_a_tier_records_it_as_sent_shows_it_on_the_message_and_sends_it_to_the_agent() {
    for tier in ["plan", "edits", "auto", "bypass"] {
        let h = tiered().await;
        let thread = new_thread_id();
        let frames = h
            .run(
                "plain",
                ALICE,
                &run_with(&thread, "run-1", "echo go", &json!(tier)),
            )
            .await
            .all()
            .await;
        assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
        // the log: stored as sent
        let events = h.events(ALICE, &thread).await;
        assert_eq!(events[0]["kind"], "user_message");
        assert_eq!(events[0]["data"]["tier"], tier);
        // the screen: on the START of the message, which a viewer that connects is shown
        let viewed = h.connect_run(&thread, ALICE, None).await.all().await;
        let start = viewed
            .iter()
            .find(|f| f.kind() == "TEXT_MESSAGE_START" && f.event["role"] == "user")
            .expect("the user message is shown");
        assert_eq!(start.event["metadata"]["vymalo.tier"], tier);
        // the agent: told it with the message
        assert_eq!(
            tier_sent(&h),
            [Some(AccessTier::parse(tier).unwrap())],
            "{tier}"
        );
        // and what it echoed is in the log, beside the status that carried it
        let echoed: Vec<&Value> = events
            .iter()
            .filter(|e| e["kind"] == "agent_status")
            .filter(|e| e["data"].get("tier").is_some())
            .collect();
        assert_eq!(echoed.len(), 1, "{events:?}");
        assert_eq!(echoed[0]["data"]["tier"], tier);
    }
}

#[tokio::test]
async fn a_run_without_a_tier_or_with_none_is_the_run_it_always_was() {
    let h = tiered().await;
    for (n, props) in [
        json!({}),
        json!({"forwardedProps": {}}),
        json!({"forwardedProps": {"vymalo.tier": null}}),
    ]
    .into_iter()
    .enumerate()
    {
        let thread = new_thread_id();
        let body = input_with(&thread, &format!("run-{n}"), &[("m", "echo hi")], props);
        let frames = h.run("plain", ALICE, &body).await.all().await;
        assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
        let events = h.events(ALICE, &thread).await;
        assert!(events[0]["data"].get("tier").is_none(), "{events:?}");
        let viewed = h.connect_run(&thread, ALICE, None).await.all().await;
        let start = viewed
            .iter()
            .find(|f| f.kind() == "TEXT_MESSAGE_START" && f.event["role"] == "user")
            .unwrap();
        assert!(start.event["metadata"].get("vymalo.tier").is_none());
    }
    // no tier is sent, and the card is not read for one
    assert!(tier_sent(&h).iter().all(Option::is_none));
}

#[tokio::test]
async fn a_follow_up_names_its_own_tier_and_a_later_one_that_names_none_sends_none() {
    let h = tiered().await;
    let thread = new_thread_id();
    h.run(
        "plain",
        ALICE,
        &run_with(&thread, "run-1", "echo one", &json!("plan")),
    )
    .await
    .all()
    .await;
    h.wait_state(ALICE, &thread, "done").await;
    h.run(
        "plain",
        ALICE,
        &run_with(&thread, "run-2", "echo two", &json!("bypass")),
    )
    .await
    .all()
    .await;
    h.wait_state(ALICE, &thread, "done").await;
    h.run(
        "plain",
        ALICE,
        &input(&thread, "run-3", &[("m-3", "echo three")]),
    )
    .await
    .all()
    .await;
    h.wait_state(ALICE, &thread, "done").await;
    let events = h.events(ALICE, &thread).await;
    let logged: Vec<Option<&str>> = events
        .iter()
        .filter(|e| e["kind"] == "user_message")
        .map(|e| e["data"]["tier"].as_str())
        .collect();
    assert_eq!(logged, [Some("plan"), Some("bypass"), None]);
    assert_eq!(
        tier_sent(&h),
        [Some(AccessTier::Plan), Some(AccessTier::Bypass), None],
        "a task never inherits the tier of the one before"
    );
}

#[tokio::test]
async fn a_retry_of_the_same_run_does_not_write_the_message_twice() {
    let h = tiered().await;
    let thread = new_thread_id();
    let body = run_with(&thread, "run-1", "echo go", &json!("edits"));
    h.run("plain", ALICE, &body).await.all().await;
    h.run("plain", ALICE, &body).await.all().await;
    let events = h.events(ALICE, &thread).await;
    assert_eq!(
        events
            .iter()
            .filter(|e| e["kind"] == "user_message")
            .count(),
        1
    );
    assert_eq!(tier_sent(&h).len(), 1);
}

#[tokio::test]
async fn a_member_that_is_not_one_of_the_four_words_is_400_and_nothing_is_written() {
    let h = tiered().await;
    for (what, bad) in [
        ("a number", json!(3)),
        ("a boolean", json!(true)),
        ("an array", json!(["plan"])),
        ("an object", json!({"tier": "plan"})),
        ("another case", json!("Plan")),
        ("an unknown word", json!("yolo")),
        ("a near miss", json!("plan ")),
        ("an empty string", json!("")),
    ] {
        let thread = new_thread_id();
        let body = run_with(&thread, "run-1", "echo go", &bad);
        let problem = h.refused("plain", Some(ALICE), &body).await.problem(400);
        assert!(
            problem["detail"].as_str().unwrap().contains("vymalo.tier"),
            "{what}: {problem}"
        );
        assert_nothing_written(&h, &thread).await;
    }
}

#[tokio::test]
async fn a_malformed_member_is_400_on_a_run_that_continues_a_thread_or_attaches_to_one_too() {
    let h = tiered().await;
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
    // a new message, and the run that is only an attach (the same run again)
    for body in [
        run_with(&thread, "run-2", "echo more", &json!("yolo")),
        run_with(&thread, "run-1", "echo hi", &json!(7)),
    ] {
        h.refused("plain", Some(ALICE), &body).await.problem(400);
    }
    assert_eq!(h.events(ALICE, &thread).await.len(), before);
    assert_eq!(h.agent.sends().len(), sends);
}

#[tokio::test]
async fn a_tier_on_a_run_that_has_no_message_to_carry_it_is_ignored() {
    let h = tiered().await;
    let thread = new_thread_id();
    let body = run_with(&thread, "run-1", "echo hi", &json!("plan"));
    h.run("plain", ALICE, &body).await.all().await;
    h.wait_state(ALICE, &thread, "done").await;
    let before = h.events(ALICE, &thread).await.len();
    // the same run again is an attach: its tier is not written anywhere
    let again = input_with(
        &thread,
        "run-1",
        &[],
        json!({"forwardedProps": {"vymalo.tier": "bypass"}}),
    );
    let _ = h.run("plain", ALICE, &again).await.all().await;
    assert_eq!(h.events(ALICE, &thread).await.len(), before);
    assert_eq!(tier_sent(&h), [Some(AccessTier::Plan)]);
}

#[tokio::test]
async fn an_agent_whose_card_does_not_honour_the_tier_is_422_and_nothing_is_written() {
    // not listed at all
    let h = Harness::start().await;
    let thread = new_thread_id();
    let problem = h
        .refused(
            "plain",
            Some(ALICE),
            &run_with(&thread, "run-1", "echo go", &json!("plan")),
        )
        .await
        .problem(422);
    assert_eq!(
        problem["detail"],
        "this agent does not take the tier 'plan'"
    );
    assert_nothing_written(&h, &thread).await;

    // listed, and not this tier
    let h = Harness::start().await;
    h.agent.set_access_tier("plain", Some(offer(&["plan"])));
    let thread = new_thread_id();
    let problem = h
        .refused(
            "plain",
            Some(ALICE),
            &run_with(&thread, "run-1", "echo go", &json!("bypass")),
        )
        .await
        .problem(422);
    assert_eq!(
        problem["detail"],
        "this agent does not take the tier 'bypass'"
    );
    assert_nothing_written(&h, &thread).await;
    // and the one it lists goes
    let ok = new_thread_id();
    let frames = h
        .run(
            "plain",
            ALICE,
            &run_with(&ok, "run-2", "echo go", &json!("plan")),
        )
        .await
        .all()
        .await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
}

#[tokio::test]
async fn a_card_that_cannot_be_read_is_503_with_a_retry_and_nothing_is_written() {
    let h = tiered().await;
    h.agent.set_card_down("plain", true);
    let thread = new_thread_id();
    let resp = h
        .refused(
            "plain",
            Some(ALICE),
            &run_with(&thread, "run-1", "echo go", &json!("plan")),
        )
        .await;
    assert_eq!(resp.status, 503);
    let problem = resp.problem(503);
    assert_eq!(
        problem["detail"],
        "the card of the agent plain could not be read"
    );
    assert_nothing_written(&h, &thread).await;
    // a run with no tier does not read the card for one
    let thread = new_thread_id();
    let frames = h
        .run(
            "plain",
            ALICE,
            &input(&thread, "run-2", &[("m", "echo hi")]),
        )
        .await
        .all()
        .await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
}

#[tokio::test]
async fn a_tier_above_what_the_role_allows_is_422_and_one_up_to_it_goes() {
    let capped = RoleGrant {
        permissions: BTreeSet::from([
            Permission::AgentRead,
            Permission::AgentInvoke,
            Permission::ThreadRead,
            Permission::ThreadWrite,
        ]),
        agents: AgentScope::All,
        max_tier: Some(AccessTier::Edits),
    };
    let policy = Policy::new(
        [(Role::new("careful"), capped)].into(),
        Some(Role::new("careful")),
    )
    .unwrap();
    let h = Harness::start_with_policy(policy).await;
    h.agent
        .set_access_tier("plain", Some(offer(&["plan", "edits", "auto", "bypass"])));
    let thread = new_thread_id();
    let problem = h
        .refused(
            "plain",
            Some(ALICE),
            &run_with(&thread, "run-1", "echo go", &json!("bypass")),
        )
        .await
        .problem(422);
    assert_eq!(
        problem["detail"],
        "the tier 'bypass' is above what your role allows"
    );
    assert_nothing_written(&h, &thread).await;
    let ok = new_thread_id();
    let frames = h
        .run(
            "plain",
            ALICE,
            &run_with(&ok, "run-2", "echo go", &json!("edits")),
        )
        .await
        .all()
        .await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
    assert_eq!(tier_sent(&h), [Some(AccessTier::Edits)]);
}

#[tokio::test]
async fn the_first_message_of_a_fork_names_its_own_tier() {
    let h = tiered().await;
    let from = new_thread_id();
    h.run(
        "plain",
        ALICE,
        &input(&from, "run-p", &[("m-p", "echo one")]),
    )
    .await
    .all()
    .await;
    h.wait_state(ALICE, &from, "done").await;
    let fork = new_thread_id();
    let body = input_with(
        &fork,
        "run-f",
        &[("m-f", "echo two")],
        json!({"forwardedProps": {"vymalo.fork": {"from": from, "after": 1}, "vymalo.tier": "plan"}}),
    );
    let frames = h.run("plain", ALICE, &body).await.all().await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
    let events = h.events(ALICE, &fork).await;
    let messages: Vec<Option<&str>> = events
        .iter()
        .filter(|e| e["kind"] == "user_message")
        .map(|e| e["data"]["tier"].as_str())
        .collect();
    assert_eq!(messages.last(), Some(&Some("plan")));
    assert_eq!(tier_sent(&h).last(), Some(&Some(AccessTier::Plan)));
    // a fork that names a tier its agent does not take is refused, and nothing is made
    let refused = new_thread_id();
    h.agent.set_access_tier("plain", Some(offer(&["auto"])));
    let body = input_with(
        &refused,
        "run-g",
        &[("m-g", "echo three")],
        json!({"forwardedProps": {"vymalo.fork": {"from": from, "after": 1}, "vymalo.tier": "plan"}}),
    );
    h.refused("plain", Some(ALICE), &body).await.problem(422);
    assert!(
        h.app
            .thread_for_tools(refused.parse().unwrap())
            .await
            .unwrap()
            .is_none()
    );
}
