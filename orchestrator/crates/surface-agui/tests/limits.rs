//! A person's messages per minute (ADR 0081, decisions 2 and 4): a message over the role's
//! `messages_per_minute` is refused with `429`, `Retry-After` and a problem that **names the limit
//! and its value**, before anything is written and before any agent is asked. A message under the
//! limit goes through, and a deployment that sets no limit refuses nothing.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_app::Policy;
use orch_core::{ThreadRecord, UserId};
use orch_ports::{Role, ThreadListing, ThreadStore as _};
use serde_json::json;
use support::*;

/// The default roles (a person without a role is a `user`), with `max` messages a minute for
/// `user`.
fn limited(max: u32) -> Policy {
    Policy::new(orch_app::built_in_roles(), Some(Role::new("user")))
        .unwrap()
        .with_messages_per_minute("user", max)
}

/// The messages of one run, as `(id, text)`.
fn one(text: &str) -> Vec<(&str, &str)> {
    vec![("m1", text)]
}

/// The owner's threads, newest first.
async fn threads_of(h: &Harness, owner: &UserId) -> Vec<ThreadRecord> {
    h.store
        .list_threads(owner, ThreadListing::recent(None, 50, false))
        .await
        .unwrap()
}

#[tokio::test]
async fn refusal_names_the_limit() {
    const MAX: u32 = 2;
    let h = Harness::start_with_policy(limited(MAX)).await;
    let alice = UserId::new(ALICE);

    // Under the limit: two messages, each on a thread of its own, are accepted and reach the agent.
    for n in 0..MAX {
        let thread = new_thread_id();
        let mut run = h
            .run(
                "plain",
                ALICE,
                &input(&thread, &format!("r{n}"), &one("hello")),
            )
            .await;
        let frames = run.through_run().await;
        assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
    }
    let under = threads_of(&h, &alice).await;
    assert_eq!(
        under.len(),
        MAX as usize,
        "both messages under the limit were written"
    );
    let sends = h.agent.sends().len();
    assert_eq!(
        sends, MAX as usize,
        "both messages under the limit reached the agent"
    );

    // Over the limit: the next message is refused, on a new thread, before anything is written.
    let thread = new_thread_id();
    let refused = h
        .refused(
            "plain",
            Some(ALICE),
            &input(&thread, "r3", &one("one message too many")),
        )
        .await;
    let problem = refused.problem(429);

    // `Retry-After` is a whole number of seconds: when the oldest message in the window leaves it.
    let retry_after = refused
        .headers
        .get("retry-after")
        .expect("a 429 for a message limit carries Retry-After")
        .to_str()
        .unwrap()
        .parse::<u64>()
        .expect("Retry-After is a whole number of seconds");
    assert!(
        (1..=60).contains(&retry_after),
        "Retry-After is the seconds left in the minute: {retry_after}"
    );

    // The body names the limit, its value and the current usage.
    assert_eq!(problem["code"], "over_limit", "{problem}");
    assert_eq!(problem["limit"], "messages_per_minute", "{problem}");
    assert_eq!(problem["inUse"], json!(MAX), "{problem}");
    assert_eq!(problem["max"], json!(MAX), "{problem}");

    // Nothing reached an agent, and no thread row was written for the refused message.
    assert_eq!(
        h.agent.sends().len(),
        sends,
        "the refusal is before the dispatch: no agent was asked"
    );
    assert!(
        h.store
            .get_thread(None, thread.parse().unwrap())
            .await
            .unwrap()
            .is_none(),
        "the refused message wrote no thread"
    );
    assert_eq!(
        threads_of(&h, &alice).await.len(),
        MAX as usize,
        "the refused message wrote nothing"
    );
}

#[tokio::test]
async fn no_limit_refuses_nothing() {
    // The default policy sets no `messages_per_minute` (ADR 0081, decision 2): nothing is refused,
    // however many messages arrive in the minute.
    let h = Harness::start().await;
    for n in 0..5 {
        let thread = new_thread_id();
        let mut run = h
            .run(
                "plain",
                ALICE,
                &input(&thread, &format!("r{n}"), &one("hello")),
            )
            .await;
        let frames = run.through_run().await;
        assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED", "message {n}");
    }
    assert_eq!(h.agent.sends().len(), 5);
}
