//! The token allowance of a role (ADR 0081, `tokensPerPeriod`): a person who spent it is refused
//! before anything is written or sent to an agent, and a role with no allowance changes nothing.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::BTreeMap;

use jiff::Timestamp;
use orch_app::{AppConfig, AppError, Policy};
use orch_core::{
    Actor, AgentId, AgentTarget, EventBody, ModelUsageTotalData, ThreadId, ThreadState,
    TokenCounts, UserId, UserMessageData,
};
use orch_ports::memory::MemoryStore;
use orch_ports::{Commit, NewEvent, NewThreadRecord, Role, ThreadStore};
use support::{World, alice, bob, events};
use uuid::Uuid;

fn app_with_allowance(world: &World, tokens: Option<u64>) -> std::sync::Arc<support::TestApp> {
    let mut allowances = BTreeMap::new();
    if let Some(tokens) = tokens {
        allowances.insert(Role::new("user"), tokens);
    }
    world.app_with(AppConfig {
        policy: Policy::default().with_allowances(allowances),
        ..AppConfig::default()
    })
}

/// A thread of `owner` whose log holds one finished task of `tokens` tokens, this month.
async fn thread_that_spent(store: &MemoryStore, owner: &UserId, tokens: u64) -> ThreadId {
    let now: Timestamp = Timestamp::now();
    let id = ThreadId(Uuid::now_v7());
    let events = vec![
        NewEvent {
            at: now,
            actor: Actor::user(owner),
            body: EventBody::UserMessage(UserMessageData::new("hi")),
            idempotency_key: None,
        },
        NewEvent {
            at: now,
            actor: Actor::agent(&AgentId::new("coder"), None),
            body: EventBody::ModelUsageTotal(ModelUsageTotalData {
                job: 1,
                agent: "coder".into(),
                task: "t-1".into(),
                path: Vec::new(),
                totals: vec![orch_core::ModelTokens {
                    provider: Some("openai".into()),
                    model: "glm".into(),
                    tokens: TokenCounts {
                        input_tokens: tokens,
                        output_tokens: 0,
                        total_tokens: tokens,
                        ..TokenCounts::default()
                    },
                }],
                sandbox: None,
                counted: true,
            }),
            idempotency_key: None,
        },
    ];
    store
        .create_thread(
            NewThreadRecord {
                id,
                owner: owner.clone(),
                title: "t".into(),
                description: None,
                target: AgentTarget {
                    agent_id: AgentId::new("coder"),
                    release: None,
                },
                context_id: None,
                rail_parent: None,
                child_of: None,
                now,
            },
            Commit {
                new_state: ThreadState::Working,
                job: None,
                events,
                outbox: Vec::new(),
                binding: None,
                now,
                lease: None,
                watches: Vec::new(),
                timers: Vec::new(),
                inbox: None,
                finishes_outbox: None,
                title: None,
                description: None,
                sharing: None,
                skip_unsent_delegates: false,
            },
        )
        .await
        .unwrap();
    id
}

#[tokio::test]
async fn a_spent_allowance_refuses_the_message_before_anything_is_written_or_sent() {
    let world = World::new();
    let app = app_with_allowance(&world, Some(1_000));
    let id = thread_that_spent(&world.store, &alice(), 1_000).await;
    let before = events(&app, &alice(), id).await.len();

    let refused = app
        .post_message(&alice(), id, "one more".into())
        .await
        .unwrap_err();

    assert!(
        matches!(refused, AppError::AllowanceSpent { limit: 1_000 }),
        "{refused:?}"
    );
    assert!(refused.to_string().contains("1000"), "names the limit");
    assert_eq!(
        events(&app, &alice(), id).await.len(),
        before,
        "nothing written"
    );
    assert!(world.agent.sends().is_empty(), "no agent request");
}

#[tokio::test]
async fn stop_and_send_is_refused_the_same_way() {
    let world = World::new();
    let app = app_with_allowance(&world, Some(500));
    let id = thread_that_spent(&world.store, &alice(), 900).await;

    let refused = app
        .stop_and_send(&alice(), id, "stop".into())
        .await
        .unwrap_err();

    assert!(matches!(refused, AppError::AllowanceSpent { limit: 500 }));
    assert!(world.agent.sends().is_empty());
}

#[tokio::test]
async fn under_the_allowance_the_message_passes() {
    let world = World::new();
    let app = app_with_allowance(&world, Some(1_000));
    let id = thread_that_spent(&world.store, &alice(), 999).await;

    app.post_message(&alice(), id, "still room".into())
        .await
        .unwrap();
}

#[tokio::test]
async fn another_persons_spending_is_not_counted() {
    let world = World::new();
    let app = app_with_allowance(&world, Some(1_000));
    thread_that_spent(&world.store, &bob(), 5_000).await;
    let id = thread_that_spent(&world.store, &alice(), 1).await;

    app.post_message(&alice(), id, "mine".into()).await.unwrap();
}

#[tokio::test]
async fn with_no_allowance_set_nothing_changes() {
    let world = World::new();
    let app = app_with_allowance(&world, None);
    let id = thread_that_spent(&world.store, &alice(), 50_000_000).await;

    app.post_message(&alice(), id, "no plan, no limit".into())
        .await
        .unwrap();
}
