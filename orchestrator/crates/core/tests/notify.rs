//! Slice N1 of ADR 0077: the fold of a log into notification intents, pure.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use orch_core::{
    Event, Notify, NotifyClass, NotifyKind, NotifyTtl, ThreadId, ThreadRole,
    collapse_notifications, fold_notifications,
};
use serde_json::{Value, json};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../docs/api/examples/notifications")
}

fn thread() -> ThreadId {
    ThreadId(uuid::Uuid::from_u128(1))
}

fn render(n: &Notify) -> Value {
    match n {
        Notify::Intent(i) => json!({"intent": {
            "class": match i.class { NotifyClass::NeedsYou => "needs_you", NotifyClass::Done => "done" },
            "kind": match i.kind {
                NotifyKind::NeedsYou => "needs_you", NotifyKind::Done => "done",
                NotifyKind::Failed => "failed", NotifyKind::PullRequest => "pull_request" },
            "cause": i.cause, "count": i.count,
            "expires": i.expires.map(|t| t.to_string()),
        }}),
        Notify::Withdraw { class, .. } => json!({"withdraw": {
            "class": match class { NotifyClass::NeedsYou => "needs_you", NotifyClass::Done => "done" }
        }}),
    }
}

fn load(name: &str) -> (ThreadRole, i64, Vec<Event>, Vec<Value>) {
    let text = std::fs::read_to_string(dir().join(format!("{name}.json"))).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    let role = if v["role"] == "child" {
        ThreadRole::Child
    } else {
        ThreadRole::Top
    };
    let events = serde_json::from_value(v["events"].clone()).unwrap();
    (
        role,
        v["after"].as_i64().unwrap(),
        events,
        v["expect"].as_array().unwrap().clone(),
    )
}

fn run(name: &str) -> (Vec<Value>, Vec<Value>) {
    let (role, after, events, expect) = load(name);
    let got = fold_notifications(thread(), role, &NotifyTtl::default(), after, &events);
    (got.iter().map(render).collect(), expect)
}

macro_rules! vector {
    ($test:ident, $file:literal) => {
        #[test]
        fn $test() {
            let (got, expect) = run($file);
            assert_eq!(got, expect);
        }
    };
}

vector!(
    a_wait_makes_a_needs_you_and_the_answer_withdraws_it,
    "needs-you-then-answer"
);
vector!(
    an_end_and_a_pull_request_make_two_intents,
    "done-and-pull-request"
);
vector!(a_failure_makes_a_failed_intent, "failed");
vector!(
    a_persons_own_cancel_makes_no_intent,
    "own-cancel-makes-none"
);
vector!(
    a_child_waits_are_told_and_its_end_is_not,
    "child-end-makes-none-child-wait-does"
);
vector!(
    a_replayed_input_makes_no_second_intent,
    "replay-makes-no-second-intent"
);

#[test]
fn folding_the_whole_log_twice_makes_each_intent_once() {
    let (role, _, events, _) = load("done-and-pull-request");
    let all = fold_notifications(thread(), role, &NotifyTtl::default(), 0, &events);
    let again = fold_notifications(thread(), role, &NotifyTtl::default(), 0, &events);
    assert_eq!(all, again);
    let last = events.last().unwrap().seq;
    assert!(fold_notifications(thread(), role, &NotifyTtl::default(), last, &events).is_empty());
}

#[test]
fn a_persons_message_without_a_wait_withdraws_nothing() {
    let (role, _, events, _) = load("failed");
    let got = fold_notifications(thread(), role, &NotifyTtl::default(), 0, &events);
    assert!(!got.iter().any(|n| matches!(n, Notify::Withdraw { .. })));
}

#[test]
fn a_newer_intent_of_the_same_thread_and_class_supersedes_the_older() {
    let (_, _, events, _) = load("done-and-pull-request");
    let got = fold_notifications(thread(), ThreadRole::Top, &NotifyTtl::default(), 0, &events);
    let kept = collapse_notifications(got);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].kind, NotifyKind::Done);
    assert_eq!(kept[0].cause, 4);
}

#[test]
fn an_answered_wait_leaves_nothing_to_send() {
    let (role, after, events, _) = load("needs-you-then-answer");
    let got = fold_notifications(thread(), role, &NotifyTtl::default(), after, &events);
    assert!(collapse_notifications(got).is_empty());
}

#[test]
fn two_waits_count_two_until_the_person_answers() {
    let (_, _, mut events, _) = load("needs-you-then-answer");
    events.truncate(3);
    let mut second = events[2].clone();
    second.seq = 4;
    events.push(second);
    let got = fold_notifications(thread(), ThreadRole::Top, &NotifyTtl::default(), 0, &events);
    let counts: Vec<u32> = got
        .iter()
        .filter_map(|n| {
            if let Notify::Intent(i) = n {
                Some(i.count)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(counts, [1, 2]);
}
